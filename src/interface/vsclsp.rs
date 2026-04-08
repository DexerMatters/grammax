use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use rouille::url::Url;
use tower_lsp::lsp_types::SemanticTokensRegistrationOptions;
use tower_lsp::{Client, LanguageServer, LspService, Server, jsonrpc, lsp_types};

use crate::grammar::Grammar;
use crate::interface::Interface;
use crate::runtime::Down;
use crate::runtime::compiler::{ContainsPath, Here, TypedTree};
use crate::runtime::dispatcher::GlobalEventDispatcher;
use crate::scheme::layers::ParseTreeIR;
use crate::scheme::{Command, IR, Range, ResolveOutcome, SourceText, URI};

pub struct LanguageServerInterface<Tree: TypedTree, I: LanguageServerHandle<Tree>> {
    client: OnceLock<Client>,
    opened_documents: Arc<Mutex<HashMap<URI, String>>>,
    ged: GlobalEventDispatcher,
    handle: I,
    _marker: std::marker::PhantomData<fn() -> (Tree, I)>,
}

impl<Tree, I> LanguageServerInterface<Tree, I>
where
    Tree: TypedTree
        + ContainsPath<Here, Target = SourceText>
        + ContainsPath<Down<Here>, Target = ParseTreeIR>
        + 'static,
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
    Tree: TypedTree
        + ContainsPath<Here, Target = SourceText>
        + ContainsPath<Down<Here>, Target = ParseTreeIR>,
    I: LanguageServerHandle<Tree> + 'static,
{
    fn new(ged: GlobalEventDispatcher, _grammar: &'static Grammar) -> Self
    where
        Self: Sized,
    {
        Self {
            client: OnceLock::new(),
            opened_documents: Arc::new(Mutex::new(HashMap::new())),
            handle: I::new(),
            ged,
            _marker: std::marker::PhantomData,
        }
    }

    fn ged(&self) -> &GlobalEventDispatcher {
        &self.ged
    }

    fn resolve_source(&self, index: <SourceText as IR>::Ix) -> ResolveOutcome<SourceText>
    where
        Self: Sized,
    {
        if let Ok(opened_documents) = self.opened_documents.lock() {
            if let Some(text) = opened_documents.get(&index.uri) {
                let transaction = vec![
                    Command::Create {
                        id: 0,
                        value: internment::Intern::new(text.clone()),
                    },
                    Command::Insert { index, id: 0 },
                ];
                return ResolveOutcome::Done(Arc::new(transaction));
            }
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
    Tree: TypedTree
        + ContainsPath<Here, Target = SourceText>
        + ContainsPath<Down<Here>, Target = ParseTreeIR>
        + 'static,
    I: LanguageServerHandle<Tree> + Send + Sync + 'static,
{
    async fn initialize(
        &self,
        _params: lsp_types::InitializeParams,
    ) -> jsonrpc::Result<lsp_types::InitializeResult> {
        let config = self.handle.configure();
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
                semantic_tokens_provider: Some(
                    lsp_types::SemanticTokensServerCapabilities::SemanticTokensRegistrationOptions(
                        SemanticTokensRegistrationOptions {
                            text_document_registration_options: {
                                lsp_types::TextDocumentRegistrationOptions {
                                    document_selector: Some(vec![lsp_types::DocumentFilter {
                                        language: Some(config.language_id),
                                        scheme: Some("file".to_string()),
                                        pattern: Some(format!(
                                            "*.{{{}}}",
                                            config.file_extensions.join(",")
                                        )),
                                    }]),
                                }
                            },
                            semantic_tokens_options: lsp_types::SemanticTokensOptions {
                                work_done_progress_options:
                                    lsp_types::WorkDoneProgressOptions::default(),
                                legend: lsp_types::SemanticTokensLegend {
                                    token_types: config.token_types.to_vec(),
                                    token_modifiers: config.token_modifiers.to_vec(),
                                },
                                range: Some(true),
                                full: Some(lsp_types::SemanticTokensFullOptions::Bool(true)),
                            },
                            static_registration_options:
                                lsp_types::StaticRegistrationOptions::default(),
                        },
                    ),
                ),
                ..Default::default()
            },
            ..Default::default()
        })
    }

    async fn did_open(&self, params: lsp_types::DidOpenTextDocumentParams) {
        let uri: URI = params.text_document.uri.into();
        if let Ok(mut opened_documents) = self.opened_documents.lock() {
            opened_documents.insert(uri, params.text_document.text);
        }
    }

    async fn shutdown(&self) -> jsonrpc::Result<()> {
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct LanguageServerConfig {
    pub language_id: String,
    pub file_extensions: Vec<String>,
    pub token_types: &'static [lsp_types::SemanticTokenType],
    pub token_modifiers: &'static [lsp_types::SemanticTokenModifier],
}

impl Default for LanguageServerConfig {
    fn default() -> Self {
        Self {
            language_id: "plaintext".to_string(),
            file_extensions: vec!["txt".to_string()],
            token_types: &[],
            token_modifiers: &[],
        }
    }
}

pub trait LanguageServerHandle<Tree: TypedTree> {
    fn new() -> Self
    where
        Self: Sized;

    fn configure(&self) -> LanguageServerConfig;
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
