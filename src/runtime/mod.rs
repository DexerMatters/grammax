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

mod metrics;
mod reparser;
mod strategy;

pub use metrics::EditMetrics;
pub use reparser::ReparserConfig;

#[cfg(test)]
mod tests;

#[derive(Debug, Clone)]
pub enum Action {
    Insert { offset: usize, text: String },
    Delete { span: Span },
    Update { span: Span, text: String },

    // Control actions
    Run,
    Pause,
    Resume,
    Exit,
}

#[derive(Default)]
pub struct RuntimeListener {
    before_update: Option<Box<dyn Fn() + Send>>,
    after_update: Option<Box<dyn Fn(UpdateResult, std::time::Duration) + Send>>,
    on_pause: Option<Box<dyn Fn() + Send>>,
    on_resume: Option<Box<dyn Fn() + Send>>,
    on_interrupt: Option<Box<dyn Fn() + Send>>,
}

pub struct UpdateResult<'a> {
    pub messages: ParserMessages,
    pub current_tree: &'a RedNode,
    pub reparsed_tree: &'a RedNode,
    pub current_parser: &'a Parser,
    pub source_text: &'a str,
    pub newly_computed_nodes: Vec<Span>,
    pub newly_computed_tokens: Vec<Span>,
    pub semantic_commands: Vec<crate::semantic::Command>,
    pub metrics: EditMetrics,
}

impl<'a> UpdateResult<'a> {
    pub fn new(
        messages: ParserMessages,
        current_tree: &'a RedNode,
        reparsed_tree: &'a RedNode,
        current_parser: &'a Parser,
        source_text: &'a str,
        newly_computed_nodes: Vec<Span>,
        newly_computed_tokens: Vec<Span>,
        semantic_commands: Vec<crate::semantic::Command>,
        metrics: EditMetrics,
    ) -> Self {
        Self {
            messages,
            current_tree,
            reparsed_tree,
            current_parser,
            source_text,
            newly_computed_nodes,
            newly_computed_tokens,
            semantic_commands,
            metrics,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub parser: ParserConfig,
    pub reparser: ReparserConfig,
    pub incremental_reuse_enabled: bool,
    pub incremental_reuse_cache_capacity: usize,
    pub incremental_reuse_cache_failures: bool,
}

impl RuntimeConfig {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            parser: ParserConfig::default(),
            reparser: ReparserConfig::default(),
            incremental_reuse_enabled: true,
            incremental_reuse_cache_capacity: 4096,
            incremental_reuse_cache_failures: true,
        }
    }
}

pub struct Interactive {
    grammar: Grammar,
    runtime_config: RuntimeConfig,
    runtime_listener: Option<RuntimeListener>,
    parser_listener: Option<ParserListener>,
}

impl Interactive {
    pub fn new(grammar: Grammar) -> Interactive {
        Interactive {
            grammar,
            runtime_config: RuntimeConfig::default(),
            runtime_listener: None,
            parser_listener: None,
        }
    }

    pub fn with_config(mut self, runtime_config: RuntimeConfig) -> Self {
        self.runtime_config = runtime_config;
        self
    }

    pub fn with_reparser_config(mut self, reparser_config: ReparserConfig) -> Self {
        self.runtime_config.reparser = reparser_config;
        self
    }

    pub fn with_listener(mut self, listener: RuntimeListener) -> Self {
        self.runtime_listener = Some(listener);
        self
    }

    pub fn with_parser_config(mut self, config: ParserConfig) -> Self {
        self.runtime_config.parser = config;
        self
    }

    pub fn with_incremental_reuse(
        mut self,
        enabled: bool,
        cache_capacity: usize,
        cache_failures: bool,
    ) -> Self {
        self.runtime_config.incremental_reuse_enabled = enabled;
        self.runtime_config.incremental_reuse_cache_capacity = cache_capacity.max(1);
        self.runtime_config.incremental_reuse_cache_failures = cache_failures;
        self
    }

    pub fn with_parser_listener(mut self, listener: ParserListener) -> Self {
        self.parser_listener = Some(listener);
        self
    }

