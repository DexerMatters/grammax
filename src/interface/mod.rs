use crossbeam::channel;
use serde::Serialize;
use std::marker::PhantomData;

use crate::{
    grammar,
    runtime::{
        self,
        compiler::{ContainsPath, Here, TypedTree},
        dispatcher::GlobalEventDispatcher,
    },
    scheme::{self, DocumentSpan, Span, URI},
    utils,
};

pub mod cli;
#[cfg(feature = "vsclsp")]
pub mod vsclsp;
#[cfg(feature = "webui")]
pub mod webui;

#[cfg(test)]
mod tests;

pub trait Interface<Tree: TypedTree> {
    fn new(ged: GlobalEventDispatcher, grammar: &'static grammar::Grammar) -> Self
    where
        Self: Sized;
    fn ged(&self) -> &GlobalEventDispatcher;

    fn shutdown(&self) -> runtime::RuntimeResult<()>
    where
        Self: Sized,
    {
        request(self, runtime::RuntimeRequest::Shutdown).map(|_| ())
    }

    fn query_layer<Path>(
        &self,
        revision: Option<runtime::RevisionId>,
        index: <<Tree as ContainsPath<Path>>::Target as scheme::IR>::Ix,
    ) -> runtime::RuntimeResult<<<Tree as ContainsPath<Path>>::Target as scheme::IR>::Value>
    where
        Tree: ContainsPath<Path>,
        <<Tree as ContainsPath<Path>>::Target as scheme::IR>::Value: Send + Sync + 'static,
        <<Tree as ContainsPath<Path>>::Target as scheme::IR>::Ix: Serialize + Send + Sync + 'static,
        Self: Sized,
    {
        match request(
            self,
            runtime::RuntimeRequest::QueryLayer {
                layer_path: <Tree as ContainsPath<Path>>::runtime_path(),
                revision,
                index: utils::Payload::new_serializable(index),
            },
        )? {
            runtime::RuntimeSignal::QueryResult { value, .. } => value
                .downcast::<<<Tree as ContainsPath<Path>>::Target as scheme::IR>::Value>()
                .ok_or_else(|| runtime::RuntimeError::UnexpectedRequestType),
            other => Err(runtime::RuntimeError::UndefinedBehavior {
                message: format!("unexpected signal for query request: {other:?}"),
            }),
        }
    }

    fn query_source_text(
        &self,
        revision: Option<runtime::RevisionId>,
        uri: &URI,
        span: Span,
    ) -> runtime::RuntimeResult<String>
    where
        Tree: ContainsPath<Here, Target = scheme::SourceText>,
        Self: Sized,
    {
        self.query_layer::<Here>(revision, DocumentSpan { uri: *uri, span })
    }

    fn edit_source_text(
        &self,
        uri: &URI,
        start: usize,
        end: usize,
        text: &str,
    ) -> runtime::RuntimeResult<runtime::RevisionId>
    where
        Self: Sized,
    {
        match request(
            self,
            runtime::RuntimeRequest::ApplyTextEdit {
                uri: *uri,
                span: Span::new(start, end),
                text: text.to_string(),
            },
        )? {
            runtime::RuntimeSignal::Accepted { revision } => Ok(revision),
            other => Err(runtime::RuntimeError::UndefinedBehavior {
                message: format!("unexpected signal for apply request: {other:?}"),
            }),
        }
    }

    fn edit_source_text_till<Path>(
        &self,
        uri: &URI,
        start: usize,
        end: usize,
        text: &str,
    ) -> runtime::RuntimeResult<(
        runtime::RevisionId,
        scheme::Transaction<<Tree as ContainsPath<Path>>::Target>,
    )>
    where
        Tree: ContainsPath<Path>,
        <Tree as ContainsPath<Path>>::Target: scheme::IR + 'static,
        <<Tree as ContainsPath<Path>>::Target as scheme::IR>::Value:
            Serialize + Send + Sync + 'static,
        <<Tree as ContainsPath<Path>>::Target as scheme::IR>::Ix: Serialize + Send + Sync + 'static,
        Self: Sized,
    {
        match request(
            self,
            runtime::RuntimeRequest::ApplyAndFetch {
                uri: *uri,
                span: Span::new(start, end),
                text: text.to_string(),
                layer_path: <Tree as ContainsPath<Path>>::runtime_path(),
            },
        )? {
            runtime::RuntimeSignal::EditResult {
                value, revision, ..
            } => Ok((
                revision,
                value
                    .downcast::<scheme::Transaction<<Tree as ContainsPath<Path>>::Target>>()
                    .ok_or_else(|| runtime::RuntimeError::UnexpectedRequestType)?,
            )),
            other => Err(runtime::RuntimeError::UndefinedBehavior {
                message: format!("unexpected signal for ApplyAndFetch: {other:?}"),
            }),
        }
    }
}

fn request<Tree: TypedTree, I: Interface<Tree>>(
    this: &I,
    request: runtime::RuntimeRequest,
) -> runtime::RuntimeResult {
    let (reply_tx, reply_rx) = channel::bounded(1);
    let envelope = runtime::RuntimeEnvelope {
        request,
        reply: reply_tx,
    };
    match this.ged().envelope_tx().try_send(envelope) {
        Ok(()) => reply_rx
            .recv()
            .map_err(|_| runtime::RuntimeError::ChannelClosed)?,
        Err(channel::TrySendError::Full(_)) => Err(runtime::RuntimeError::QueueFull),
        Err(channel::TrySendError::Disconnected(_)) => Err(runtime::RuntimeError::ChannelClosed),
    }
}

pub struct BasicInterface<Tree: TypedTree> {
    ged: GlobalEventDispatcher,
    current_uri: URI,
    _marker: PhantomData<fn() -> Tree>,
}

impl<Tree: TypedTree> Interface<Tree> for BasicInterface<Tree>
where
    Tree: ContainsPath<Here, Target = scheme::SourceText>,
{
    fn new(ged: GlobalEventDispatcher, _grammar: &'static grammar::Grammar) -> Self {
        Self {
            ged,
            current_uri: URI::default(),
            _marker: PhantomData,
        }
    }

    fn ged(&self) -> &GlobalEventDispatcher {
        &self.ged
    }
}

impl<Tree> BasicInterface<Tree>
where
    Tree: TypedTree + ContainsPath<Here, Target = scheme::SourceText>,
{
    pub fn switch_uri(&mut self, uri: impl Into<URI>) {
        self.current_uri = uri.into();
    }

    pub fn current_uri(&self) -> URI {
        self.current_uri
    }

    pub fn insert(&self, offset: usize, text: &str) -> runtime::RuntimeResult<runtime::RevisionId> {
        self.edit_source_text(&self.current_uri, offset, offset, text)
    }

    pub fn delete(&self, start: usize, end: usize) -> runtime::RuntimeResult<runtime::RevisionId> {
        self.edit_source_text(&self.current_uri, start, end, "")
    }

    pub fn replace(
        &self,
        start: usize,
        end: usize,
        text: &str,
    ) -> runtime::RuntimeResult<runtime::RevisionId> {
        self.edit_source_text(&self.current_uri, start, end, text)
    }
}
