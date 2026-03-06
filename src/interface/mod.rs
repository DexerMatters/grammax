use crossbeam::channel;

use crate::{grammar, runtime, utils};

#[cfg(feature = "webui")]
pub mod webui;

#[cfg(feature = "vsclsp")]
pub mod vsclsp;

pub trait Interface {
    fn new(
        sender: channel::Sender<runtime::RuntimeEnvelope>,
        grammar: &'static grammar::Grammar,
    ) -> Self
    where
        Self: Sized;
    fn sender(&self) -> &channel::Sender<runtime::RuntimeEnvelope>;
    fn request(&self, request: runtime::RuntimeRequest) -> runtime::RuntimeResult {
        let (reply_tx, reply_rx) = channel::bounded(1);
        let envelope = runtime::RuntimeEnvelope {
            request,
            reply: reply_tx,
        };
        match self.sender().try_send(envelope) {
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
    sender: channel::Sender<runtime::RuntimeEnvelope>,
}

impl Interface for BasicInterface {
    fn new(
        sender: channel::Sender<runtime::RuntimeEnvelope>,
        _: &'static grammar::Grammar,
    ) -> Self {
        Self { sender }
    }

    fn sender(&self) -> &channel::Sender<runtime::RuntimeEnvelope> {
        &self.sender
    }
}

impl BasicInterface {
    pub fn update_with_policy(
        &self,
        start: usize,
        end: usize,
        text: &str,
        completion: runtime::CompletionPolicy,
    ) -> runtime::RuntimeResult {
        self.request(runtime::RuntimeRequest::ApplyTextEdit {
            span: utils::Span::new(start, end),
            text: text.to_string(),
            completion,
        })
    }

    pub fn update(&self, start: usize, end: usize, text: &str) -> runtime::RuntimeResult {
        self.update_with_policy(start, end, text, runtime::CompletionPolicy::Settled)
    }

    pub fn insert(&self, offset: usize, text: &str) -> runtime::RuntimeResult {
        self.update_with_policy(offset, offset, text, runtime::CompletionPolicy::Settled)
    }

    pub fn delete(&self, start: usize, end: usize) -> runtime::RuntimeResult {
        self.update_with_policy(start, end, "", runtime::CompletionPolicy::Settled)
    }

    pub fn shutdown(&self) -> runtime::RuntimeResult {
        self.request(runtime::RuntimeRequest::Shutdown)
    }
}