    pub fn finish(self) -> InteractiveInstance {
        InteractiveInstance::init(
            self.grammar,
            self.runtime_config,
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
        runtime_config: RuntimeConfig,
        runtime_listener: RuntimeListener,
        parser_listener: ParserListener,
    ) -> Self {
        let (sender, inthread_receiver) = mpsc::channel();
        let thread_handle = thread::spawn(move || {
            let mut parser = Parser::new(grammar)
                .with_config(runtime_config.parser)
                .with_listener(parser_listener);
            parser.configure_reuse(
                runtime_config.incremental_reuse_enabled,
                runtime_config.incremental_reuse_cache_capacity,
                runtime_config.incremental_reuse_cache_failures,
            );
            let alloc = parser.alloc.clone();
            let parsec::Result { root: cursor, .. } = parser.parse_text("");

            Runtime {
                text: "".to_string(),
                parser,
                cursor: Reparser::new(cursor, alloc).with_config(runtime_config.reparser),
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

    pub fn update(
        &self,
        start: usize,
        end: usize,
        text: &str,
    ) -> Result<(), mpsc::SendError<Action>> {
        self.sender.send(Action::Update {
            span: Span::new(start, end),
            text: text.to_string(),
        })
    }

    pub fn insert(&self, offset: usize, text: &str) -> Result<(), mpsc::SendError<Action>> {
        self.sender.send(Action::Insert {
            offset,
            text: text.to_string(),
        })
    }

    pub fn delete(&self, start: usize, end: usize) -> Result<(), mpsc::SendError<Action>> {
        self.sender.send(Action::Delete {
            span: Span::new(start, end),
        })
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
                    self.runtime_listener.on_interrupt.as_ref().map(|listener| {
                        (listener)();
                    });
                    break;
                }
                _ => { /* Ignored */ }
            }
        }
        if self.mode == Mode::Interrupted {
            self.runtime_listener.on_interrupt.as_ref().map(|listener| {
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
                    self.runtime_listener.on_pause.as_ref().map(|listener| {
                        (listener)();
                    });
                    break;
                }
                Action::Insert { offset, text } => {
                    self.runtime_listener
                        .before_update
                        .as_ref()
                        .map(|listener| {
                            (listener)();
                        });
                    let start = std::time::Instant::now();
                    self.text.insert_str(offset, &text);
                    let span = Span::new(offset, offset);
                    let mut metrics = EditMetrics::new();
                    let result = self.cursor.handle_edit(
                        &mut self.parser,
                        span,
                        text.len(),
                        &self.text,
                        Some(&mut metrics),
                    );
                    let duration = start.elapsed();
                    self.runtime_listener.after_update.as_ref().map(|listener| {
                        (listener)(
                            UpdateResult::new(
                                result.messages,
                                &self.cursor.current,
                                &result.reparsed_tree,
                                &self.parser,
                                &self.text,
                                result.newly_computed_nodes,
                                result.newly_computed_tokens,
                                result.semantic_commands,
                                metrics,
                            ),
                            duration,
                        );
                    });
                }
                Action::Delete { span } => {
                    self.runtime_listener
                        .before_update
                        .as_ref()
                        .map(|listener| {
                            (listener)();
                        });
                    let start = std::time::Instant::now();
                    self.text.replace_range(ops::Range::from(span), "");
                    let mut metrics = EditMetrics::new();
                    let result = self.cursor.handle_edit(
                        &mut self.parser,
                        span,
                        0,
                        &self.text,
                        Some(&mut metrics),
                    );
                    let duration = start.elapsed();
                    self.runtime_listener.after_update.as_ref().map(|listener| {
                        (listener)(
                            UpdateResult::new(
                                result.messages,
                                &self.cursor.current,
                                &result.reparsed_tree,
                                &self.parser,
                                &self.text,
                                result.newly_computed_nodes,
                                result.newly_computed_tokens,
                                result.semantic_commands,
                                metrics,
                            ),
                            duration,
                        );
                    });
                }
                Action::Update { span, text } => {
                    self.runtime_listener
                        .before_update
                        .as_ref()
                        .map(|listener| {
                            (listener)();
                        });
                    let start = std::time::Instant::now();
                    let new_len = text.len();
                    self.text.replace_range(ops::Range::from(span), &text);
                    let mut metrics = EditMetrics::new();
                    let result = self.cursor.handle_edit(
                        &mut self.parser,
                        span,
                        new_len,
                        &self.text,
                        Some(&mut metrics),
                    );
                    let duration = start.elapsed();
                    self.runtime_listener.after_update.as_ref().map(|listener| {
                        (listener)(
                            UpdateResult::new(
                                result.messages,
                                &self.cursor.current,
                                &result.reparsed_tree,
                                &self.parser,
                                &self.text,
                                result.newly_computed_nodes,
                                result.newly_computed_tokens,
                                result.semantic_commands,
                                metrics,
                            ),
                            duration,
                        );
                    });
                }
                Action::Exit => {
                    self.mode = Mode::Interrupted;
                    self.runtime_listener.on_interrupt.as_ref().map(|listener| {
                        (listener)();
                    });
                    break;
                }
                Action::Run => { /* Already running */ }
                Action::Resume => { /* Already running */ }
            }
        }
        if self.mode == Mode::Paused {
            self.run_paused_mode();
        }
    }

    fn run_paused_mode(&mut self) {
        while let Ok(action) = self.receiver.recv() {
            match action {
                Action::Resume => {
                    self.mode = Mode::Running;
                    self.runtime_listener.on_resume.as_ref().map(|listener| {
                        (listener)();
                    });
                    break;
                }
                Action::Exit => {
                    self.mode = Mode::Interrupted;
                    self.runtime_listener.on_interrupt.as_ref().map(|listener| {
                        (listener)();
                    });
                    return;
                }
                _ => { /* Ignored */ }
            }
        }
        if self.mode == Mode::Running {
            self.run_running_mode();
        }
    }
}

impl_listener!(
    RuntimeListener,
    before_update(),
    after_update(UpdateResult, std::time::Duration),
    on_pause(),
    on_resume(),
    on_interrupt()
);
