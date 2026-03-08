use crossbeam::channel;

use crate::{
    scheme::{self, LayerName, PassId, layers::SourceText},
    utils::Span,
};

pub type RevisionId = u64;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeEvent {
    pub revision: RevisionId,
    pub layer: LayerName,
    pub milestone: PassId,
    pub payload: serde_json::Value,
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

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum RuntimeSignal {
    Accepted {
        revision: RevisionId,
    },
    Event {
        event: RuntimeEvent,
    },
    QueryResult {
        layer: LayerName,
        value: serde_json::Value,
    },
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
        txn: scheme::Transaction<SourceText>,
        completion: CompletionPolicy,
    },
    ApplyTopTxn {
        txn: serde_json::Value,
        completion: CompletionPolicy,
    },
    QueryLayer {
        layer: LayerName,
        index: serde_json::Value,
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

pub type RuntimeResult<T = RuntimeSignal> = Result<T, RuntimeError>;

#[derive(Debug)]
pub struct RuntimeEnvelope {
    pub request: RuntimeRequest,
    pub reply: channel::Sender<RuntimeResult>,
}
