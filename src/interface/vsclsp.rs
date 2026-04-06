use rouille::url::Url;
use std::sync::Arc;
use tower_lsp::lsp_types::{DidChangeTextDocumentParams, DidOpenTextDocumentParams};
use tower_lsp::{LanguageServer, lsp_types};

use crate::interface::{Interface, LayerResult};
use crate::runtime::{
    self,
    compiler::{ContainsPath, Here, TypedTree},
};
use crate::scheme::{self, Command, DocumentSpan, Range, ResolveOutcome, SourceText, Span, URI};

pub trait LspInterface<Tree: TypedTree>: LanguageServer + Interface<Tree>
where
    Tree: ContainsPath<Here, Target = SourceText>,
{
    fn resolve_missing_uri(index: DocumentSpan) -> ResolveOutcome<SourceText>
    where
        Self: Sized,
    {
        if index.uri.scheme.as_ref().as_str() != "file" {
            return ResolveOutcome::Impossible;
        }

        let path = index.uri.path.as_ref().as_str();
        match std::fs::read_to_string(path) {
            Ok(text) => ResolveOutcome::Done(Arc::new(vec![
                Command::Create {
                    id: 0,
                    value: text.into(),
                },
                Command::Insert {
                    index: DocumentSpan {
                        uri: index.uri,
                        span: Span::new(0, 0),
                    },
                    id: 0,
                },
            ])),
            Err(_) => ResolveOutcome::Impossible,
        }
    }

    fn open_document(&self, params: &DidOpenTextDocumentParams) -> LayerResult<Tree, Here, ()>
    where
        Self: Sized,
    {
        let uri = &params.text_document.uri;
        let uri = URI::from(uri);
        let text = &params.text_document.text;

        // The document is already loaded
        if self.check_if_uri_exists(&uri)? {
            return Ok(());
        }

        self.edit_source_text(&uri, 0, 0, text)?;

        Ok(())
    }

    fn edit_document(
        &self,
        params: &DidChangeTextDocumentParams,
    ) -> LayerResult<Tree, Here, Vec<runtime::RevisionId>>
    where
        Self: Sized,
    {
        let mut revisions = Vec::new();
        for change in params.content_changes.iter() {
            let uri = &params.text_document.uri;
            let uri = URI::from(uri);
            let source = self.query_source_text(None, &uri, Span::new(0, usize::MAX))?;
            let Span { start, end } = match change.range {
                Some(range) => Range::from(range).to_span(&source),
                None => Span {
                    start: 0,
                    end: usize::MAX,
                },
            };
            let revision = self.edit_source_text(&uri, start, end, &change.text)?;
            revisions.push(revision);
        }

        Ok(revisions)
    }

    fn edit_document_till<Path>(
        &self,
        params: &DidChangeTextDocumentParams,
    ) -> Result<
        Vec<(
            runtime::RevisionId,
            scheme::Transaction<<Tree as ContainsPath<Path>>::Target>,
        )>,
        runtime::RuntimeError<<<Tree as ContainsPath<Path>>::Target as scheme::IR>::Fault>,
    >
    where
        Tree: ContainsPath<Path>,
        <Tree as ContainsPath<Path>>::Target: scheme::IR + 'static,
        <<Tree as ContainsPath<Path>>::Target as scheme::IR>::Fault: std::fmt::Debug,
        Self: Sized,
    {
        let mut results = Vec::new();
        for change in params.content_changes.iter() {
            let uri = &params.text_document.uri;
            let uri = URI::from(uri);
            let source = self
                .query_source_text(None, &uri, Span::new(0, usize::MAX))
                .map_err(|e| runtime::RuntimeError::UndefinedBehavior {
                    message: format!("Failed to query source text: {e:?}"),
                })?;
            let Span { start, end } = match change.range {
                Some(range) => Range::from(range).to_span(&source),
                None => Span {
                    start: 0,
                    end: usize::MAX,
                },
            };
            let (revision, transaction) =
                self.edit_source_text_till::<Path>(&uri, start, end, &change.text)?;
            results.push((revision, transaction));
        }

        Ok(results)
    }
}

impl From<Url> for URI {
    fn from(url: Url) -> Self {
        URI::new(url.scheme(), url.path())
    }
}

impl From<&Url> for URI {
    fn from(url: &Url) -> Self {
        URI::new(url.scheme(), url.path())
    }
}

impl From<URI> for Url {
    fn from(uri: URI) -> Self {
        Url::parse(&format!("{}://{}", uri.scheme, uri.path)).unwrap()
    }
}

impl From<&URI> for Url {
    fn from(uri: &URI) -> Self {
        Url::parse(&format!("{}://{}", uri.scheme, uri.path)).unwrap()
    }
}

impl From<lsp_types::Range> for Range {
    fn from(range: lsp_types::Range) -> Self {
        Range::new(
            (range.start.line, range.start.character),
            (range.end.line, range.end.character),
        )
    }
}

impl From<Range> for lsp_types::Range {
    fn from(range: Range) -> Self {
        lsp_types::Range {
            start: lsp_types::Position {
                line: range.start.0,
                character: range.start.1,
            },
            end: lsp_types::Position {
                line: range.end.0,
                character: range.end.1,
            },
        }
    }
}
