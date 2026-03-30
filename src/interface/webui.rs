use std::marker::PhantomData;
use std::net::{SocketAddr, TcpListener};

use color_print::cprintln;
use rust_embed::Embed;

use crate::scheme::{Span, URI};
use crate::{
    grammar,
    interface::Interface,
    runtime::{
        self,
        compiler::{ContainsPath, Down, Here, TypedTree},
        dispatcher::GlobalEventDispatcher,
    },
    scheme::{
        Command,
        layers::{DocumentNodePath, ParseTreeIR, SourceText},
    },
};

#[derive(Embed)]
#[folder = "frontend/dist/"]
#[include = "**/*"]
struct Asset;

#[derive(Clone, serde::Serialize)]
struct RuleInfo {
    idx: usize,
    name: &'static str,
    description: &'static str,
}

#[derive(Clone, serde::Serialize)]
struct TerminalInfo {
    idx: usize,
    display: String,
}

#[derive(serde::Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum WebAction {
    ApplyTextEdit { span: Span, text: String },
    GetSource,
    GetTree,
    Shutdown,
}

pub struct WebPreviewInterface<Tree: TypedTree> {
    ged: GlobalEventDispatcher,
    grammar: &'static grammar::Grammar,
    rule_infos: &'static Vec<RuleInfo>,
    terminal_infos: &'static Vec<TerminalInfo>,
    host: &'static str,
    port: u16,
    _marker: PhantomData<fn() -> Tree>,
}

impl<Tree: TypedTree> Interface<Tree> for WebPreviewInterface<Tree>
where
    Tree: ContainsPath<Here, Target = SourceText> + ContainsPath<Down<Here>, Target = ParseTreeIR>,
{
    fn new(ged: GlobalEventDispatcher, grammar: &'static grammar::Grammar) -> Self {
        Self {
            ged,
            grammar,
            rule_infos: serialize_rule_infos(grammar),
            terminal_infos: serialize_terminal_infos(grammar),
            host: "127.0.0.1",
            port: 8080,
            _marker: PhantomData,
        }
    }

    fn ged(&self) -> &GlobalEventDispatcher {
        &self.ged
    }
}

