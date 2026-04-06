pub(crate) mod delta;
pub mod lowering;
pub(crate) mod metrics;
pub mod parsing;
pub mod reparser;
pub(crate) mod strategy;

pub use lowering::{AstMapAction, AstMapCtx, AstMapIntent, AstMapper, AstNode, IncrementalLowerer};
pub use parsing::ParserPass;

use crate::scheme::{Command, IR, LayerObserver, Pass};

pub struct Identity;

impl<U> Pass<U, U> for Identity
where
    U: IR + Clone + Send + 'static,
    U::Ix: Clone,
    U::Value: Clone,
{
    fn push(
        &mut self,
        _upstream: &LayerObserver<U>,
        _downstream: &U,
        txn: &[Command<U>],
    ) -> Vec<Command<U>> {
        txn.iter().map(Command::clone_fields).collect()
    }
}
