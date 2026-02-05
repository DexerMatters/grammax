use std::{
    ops,
    sync::mpsc,
    thread::{self, JoinHandle},
};

use crate::{
    grammar::Grammar,
    impl_listener,
    parsec::{self, Parser, ParserConfig, ParserListener, msg::ParserMessages, tree::RedNode},
    runtime::reparser::Reparser,
    utils::Span,
};

mod reparser;

pub use reparser::ReparserConfig;

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

#[derive(Default)]
pub struct RuntimeListener {
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

pub struct Interactive {
    grammar: Grammar,
    config: Option<ParserConfig>,
    reparser_config: Option<ReparserConfig>,
    runtime_listener: Option<RuntimeListener>,
    parser_listener: Option<ParserListener>,
}

impl Interactive {
    pub fn new(grammar: Grammar) -> Interactive {
        Interactive {
            grammar,
            config: None,
            reparser_config: None,
            runtime_listener: None,
            parser_listener: None,
        }
    }

    pub fn with_config(mut self, reparser_config: ReparserConfig) -> Self {
        self.reparser_config = Some(reparser_config);
        self
    }

    pub fn with_listener(mut self, listener: RuntimeListener) -> Self {
        self.runtime_listener = Some(listener);
        self
    }

    pub fn with_parser_config(mut self, config: ParserConfig) -> Self {
        self.config = Some(config);
        self
    }

    pub fn with_parser_listener(mut self, listener: ParserListener) -> Self {
        self.parser_listener = Some(listener);
        self
    }

    pub fn finish(self) -> InteractiveInstance {
        InteractiveInstance::init(
            self.grammar,
            self.config.unwrap_or_else(ParserConfig::default),
            self.reparser_config.unwrap_or_else(ReparserConfig::default),
            self.runtime_listener.unwrap_or_default(),
            self.parser_listener.unwrap_or_default(),
        )
    }
}

pub struct InteractiveInstance {
    sender: mpsc::Sender<Action>,
    thread_handle: JoinHandle<()>,
}

impl InteractiveInstance {
    pub(crate) fn init(
        grammar: Grammar,
        config: ParserConfig,
        reparser_config: ReparserConfig,
        runtime_listener: RuntimeListener,
        parser_listener: ParserListener,
    ) -> Self {
        let (sender, inthread_receiver) = mpsc::channel();
        let thread_handle = thread::spawn(move || {
            let mut parser = Parser::new(grammar)
                .with_config(config)
                .with_listener(parser_listener);
            let alloc = parser.alloc.clone();
            let parsec::Result { root: cursor, .. } = parser.parse_text("");

            Runtime {
                text: "".to_string(),
                parser,
                cursor: Reparser::new(cursor, alloc).with_config(reparser_config),
                receiver: inthread_receiver,
                mode: Mode::Ready,
                runtime_listener,
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
    runtime_listener: RuntimeListener,
    cursor: Reparser,
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
                    self.runtime_listener
                        .on_interrupted
                        .as_ref()
                        .map(|listener| {
                            (listener)();
                        });
                    break;
                }
                _ => { /* Ignored */ }
            }
        }
        if self.mode == Mode::Interrupted {
            self.runtime_listener
                .on_interrupted
                .as_ref()
                .map(|listener| {
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
                    self.runtime_listener.on_paused.as_ref().map(|listener| {
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
                    self.runtime_listener.on_updated.as_ref().map(|listener| {
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
                    self.runtime_listener.on_updated.as_ref().map(|listener| {
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
                    self.runtime_listener.on_updated.as_ref().map(|listener| {
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
                    self.runtime_listener
                        .on_interrupted
                        .as_ref()
                        .map(|listener| {
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
                    self.runtime_listener.on_resumed.as_ref().map(|listener| {
                        (listener)();
                    });
                    break;
                }
                Action::Exit => {
                    self.mode = Mode::Interrupted;
                    self.runtime_listener
                        .on_interrupted
                        .as_ref()
                        .map(|listener| {
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

impl_listener!(
    RuntimeListener,
    on_updated(UpdateResult),
    on_paused(),
    on_resumed(),
    on_interrupted()
);
