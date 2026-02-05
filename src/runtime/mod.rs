use std::{
    ops,
    sync::{Arc, mpsc},
    thread::{self, JoinHandle},
};

use concurrent_queue::ConcurrentQueue;

use crate::{
    grammar::Grammar,
    parsec::{
        msg::ParserMessages,
        parser::{self, Parser},
        tree::{RedNode, TreeAlloc},
    },
    runtime::reparser::Reparser,
    utils::Span,
};

mod reparser;

#[cfg(test)]
mod tests;

#[derive(Debug, Clone)]
pub enum Action {
    Update { span: Span, text: String },
    Insert { offset: usize, text: String },
    Delete { span: Span },

    // Control actions
    Run,
    Pause,
    Resume,
    Exit,
}

pub struct Listener {
    on_updated: Option<Box<dyn Fn(UpdateResult) + Send>>,
    on_paused: Option<Box<dyn Fn() + Send>>,
    on_resumed: Option<Box<dyn Fn() + Send>>,
    on_interrupted: Option<Box<dyn Fn() + Send>>,
}

pub struct UpdateResult<'a> {
    pub messages: ParserMessages,
    pub current_tree: &'a RedNode,
    pub reparsed_tree: &'a RedNode,
    pub current_parser: &'a Parser,
    pub source_text: &'a str,
}

impl<'a> UpdateResult<'a> {
    pub fn new(
        messages: ParserMessages,
        current_tree: &'a RedNode,
        reparsed_tree: &'a RedNode,
        current_parser: &'a Parser,
        source_text: &'a str,
    ) -> Self {
        Self {
            messages,
            current_tree,
            reparsed_tree,
            current_parser,
            source_text,
        }
    }
}

impl Listener {
    pub fn new() -> Self {
        Self {
            on_updated: None,
            on_paused: None,
            on_resumed: None,
            on_interrupted: None,
        }
    }
    pub fn with_on_updated<F>(mut self, f: F) -> Self
    where
        F: Fn(UpdateResult) + Send + 'static,
    {
        self.on_updated = Some(Box::new(f));
        self
    }
    pub fn with_on_paused<F>(mut self, f: F) -> Self
    where
        F: Fn() + Send + 'static,
    {
        self.on_paused = Some(Box::new(f));
        self
    }
    pub fn with_on_resumed<F>(mut self, f: F) -> Self
    where
        F: Fn() + Send + 'static,
    {
        self.on_resumed = Some(Box::new(f));
        self
    }
    pub fn with_on_interrupted<F>(mut self, f: F) -> Self
    where
        F: Fn() + Send + 'static,
    {
        self.on_interrupted = Some(Box::new(f));
        self
    }
}

pub struct Interactive {
    sender: mpsc::Sender<Action>,
    thread_handle: JoinHandle<()>,
}

impl Interactive {
    pub fn new(grammar: Grammar, config: parser::ParserConfig) -> Self {
        Self::init(grammar, config, Listener::new())
    }
    pub fn new_with_listener(
        grammar: Grammar,
        config: parser::ParserConfig,
        listener: Listener,
    ) -> Self {
        Self::init(grammar, config, listener)
    }
    fn init(grammar: Grammar, config: parser::ParserConfig, listener: Listener) -> Self {
        let (sender, inthread_receiver) = mpsc::channel();
        let sender_clone = sender.clone();
        let thread_handle = thread::spawn(move || {
            let mut parser = Parser::new_with_config(grammar, config);
            let alloc = parser.alloc.clone();
            let parser::Result {
                root: cursor,
                messages,
            } = parser.parse_text("");

            Runtime {
                text: "".to_string(),
                parser,
                messages,
                cursor: Reparser::new(cursor, alloc),
                self_sender: sender_clone,
                receiver: inthread_receiver,
                mode: Mode::Ready,
                listener,
            }
            .run_ready_mode();
        });

        Self {
            sender,
            thread_handle,
        }
    }

    pub fn update(&self, span: Span, text: String) -> Result<(), mpsc::SendError<Action>> {
        self.sender.send(Action::Update { span, text })
    }

    pub fn insert(&self, offset: usize, text: String) -> Result<(), mpsc::SendError<Action>> {
        self.sender.send(Action::Insert { offset, text })
    }

    pub fn delete(&self, span: Span) -> Result<(), mpsc::SendError<Action>> {
        self.sender.send(Action::Delete { span })
    }

    pub fn pause(&self) -> Result<(), mpsc::SendError<Action>> {
        self.sender.send(Action::Pause)
    }

