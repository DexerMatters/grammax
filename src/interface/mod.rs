use crossbeam::channel;

use crate::{runtime, utils};

#[cfg(feature = "webui")]
pub mod webui;

#[cfg(feature = "vsclsp")]
pub mod vsclsp;

pub trait Interface {
    fn new(sender: channel::Sender<runtime::RuntimeRequest>) -> Self
    where
        Self: Sized;
    fn sender(&self) -> &channel::Sender<runtime::RuntimeRequest>;
    fn request(&self, action: runtime::Action) -> runtime::RuntimeResult {
        let (reply_tx, reply_rx) = channel::bounded(1);
        let request = runtime::RuntimeRequest {
            action,
            reply: reply_tx,
        };
        match self.sender().try_send(request) {
            Ok(()) => reply_rx
                .recv()
                .map_err(|_| runtime::RuntimeError::ChannelClosed)?,
            Err(channel::TrySendError::Full(_)) => Err(runtime::RuntimeError::QueueFull),
            Err(channel::TrySendError::Disconnected(_)) => {
                Err(runtime::RuntimeError::ChannelClosed)
            }
        }
    }
}

pub struct BasicInterface {
    sender: channel::Sender<runtime::RuntimeRequest>,
}

impl Interface for BasicInterface {
    fn new(sender: channel::Sender<runtime::RuntimeRequest>) -> Self {
        Self { sender }
    }

    fn sender(&self) -> &channel::Sender<runtime::RuntimeRequest> {
        &self.sender
    }
}

impl BasicInterface {
    pub fn update(&self, start: usize, end: usize, text: &str) -> runtime::RuntimeResult {
        self.request(runtime::Action::Update {
            span: utils::Span::new(start, end),
            text: text.to_string(),
        })
    }

    pub fn insert(&self, offset: usize, text: &str) -> runtime::RuntimeResult {
        self.request(runtime::Action::Insert {
            offset,
            text: text.to_string(),
        })
    }

    pub fn delete(&self, start: usize, end: usize) -> runtime::RuntimeResult {
        self.request(runtime::Action::Delete {
            span: utils::Span::new(start, end),
        })
    }

    pub fn pause(&self) -> runtime::RuntimeResult {
        self.request(runtime::Action::Pause)
    }

    pub fn resume(&self) -> runtime::RuntimeResult {
        self.request(runtime::Action::Resume)
    }

    pub fn run(&self) -> runtime::RuntimeResult {
        self.request(runtime::Action::Run)
    }

    pub fn exit(&self) -> runtime::RuntimeResult {
        self.request(runtime::Action::Exit)
    }
}
