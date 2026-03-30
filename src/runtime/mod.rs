//! Runtime terraced facade.
//!
//! - [`ComposedCompiler`]: composable compiler runtime built from IR/pass stages.
//! - [`GlobalEventDispatcher`]: unified event/request loop (GED).

pub mod compiler;
pub mod dispatcher;
pub mod protocol;
pub mod service;

pub use crate::scheme::passes::ParserPass;
pub use crate::scheme::{LayerName, PassId};
pub use compiler::{
    Another, Build, ContainsPath, Down, End, Fork, Here, LayerObserver, Then, TypedTree,
};
pub use protocol::{RevisionId, RuntimeError, RuntimePath};
pub(crate) use protocol::{
    RuntimeEnvelope, RuntimeRequest, RuntimeResult, RuntimeSignal, RuntimeWireResult,
};
pub use service::RuntimeService;

#[cfg(test)]
mod tests;