    pub fn join(self) -> thread::Result<()> {
        self.thread_handle.join()
    }

    pub fn resume(&self) -> Result<(), mpsc::SendError<Action>> {
        self.sender.send(Action::Resume)
    }

    pub fn run(&self) -> Result<(), mpsc::SendError<Action>> {
        self.sender.send(Action::Run)
    }

    pub fn exit(&self) -> Result<(), mpsc::SendError<Action>> {
        self.sender.send(Action::Exit)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Ready,
    Running,
    Paused,
    Interrupted,
}

pub struct Runtime {
    text: String,
    parser: Parser,
    messages: ParserMessages,
    listener: Listener,
    cursor: Reparser,
    self_sender: mpsc::Sender<Action>,
    receiver: mpsc::Receiver<Action>,

    mode: Mode,
}

impl Runtime {
    fn run_ready_mode(&mut self) {
        while let Ok(action) = self.receiver.recv() {
            match action {
                Action::Run => {
                    self.mode = Mode::Running;
                    break;
                }
                Action::Exit => {
                    self.mode = Mode::Interrupted;
                    self.listener.on_interrupted.as_ref().map(|listener| {
                        (listener)();
                    });
                    break;
                }
                _ => { /* Ignored */ }
            }
        }
        if self.mode == Mode::Interrupted {
            self.listener.on_interrupted.as_ref().map(|listener| {
                (listener)();
            });
            return;
        }
        self.run_running_mode();
    }

    fn run_running_mode(&mut self) {
        while let Ok(action) = self.receiver.recv() {
            match action {
                Action::Pause => {
                    self.mode = Mode::Paused;
                    self.listener.on_paused.as_ref().map(|listener| {
                        (listener)();
                    });
                    break;
                }
                Action::Update { span, text } => {
                    let old_len = span.len();
                    let new_len = text.len();
                    self.text.replace_range(ops::Range::from(span), &text);
                    self.parser
                        .apply_edit(&self.text, span.start, old_len, new_len);
                    let result = self.cursor.handle_edit(&mut self.parser, span, text.len());
                    self.listener.on_updated.as_ref().map(|listener| {
                        (listener)(UpdateResult::new(
                            result.messages,
                            &self.cursor.current,
                            &result.reparsed_tree,
                            &self.parser,
                            &self.text,
                        ));
                    });
                }
                Action::Insert { offset, text } => {
                    let new_len = text.len();
                    self.text.insert_str(offset, &text);
                    self.parser.apply_edit(&self.text, offset, 0, new_len);
                    let span = Span::new(offset, offset);
                    let result = self.cursor.handle_edit(&mut self.parser, span, text.len());
                    self.listener.on_updated.as_ref().map(|listener| {
                        (listener)(UpdateResult::new(
                            result.messages,
                            &self.cursor.current,
                            &result.reparsed_tree,
                            &self.parser,
                            &self.text,
                        ));
                    });
                }
                Action::Delete { span } => {
                    let old_len = span.len();
                    self.text.replace_range(ops::Range::from(span), "");
                    self.parser.apply_edit(&self.text, span.start, old_len, 0);
                    let result = self.cursor.handle_edit(&mut self.parser, span, 0);
                    self.listener.on_updated.as_ref().map(|listener| {
                        (listener)(UpdateResult::new(
                            result.messages,
                            &self.cursor.current,
                            &result.reparsed_tree,
                            &self.parser,
                            &self.text,
                        ));
                    });
                }
                Action::Exit => {
                    self.mode = Mode::Interrupted;
                    self.listener.on_interrupted.as_ref().map(|listener| {
                        (listener)();
                    });
                    break;
                }
                Action::Run => { /* Already running */ }
                Action::Resume => { /* Already running */ }
            }
        }
        if self.mode == Mode::Paused {
            return;
        }
        self.run_paused_mode();
    }

    fn run_paused_mode(&mut self) {
        while let Ok(action) = self.receiver.recv() {
            match action {
                Action::Resume => {
                    self.mode = Mode::Running;
                    self.listener.on_resumed.as_ref().map(|listener| {
                        (listener)();
                    });
                    break;
                }
                Action::Exit => {
                    self.mode = Mode::Interrupted;
                    self.listener.on_interrupted.as_ref().map(|listener| {
                        (listener)();
                    });
                    return;
                }
                _ => { /* Ignored */ }
            }
        }
        if self.mode == Mode::Interrupted {
            return;
        }
        self.run_running_mode();
    }
}
