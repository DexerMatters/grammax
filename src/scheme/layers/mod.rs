//! Concrete IR layer definitions for the terraced compiler model.
//!
//! There are three layers, each expressing an [`crate::scheme::IR`]:
//!
//! | Layer | Type               | Module     |
//! |-------|--------------------|------------|
//! | 1     | [`SourceText`]     | [`source`] |
//! | 2     | [`RedGreenTreeIR`] | [`cst`]    |
//! | 3     | [`AstArena<T>`]    | [`ast`]    |

pub mod ast;
pub mod cst;
pub mod source;

pub use ast::{ASTCell, AstArena, AstDelta};
pub use cst::{Command, NodePath, ParseNodeValue, ParseTreeIR, ParserCommand, RedGreenTreeIR};
pub use source::SourceText;
