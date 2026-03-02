use std::{
    error,
    marker::PhantomData,
    ops::{self, Deref},
    panic::{AssertUnwindSafe, catch_unwind},
    thread::{self, JoinHandle},
};

use crossbeam::channel;
use serde::Serialize;

use crate::{
    grammar::Grammar,
    interface::Interface,
    parsec::{self, Parser, ParserConfig, ParserListener, msg::ParserMessages, tree::RedNode},
    runtime::reparser::{ReparseError, Reparser},
    semantic::{ASTCell, AstArena, AstDelta, AstMapper, Command, IncrementalLowerer},
    utils::Span,
};

mod delta;
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

    GetSource,

    Run,
    Pause,
    Resume,
    Exit,
}

impl<'de> serde::Deserialize<'de> for Action {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        #[serde(tag = "type")]
        #[serde(rename_all = "camelCase")]
        enum ActionHelper {
            Insert {
                offset: usize,
                text: String,
            },
            Delete {
                start: usize,
                end: usize,
            },
            Update {
                start: usize,
                end: usize,
                text: String,
            },
            GetSource,
            Run,
            Pause,
            Resume,
            Exit,
        }

        let helper = ActionHelper::deserialize(deserializer)?;
        Ok(match helper {
            ActionHelper::Insert { offset, text } => Action::Insert { offset, text },
            ActionHelper::Delete { start, end } => Action::Delete {
                span: Span::new(start, end),
            },
            ActionHelper::Update { start, end, text } => Action::Update {
                span: Span::new(start, end),
                text,
            },
            ActionHelper::GetSource => Action::GetSource,
            ActionHelper::Run => Action::Run,
            ActionHelper::Pause => Action::Pause,
            ActionHelper::Resume => Action::Resume,
            ActionHelper::Exit => Action::Exit,
        })
    }
}

