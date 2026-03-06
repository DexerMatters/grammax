use crossbeam::channel;

use crate::{
    scheme::{self, layers::SourceText},
    utils::Span,
};

pub type RevisionId = u64;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeEvent {
    pub revision: RevisionId,
    pub milestone: String,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CompletionPolicy {
    Enqueued,
    Settled,
    Milestone(String),
}

#[derive(Debug, serde::Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum RuntimeRequest {
    ApplyTextEdit {
        span: Span,
        text: String,
        completion: CompletionPolicy,
    },
    ApplySourceTxn {
        txn: scheme::Transaction<SourceText>,
        completion: CompletionPolicy,
    },
    Shutdown,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum RuntimeResponse {
    Accepted {
        revision: RevisionId,
    },
    Completed {
        revision: RevisionId,
        event: RuntimeEvent,
    },
    Ack,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum RuntimeError {
    QueueFull,
    ChannelClosed,
    InvalidRequest { message: String },
}

pub type RuntimeResult<T = RuntimeResponse> = Result<T, RuntimeError>;

#[derive(Debug)]
pub struct RuntimeEnvelope {
    pub request: RuntimeRequest,
    pub reply: channel::Sender<RuntimeResult>,
}
