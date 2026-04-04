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

pub(crate) use ast::AstTxnBuilder;
pub use ast::{AstArena, AstArenaFault, AstCell, AstDelta, AstVec};
pub use cst::{
    DocumentNodePath, NodePath, ParseNodeValue, ParseTreeFault, ParseTreeIR, ParseTreeQuery,
    ParseTreeValue,
};
pub use source::{SourceAtom, SourceFault, SourceText};
