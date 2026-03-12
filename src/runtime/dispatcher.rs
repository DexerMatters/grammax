use std::{sync::Arc, thread};

use crossbeam::channel;

use crate::{scheme, utils::Span};

use super::{
    compiler::ComposedCompiler,
    payload::Payload,
    protocol::{
        RevisionId, RuntimeEnvelope, RuntimeError, RuntimeEvent, RuntimePath, RuntimeRequest,
        RuntimeResult, RuntimeSignal,
    },
};

// ─── Internal types ──────────────────────────────────────────────────────────

/// Commands that can be sent to the GED loop outside of the envelope channel.
/// Currently only `Subscribe` needs this path.
pub(super) enum SubCommand {
    Subscribe {
        layer_path: Option<RuntimePath>,
        sender: channel::Sender<RuntimeEvent>,
    },
}

struct Subscriber {
    layer_path: Option<RuntimePath>,
    sender: channel::Sender<RuntimeEvent>,
}

struct PendingQuery {
    revision: RevisionId,
    layer_path: RuntimePath,
    index: Payload,
    reply: channel::Sender<RuntimeResult>,
}

// ─── Subscription handle ────────────────────────────────────────────────────

/// Owned handle for a single subscriber.  Dropping it automatically
/// unregisters the subscription (the GED prunes disconnected senders).
pub struct SubscriptionHandle {
    rx: channel::Receiver<RuntimeEvent>,
}

impl SubscriptionHandle {
    /// Block until an event with exactly `revision` arrives; skip older ones.
    pub fn rev(&self, revision: RevisionId) -> Result<RuntimeEvent, RuntimeError> {
        loop {
            match self.rx.recv() {
                Ok(event) if event.revision == revision => return Ok(event),
                Ok(_) => continue,
                Err(_) => return Err(RuntimeError::ChannelClosed),
            }
        }
    }

    /// Like [`rev`] but also downcasts the event payload to `T`.
    pub fn rev_as<T: Clone + 'static>(&self, revision: RevisionId) -> Result<T, RuntimeError> {
        let event = self.rev(revision)?;
        event
            .payload
            .downcast_ref::<T>()
            .cloned()
            .ok_or_else(|| RuntimeError::InvalidRequest {
                message: "subscription event payload type mismatch".to_string(),
            })
    }
}

// ─── Public handle ────────────────────────────────────────────────────────────

/// Handle to the running GED loop.  Cheaply cloneable via `Arc`.
#[derive(Clone)]
pub struct GlobalEventDispatcher {
    /// Envelope channel shared with all [`Interface`] impls.
    pub(super) envelope_tx: channel::Sender<RuntimeEnvelope>,
    /// Internal channel for Subscribe requests.
    pub(super) sub_tx: channel::Sender<SubCommand>,
}

impl GlobalEventDispatcher {
    /// Spin up the GED loop and return a handle to it.
    pub fn start(
        compiler: ComposedCompiler,
        evt_rx: channel::Receiver<RuntimeEvent>,
    ) -> (Self, thread::JoinHandle<()>) {
        let (envelope_tx, envelope_rx) = channel::bounded::<RuntimeEnvelope>(1024);
        let (sub_tx, sub_rx) = channel::unbounded::<SubCommand>();

        let handle = thread::spawn(move || {
            ged_loop(envelope_rx, sub_rx, evt_rx, compiler);
        });

        (
            Self {
                envelope_tx,
                sub_tx,
            },
            handle,
        )
    }

    /// Return a clone of the envelope sender for direct use in [`Interface`] impls.
    pub fn envelope_tx(&self) -> channel::Sender<RuntimeEnvelope> {
        self.envelope_tx.clone()
    }

    /// Subscribe to pipeline events, optionally filtered to a specific layer path.
    /// Returns a [`SubscriptionHandle`] with ergonomic helpers for awaiting revisions.
    pub fn subscribe(&self, layer_path: Option<RuntimePath>) -> SubscriptionHandle {
        let (tx, rx) = channel::unbounded();
        let _ = self.sub_tx.send(SubCommand::Subscribe {
            layer_path,
            sender: tx,
        });
        SubscriptionHandle { rx }
    }
}

// ─── GED loop ─────────────────────────────────────────────────────────────────

