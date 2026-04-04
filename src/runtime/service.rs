use std::{
    marker::PhantomData,
    ops::{Deref, DerefMut},
};

use crossbeam::channel;

use crate::{
    grammar::Grammar,
    interface::{BasicInterface, Interface},
};

use super::{
    compiler::{ComposedCompiler, TypedTree},
    dispatcher::GlobalEventDispatcher,
    protocol::RuntimeEvent,
};

pub struct RuntimeService<Tree: TypedTree, Impl = BasicInterface<Tree>> {
    api: Impl,
    _marker: PhantomData<fn() -> Tree>,
}

impl<Tree, Impl> RuntimeService<Tree, Impl>
where
    Tree: TypedTree + 'static,
{
    /// Build the compiler pipeline and start the GED event loop.
    pub(crate) fn new<F>(grammar: &'static Grammar, f: F) -> Self
    where
        Impl: Interface<Tree>,
        F: FnOnce(Option<channel::Sender<RuntimeEvent>>) -> ComposedCompiler<Tree> + Send + 'static,
    {
        let (evt_tx, evt_rx) = channel::unbounded::<RuntimeEvent>();
        let compiler = f(Some(evt_tx));
        let ged = GlobalEventDispatcher::start(compiler, evt_rx);
        let api = Impl::new(ged, grammar);
        Self {
            api,
            _marker: PhantomData,
        }
    }
}

impl<Tree: TypedTree, Impl> Deref for RuntimeService<Tree, Impl> {
    type Target = Impl;

    fn deref(&self) -> &Self::Target {
        &self.api
    }
}

impl<Tree: TypedTree, Impl> DerefMut for RuntimeService<Tree, Impl> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.api
    }
}
