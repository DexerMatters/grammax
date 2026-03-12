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

pub use ast::{ASTCell, AstArena, AstArenaError, AstDelta};
pub use cst::{NodePath, ParseNodeValue, ParseTreeError, ParseTreeIR, ParseTreeQuery};
pub use source::SourceText;
