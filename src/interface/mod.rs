use serde::{Serialize, de::DeserializeOwned};
use std::{any::type_name, marker::PhantomData};

use crate::{
    grammar,
    runtime::{
        self,
        compiler::{ContainsPath, Here, TypedTree},
        dispatcher::GlobalEventDispatcher,
    },
    scheme::{self, URI},
    utils,
};

pub mod cli;
#[cfg(feature = "vsclsp")]
pub mod vsclsp;
#[cfg(feature = "webui")]
pub mod webui;

#[cfg(test)]
mod tests;

/// Type-safe result alias that preserves the error type for a tree path.
/// References the error type directly from the IR, no need for extra witness traits.
pub type LayerResult<Tree, Path, T> =
    Result<T, runtime::RuntimeError<<<Tree as ContainsPath<Path>>::Target as scheme::IR>::Fault>>;

pub trait Interface<Tree: TypedTree> {
    fn new(ged: GlobalEventDispatcher, grammar: &'static grammar::Grammar) -> Self
    where
        Self: Sized;
    fn ged(&self) -> &GlobalEventDispatcher;

    fn resolve_source(
        &self,
        _index: <scheme::SourceText as scheme::IR>::Query,
    ) -> scheme::ResolveOutcome<scheme::SourceText>
    where
        Self: Sized,
    {
        scheme::ResolveOutcome::Impossible
    }

    fn shutdown(&self) -> LayerResult<Tree, Here, ()>
    where
        Tree: ContainsPath<Here, Target = scheme::SourceText>,
        Self: Sized,
    {
        request(self, runtime::RuntimeRequest::Shutdown)
            .map_err(|err| {
                map_runtime_error_payload::<
                    <<Tree as ContainsPath<Here>>::Target as scheme::IR>::Fault,
                >(err)
            })
            .map(|_| ())
    }

    fn query_layer<Path>(
        &self,
        revision: Option<runtime::RevisionId>,
        index: <<Tree as ContainsPath<Path>>::Target as scheme::IR>::Query,
    ) -> LayerResult<Tree, Path, <<Tree as ContainsPath<Path>>::Target as scheme::IR>::Answer>
    where
        Tree: ContainsPath<Path>,
        <<Tree as ContainsPath<Path>>::Target as scheme::IR>::Answer: Send + Sync + 'static,
        <<Tree as ContainsPath<Path>>::Target as scheme::IR>::Fault: 'static,
        <<Tree as ContainsPath<Path>>::Target as scheme::IR>::Query:
            Serialize + DeserializeOwned + Send + Sync + 'static,
        Self: Sized,
    {
        match request(
            self,
            runtime::RuntimeRequest::QueryLayer {
                layer_path: <Tree as ContainsPath<Path>>::runtime_path(),
                revision,
                index: utils::Payload::new_serializable(index),
            },
        )
        .map_err(|err| {
            map_runtime_error_payload::<<<Tree as ContainsPath<Path>>::Target as scheme::IR>::Fault>(err)
        })?
        {
            runtime::RuntimeSignal::QueryResult { value, .. } => value
                .downcast::<<<Tree as ContainsPath<Path>>::Target as scheme::IR>::Answer>()
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
        span: scheme::Span,
    ) -> LayerResult<Tree, Here, scheme::SourceAtom>
    where
        Tree: ContainsPath<Here, Target = scheme::SourceText>,
        Self: Sized,
    {
        self.query_layer::<Here>(revision, scheme::DocumentSpan { uri: *uri, span })
    }

    fn get_source_text(&self, uri: &URI) -> LayerResult<Tree, Here, scheme::SourceAtom>
    where
        Tree: ContainsPath<Here, Target = scheme::SourceText>,
        Self: Sized,
    {
        self.query_source_text(None, uri, scheme::Span::new(0, usize::MAX))
    }

    fn check_if_uri_exists(&self, uri: &URI) -> LayerResult<Tree, Here, bool>
    where
        Tree: ContainsPath<Here, Target = scheme::SourceText>,
        Self: Sized,
    {
        match self.query_source_text(None, uri, scheme::Span::new(0, 0)) {
            Ok(_) => Ok(true),
            Err(runtime::RuntimeError::ResourceAbsent) => Ok(false),
            Err(err) => Err(err),
        }
    }

    fn edit_source_text(
        &self,
        uri: &URI,
        start: usize,
        end: usize,
        text: &str,
    ) -> LayerResult<Tree, Here, runtime::RevisionId>
    where
        Tree: ContainsPath<Here, Target = scheme::SourceText>,
        Self: Sized,
    {
        match request(
            self,
            runtime::RuntimeRequest::ApplyTextEdit {
                uri: *uri,
                span: scheme::Span::new(start, end),
                text: text.to_string(),
            },
        )
        .map_err(|err| {
            map_runtime_error_payload::<<<Tree as ContainsPath<Here>>::Target as scheme::IR>::Fault>(err)
        })?
        {
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
    ) -> Result<
        (
            runtime::RevisionId,
            scheme::Transaction<<Tree as ContainsPath<Path>>::Target>,
        ),
        runtime::RuntimeError<<<Tree as ContainsPath<Path>>::Target as scheme::IR>::Fault>,
    >
    where
        Tree: ContainsPath<Path>,
        <Tree as ContainsPath<Path>>::Target: scheme::IR + 'static,
        Self: Sized,
    {
        match request(
            self,
            runtime::RuntimeRequest::ApplyAndFetch {
                uri: *uri,
                span: scheme::Span::new(start, end),
                text: text.to_string(),
                layer_path: <Tree as ContainsPath<Path>>::runtime_path(),
            },
        )
        .map_err(|err| {
            map_runtime_error_payload::<<<Tree as ContainsPath<Path>>::Target as scheme::IR>::Fault>(err)
        })?
        {
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
) -> runtime::RuntimeWireResult {
    this.ged().request(request)
}

/// Map a wire-level error (with Payload) to a typed error.
/// The Err type parameter must match what was actually sent from the target layer.
fn map_runtime_error_payload<Err: 'static>(
    err: runtime::RuntimeError<utils::Payload>,
) -> runtime::RuntimeError<Err> {
    match err {
        runtime::RuntimeError::QueueFull => runtime::RuntimeError::QueueFull,
        runtime::RuntimeError::ChannelClosed => runtime::RuntimeError::ChannelClosed,
        runtime::RuntimeError::InvalidQuery => runtime::RuntimeError::InvalidQuery,
        runtime::RuntimeError::InvalidRequest { message } => {
            runtime::RuntimeError::InvalidRequest { message }
        }
        runtime::RuntimeError::InvalidRequestFromTarget { err } => {
            let debug_payload = format!("{err:?}");
            match err.downcast::<Err>() {
                Some(err) => runtime::RuntimeError::InvalidRequestFromTarget { err },
                None => runtime::RuntimeError::UndefinedBehavior {
                    message: format!(
                        "target error type mismatch: expected {}, got {debug_payload}",
                        type_name::<Err>(),
                    ),
                },
            }
        }
        runtime::RuntimeError::ResourceAbsent => runtime::RuntimeError::ResourceAbsent,
        runtime::RuntimeError::UnexpectedRequestType => {
            runtime::RuntimeError::UnexpectedRequestType
        }
        runtime::RuntimeError::UndefinedBehavior { message } => {
            runtime::RuntimeError::UndefinedBehavior { message }
        }
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

    pub fn insert(
        &self,
        offset: usize,
        text: &str,
    ) -> LayerResult<Tree, Here, runtime::RevisionId> {
        self.edit_source_text(&self.current_uri, offset, offset, text)
    }

    pub fn delete(&self, start: usize, end: usize) -> LayerResult<Tree, Here, runtime::RevisionId> {
        self.edit_source_text(&self.current_uri, start, end, "")
    }

    pub fn replace(
        &self,
        start: usize,
        end: usize,
        text: &str,
    ) -> LayerResult<Tree, Here, runtime::RevisionId> {
        self.edit_source_text(&self.current_uri, start, end, text)
    }
}
