pub(crate) mod delta;
pub mod lowering;
pub(crate) mod metrics;
pub mod parsing;
pub mod reparser;
pub(crate) mod strategy;

pub use lowering::{AstMapAction, AstMapCtx, AstMapIntent, AstMapper, AstNode, IncrementalLowerer};
pub use parsing::ParserPass;

use crate::scheme::{IR, Pass, Transaction};

pub struct Identity;

impl<U: IR + Clone + Send + 'static> Pass<U, U> for Identity {
    type Error = std::convert::Infallible;

    fn transform(
        &mut self,
        _upstream: &U,
        _downstream: &U,
        txn: Transaction<U>,
    ) -> Result<Transaction<U>, Self::Error> {
        Ok(txn)
    }
}