impl<Tree> WebPreviewInterface<Tree>
where
    Tree: TypedTree
        + ContainsPath<Here, Target = SourceText>
        + ContainsPath<Down<Here>, Target = ParseTreeIR>
        + 'static,
{
    pub fn configure(&mut self, host: &'static str, port: u16) {
        self.host = host;
        self.port = port;
    }

    pub fn url(&self) -> String {
        format!("http://{}:{}/", self.host, self.port)
    }

    pub fn addr(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    pub fn uri() -> URI {
        URI::new("file", "preview")
    }

    pub fn run(&self) -> runtime::RuntimeResult<()> {
        let mut port = self.port;
        while !is_port_free(self.host, port) {
            cprintln!(
                "<yellow>Port {} is in use, trying {}...</yellow>",
                port,
                port + 1
            );
            port += 1;
        }

        let addr = format!("{}:{}", self.host, port);
        let this_clone = self.clone();
        let server = rouille::Server::new(addr, move |request| {
            let mut path = request.raw_url().trim_start_matches('/').to_string();

            if path.starts_with("api/") {
                return this_clone.resolve_api_request(&path, request);
            }

            if path.is_empty() {
                path = "index.html".to_string();
            }

            let content = match Asset::get(path.as_str()) {
                Some(content) => content,
                None => {
                    return rouille::Response::empty_404();
                }
            };

            let mime = mime_guess::from_path(&path)
                .first_or_octet_stream()
                .to_string();
            rouille::Response::from_data(mime, content.data.into_owned())
        })
        .map_err(|e| runtime::RuntimeError::InvalidRequest {
            message: e.to_string(),
        })?;

        let url = format!("http://{}:{}/", self.host, port);
        cprintln!("Web preview server running at <green>{}</green>.", url);
        cprintln!("Press Ctrl+C to stop the server.");

        if let Err(e) = webbrowser::open(&url) {
            cprintln!("<red>Failed to open web browser: {}</red>", e);
        }

        let (handler, sender_to_stop) = server.stoppable();

        // Clone the sender for the ctrl+c handler before moving it
        let sender_clone = sender_to_stop.clone();
        ctrlc::set_handler(move || {
            cprintln!("\nStopping web preview server...");
            let _ = sender_clone.send(());
        })
        .map_err(|e| runtime::RuntimeError::InvalidRequest {
            message: format!("Failed to set Ctrl+C handler: {}", e),
        })?;

        let _ = handler.join();

        Ok(())
    }

    fn resolve_api_request(&self, path: &str, request: &rouille::Request) -> rouille::Response {
        match path {
            "api/action" => {
                let body: WebAction = rouille::try_or_400!(rouille::input::json_input(request));
                match body {
                    WebAction::ApplyTextEdit { span, text } => self
                        .edit_source_text_till::<Down<Here>>(
                            &Self::uri(),
                            span.start,
                            span.end,
                            &text,
                        )
                        .map(|(_, transaction)| {
                            rouille::Response::json(&commands_to_web_json(&transaction))
                        })
                        .map_err(|e| {
                            rouille::Response::json(
                                &runtime::RuntimeError::<String>::UndefinedBehavior {
                                    message: format!("{e:?}"),
                                },
                            )
                            .with_status_code(500)
                        })
                        .unwrap_or_else(|resp| resp),
                    WebAction::GetSource => {
                        match self.query_source_text(None, &Self::uri(), Span::new(0, usize::MAX)) {
                            Ok(source) => rouille::Response::json(&source),
                            Err(e) => rouille::Response::json(&e).with_status_code(500),
                        }
                    }
                    WebAction::GetTree => match self.build_tree_snapshot(None) {
                        Ok(commands) => rouille::Response::json(&commands),
                        Err(e) => rouille::Response::json(&e).with_status_code(500),
                    },
                    WebAction::Shutdown => match self.shutdown() {
                        Ok(_) => rouille::Response::json(&serde_json::json!({})),
                        Err(e) => rouille::Response::json(&e).with_status_code(500),
                    },
                }
            }
            "api/rules" => rouille::Response::json(&self.rule_infos),
            "api/terminals" => rouille::Response::json(&self.terminal_infos),
            _ => rouille::Response::empty_404(),
        }
    }

    fn build_tree_snapshot(
        &self,
        revision: Option<runtime::RevisionId>,
    ) -> runtime::RuntimeResult<serde_json::Value> {
        let source = <Self as Interface<Tree>>::query_source_text(
            self,
            revision,
            &Self::uri(),
            Span::new(0, usize::MAX),
        )
        .map_err(|e| runtime::RuntimeError::UndefinedBehavior {
            message: format!("{e:?}"),
        })?;
        let source_ref = source.as_ref().as_str();

        let mut parser = crate::parsec::Parser::new(self.grammar);
        let crate::parsec::Result { root, .. } = parser.parse_text(source_ref);
        let commands = crate::scheme::passes::delta::generate_commands_for_full_tree(
            &parser.alloc,
            &Self::uri(),
            root.green,
            source_ref,
        );

        Ok(commands_to_web_json(&commands))
    }
}

impl<Tree: TypedTree> Clone for WebPreviewInterface<Tree> {
    fn clone(&self) -> Self {
        Self {
            ged: self.ged.clone(),
            grammar: self.grammar,
            rule_infos: self.rule_infos,
            terminal_infos: self.terminal_infos,
            host: self.host,
            port: self.port,
            _marker: PhantomData,
        }
    }
}

fn is_port_free(host: &str, port: u16) -> bool {
    let check_host = if host == "127.0.0.1" || host.is_empty() {
        "127.0.0.1"
    } else {
        host
    };

    match format!("{}:{}", check_host, port).parse::<SocketAddr>() {
        Ok(addr) => TcpListener::bind(addr).is_ok(),
        Err(_) => false,
    }
}

fn commands_to_web_json(commands: &[Command<ParseTreeIR>]) -> serde_json::Value {
    use crate::scheme::Command;
    use crate::scheme::layers::ParseTreeQuery;

    fn path_to_json(path: &DocumentNodePath) -> serde_json::Value {
        serde_json::Value::Array(
            path.1
                .iter()
                .map(|&i| serde_json::Value::Number(i.into()))
                .collect(),
        )
    }

    let items: Vec<serde_json::Value> = commands
        .iter()
        .filter_map(|cmd| match cmd {
            Command::Create { id, value } => {
                let value_json = serde_json::to_value(value).ok()?;
                Some(serde_json::json!({ "type": "create", "id": id, "value": value_json }))
            }
            Command::Insert {
                index: ParseTreeQuery::Path(path),
                id,
            } => Some(
                serde_json::json!({ "type": "insert", "index": path_to_json(&path), "id": id }),
            ),
            Command::Delete {
                index: ParseTreeQuery::Path(path),
            } => Some(serde_json::json!({ "type": "delete", "index": path_to_json(&path) })),
            Command::Replace {
                index: ParseTreeQuery::Path(path),
                id,
            } => Some(
                serde_json::json!({ "type": "replace", "index": path_to_json(&path), "id": id }),
            ),
            _ => None,
        })
        .collect();

    serde_json::Value::Array(items)
}

fn serialize_rule_infos(grammar: &'static grammar::Grammar) -> &'static Vec<RuleInfo> {
    let infos = &grammar.table.rules;

    let mut rule_infos = Vec::new();
    for (idx, rule) in infos.iter().enumerate() {
        let info = RuleInfo {
            idx,
            name: rule.name,
            description: rule.description,
        };
        rule_infos.push(info);
    }
    Box::leak(Box::new(rule_infos))
}

fn serialize_terminal_infos(grammar: &'static grammar::Grammar) -> &'static Vec<TerminalInfo> {
    let mut terminal_infos = Vec::new();
    for (idx, terminal) in grammar.table.terminals.iter().enumerate() {
        terminal_infos.push(TerminalInfo {
            idx,
            display: terminal.display(),
        });
    }
    Box::leak(Box::new(terminal_infos))
}
