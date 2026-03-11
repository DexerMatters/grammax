use std::{
    collections::HashMap,
    ops::Deref,
    sync::{Arc, Mutex},
    thread,
};

use crossbeam::channel;

use crate::{
    grammar::Grammar,
    interface::{BasicInterface, Interface},
    scheme::{self, layers::SourceText},
    utils::Span,
};

use super::{
    compiler::ComposedCompiler,
    protocol::{
        RuntimeEnvelope, RuntimeError, RuntimeEvent, RuntimeRequest, RuntimeResult,
        RuntimeSelector, RuntimeSignal,
    },
};

type PendingReplies = HashMap<u64, (RuntimeSelector, channel::Sender<RuntimeResult>)>;

#[derive(Clone)]
struct Subscriber {
    selector: RuntimeSelector,
    sender: channel::Sender<RuntimeSignal>,
}

enum LoopControl {
    Continue,
    Break,
}

pub struct RuntimeService<Impl = BasicInterface> {
    sender: channel::Sender<RuntimeEnvelope>,
    subscribers: Arc<Mutex<Vec<Subscriber>>>,
    api: Impl,
    _handle: thread::JoinHandle<()>,
}

impl<Impl: Interface> RuntimeService<Impl> {
    pub fn new<F>(grammar: &'static Grammar, f: F) -> Self
    where
        F: FnOnce(Option<channel::Sender<RuntimeEvent>>) -> ComposedCompiler + Send + 'static,
    {
        let (req_tx, req_rx) = channel::bounded::<RuntimeEnvelope>(1024);
        let (evt_tx, evt_rx) = channel::unbounded::<RuntimeEvent>();
        let subscribers = Arc::new(Mutex::new(Vec::<Subscriber>::new()));

        let handle = {
            let subscribers = Arc::clone(&subscribers);
            thread::spawn(move || {
                let compiler = f(Some(evt_tx));
                run_runtime_loop(req_rx, evt_rx, subscribers, compiler);
            })
        };

        let api = Impl::new(req_tx.clone(), grammar);

        Self {
            sender: req_tx,
            subscribers,
            api,
            _handle: handle,
        }
    }

    pub fn sender(&self) -> &channel::Sender<RuntimeEnvelope> {
        &self.sender
    }

    pub fn subscribe(&self, selector: RuntimeSelector) -> channel::Receiver<RuntimeSignal> {
        let (tx, rx) = channel::unbounded();
        if let Ok(mut outs) = self.subscribers.lock() {
            outs.push(Subscriber {
                selector,
                sender: tx,
            });
        }
        rx
    }

    pub fn request(&self, request: RuntimeRequest) -> RuntimeResult {
        let (reply_tx, reply_rx) = channel::bounded(1);
        self.sender
            .send(RuntimeEnvelope {
                request,
                reply: reply_tx,
            })
            .map_err(|_| RuntimeError::ChannelClosed)?;
        reply_rx.recv().map_err(|_| RuntimeError::ChannelClosed)?
    }
}

impl<Impl> Deref for RuntimeService<Impl> {
    type Target = Impl;

