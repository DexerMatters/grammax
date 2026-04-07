use std::sync::{Arc, OnceLock};

use rouille::url::Url;
use tower_lsp::{Client, LanguageServer, LspService, Server, jsonrpc, lsp_types};

use crate::grammar::Grammar;
use crate::interface::Interface;
use crate::runtime::compiler::{ContainsPath, Here, TypedTree};
use crate::runtime::dispatcher::GlobalEventDispatcher;
use crate::scheme::{Command, DocumentSpan, Range, ResolveOutcome, SourceText, URI};

pub trait LanguageServerHandle<Tree: TypedTree> {
    fn resolve(&self, uri: &URI) -> Option<String>;
}

pub struct LanguageServerInterface<Tree: TypedTree, I: LanguageServerHandle<Tree>> {
    client: OnceLock<Client>,
    ged: GlobalEventDispatcher,
    _marker: std::marker::PhantomData<fn() -> (Tree, I)>,
}

impl<Tree, I> LanguageServerInterface<Tree, I>
where
    Tree: TypedTree + 'static,
    I: LanguageServerHandle<Tree> + Send + Sync + 'static,
{
    pub async fn run(self) {
        let (service, socket) = LspService::new(|client| {
            let _ = self.client.set(client);
            self
        });
        let stdin = tokio::io::stdin();
        let stdout = tokio::io::stdout();
        Server::new(stdin, stdout, socket).serve(service).await;
    }
}

impl<Tree, I> Interface<Tree> for LanguageServerInterface<Tree, I>
where
    Tree: TypedTree + ContainsPath<Here, Target = SourceText>,
    I: LanguageServerHandle<Tree> + 'static,
{
    fn new(ged: GlobalEventDispatcher, _grammar: &'static Grammar) -> Self
    where
        Self: Sized,
    {
        Self {
            client: OnceLock::new(),
            ged,
            _marker: std::marker::PhantomData,
        }
    }

    fn ged(&self) -> &GlobalEventDispatcher {
        &self.ged
    }

    fn resolve_source(&self, index: DocumentSpan) -> ResolveOutcome<SourceText>
    where
        Self: Sized,
    {
        if !index.uri.valid() {
            return ResolveOutcome::Impossible;
        }
        let Ok(text) = index.uri.fetch_text() else {
            return ResolveOutcome::Impossible;
        };
        let transaction = vec![
            Command::Create {
                id: 0,
                value: internment::Intern::new(text),
            },
            Command::Insert { index, id: 0 },
        ];
        ResolveOutcome::Done(Arc::new(transaction))
    }
}

#[tower_lsp::async_trait]
impl<Tree, I> LanguageServer for LanguageServerInterface<Tree, I>
where
    Tree: TypedTree + 'static,
    I: LanguageServerHandle<Tree> + Send + Sync + 'static,
{
    async fn initialize(
        &self,
        _params: lsp_types::InitializeParams,
    ) -> jsonrpc::Result<lsp_types::InitializeResult> {
        Ok(lsp_types::InitializeResult {
            capabilities: lsp_types::ServerCapabilities {
                hover_provider: Some(lsp_types::HoverProviderCapability::Simple(true)),
                completion_provider: Some(lsp_types::CompletionOptions::default()),
                workspace: Some(lsp_types::WorkspaceServerCapabilities {
                    workspace_folders: Some(lsp_types::WorkspaceFoldersServerCapabilities {
                        supported: Some(true),
                        change_notifications: Some(lsp_types::OneOf::Left(true)),
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        })
    }

    async fn did_open(&self, params: lsp_types::DidOpenTextDocumentParams) {
        let uri: URI = params.text_document.uri.into();
        if !uri.valid() {
            return;
        }
    }

    async fn shutdown(&self) -> jsonrpc::Result<()> {
        Ok(())
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
