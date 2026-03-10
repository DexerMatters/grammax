use std::fmt::Display;

use crossbeam::channel;

use crate::{
    scheme::{LayerName, PassId},
    utils::Span,
};

use super::payload::Payload;

pub type RevisionId = u64;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeEvent {
    pub revision: RevisionId,
    pub layer: LayerName,
    pub milestone: PassId,
    pub payload: Payload,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CompletionPolicy {
    Enqueued,
    Settled,
    Layer(LayerName),
    Milestone(PassId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RuntimeSignalKind {
    Accepted,
    Event,
    QueryResult,
    Ack,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum RuntimeSignal {
    Accepted { revision: RevisionId },
    Event { event: RuntimeEvent },
    QueryResult { layer: LayerName, value: Payload },
    Ack,
}

impl RuntimeSignal {
    pub fn kind(&self) -> RuntimeSignalKind {
        match self {
            Self::Accepted { .. } => RuntimeSignalKind::Accepted,
            Self::Event { .. } => RuntimeSignalKind::Event,
            Self::QueryResult { .. } => RuntimeSignalKind::QueryResult,
            Self::Ack => RuntimeSignalKind::Ack,
        }
    }

    pub fn revision(&self) -> Option<RevisionId> {
        match self {
            Self::Accepted { revision } => Some(*revision),
            Self::Event { event } => Some(event.revision),
            Self::QueryResult { .. } | Self::Ack => None,
        }
    }

    pub fn event(&self) -> Option<&RuntimeEvent> {
        match self {
            Self::Event { event } => Some(event),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSelector {
    pub revision: Option<RevisionId>,
    pub kind: Option<RuntimeSignalKind>,
    pub completion: Option<CompletionPolicy>,
}

impl RuntimeSelector {
    pub fn any() -> Self {
        Self::default()
    }

    pub fn events() -> Self {
        Self::default().with_kind(RuntimeSignalKind::Event)
    }

    pub fn revision(revision: RevisionId) -> Self {
        Self {
            revision: Some(revision),
            ..Self::default()
        }
    }

    pub fn with_kind(mut self, kind: RuntimeSignalKind) -> Self {
        self.kind = Some(kind);
        self
    }

    pub fn with_completion(mut self, completion: CompletionPolicy) -> Self {
        self.completion = Some(completion);
        self
    }
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
        txn: Payload,
        completion: CompletionPolicy,
    },
    ApplyTopTxn {
        txn: Payload,
        completion: CompletionPolicy,
    },
    QueryLayer {
        layer: LayerName,
        index: Payload,
    },
    Shutdown,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum RuntimeError {
    QueueFull,
    ChannelClosed,
    InvalidRequest { message: String },
}

impl Display for RuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::QueueFull => write!(f, "Runtime queue is full"),
            Self::ChannelClosed => write!(f, "Runtime channel is closed"),
            Self::InvalidRequest { message } => write!(f, "Invalid request: {}", message),
        }
    }
}

pub type RuntimeResult<T = RuntimeSignal> = Result<T, RuntimeError>;

#[derive(Debug)]
pub struct RuntimeEnvelope {
    pub request: RuntimeRequest,
    pub reply: channel::Sender<RuntimeResult>,
}