    fn deref(&self) -> &Self::Target {
        &self.api
    }
}

fn run_runtime_loop(
    req_rx: channel::Receiver<RuntimeEnvelope>,
    evt_rx: channel::Receiver<RuntimeEvent>,
    subscribers: Arc<Mutex<Vec<Subscriber>>>,
    mut compiler: ComposedCompiler,
) {
    let settled_layer_path = compiler
        .settled_layer_path()
        .cloned()
        .unwrap_or_else(super::protocol::RuntimePath::root);
    let settled_pass_path = compiler
        .settled_pass_path()
        .cloned()
        .unwrap_or_else(super::protocol::RuntimePath::root);

    let mut pending = PendingReplies::new();

    loop {
        crossbeam::select! {
            recv(req_rx) -> msg => {
                if matches!(
                    handle_request_message(msg, &mut compiler, &mut pending),
                    LoopControl::Break
                ) {
                    break;
                }
            }
            recv(evt_rx) -> evt => {
                if matches!(
                    handle_event_message(
                        evt,
                        &subscribers,
                        &mut pending,
                        &settled_layer_path,
                        &settled_pass_path,
                    ),
                    LoopControl::Break
                ) {
                    break;
                }
            }
        }
    }
}

fn handle_request_message(
    msg: Result<RuntimeEnvelope, channel::RecvError>,
    compiler: &mut ComposedCompiler,
    pending: &mut PendingReplies,
) -> LoopControl {
    let Ok(RuntimeEnvelope { request, reply }) = msg else {
        compiler.shutdown();
        fail_pending(pending, RuntimeError::ChannelClosed);
        return LoopControl::Break;
    };

    handle_request(request, reply, compiler, pending)
}

fn handle_request(
    request: RuntimeRequest,
    reply: channel::Sender<RuntimeResult>,
    compiler: &mut ComposedCompiler,
    pending: &mut PendingReplies,
) -> LoopControl {
    match request {
        RuntimeRequest::ApplyTextEdit { span, text } => {
            let txn = edit_to_txn(span, text);
            finish_submit(compiler.submit_source(txn), reply, pending);
            LoopControl::Continue
        }
        RuntimeRequest::QueryLayer { layer_path, index } => {
            let response = compiler
                .query(layer_path.clone(), index)
                .map(|value| RuntimeSignal::QueryResult { layer_path, value });
            let _ = reply.send(response);
            LoopControl::Continue
        }
        RuntimeRequest::Shutdown => {
            compiler.shutdown();
            let _ = reply.send(Ok(RuntimeSignal::Ack));
            fail_pending(pending, RuntimeError::ChannelClosed);
            LoopControl::Break
        }
    }
}

fn finish_submit(
    result: RuntimeResult<u64>,
    reply: channel::Sender<RuntimeResult>,
    pending: &mut PendingReplies,
) {
    match result {
        Ok(revision) => {
            pending.insert(
                revision,
                (RuntimeSelector::revision(revision), reply),
            );
        }
        Err(err) => {
            let _ = reply.send(Err(err));
        }
    }
}

fn handle_event_message(
    evt: Result<RuntimeEvent, channel::RecvError>,
    subscribers: &Arc<Mutex<Vec<Subscriber>>>,
    pending: &mut PendingReplies,
    settled_layer_path: &super::protocol::RuntimePath,
    settled_pass_path: &super::protocol::RuntimePath,
) -> LoopControl {
    let Ok(event) = evt else {
        fail_pending(pending, RuntimeError::ChannelClosed);
        return LoopControl::Break;
    };

    let signal = RuntimeSignal::Event {
        event: event.clone(),
    };
    broadcast_signal(subscribers, &signal, settled_layer_path, settled_pass_path);
    resolve_pending_signal(signal, pending, settled_layer_path, settled_pass_path);
    LoopControl::Continue
}

fn broadcast_signal(
    subscribers: &Arc<Mutex<Vec<Subscriber>>>,
    signal: &RuntimeSignal,
    settled_layer_path: &super::protocol::RuntimePath,
    settled_pass_path: &super::protocol::RuntimePath,
) {
    if let Ok(mut outs) = subscribers.lock() {
        outs.retain(|sub| {
            if !signal_matches(&sub.selector, signal, settled_layer_path, settled_pass_path) {
                return true;
            }
            sub.sender.send(signal.clone()).is_ok()
        });
    }
}

fn resolve_pending_signal(
    signal: RuntimeSignal,
    pending: &mut PendingReplies,
    settled_layer_path: &super::protocol::RuntimePath,
    settled_pass_path: &super::protocol::RuntimePath,
) {
    let Some(revision) = signal.revision() else {
        return;
    };

    let Some((selector, reply)) = pending.remove(&revision) else {
        return;
    };

    if let Some(message) = signal_error_message(&signal) {
        let _ = reply.send(Err(RuntimeError::InvalidRequest { message }));
        return;
    }

    if signal_matches(&selector, &signal, settled_layer_path, settled_pass_path) {
        let _ = reply.send(Ok(signal));
        return;
    }

    pending.insert(revision, (selector, reply));
}

fn signal_matches(
    selector: &RuntimeSelector,
    signal: &RuntimeSignal,
    settled_layer_path: &super::protocol::RuntimePath,
    settled_pass_path: &super::protocol::RuntimePath,
) -> bool {
    if let Some(kind) = selector.kind {
        if signal.kind() != kind {
            return false;
        }
    }

    if let Some(revision) = selector.revision {
        if signal.revision() != Some(revision) {
            return false;
        }
    }

    signal
        .event()
        .map(|event| {
            &event.layer_path == settled_layer_path || &event.pass_path == settled_pass_path
        })
        .unwrap_or(false)
}

fn signal_error_message(signal: &RuntimeSignal) -> Option<String> {
    let event = signal.event()?;
    if !event.is_error {
        return None;
    }

    event
        .payload
        .to_json()
        .get("message")
        .and_then(|message| message.as_str())
        .map(ToString::to_string)
        .or_else(|| Some("runtime pipeline error".to_string()))
}

fn fail_pending(pending: &mut PendingReplies, err: RuntimeError) {
    for (_, (_, reply)) in pending.drain() {
        let _ = reply.send(Err(err.clone()));
    }
}

fn edit_to_txn(span: Span, text: String) -> scheme::Transaction<SourceText> {
    if span.start == span.end && text.is_empty() {
        return std::sync::Arc::new(Vec::new());
    }
    if span.start == span.end {
        return std::sync::Arc::new(vec![
            scheme::Command::Create { id: 0, value: text },
            scheme::Command::Insert { index: span, id: 0 },
        ]);
    }
    if text.is_empty() {
        return std::sync::Arc::new(vec![scheme::Command::Delete { index: span }]);
    }
    std::sync::Arc::new(vec![
        scheme::Command::Create { id: 0, value: text },
        scheme::Command::Replace { index: span, id: 0 },
    ])
}