impl Action {
    pub fn kind(&self) -> RuntimeAction {
        match self {
            Action::Insert { .. } => RuntimeAction::Insert,
            Action::Delete { .. } => RuntimeAction::Delete,
            Action::Update { .. } => RuntimeAction::Update,
            Action::Run => RuntimeAction::Run,
            Action::Pause => RuntimeAction::Pause,
            Action::Resume => RuntimeAction::Resume,
            Action::Exit => RuntimeAction::Exit,
            Action::GetSource => RuntimeAction::Get,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum RuntimeAction {
    Insert,
    Delete,
    Update,
    Run,
    Pause,
    Resume,
    Exit,
    Get,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum RuntimeMode {
    Ready,
    Running,
    Paused,
    Interrupted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ListenerHook {
    BeforeUpdate,
    AfterUpdate,
    OnPause,
    OnResume,
    OnInterrupt,
    OnError,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub enum RuntimeError {
    QueueFull,
    ChannelClosed,
    WorkerPanicked,
    GeneralError(#[serde(skip_serializing)] Box<dyn error::Error + Send + Sync>),
    InvalidOffset {
        offset: usize,
        text_len: usize,
    },
    InvalidRange {
        start: usize,
        end: usize,
        text_len: usize,
    },
    InvalidMode {
        mode: RuntimeMode,
        action: RuntimeAction,
    },
    NoIncrementalCandidate {
        span: Span,
        delta: isize,
        candidates_collected: usize,
    },
    ListenerPanic {
        hook: ListenerHook,
    },
}

impl From<ReparseError> for RuntimeError {
    fn from(value: ReparseError) -> Self {
        match value {
            ReparseError::NoIncrementalCandidate {
                span,
                delta,
                candidates_collected,
            } => RuntimeError::NoIncrementalCandidate {
                span,
                delta,
                candidates_collected,
            },
        }
    }
}

pub struct RuntimeListener<T = ()> {
    before_update: Option<Box<dyn Fn() + Send>>,
    after_update: Option<Box<dyn Fn(UpdateResult<T>) + Send>>,
    on_pause: Option<Box<dyn Fn() + Send>>,
    on_resume: Option<Box<dyn Fn() + Send>>,
    on_interrupt: Option<Box<dyn Fn() + Send>>,
    on_error: Option<Box<dyn Fn(&RuntimeError) + Send>>,
}

impl<T> Default for RuntimeListener<T> {
    fn default() -> Self {
        Self {
            before_update: None,
            after_update: None,
            on_pause: None,
            on_resume: None,
            on_interrupt: None,
            on_error: None,
        }
    }
}

impl<T> RuntimeListener<T> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn before_update(mut self, callback: impl Fn() + Send + 'static) -> Self {
        self.before_update = Some(Box::new(callback));
        self
    }

    pub fn after_update(mut self, callback: impl Fn(UpdateResult<T>) + Send + 'static) -> Self {
        self.after_update = Some(Box::new(callback));
        self
    }

    pub fn on_pause(mut self, callback: impl Fn() + Send + 'static) -> Self {
        self.on_pause = Some(Box::new(callback));
        self
    }

    pub fn on_resume(mut self, callback: impl Fn() + Send + 'static) -> Self {
        self.on_resume = Some(Box::new(callback));
        self
    }

    pub fn on_interrupt(mut self, callback: impl Fn() + Send + 'static) -> Self {
        self.on_interrupt = Some(Box::new(callback));
        self
    }

    pub fn on_error(mut self, callback: impl Fn(&RuntimeError) + Send + 'static) -> Self {
        self.on_error = Some(Box::new(callback));
        self
    }
}

pub struct UpdateResult<'a, T = ()> {
    pub messages: ParserMessages,
    pub current_tree: &'a RedNode,
    pub reparsed_tree: &'a RedNode,
    pub current_parser: &'a Parser,
    pub source_text: &'a str,
    pub newly_computed_nodes: Vec<Span>,
    pub newly_computed_tokens: Vec<Span>,
    pub semantic_commands: Vec<Command>,
    pub semantic_ir_delta: Option<AstDelta<T>>,
    pub semantic_ir_root: Option<&'a T>,
    pub semantic_ir_root_cell: Option<ASTCell<T>>,
    pub semantic_ir_arena: Option<&'a AstArena<T>>,
    pub metrics: EditMetrics,
}

impl<'a, T> UpdateResult<'a, T> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        messages: ParserMessages,
        current_tree: &'a RedNode,
        reparsed_tree: &'a RedNode,
        current_parser: &'a Parser,
        source_text: &'a str,
        newly_computed_nodes: Vec<Span>,
        newly_computed_tokens: Vec<Span>,
        semantic_commands: Vec<Command>,
        semantic_ir_delta: Option<AstDelta<T>>,
        semantic_ir_root: Option<&'a T>,
        semantic_ir_root_cell: Option<ASTCell<T>>,
        semantic_ir_arena: Option<&'a AstArena<T>>,
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
            semantic_ir_delta,
            semantic_ir_root,
            semantic_ir_root_cell,
            semantic_ir_arena,
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
    pub action_queue_capacity: usize,
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
            action_queue_capacity: 1024,
        }
    }
}

pub struct Interactive<T = (), M = ()> {
    grammar: &'static Grammar,
    runtime_config: RuntimeConfig,
    runtime_listener: Option<RuntimeListener<T>>,
    parser_listener: Option<ParserListener>,
    semantic_map: Option<M>,
    _semantic_ty: PhantomData<T>,
}

impl Interactive<(), ()> {
    pub fn new(grammar: &'static Grammar) -> Interactive<(), ()> {
        Interactive {
            grammar,
            runtime_config: RuntimeConfig::default(),
            runtime_listener: None,
            parser_listener: None,
            semantic_map: None,
            _semantic_ty: PhantomData,
        }
    }

    pub fn with_map<T, M>(self, map: M) -> Interactive<T, M>
    where
        T: Clone + PartialEq + 'static,
        M: AstMapper<T> + Send + 'static,
    {
        assert!(
            self.runtime_listener.is_none(),
            "call `with_map` before `with_listener` so listener can be typed with IR node type",
        );
        Interactive {
            grammar: self.grammar,
            runtime_config: self.runtime_config,
            runtime_listener: None,
            parser_listener: self.parser_listener,
            semantic_map: Some(map),
            _semantic_ty: PhantomData,
        }
    }
}

impl<T, M> Interactive<T, M>
where
    T: Clone + PartialEq + 'static,
    M: AstMapper<T> + Send + 'static,
{
    pub fn with_config(mut self, runtime_config: RuntimeConfig) -> Self {
        self.runtime_config = runtime_config;
        self
    }

    pub fn with_reparser_config(mut self, reparser_config: ReparserConfig) -> Self {
        self.runtime_config.reparser = reparser_config;
        self
    }

    pub fn with_listener(mut self, listener: RuntimeListener<T>) -> Self {
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

    pub fn with_action_queue_capacity(mut self, capacity: usize) -> Self {
        self.runtime_config.action_queue_capacity = capacity.max(1);
        self
    }

    pub fn with_parser_listener(mut self, listener: ParserListener) -> Self {
        self.parser_listener = Some(listener);
        self
    }

    pub fn finish<I: Interface>(self) -> InteractiveInstance<I> {
        InteractiveInstance::init(
            self.grammar,
            self.runtime_config,
            self.runtime_listener.unwrap_or_default(),
            self.parser_listener.unwrap_or_default(),
            self.semantic_map,
        )
    }
}

pub type RuntimeResult<T = Option<RuntimeResponse>> = Result<T, RuntimeError>;

#[derive(Debug, Clone)]
pub struct RuntimeRequest {
    pub(crate) action: Action,
    pub(crate) reply: channel::Sender<RuntimeResult>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(untagged)]
pub enum RuntimeResponse {
    Commands(Vec<Command>),
    String(String),
}

pub struct InteractiveInstance<I> {
    thread_handle: JoinHandle<()>,
    api: I,
}

impl<I: Interface> Deref for InteractiveInstance<I> {
    type Target = I;

    fn deref(&self) -> &Self::Target {
        &self.api
    }
}

impl<I: Interface> InteractiveInstance<I> {
    pub(crate) fn init<T, M>(
        grammar: &'static Grammar,
        runtime_config: RuntimeConfig,
        runtime_listener: RuntimeListener<T>,
        parser_listener: ParserListener,
        semantic_map: Option<M>,
    ) -> Self
    where
        T: Clone + PartialEq + 'static,
        M: AstMapper<T> + Send + 'static,
    {
        let (sender, receiver) = channel::bounded(runtime_config.action_queue_capacity.max(1));
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
            let semantic_map = semantic_map.map(|map| IncrementalLowerer::new(parser.grammar, map));

            let mut runtime = Runtime {
                text: String::new(),
                parser,
                runtime_listener,
                cursor: Reparser::new(cursor, alloc).with_config(runtime_config.reparser),
                semantic_map,
                semantic_map_initialized: false,
                receiver,
                mode: RuntimeMode::Ready,
            };
            runtime.run_event_loop();
        });

        Self {
            thread_handle,
            api: I::new(sender, grammar),
        }
    }

    pub fn api(&self) -> &I {
        &self.api
    }

    pub fn join(self) -> thread::Result<()> {
        self.thread_handle.join()
    }
}

struct StagedEdit {
    span: Span,
    new_len: usize,
    text: String,
}

pub struct Runtime<T, M> {
    text: String,
    parser: Parser,
    runtime_listener: RuntimeListener<T>,
    cursor: Reparser,
    semantic_map: Option<IncrementalLowerer<T, M>>,
    semantic_map_initialized: bool,
    receiver: channel::Receiver<RuntimeRequest>,
    mode: RuntimeMode,
}

impl<T, M> Runtime<T, M>
where
    T: Clone + PartialEq + 'static,
    M: AstMapper<T> + Send + 'static,
{
    fn run_event_loop(&mut self) {
        while let Ok(request) = self.receiver.recv() {
            let mut result = self.handle_action(request.action);
            if let Err(error) = result.as_ref() {
                if let Err(listener_error) = self.emit_error(error) {
                    result = Err(listener_error);
                }
            }
            let _ = request.reply.send(result);
            if self.mode == RuntimeMode::Interrupted {
                break;
            }
        }
    }

    fn emit_error(&self, error: &RuntimeError) -> RuntimeResult {
        if let Some(listener) = self.runtime_listener.on_error.as_ref() {
            catch_unwind(AssertUnwindSafe(|| (listener)(error))).map_err(|_| {
                RuntimeError::ListenerPanic {
                    hook: ListenerHook::OnError,
                }
            })?;
        }
        Ok(None)
    }

    fn call_simple_listener(
        &self,
        hook: ListenerHook,
        callback: Option<&Box<dyn Fn() + Send>>,
    ) -> RuntimeResult {
        let Some(callback) = callback else {
            return Ok(None);
        };
        catch_unwind(AssertUnwindSafe(|| (callback)()))
            .map(|_| None)
            .map_err(|_| RuntimeError::ListenerPanic { hook })
    }

    fn call_after_update_listener(&self, result: UpdateResult<T>) -> RuntimeResult {
        let Some(listener) = self.runtime_listener.after_update.as_ref() else {
            return Ok(None);
        };
        catch_unwind(AssertUnwindSafe(|| (listener)(result)))
            .map(|_| None)
            .map_err(|_| RuntimeError::ListenerPanic {
                hook: ListenerHook::AfterUpdate,
            })
    }

    fn ensure_mode(&self, expected: RuntimeMode, action: RuntimeAction) -> RuntimeResult {
        if self.mode == expected {
            return Ok(None);
        }
        Err(RuntimeError::InvalidMode {
            mode: self.mode,
            action,
        })
    }

    fn semantic_delta(&mut self, commands: &[Command]) -> Option<AstDelta<T>> {
        let map = self.semantic_map.as_mut()?;
        if !self.semantic_map_initialized {
            self.semantic_map_initialized = true;
            Some(map.apply_parse_delta_with_source(commands, &self.text))
        } else {
            Some(map.apply_parse_delta_with_source(commands, &self.text))
        }
    }

    fn semantic_root(&self) -> (Option<ASTCell<T>>, Option<&AstArena<T>>, Option<&T>) {
        let Some(map) = self.semantic_map.as_ref() else {
            return (None, None, None);
        };
        let arena = map.arena();
        let root = map.root_ast();
        let node = root.and_then(|id| arena.get(id));
        (root, Some(arena), node)
    }

    fn validate_offset(&self, offset: usize) -> RuntimeResult {
        if offset <= self.text.len() {
            return Ok(None);
        }
        Err(RuntimeError::InvalidOffset {
            offset,
            text_len: self.text.len(),
        })
    }

    fn validate_span(&self, span: Span) -> RuntimeResult {
        if span.start <= span.end && span.end <= self.text.len() {
            return Ok(None);
        }
        Err(RuntimeError::InvalidRange {
            start: span.start,
            end: span.end,
            text_len: self.text.len(),
        })
    }

    fn stage_edit(&self, action: &Action) -> RuntimeResult<StagedEdit> {
        match action {
            Action::Insert { offset, text } => {
                self.validate_offset(*offset)?;
                let mut next = self.text.clone();
                next.insert_str(*offset, text);
                Ok(StagedEdit {
                    span: Span::new(*offset, *offset),
                    new_len: text.len(),
                    text: next,
                })
            }
            Action::Delete { span } => {
                self.validate_span(*span)?;
                let mut next = self.text.clone();
                next.replace_range(ops::Range::from(*span), "");
                Ok(StagedEdit {
                    span: *span,
                    new_len: 0,
                    text: next,
                })
            }
            Action::Update { span, text } => {
                self.validate_span(*span)?;
                let mut next = self.text.clone();
                next.replace_range(ops::Range::from(*span), text);
                Ok(StagedEdit {
                    span: *span,
                    new_len: text.len(),
                    text: next,
                })
            }
            _ => Err(RuntimeError::InvalidMode {
                mode: self.mode,
                action: action.kind(),
            }),
        }
    }

    fn handle_edit_request(&mut self, action: Action) -> RuntimeResult {
        let staged = self.stage_edit(&action)?;

        self.call_simple_listener(
            ListenerHook::BeforeUpdate,
            self.runtime_listener.before_update.as_ref(),
        )?;

        let mut metrics = EditMetrics::new();

        let previous_messages = self.parser.messages.clone();
        let previous_nodes = self.parser.newly_computed_nodes.clone();
        let previous_tokens = self.parser.newly_computed_tokens.clone();
        let previous_cursor = self.cursor.current.clone();

        let result = match self.cursor.handle_edit(
            &mut self.parser,
            staged.span,
            staged.new_len,
            &staged.text,
            Some(&mut metrics),
        ) {
            Ok(result) => result,
            Err(error) => {
                self.parser.set_text(&self.text);
                self.parser.messages = previous_messages;
                self.parser.newly_computed_nodes = previous_nodes;
                self.parser.newly_computed_tokens = previous_tokens;
                self.cursor.current = previous_cursor;
                return Err(error.into());
            }
        };

        self.text = staged.text;

        let semantic_ir_delta = self.semantic_delta(&result.semantic_commands);
        let (semantic_ir_root_cell, semantic_ir_arena, semantic_ir_root) = self.semantic_root();

        let commands = result.semantic_commands.clone();

        let update = UpdateResult::new(
            result.messages,
            &self.cursor.current,
            &result.reparsed_tree,
            &self.parser,
            &self.text,
            result.newly_computed_nodes,
            result.newly_computed_tokens,
            result.semantic_commands,
            semantic_ir_delta,
            semantic_ir_root,
            semantic_ir_root_cell,
            semantic_ir_arena,
            metrics,
        );

        self.call_after_update_listener(update)?;

        Ok(Some(RuntimeResponse::Commands(commands)))
    }

    fn handle_action(&mut self, action: Action) -> RuntimeResult {
        match action {
            Action::Run => {
                self.ensure_mode(RuntimeMode::Ready, RuntimeAction::Run)?;
                self.mode = RuntimeMode::Running;
                Ok(None)
            }
            Action::Pause => {
                self.ensure_mode(RuntimeMode::Running, RuntimeAction::Pause)?;
                self.mode = RuntimeMode::Paused;
                self.call_simple_listener(
                    ListenerHook::OnPause,
                    self.runtime_listener.on_pause.as_ref(),
                )
            }
            Action::Resume => {
                self.ensure_mode(RuntimeMode::Paused, RuntimeAction::Resume)?;
                self.mode = RuntimeMode::Running;
                self.call_simple_listener(
                    ListenerHook::OnResume,
                    self.runtime_listener.on_resume.as_ref(),
                )
            }
            Action::Exit => {
                let call_interrupt = self.mode != RuntimeMode::Interrupted;
                self.mode = RuntimeMode::Interrupted;
                if call_interrupt {
                    self.call_simple_listener(
                        ListenerHook::OnInterrupt,
                        self.runtime_listener.on_interrupt.as_ref(),
                    )?;
                }
                Ok(None)
            }
            Action::GetSource => {
                self.ensure_mode(RuntimeMode::Running, RuntimeAction::Get)?;
                Ok(Some(RuntimeResponse::String(
                    self.parser.text().to_string(),
                )))
            }
            edit @ Action::Insert { .. }
            | edit @ Action::Delete { .. }
            | edit @ Action::Update { .. } => {
                self.ensure_mode(RuntimeMode::Running, edit.kind())?;
                self.handle_edit_request(edit)
            }
        }
    }
}
