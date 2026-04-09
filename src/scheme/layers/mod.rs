pub mod ast;
pub mod cst;
pub mod scope;
pub mod source;

pub(crate) use ast::AstTxnBuilder;
pub use ast::{AstArena, AstArenaFault, AstCell, AstDelta, AstVec};
pub use cst::{
    DocumentNodePath, NodePath, ParseNodeValue, ParseTreeFault, ParseTreeIR, ParseTreeQuery,
    ParseTreeValue,
};
pub use source::{SourceAtom, SourceFault, SourceText};

// ── Demand wiring ─────────────────────────────────────────────────────────────
// Declares, once per index-type pair, which upstream index unblocks a missing
// downstream index. No pass needs to declare this — it's automatic.

use crate::scheme::{Demand, DocumentSpan, Span};
use std::fmt;

// Cross-layer wiring: downstream query → upstream index that resolves it.
impl Demand<source::SourceText> for cst::ParseTreeQuery {
    fn upstream_index(&self) -> Option<DocumentSpan> {
        let uri = match self {
            cst::ParseTreeQuery::Path(dnp) => dnp.0,
            cst::ParseTreeQuery::Message(uri) => *uri,
            cst::ParseTreeQuery::Allocator => return None,
        };
        Some(DocumentSpan {
            uri,
            span: Span::new(0, usize::MAX),
        })
    }
}

impl Demand<cst::ParseTreeIR> for cst::DocumentNodePath {
    fn upstream_index(&self) -> Option<cst::ParseTreeQuery> {
        Some(cst::ParseTreeQuery::Path(cst::DocumentNodePath(
            self.0,
            vec![],
        )))
    }
}

// Trivial self-demand impls for Identity pass (fanout) — no upstream to poke.
impl Demand<source::SourceText> for crate::scheme::DocumentSpan {
    fn upstream_index(&self) -> Option<crate::scheme::DocumentSpan> {
        None
    }
}
impl Demand<cst::ParseTreeIR> for cst::ParseTreeQuery {
    fn upstream_index(&self) -> Option<cst::ParseTreeQuery> {
        None
    }
}
impl<T: fmt::Debug + Clone + PartialEq + Send + 'static> Demand<ast::AstArena<T>>
    for cst::DocumentNodePath
{
    fn upstream_index(&self) -> Option<cst::DocumentNodePath> {
        None
    }
}
