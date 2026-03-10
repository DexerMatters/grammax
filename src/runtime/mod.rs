//! Runtime terraced facade.
//!
//! - [`ComposedCompiler`]: composable compiler runtime built from IR/pass stages.

pub mod compiler;
pub mod payload;
pub mod protocol;
pub mod service;

pub use crate::scheme::layers::cst::Command;
pub use crate::scheme::passes::{ParserPass, Reparser, ReparserConfig};
pub use crate::scheme::{LayerName, PassId};
pub use compiler::{CompilerBuilder, ComposedCompiler, ExpectLayer, ExpectPass, LayerObserver};
pub use payload::{Payload, SerdeAny};
pub use protocol::{
    CompletionPolicy, RevisionId, RuntimeEnvelope, RuntimeError, RuntimeEvent, RuntimeRequest,
    RuntimeResult, RuntimeSelector, RuntimeSignal, RuntimeSignalKind,
};
pub use service::RuntimeService;

#[cfg(test)]
mod tests;
