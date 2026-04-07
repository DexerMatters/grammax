use std::{
    marker::PhantomData,
    ops::{Deref, DerefMut},
    sync::{Arc, Mutex, Weak},
};

use crossbeam::channel;

use crate::{
    grammar::Grammar,
    interface::{BasicInterface, Interface},
};

use super::{
    compiler::{ComposedCompiler, SourceResolveHook, TypedTree},
    dispatcher::GlobalEventDispatcher,
    protocol::RuntimeEvent,
};

pub struct RuntimeService<Tree: TypedTree, Impl = BasicInterface<Tree>> {
    api: Arc<Impl>,
    _marker: PhantomData<fn() -> Tree>,
}

impl<Tree, Impl> RuntimeService<Tree, Impl>
where
    Tree: TypedTree + 'static,
{
    /// Build the compiler pipeline and start the GED event loop.
    pub(crate) fn new<F>(grammar: &'static Grammar, f: F) -> Self
    where
        Impl: Interface<Tree> + Send + Sync + 'static,
        F: FnOnce(
                Option<channel::Sender<RuntimeEvent>>,
                SourceResolveHook,
            ) -> ComposedCompiler<Tree>
            + Send
            + 'static,
    {
        let (evt_tx, evt_rx) = channel::unbounded::<RuntimeEvent>();
        let resolver_ref: Arc<Mutex<Weak<Impl>>> = Arc::new(Mutex::new(Weak::new()));
        let source_resolve = {
            let resolver_ref = Arc::clone(&resolver_ref);
            Arc::new(move |index| {
                let resolver = resolver_ref
                    .lock()
                    .ok()
                    .map(|guard| guard.clone())
                    .and_then(|weak| weak.upgrade());

                match resolver {
                    Some(api) => api.resolve_source(index),
                    None => crate::scheme::ResolveOutcome::Impossible,
                }
            })
        };
        let compiler = f(Some(evt_tx), source_resolve);
        let ged = GlobalEventDispatcher::start(compiler, evt_rx);
        let api = Arc::new(Impl::new(ged, grammar));
        if let Ok(mut guard) = resolver_ref.lock() {
            *guard = Arc::downgrade(&api);
        }
        Self {
            api,
            _marker: PhantomData,
        }
    }
}

impl<Tree: TypedTree, Impl> Deref for RuntimeService<Tree, Impl> {
    type Target = Impl;

    fn deref(&self) -> &Self::Target {
        self.api.as_ref()
    }
}

impl<Tree: TypedTree, Impl> DerefMut for RuntimeService<Tree, Impl> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        Arc::get_mut(&mut self.api)
            .expect("runtime interface is shared; mutable access is not available")
    }
}
