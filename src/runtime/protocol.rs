use std::fmt::Display;

use crossbeam::channel;

use crate::utils::Span;

use super::payload::Payload;

pub type RevisionId = u64;

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct RuntimePath(pub Vec<u32>);

impl RuntimePath {
    pub fn root() -> Self {
        Self(Vec::new())
    }

    pub fn child(&self, branch_index: u32) -> Self {
        let mut next = self.0.clone();
        next.push(branch_index);
        Self(next)
    }
}

impl Display for RuntimePath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "path:")?;
        if self.0.is_empty() {
            write!(f, "/")
        } else {
            for seg in &self.0 {
                write!(f, "/{seg}")?;
            }
            Ok(())
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeEvent {
    pub revision: RevisionId,
    pub layer_path: RuntimePath,
    pub pass_path: RuntimePath,
    pub is_error: bool,
    pub payload: Payload,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum RuntimeSignal {
    Accepted {
        revision: RevisionId,
    },
    Event {
        event: RuntimeEvent,
    },
    QueryResult {
        layer_path: RuntimePath,
        value: Payload,
    },
    Ack,
}

impl RuntimeSignal {
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

#[derive(Debug, serde::Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum RuntimeRequest {
    ApplyTextEdit {
        span: Span,
        text: String,
    },
    QueryLayer {
        layer_path: RuntimePath,
        revision: Option<RevisionId>,
        index: Payload,
    },
    Shutdown,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum RuntimeError {
    QueueFull,
    ChannelClosed,
    InvalidQuery,
    InvalidRequest { message: String },
}

impl Display for RuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::QueueFull => write!(f, "Runtime queue is full"),
            Self::ChannelClosed => write!(f, "Runtime channel is closed"),
            Self::InvalidQuery => write!(f, "Invalid query"),
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
