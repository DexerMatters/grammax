use std::{ops::Deref, thread};

use crossbeam::channel;

use crate::{
    grammar::Grammar,
    interface::{BasicInterface, Interface},
};

use super::{
    compiler::ComposedCompiler,
    dispatcher::{GlobalEventDispatcher, SubscriptionHandle},
    protocol::{RuntimeEvent, RuntimePath},
};

pub struct RuntimeService<Impl = BasicInterface> {
    ged: GlobalEventDispatcher,
    api: Impl,
    _handle: thread::JoinHandle<()>,
}

impl<Impl: Interface> RuntimeService<Impl> {
    /// Build the compiler pipeline and start the GED event loop.
    pub fn new<F>(grammar: &'static Grammar, f: F) -> Self
    where
        F: FnOnce(Option<channel::Sender<RuntimeEvent>>) -> ComposedCompiler + Send + 'static,
    {
        let (evt_tx, evt_rx) = channel::unbounded::<RuntimeEvent>();
        let compiler = f(Some(evt_tx));
        let (ged, handle) = GlobalEventDispatcher::start(compiler, evt_rx);
        let api = Impl::new(ged.clone(), grammar);
        Self {
            ged,
            api,
            _handle: handle,
        }
    }

    /// Subscribe to pipeline events, optionally filtered to `layer_path`.
    /// Pass `None` to receive events from every layer.
    pub fn subscribe(&self, layer_path: Option<RuntimePath>) -> SubscriptionHandle {
        self.ged.subscribe(layer_path)
    }

    pub fn ged(&self) -> &GlobalEventDispatcher {
        &self.ged
    }
}

impl<Impl> Deref for RuntimeService<Impl> {
    type Target = Impl;

    fn deref(&self) -> &Self::Target {
        &self.api
    }
}
