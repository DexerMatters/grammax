pub(crate) mod delta;
pub mod lowering;
pub mod lowering_sg;
pub(crate) mod metrics;
pub mod parsing;
pub mod reparser;
pub(crate) mod strategy;

pub use lowering::{AstMapAction, AstMapCtx, AstMapIntent, AstMapper, AstNode, IncrementalLowerer};
pub use parsing::ParserPass;

use crate::scheme::{IR, LayerCommand, LayerObserver, Pass};

pub struct Identity;

impl<U> Pass<U, U> for Identity
where
    U: IR + Clone + Send + 'static,
    U::Index: Clone,
    U::Value: Clone,
{
    fn push(
        &mut self,
        _upstream: &LayerObserver<U>,
        _downstream: &U,
        txn: &[LayerCommand<U>],
    ) -> Vec<LayerCommand<U>> {
        txn.iter().map(|cmd| cmd.clone_fields()).collect()
    }
}
