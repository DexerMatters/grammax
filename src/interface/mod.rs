use crossbeam::channel;

use crate::{grammar, runtime, utils};

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
    fn sender(&self) -> &channel::Sender<runtime::RuntimeEnvelope>;
    fn request(&self, request: runtime::RuntimeRequest) -> runtime::RuntimeResult {
        let (reply_tx, reply_rx) = channel::bounded(1);
        let envelope = runtime::RuntimeEnvelope {
            request,
            reply: reply_tx,
        };
        match self.sender().try_send(envelope) {
            Ok(()) => reply_rx
                .recv()
                .map_err(|_| runtime::RuntimeError::ChannelClosed)?,
            Err(channel::TrySendError::Full(_)) => Err(runtime::RuntimeError::QueueFull),
            Err(channel::TrySendError::Disconnected(_)) => {
                Err(runtime::RuntimeError::ChannelClosed)
            }
        }
    }
}

pub struct BasicInterface {
    sender: channel::Sender<runtime::RuntimeEnvelope>,
}

impl Interface for BasicInterface {
    fn new(ged: runtime::GlobalEventDispatcher, _grammar: &'static grammar::Grammar) -> Self {
        Self {
            sender: ged.envelope_tx(),
        }
    }

    fn sender(&self) -> &channel::Sender<runtime::RuntimeEnvelope> {
        &self.sender
    }
}

impl BasicInterface {
    pub fn update(
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
            other => Err(runtime::RuntimeError::InvalidRequest {
                message: format!("unexpected signal for apply request: {other:?}"),
            }),
        }
    }

    pub fn insert(&self, offset: usize, text: &str) -> runtime::RuntimeResult<runtime::RevisionId> {
        self.update(offset, offset, text)
    }

    pub fn delete(&self, start: usize, end: usize) -> runtime::RuntimeResult<runtime::RevisionId> {
        self.update(start, end, "")
    }

    pub fn query_layer<I>(
        &self,
        layer_path: runtime::RuntimePath,
        revision: Option<runtime::RevisionId>,
        index: I,
    ) -> runtime::RuntimeResult<runtime::Payload>
    where
        I: serde::Serialize + Send + Sync + 'static,
    {
        match self.request(runtime::RuntimeRequest::QueryLayer {
            layer_path,
            revision,
            index: runtime::Payload::new(index),
        })? {
            runtime::RuntimeSignal::QueryResult { value, .. } => Ok(value),
            other => Err(runtime::RuntimeError::InvalidRequest {
                message: format!("unexpected signal for query request: {other:?}"),
            }),
        }
    }

    pub fn query_source_text(&self, span: utils::Span) -> runtime::RuntimeResult<String> {
        self.query_layer(runtime::RuntimePath::root(), None, span)?
            .downcast_ref::<String>()
            .cloned()
            .ok_or_else(|| runtime::RuntimeError::InvalidRequest {
                message: "source text query result was not a String".to_string(),
            })
    }

    pub fn shutdown(&self) -> runtime::RuntimeResult {
        self.request(runtime::RuntimeRequest::Shutdown)
    }
}