fn ged_loop(
    envelope_rx: channel::Receiver<RuntimeEnvelope>,
    sub_rx: channel::Receiver<SubCommand>,
    evt_rx: channel::Receiver<RuntimeEvent>,
    mut compiler: ComposedCompiler,
) {
    let settled_layer_path = compiler
        .settled_layer_path()
        .cloned()
        .unwrap_or_else(RuntimePath::root);
    let settled_pass_path = compiler
        .settled_pass_path()
        .cloned()
        .unwrap_or_else(RuntimePath::root);

    let mut settled_revision: RevisionId = 0;
    let mut pending_queries: Vec<PendingQuery> = Vec::new();
    let mut subscribers: Vec<Subscriber> = Vec::new();

    loop {
        crossbeam::select! {
            // ── Interface requests ──────────────────────────────────────────
            recv(envelope_rx) -> msg => {
                match msg {
                    Err(_) => break,
                    Ok(RuntimeEnvelope { request, reply }) => {
                        if !handle_request(
                            request,
                            reply,
                            &mut compiler,
                            &mut pending_queries,
                            settled_revision,
                        ) {
                            fail_pending(&mut pending_queries, RuntimeError::ChannelClosed);
                            break;
                        }
                    }
                }
            }

            // ── Subscription requests ───────────────────────────────────────
            recv(sub_rx) -> msg => {
                if let Ok(SubCommand::Subscribe { layer_path, sender }) = msg {
                    subscribers.push(Subscriber { layer_path, sender });
                }
            }

            // ── Pipeline events ─────────────────────────────────────────────
            recv(evt_rx) -> msg => {
                match msg {
                    Err(_) => {
                        fail_pending(&mut pending_queries, RuntimeError::ChannelClosed);
                        break;
                    }
                    Ok(event) => {
                        handle_event(
                            event,
                            &mut subscribers,
                            &mut pending_queries,
                            &mut settled_revision,
                            &mut compiler,
                            &settled_layer_path,
                            &settled_pass_path,
                        );
                    }
                }
            }
        }
    }
}

// ─── Request handling ─────────────────────────────────────────────────────────

/// Returns `false` if the loop should stop (Shutdown received).
fn handle_request(
    request: RuntimeRequest,
    reply: channel::Sender<RuntimeResult>,
    compiler: &mut ComposedCompiler,
    pending_queries: &mut Vec<PendingQuery>,
    settled_revision: RevisionId,
) -> bool {
    match request {
        // EditSource — submit and reply immediately with Accepted.
        RuntimeRequest::ApplyTextEdit { span, text } => {
            let txn = edit_to_txn(span, text);
            let result = compiler
                .submit_source(txn)
                .map(|revision| RuntimeSignal::Accepted { revision });
            let _ = reply.send(result);
            true
        }

        // Query — execute now or park until the target revision is settled.
        RuntimeRequest::QueryLayer {
            layer_path,
            revision,
            index,
        } => {
            if revision.is_some_and(|target| target > settled_revision) {
                pending_queries.push(PendingQuery {
                    revision: revision.unwrap(),
                    layer_path,
                    index,
                    reply,
                });
            } else {
                let _ = reply.send(execute_query(compiler, layer_path, index));
            }
            true
        }

        // Shutdown — stop everything.
        RuntimeRequest::Shutdown => {
            compiler.shutdown();
            let _ = reply.send(Ok(RuntimeSignal::Ack));
            false
        }
    }
}

// ─── Event handling ───────────────────────────────────────────────────────────

fn handle_event(
    event: RuntimeEvent,
    subscribers: &mut Vec<Subscriber>,
    pending_queries: &mut Vec<PendingQuery>,
    settled_revision: &mut RevisionId,
    compiler: &mut ComposedCompiler,
    settled_layer_path: &RuntimePath,
    settled_pass_path: &RuntimePath,
) {
    // Advance the settled revision when the event belongs to the settled layer/pass.
    if &event.layer_path == settled_layer_path || &event.pass_path == settled_pass_path {
        *settled_revision = (*settled_revision).max(event.revision);
        flush_pending_queries(*settled_revision, pending_queries, compiler);
    }

    // Fan-out to subscribers (drop disconnected ones).
    subscribers.retain(|sub| {
        let matches = sub
            .layer_path
            .as_ref()
            .map_or(true, |lp| lp == &event.layer_path);
        if matches {
            sub.sender.send(event.clone()).is_ok()
        } else {
            true // keep subscriber but don't send this event
        }
    });
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn execute_query(
    compiler: &mut ComposedCompiler,
    layer_path: RuntimePath,
    index: Payload,
) -> RuntimeResult {
    compiler
        .query(layer_path.clone(), index)
        .map(|value| RuntimeSignal::QueryResult { layer_path, value })
}

fn flush_pending_queries(
    settled_revision: RevisionId,
    pending_queries: &mut Vec<PendingQuery>,
    compiler: &mut ComposedCompiler,
) {
    let mut rest = Vec::with_capacity(pending_queries.len());
    for pq in pending_queries.drain(..) {
        if pq.revision > settled_revision {
            rest.push(pq);
        } else {
            let _ = pq
                .reply
                .send(execute_query(compiler, pq.layer_path, pq.index));
        }
    }
    *pending_queries = rest;
}

fn fail_pending(pending_queries: &mut Vec<PendingQuery>, err: RuntimeError) {
    for pq in pending_queries.drain(..) {
        let _ = pq.reply.send(Err(err.clone()));
    }
}

fn edit_to_txn(span: Span, text: String) -> scheme::Transaction<scheme::layers::SourceText> {
    use scheme::Command;
    if span.start == span.end && text.is_empty() {
        return Arc::new(Vec::new());
    }
    if span.start == span.end {
        return Arc::new(vec![
            Command::Create { id: 0, value: text },
            Command::Insert { index: span, id: 0 },
        ]);
    }
    if text.is_empty() {
        return Arc::new(vec![Command::Delete { index: span }]);
    }
    Arc::new(vec![
        Command::Create { id: 0, value: text },
        Command::Replace { index: span, id: 0 },
    ])
}
