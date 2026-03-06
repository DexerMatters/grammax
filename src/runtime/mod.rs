//! Runtime terraced facade.
//!
//! This module intentionally exposes only the core terraced pipeline pieces:
//! - `Compiler<T, M>`: the 3-stage pipeline driver
//! - `ParserPass`: Pass 1→2 (SourceText → RedGreenTreeIR)
//! - `Command`: shared incremental command shape for parse-tree transactions

pub mod compiler;
pub mod protocol;
pub mod service;

pub use crate::scheme::layers::cst::Command;
pub use crate::scheme::passes::{ParserPass, Reparser, ReparserConfig};
pub use compiler::Compiler;
pub use protocol::{
    CompletionPolicy, RevisionId, RuntimeEnvelope, RuntimeError, RuntimeEvent, RuntimeRequest,
    RuntimeResponse, RuntimeResult,
};
pub use service::{RuntimeService, RuntimeServiceConfig};

#[cfg(test)]
mod tests;
