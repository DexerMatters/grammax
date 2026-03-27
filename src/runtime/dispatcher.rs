use std::{sync::Arc, thread};

use crossbeam::channel;

use crate::{
    scheme::{self, DocumentSpan, SourceAtom, Span, URI},
    utils::{self},
};

use super::{
    compiler::{ComposedCompiler, TypedTree},
    protocol::{
        RevisionId, RuntimeEnvelope, RuntimeError, RuntimeEvent, RuntimePath, RuntimeRequest,
        RuntimeSignal, RuntimeWireResult,
    },
};

// ─── Internal types ──────────────────────────────────────────────────────────

struct PendingQuery {
    revision: RevisionId,
    layer_path: RuntimePath,
    index: utils::Payload,
    reply: channel::Sender<RuntimeWireResult>,
}

/// Parked when `ApplyAndFetch` is received: waits for the pipeline to fire an
/// event on `layer_path` at exactly `revision`, then replies with `EditResult`.
struct PendingFetch {
    revision: RevisionId,
    layer_path: RuntimePath,
    reply: channel::Sender<RuntimeWireResult>,
}

#[derive(Debug, Clone)]
pub struct GlobalEventDispatcher {
    pub(crate) envelope_tx: channel::Sender<RuntimeEnvelope>,
}

impl GlobalEventDispatcher {
    pub(crate) fn start<Tree: TypedTree + 'static>(
        compiler: ComposedCompiler<Tree>,
        evt_rx: channel::Receiver<RuntimeEvent>,
    ) -> (Self, thread::JoinHandle<()>) {
        let (envelope_tx, envelope_rx) = channel::bounded::<RuntimeEnvelope>(1024);

        let handle = thread::spawn(move || {
            ged_loop(envelope_rx, evt_rx, compiler);
        });

        (Self { envelope_tx }, handle)
    }

    pub(crate) fn envelope_tx(&self) -> channel::Sender<RuntimeEnvelope> {
        self.envelope_tx.clone()
    }
}

// ─── GED loop ─────────────────────────────────────────────────────────────────

