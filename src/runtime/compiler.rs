use std::{
    fmt,
    marker::PhantomData,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread,
};

use crossbeam::channel;
use serde_json::Value;

use super::protocol::RuntimeEvent;
use crate::{
    grammar::Grammar,
    scheme::{
        self, IR, IRInstance, Pass, Pipeline,
        layers::{AstArena, RedGreenTreeIR, SourceText},
        passes::{AstMapper, IncrementalLowerer, ParserPass},
    },
};

pub struct Compiler<T, M>
where
    T: fmt::Debug + Clone + PartialEq + Send + 'static,
    M: AstMapper<T> + Send + 'static,
{
    text_sender: channel::Sender<(u64, scheme::Transaction<SourceText>)>,
    next_revision: Arc<AtomicU64>,
    stage_a_handle: thread::JoinHandle<()>,
    bridge_b_handle: thread::JoinHandle<()>,
    stage_b: Pipeline<RedGreenTreeIR, IncrementalLowerer<T, M>, AstArena<T>>,
    source_text: Arc<Mutex<String>>,
    _phantom: PhantomData<M>,
}

impl<T, M> Compiler<T, M>
where
    T: fmt::Debug + Clone + PartialEq + Send + 'static,
    M: AstMapper<T> + Send + 'static,
{
    pub fn new(grammar: &'static Grammar, mapper: M) -> Self {
        Self::new_with_events(grammar, mapper, None)
    }

    pub fn new_with_events(
        grammar: &'static Grammar,
        mapper: M,
        event_sender: Option<channel::Sender<RuntimeEvent>>,
    ) -> Self {
        let source_text = Arc::new(Mutex::new(String::new()));

        let (ast_tap_tx, ast_tap_rx) = channel::bounded::<scheme::Transaction<AstArena<T>>>(16);
        let stage_b = {
            let source_text = Arc::clone(&source_text);
            Pipeline::connect_with_tap(
                RedGreenTreeIR::default,
                move || IncrementalLowerer::new(grammar, mapper).with_source_cell(source_text),
                AstArena::<T>::default(),
                Some(ast_tap_tx),
            )
        };

        let (text_tx, text_rx) = channel::bounded::<(u64, scheme::Transaction<SourceText>)>(16);
        let (rev_b_tx, rev_b_rx) = channel::bounded::<u64>(16);

        let stage_b_sender = stage_b.clone_sender();
        let stage_a_handle = {
            let source_text = Arc::clone(&source_text);
            let event_sender = event_sender.clone();
            thread::spawn(move || {
                let mut parser_pass = ParserPass::new(grammar);
                let mut source_ir = SourceText::default();

                while let Ok((revision, txn)) = text_rx.recv() {
                    let for_pass = scheme::clone_transaction::<SourceText>(&txn);
                    if source_ir.apply_transaction(txn).is_err() {
                        continue;
                    }

                    if let Ok(mut text) = source_text.lock() {
                        *text = source_ir.text.clone();
                    }

                    let tree_txn = parser_pass
                        .transform(&source_ir, for_pass)
                        .expect("infallible");

                    if let Some(tx) = event_sender.as_ref() {
                        let _ = tx.send(RuntimeEvent {
                            revision,
                            milestone: "ir2.delta".to_string(),
                            payload: serde_json::to_value(scheme::clone_transaction::<
                                RedGreenTreeIR,
                            >(&tree_txn))
                            .unwrap_or(Value::Null),
                        });
                    }

                    let _ = stage_b_sender.send(tree_txn);
                    let _ = rev_b_tx.send(revision);
                }
            })
        };

        let bridge_b_handle = {
            let event_sender = event_sender.clone();
            thread::spawn(move || {
                while ast_tap_rx.recv().is_ok() {
                    let Ok(revision) = rev_b_rx.recv() else { break };
                    if let Some(tx) = event_sender.as_ref() {
                        let _ = tx.send(RuntimeEvent {
                            revision,
                            milestone: "ir3.done".to_string(),
                            payload: Value::Null,
                        });
                    }
                }
            })
        };

        Self {
            text_sender: text_tx,
            next_revision: Arc::new(AtomicU64::new(1)),
            stage_a_handle,
            bridge_b_handle,
            stage_b,
            source_text,
            _phantom: PhantomData,
        }
    }

    pub fn edit(&self, txn: scheme::Transaction<SourceText>) {
        let _ = self.submit(txn);
    }

    pub fn submit(&self, txn: scheme::Transaction<SourceText>) -> u64 {
        let revision = self.next_revision.fetch_add(1, Ordering::Relaxed);
        let _ = self.text_sender.send((revision, txn));
        revision
    }

    pub fn ast(&self) -> &IRInstance<AstArena<T>> {
        &self.stage_b.downstream
    }

    pub fn source_text(&self) -> String {
        self.source_text
            .lock()
            .map(|s| s.clone())
            .unwrap_or_default()
    }

    pub fn shutdown(self) {
        drop(self.text_sender);
        let _ = self.stage_a_handle.join();
        self.stage_b.shutdown();
        let _ = self.bridge_b_handle.join();
    }
}

pub fn insert_at(offset: usize, text: impl Into<String>) -> scheme::Transaction<SourceText> {
    let text = text.into();
    let span = crate::utils::Span::new(offset, offset);
    vec![
        scheme::Command::Create { id: 0, value: text },
        scheme::Command::Insert { index: span, id: 0 },
    ]
}

pub fn delete_span(span: crate::utils::Span) -> scheme::Transaction<SourceText> {
    vec![scheme::Command::Delete { index: span }]
}

pub fn replace_span(
    span: crate::utils::Span,
    text: impl Into<String>,
) -> scheme::Transaction<SourceText> {
    let text = text.into();
    vec![
        scheme::Command::Create { id: 0, value: text },
        scheme::Command::Replace { index: span, id: 0 },
    ]
}
