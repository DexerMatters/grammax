use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    thread,
};

use crossbeam::channel;

use crate::{
    grammar::Grammar,
    scheme::{self, layers::SourceText},
    utils::Span,
};

use super::{
    Compiler,
    protocol::{
        CompletionPolicy, RuntimeEnvelope, RuntimeError, RuntimeEvent, RuntimeRequest,
        RuntimeResponse, RuntimeResult,
    },
};

#[derive(Debug, Clone)]
pub struct RuntimeServiceConfig {
    pub settled_milestone: String,
}

impl Default for RuntimeServiceConfig {
    fn default() -> Self {
        Self {
            settled_milestone: "ir3.done".to_string(),
        }
    }
}

pub struct RuntimeService {
    sender: channel::Sender<RuntimeEnvelope>,
    subscribers: Arc<Mutex<Vec<channel::Sender<RuntimeEvent>>>>,
    _handle: thread::JoinHandle<()>,
}

impl RuntimeService {
    pub fn new(grammar: &'static Grammar) -> Self {
        Self::new_with_config(grammar, RuntimeServiceConfig::default())
    }

    pub fn new_with_config(grammar: &'static Grammar, config: RuntimeServiceConfig) -> Self {
        let (req_tx, req_rx) = channel::bounded::<RuntimeEnvelope>(1024);
        let (evt_tx, evt_rx) = channel::unbounded::<RuntimeEvent>();
        let subscribers = Arc::new(Mutex::new(Vec::<channel::Sender<RuntimeEvent>>::new()));

        let handle = {
            let subscribers = Arc::clone(&subscribers);
            thread::spawn(move || {
                let compiler = Compiler::<(), ()>::new_with_events(grammar, (), Some(evt_tx));
                let mut latest_revision = 0u64;
                let mut pending: HashMap<u64, (CompletionPolicy, channel::Sender<RuntimeResult>)> =
                    HashMap::new();

                loop {
                    crossbeam::select! {
                        recv(req_rx) -> msg => match msg {
                            Ok(RuntimeEnvelope { request, reply }) => {
                                match request {
                                    RuntimeRequest::ApplyTextEdit { span, text, completion } => {
                                        let txn = edit_to_txn(span, text);
                                        let revision = compiler.submit(txn);
                                        latest_revision = latest_revision.max(revision);

                                        if matches!(completion, CompletionPolicy::Enqueued) {
                                            let _ = reply.send(Ok(RuntimeResponse::Accepted { revision }));
                                        } else {
                                            pending.insert(revision, (completion, reply));
                                        }
                                    }
                                    RuntimeRequest::ApplySourceTxn { txn, completion } => {
                                        let revision = compiler.submit(txn);
                                        latest_revision = latest_revision.max(revision);

                                        if matches!(completion, CompletionPolicy::Enqueued) {
                                            let _ = reply.send(Ok(RuntimeResponse::Accepted { revision }));
                                        } else {
                                            pending.insert(revision, (completion, reply));
                                        }
                                    }
                                    RuntimeRequest::Shutdown => {
                                        compiler.shutdown();
                                        let _ = reply.send(Ok(RuntimeResponse::Ack));
                                        break;
                                    }
                                }
                            }
                            Err(_) => {
                                compiler.shutdown();
                                break;
                            }
                        },
                        recv(evt_rx) -> evt => match evt {
                            Ok(event) => {
                                latest_revision = latest_revision.max(event.revision);

                                if let Ok(mut outs) = subscribers.lock() {
                                    outs.retain(|tx| tx.send(event.clone()).is_ok());
                                }

                                if let Some((policy, reply)) = pending.remove(&event.revision) {
                                    if completion_matches(&policy, &event, &config.settled_milestone) {
                                        let _ = reply.send(Ok(RuntimeResponse::Completed {
                                            revision: event.revision,
                                            event,
                                        }));
                                    } else {
                                        pending.insert(event.revision, (policy, reply));
                                    }
                                }
                            }
                            Err(_) => break,
                        }
                    }
                }
            })
        };

        Self {
            sender: req_tx,
            subscribers,
            _handle: handle,
        }
    }

    pub fn sender(&self) -> &channel::Sender<RuntimeEnvelope> {
        &self.sender
    }

    pub fn subscribe_events(&self) -> channel::Receiver<RuntimeEvent> {
        let (tx, rx) = channel::unbounded();
        if let Ok(mut outs) = self.subscribers.lock() {
            outs.push(tx);
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

fn completion_matches(
    policy: &CompletionPolicy,
    event: &RuntimeEvent,
    settled_milestone: &str,
) -> bool {
    match policy {
        CompletionPolicy::Enqueued => true,
        CompletionPolicy::Settled => event.milestone == settled_milestone,
        CompletionPolicy::Milestone(name) => &event.milestone == name,
    }
}

fn edit_to_txn(span: Span, text: String) -> scheme::Transaction<SourceText> {
    if span.start == span.end && text.is_empty() {
        return Vec::new();
    }
    if span.start == span.end {
        return vec![
            scheme::Command::Create { id: 0, value: text },
            scheme::Command::Insert { index: span, id: 0 },
        ];
    }
    if text.is_empty() {
        return vec![scheme::Command::Delete { index: span }];
    }
    vec![
        scheme::Command::Create { id: 0, value: text },
        scheme::Command::Replace { index: span, id: 0 },
    ]
}