fn ged_loop<Tree: TypedTree>(
    envelope_rx: channel::Receiver<RuntimeEnvelope>,
    evt_rx: channel::Receiver<RuntimeEvent>,
    mut compiler: ComposedCompiler<Tree>,
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
    let mut pending_fetches: Vec<PendingFetch> = Vec::new();

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
                            &mut pending_fetches,
                            settled_revision,
                        ) {
                            fail_all(&mut pending_queries, &mut pending_fetches);
                            break;
                        }
                    }
                }
            }

            // ── Pipeline events ─────────────────────────────────────────────
            recv(evt_rx) -> msg => {
                match msg {
                    Err(_) => {
                        fail_all(&mut pending_queries, &mut pending_fetches);
                        break;
                    }
                    Ok(event) => {
                        handle_event(
                            event,
                            &mut pending_queries,
                            &mut pending_fetches,
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
fn handle_request<Tree: TypedTree>(
    request: RuntimeRequest,
    reply: channel::Sender<RuntimeWireResult>,
    compiler: &mut ComposedCompiler<Tree>,
    pending_queries: &mut Vec<PendingQuery>,
    pending_fetches: &mut Vec<PendingFetch>,
    settled_revision: RevisionId,
) -> bool {
    match request {
        // Fire-and-forget edit: reply immediately with Accepted.
        RuntimeRequest::ApplyTextEdit { uri, span, text } => {
            let txn = edit_to_txn(uri, span, text);
            let result = compiler
                .submit_source(txn)
                .map(|revision| RuntimeSignal::Accepted { revision })
                .map_err(to_wire_error);
            let _ = reply.send(result);
            true
        }

        // Edit + wait: park reply until the pipeline settles the target layer.
        RuntimeRequest::ApplyAndFetch {
            uri,
            span,
            text,
            layer_path,
        } => {
            let txn = edit_to_txn(uri, span, text);
            match compiler.submit_source(txn) {
                Ok(revision) => pending_fetches.push(PendingFetch {
                    revision,
                    layer_path,
                    reply,
                }),
                Err(e) => {
                    let _ = reply.send(Err(to_wire_error(e)));
                }
            }
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

fn handle_event<Tree: TypedTree>(
    event: RuntimeEvent,
    pending_queries: &mut Vec<PendingQuery>,
    pending_fetches: &mut Vec<PendingFetch>,
    settled_revision: &mut RevisionId,
    compiler: &mut ComposedCompiler<Tree>,
    settled_layer_path: &RuntimePath,
    settled_pass_path: &RuntimePath,
) {
    let advances = &event.layer_path == settled_layer_path || &event.pass_path == settled_pass_path;

    // Destructure to take ownership of every field without cloning.
    let RuntimeEvent {
        revision,
        layer_path,
        payload,
        ..
    } = event;

    if advances {
        *settled_revision = (*settled_revision).max(revision);
        flush_pending_queries(*settled_revision, pending_queries, compiler);
    }

    // Move payload directly into the waiting fetch reply — zero copies.
    flush_pending_fetches(revision, layer_path, payload, pending_fetches);
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn execute_query<Tree: TypedTree>(
    compiler: &mut ComposedCompiler<Tree>,
    layer_path: RuntimePath,
    index: utils::Payload,
) -> RuntimeWireResult {
    compiler
        .query(layer_path.clone(), index)
        .map(|value| RuntimeSignal::QueryResult { layer_path, value })
}

fn flush_pending_queries<Tree: TypedTree>(
    settled_revision: RevisionId,
    pending_queries: &mut Vec<PendingQuery>,
    compiler: &mut ComposedCompiler<Tree>,
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

fn flush_pending_fetches(
    revision: RevisionId,
    layer_path: RuntimePath,
    payload: utils::Payload,
    pending_fetches: &mut Vec<PendingFetch>,
) {
    let mut payload = Some(payload);
    let mut rest = Vec::with_capacity(pending_fetches.len());
    for pf in pending_fetches.drain(..) {
        if pf.revision == revision && pf.layer_path == layer_path {
            if let Some(value) = payload.take() {
                let _ = pf.reply.send(Ok(RuntimeSignal::EditResult {
                    revision,
                    layer_path: layer_path.clone(),
                    value,
                }));
            }
        } else {
            rest.push(pf);
        }
    }
    *pending_fetches = rest;
}

fn fail_all(pending_queries: &mut Vec<PendingQuery>, pending_fetches: &mut Vec<PendingFetch>) {
    for pq in pending_queries.drain(..) {
        let _ = pq.reply.send(Err(RuntimeError::ChannelClosed));
    }
    for pf in pending_fetches.drain(..) {
        let _ = pf.reply.send(Err(RuntimeError::ChannelClosed));
    }
}

fn to_wire_error(err: RuntimeError) -> RuntimeError<utils::Payload> {
    match err {
        RuntimeError::QueueFull => RuntimeError::QueueFull,
        RuntimeError::ChannelClosed => RuntimeError::ChannelClosed,
        RuntimeError::InvalidQuery => RuntimeError::InvalidQuery,
        RuntimeError::InvalidRequest { message } => RuntimeError::InvalidRequest { message },
        RuntimeError::InvalidRequestFromTarget { err } => RuntimeError::InvalidRequestFromTarget {
            err: utils::Payload::new_serializable(err),
        },
        RuntimeError::UnexpectedRequestType => RuntimeError::UnexpectedRequestType,
        RuntimeError::UndefinedBehavior { message } => RuntimeError::UndefinedBehavior { message },
    }
}

fn edit_to_txn(
    uri: URI,
    span: Span,
    text: String,
) -> scheme::Transaction<scheme::layers::SourceText> {
    use scheme::Command;
    let is_empty = text.is_empty();
    if span.start == span.end && is_empty {
        return Arc::new(Vec::new());
    }
    let doc_span = DocumentSpan { uri, span };
    if span.start == span.end {
        let staged_text: SourceAtom = text.into();
        return Arc::new(vec![
            Command::Create {
                id: 0,
                value: staged_text,
            },
            Command::Insert {
                index: doc_span,
                id: 0,
            },
        ]);
    }
    if is_empty {
        return Arc::new(vec![Command::Delete { index: doc_span }]);
    }
    let staged_text: SourceAtom = text.into();
    Arc::new(vec![
        Command::Create {
            id: 0,
            value: staged_text,
        },
        Command::Replace {
            index: doc_span,
            id: 0,
        },
    ])
}
