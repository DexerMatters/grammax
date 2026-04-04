use std::fmt::Display;

use crate::{
    scheme::{Span, URI},
    utils::Payload,
};

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

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RuntimeEvent {
    pub revision: RevisionId,
    pub layer_path: RuntimePath,
    pub pass_path: RuntimePath,
    pub is_error: bool,
    pub payload: Payload,
}

#[derive(Debug, serde::Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub(crate) enum RuntimeSignal {
    Accepted {
        revision: RevisionId,
    },
    /// Returned by `ApplyAndFetch`: the transaction produced by the requested
    /// layer for this edit, delivered once the pipeline has settled.
    EditResult {
        revision: RevisionId,
        layer_path: RuntimePath,
        value: Payload,
    },
    QueryResult {
        layer_path: RuntimePath,
        value: Payload,
    },
    Ack,
}

#[derive(Debug, serde::Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub(crate) enum RuntimeRequest {
    /// Fire-and-forget edit; replies immediately with `Accepted { revision }`.
    ApplyTextEdit {
        uri: URI,
        span: Span,
        text: String,
    },
    /// Edit and wait for `layer_path` to settle; replies with `EditResult`
    /// containing the `Transaction<I>` produced by that layer.
    ApplyAndFetch {
        uri: URI,
        span: Span,
        text: String,
        layer_path: RuntimePath,
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
pub enum RuntimeError<Err = String> {
    QueueFull,
    ChannelClosed,
    InvalidQuery,
    InvalidRequest { message: String },
    InvalidRequestFromTarget { err: Err },
    /// The queried resource does not yet exist; demand may resolve it.
    ResourceAbsent,
    UnexpectedRequestType,
    UndefinedBehavior { message: String },
}

impl<Err: Display> Display for RuntimeError<Err> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::QueueFull => write!(f, "Runtime queue is full"),
            Self::ChannelClosed => write!(f, "Runtime channel is closed"),
            Self::InvalidQuery => write!(f, "Invalid query"),
            Self::InvalidRequest { message } => write!(f, "Invalid request: {}", message),
            Self::InvalidRequestFromTarget { err } => {
                write!(f, "Invalid request from target: {}", err)
            }
            Self::ResourceAbsent => write!(f, "Resource is absent"),
            Self::UnexpectedRequestType => write!(f, "Unexpected request type"),
            Self::UndefinedBehavior { message } => write!(f, "Undefined behavior: {}", message),
        }
    }
}

pub(crate) type RuntimeResult<T = RuntimeSignal, Err = String> = Result<T, RuntimeError<Err>>;
pub(crate) type RuntimeWireResult<T = RuntimeSignal> = RuntimeResult<T, Payload>;
