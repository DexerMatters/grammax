use crossbeam::channel;
use serde::Serialize;

use crate::{
    grammar, runtime,
    scheme::{self},
    utils,
};

pub mod cli;
#[cfg(feature = "vsclsp")]
pub mod vsclsp;
#[cfg(feature = "webui")]
pub mod webui;

#[cfg(test)]
mod tests;

pub trait Interface {
    fn new(ged: runtime::GlobalEventDispatcher, grammar: &'static grammar::Grammar) -> Self
    where
        Self: Sized;
    fn ged(&self) -> &runtime::GlobalEventDispatcher;
    fn request(&self, request: runtime::RuntimeRequest) -> runtime::RuntimeResult {
        let (reply_tx, reply_rx) = channel::bounded(1);
        let envelope = runtime::RuntimeEnvelope {
            request,
            reply: reply_tx,
        };
        match self.ged().envelope_tx().try_send(envelope) {
            Ok(()) => reply_rx
                .recv()
                .map_err(|_| runtime::RuntimeError::ChannelClosed)?,
            Err(channel::TrySendError::Full(_)) => Err(runtime::RuntimeError::QueueFull),
            Err(channel::TrySendError::Disconnected(_)) => {
                Err(runtime::RuntimeError::ChannelClosed)
            }
        }
    }

    fn shutdown(&self) -> runtime::RuntimeResult {
        self.request(runtime::RuntimeRequest::Shutdown)
    }

    fn query_layer<I>(
        &self,
        layer_path: runtime::RuntimePath,
        revision: Option<runtime::RevisionId>,
        index: I::Ix,
    ) -> runtime::RuntimeResult<I::Value>
    where
        I: scheme::IR,
        <I as scheme::IR>::Value: Serialize + Clone + Send + Sync + 'static,
        <I as scheme::IR>::Ix: Serialize + Send + Sync + 'static,
    {
        match self.request(runtime::RuntimeRequest::QueryLayer {
            layer_path,
            revision,
            index: runtime::Payload::new(index),
        })? {
            runtime::RuntimeSignal::QueryResult { value, .. } => value
                .downcast_ref::<I::Value>()
                .cloned()
                .ok_or_else(|| runtime::RuntimeError::UnexpectedRequestType),
            other => Err(runtime::RuntimeError::UndefinedBehavior {
                message: format!("unexpected signal for query request: {other:?}"),
            }),
        }
    }

    fn query_source_text(
        &self,
        revision: Option<runtime::RevisionId>,
        span: utils::Span,
    ) -> runtime::RuntimeResult<String> {
        self.query_layer::<scheme::SourceText>(runtime::RuntimePath::root(), revision, span)
    }

    fn input(
        &self,
        start: usize,
        end: usize,
        text: &str,
    ) -> runtime::RuntimeResult<runtime::RevisionId> {
        match self.request(runtime::RuntimeRequest::ApplyTextEdit {
            span: utils::Span::new(start, end),
            text: text.to_string(),
        })? {
            runtime::RuntimeSignal::Accepted { revision } => Ok(revision),
            other => Err(runtime::RuntimeError::UndefinedBehavior {
                message: format!("unexpected signal for apply request: {other:?}"),
            }),
        }
    }

    fn input_till<I>(
        &self,
        start: usize,
        end: usize,
        text: &str,
        runtime_path: runtime::RuntimePath,
    ) -> runtime::RuntimeResult<scheme::Transaction<I>>
    where
        I: scheme::IR + 'static,
        <I as scheme::IR>::Value: Serialize + Clone + Send + Sync + 'static,
        <I as scheme::IR>::Ix: Serialize + Send + Sync + 'static,
    {
        let sub = self.ged().subscribe(Some(runtime_path));
        match self.request(runtime::RuntimeRequest::ApplyTextEdit {
            span: utils::Span::new(start, end),
            text: text.to_string(),
        })? {
            runtime::RuntimeSignal::Accepted { revision } => sub.rev_as(revision),
            other => Err(runtime::RuntimeError::UndefinedBehavior {
                message: format!("unexpected signal for apply request: {other:?}"),
            }),
        }
    }
}

pub struct BasicInterface {
    ged: runtime::GlobalEventDispatcher,
}

impl Interface for BasicInterface {
    fn new(ged: runtime::GlobalEventDispatcher, _grammar: &'static grammar::Grammar) -> Self {
        Self { ged }
    }

    fn ged(&self) -> &runtime::GlobalEventDispatcher {
        &self.ged
    }
}

impl BasicInterface {
    pub fn insert(&self, offset: usize, text: &str) -> runtime::RuntimeResult<runtime::RevisionId> {
        self.input(offset, offset, text)
    }

    pub fn delete(&self, start: usize, end: usize) -> runtime::RuntimeResult<runtime::RevisionId> {
        self.input(start, end, "")
    }

    pub fn replace(
        &self,
        start: usize,
        end: usize,
        text: &str,
    ) -> runtime::RuntimeResult<runtime::RevisionId> {
        self.input(start, end, text)
    }
}
