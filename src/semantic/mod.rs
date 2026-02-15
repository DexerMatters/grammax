pub mod command;
pub mod propagator;
pub mod lower;

use crate::utils::Span;

pub use command::{Command, InvalidationReason};
pub use lower::{Lower, LowerContext, SemanticTree};

pub trait SemanticNode {
    fn span(&self) -> Span;
}
