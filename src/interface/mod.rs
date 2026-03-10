use crossbeam::channel;

use crate::{grammar, runtime, scheme::LayerName, utils};

pub mod cli;
#[cfg(feature = "vsclsp")]
pub mod vsclsp;
#[cfg(feature = "webui")]
pub mod webui;

#[cfg(test)]
mod tests;

pub trait Interface {
    fn new(
        sender: channel::Sender<runtime::RuntimeEnvelope>,
        grammar: &'static grammar::Grammar,
    ) -> Self
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
    fn new(
        sender: channel::Sender<runtime::RuntimeEnvelope>,
        _grammar: &'static grammar::Grammar,
    ) -> Self {
        Self { sender }
    }

    fn sender(&self) -> &channel::Sender<runtime::RuntimeEnvelope> {
        &self.sender
    }
}

impl BasicInterface {
    pub fn update_with_policy(
        &self,
        start: usize,
        end: usize,
        text: &str,
        completion: runtime::CompletionPolicy,
    ) -> runtime::RuntimeResult {
        self.request(runtime::RuntimeRequest::ApplyTextEdit {
            span: utils::Span::new(start, end),
            text: text.to_string(),
            completion,
        })
    }

    pub fn update(&self, start: usize, end: usize, text: &str) -> runtime::RuntimeResult {
        self.update_with_policy(start, end, text, runtime::CompletionPolicy::Settled)
    }

    pub fn insert(&self, offset: usize, text: &str) -> runtime::RuntimeResult {
        self.update_with_policy(offset, offset, text, runtime::CompletionPolicy::Settled)
    }

    pub fn delete(&self, start: usize, end: usize) -> runtime::RuntimeResult {
        self.update_with_policy(start, end, "", runtime::CompletionPolicy::Settled)
    }

    pub fn submit_top_txn(
        &self,
        txn: serde_json::Value,
        completion: runtime::CompletionPolicy,
    ) -> runtime::RuntimeResult {
        self.request(runtime::RuntimeRequest::ApplyTopTxn { txn, completion })
    }

    pub fn query_layer(
        &self,
        layer: LayerName,
        index: serde_json::Value,
    ) -> runtime::RuntimeResult<runtime::Payload> {
        let expected_layer = layer;
        let signal = self.request(runtime::RuntimeRequest::QueryLayer {
            layer: expected_layer.clone(),
            index,
        })?;

        match signal {
            runtime::RuntimeSignal::QueryResult {
                layer: returned_layer,
                value,
            } if returned_layer == expected_layer => Ok(value),
            runtime::RuntimeSignal::QueryResult { layer, .. } => {
                Err(runtime::RuntimeError::InvalidRequest {
                    message: format!(
                        "query layer mismatch: expected {expected_layer}, got {layer}"
                    ),
                })
            }
            other => Err(runtime::RuntimeError::InvalidRequest {
                message: format!("unexpected signal for query request: {other:?}"),
            }),
        }
    }

    pub fn query_source_text(&self, span: utils::Span) -> runtime::RuntimeResult<String> {
        let payload = self.query_layer(
            LayerName::root(),
            serde_json::to_value(span).map_err(|err| runtime::RuntimeError::InvalidRequest {
                message: format!("failed to encode span query: {err}"),
            })?,
        )?;

        payload
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
