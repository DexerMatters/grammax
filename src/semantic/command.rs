use crate::parsec::tree::GreenId;
use crate::utils::Span;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Insert {
        green_id: GreenId,
        span: Span,
    },
    Delete {
        green_id: GreenId,
        span: Span,
    },
    Update {
        green_id: GreenId,
        span: Span,
    },

    Invalidate {
        green_id: GreenId,
        reason: InvalidationReason,
    },
}

impl Command {
    pub fn green_id(&self) -> GreenId {
        match self {
            Command::Insert { green_id, .. } => *green_id,
            Command::Delete { green_id, .. } => *green_id,
            Command::Update { green_id, .. } => *green_id,
            Command::Invalidate { green_id, .. } => *green_id,
        }
    }
    pub fn span(&self) -> Span {
        match self {
            Command::Insert { span, .. } => *span,
            Command::Delete { span, .. } => *span,
            Command::Update { span, .. } => *span,
            Command::Invalidate { .. } => Span::empty(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvalidationReason {
    DependencyChanged(GreenId),
    RawStructureChanged,
    RawTokenValueChanged,
}
