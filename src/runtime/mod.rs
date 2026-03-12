//! Runtime terraced facade.
//!
//! - [`ComposedCompiler`]: composable compiler runtime built from IR/pass stages.
//! - [`GlobalEventDispatcher`]: unified event/request loop (GED).

pub mod compiler;
pub mod dispatcher;
pub mod payload;
pub mod protocol;
pub mod service;

pub use crate::scheme::passes::{ParserPass, Reparser, ReparserConfig};
pub use crate::scheme::{LayerName, PassId};
pub use compiler::{CompilerBuilder, ComposedCompiler, ExpectLayer, ExpectPass, LayerObserver};
pub use dispatcher::GlobalEventDispatcher;
pub use payload::{Payload, SerdeAny};
pub use protocol::{
    RevisionId, RuntimeEnvelope, RuntimeError, RuntimeEvent, RuntimePath, RuntimeRequest,
    RuntimeResult, RuntimeSignal,
};
pub use service::RuntimeService;

#[cfg(test)]
mod tests;
