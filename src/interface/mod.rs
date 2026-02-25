use std::sync::mpsc;

use crate::runtime;

#[cfg(feature = "webui")]
pub mod webui;

#[cfg(feature = "vsclsp")]
pub mod vsclsp;

pub trait Interface {
    fn start(&mut self, sender: mpsc::SyncSender<runtime::RuntimeRequest>);
}
