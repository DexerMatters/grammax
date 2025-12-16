use std::sync::{
    Arc,
    mpsc::{Receiver, RecvError},
};

use concurrent_queue::ConcurrentQueue;

use crate::{grammar::Grammar, tree::*, utils::Span};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Edit {
    Update { span: Span, new_text: String },
    Insert { position: usize, new_text: String },
    Delete { span: Span },
}

#[derive(Clone)]
pub struct ParserState {
    grammar: Arc<Grammar>,
    arena: Arc<TreeAlloc>,
    ast: Arc<RedNode>,
    text: Arc<parking_lot::RwLock<String>>,
}

pub enum ParserResult {
    Complete(Arc<RedNode>),
    Incomplete(ParserError),
}

impl ParserState {
    pub fn new(grammar: Grammar) -> Self {
        let arena = TreeAlloc::new();
        let placeholder_id = arena.new_placeholder(0);
        Self {
            grammar: Arc::new(grammar),
            arena: Arc::new(arena),
            ast: Arc::new(RedNode {
                parent: None,
                green: placeholder_id,
                offset: 0,
            }),
            text: Arc::new(parking_lot::RwLock::new(String::new())),
        }
    }

    pub fn ast(&self) -> &RedNode {
        &self.ast
    }
}

#[derive(Debug, Clone)]
pub enum ParserError {
    LostConnection(RecvError),
    SpanOutOfBounds { expected: Span, actual: Span },
    PositionOutOfBounds { expected: Span, actual: usize },
}

pub struct Parser {
    state: ParserState,
    receiver: Receiver<Edit>,
    observer: Box<dyn Fn(&ParserState) + Send + Sync>,
}

impl Parser {
    pub fn new(grammar: Grammar, receiver: Receiver<Edit>) -> Self {
        Self {
            state: ParserState::new(grammar),
            receiver,
            observer: Box::new(|_state| {}),
        }
    }

    pub fn set_observer<F>(&mut self, observer: F)
    where
        F: Fn(&ParserState) + Send + Sync + 'static,
    {
        self.observer = Box::new(observer);
    }

    pub fn receive_edits(&self) -> Result<Edit, ParserError> {
        let edit = self.receiver.recv().map_err(ParserError::LostConnection)?;
        let text = &self.state.text;
        match &edit {
            Edit::Update { span, new_text } => {
                self.is_valid_span(*span)?;
                let mut text = text.write();
                text.replace_range(span.start..span.end, new_text);
            }
            Edit::Insert { position, new_text } => {
                self.is_valid_position(*position)?;
                let mut text = text.write();
                text.insert_str(*position, new_text);
            }
            Edit::Delete { span } => {
                self.is_valid_span(*span)?;
                let mut text = text.write();
                text.replace_range(span.start..span.end, "");
            }
        }
        Ok(edit)
    }

    fn is_valid_span(&self, span: Span) -> Result<(), ParserError> {
        let text = self.state.text.read();
        if span.end <= text.len() {
            Ok(())
        } else {
            Err(ParserError::SpanOutOfBounds {
                expected: span,
                actual: Span {
                    start: 0,
                    end: text.len(),
                },
            })
        }
    }

    fn is_valid_position(&self, position: usize) -> Result<(), ParserError> {
        let text = self.state.text.read();
        if position <= text.len() {
            Ok(())
        } else {
            Err(ParserError::PositionOutOfBounds {
                expected: Span {
                    start: 0,
                    end: text.len(),
                },
                actual: position,
            })
        }
    }
}
