use std::sync::OnceLock;

use rouille::url::Url;
use tower_lsp::lsp_types::{
    CompletionOptions, DidChangeTextDocumentParams, DidOpenTextDocumentParams,
    HoverProviderCapability, InitializeParams, InitializeResult, InitializedParams,
    ServerCapabilities,
};
use tower_lsp::{Client, ClientSocket, LanguageServer, LspService, Server, jsonrpc, lsp_types};

use crate::grammar::Grammar;
use crate::interface::{Interface, LayerResult};
use crate::runtime::dispatcher::GlobalEventDispatcher;
use crate::runtime::{
    self,
    compiler::{ContainsPath, Here, TypedTree},
};
use crate::scheme::{Range, URI};

pub trait LanguageServerHandle<Tree: TypedTree> {
    fn resolve(&self, uri: &URI) -> Option<String>;
}

pub struct LanguageServerInterface<Tree: TypedTree, I: LanguageServerHandle<Tree>> {
    client: OnceLock<Client>,
    ged: GlobalEventDispatcher,
    _marker: std::marker::PhantomData<fn() -> (Tree, I)>,

    codebase: boxcar::Vec<URI>,
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
    Tree: TypedTree,
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
            codebase: boxcar::Vec::new(),
        }
    }

    fn ged(&self) -> &GlobalEventDispatcher {
        &self.ged
    }
}

#[tower_lsp::async_trait]
impl<Tree, I> LanguageServer for LanguageServerInterface<Tree, I>
where
    Tree: TypedTree + 'static,
    I: LanguageServerHandle<Tree> + Send + Sync + 'static,
{
    async fn initialize(&self, params: InitializeParams) -> jsonrpc::Result<InitializeResult> {
        let workspace_uris = params
            .workspace_folders
            .unwrap_or_default()
            .into_iter()
            .map(|folder| folder.uri.into())
            .collect::<Vec<URI>>();

        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                completion_provider: Some(CompletionOptions::default()),
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
