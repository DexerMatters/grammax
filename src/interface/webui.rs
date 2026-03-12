use std::net::{SocketAddr, TcpListener};

use color_print::cprintln;
use rust_embed::Embed;

use crate::{
    grammar,
    interface::Interface,
    runtime,
    scheme::{Command, layers::ParseTreeIR},
    utils,
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
    ApplyTextEdit { span: utils::Span, text: String },
    GetSource,
    GetTree,
    Shutdown,
}

// RedGreenTreeIR is always the first downstream layer (SourceText → pass[0] → CST).
const TREE_LAYER: fn() -> runtime::RuntimePath = || runtime::RuntimePath(vec![0]);

#[derive(Clone)]
pub struct WebPreviewInterface {
    ged: runtime::GlobalEventDispatcher,
    grammar: &'static grammar::Grammar,
    rule_infos: &'static Vec<RuleInfo>,
    terminal_infos: &'static Vec<TerminalInfo>,
    host: &'static str,
    port: u16,
}

impl Interface for WebPreviewInterface {
    fn new(ged: runtime::GlobalEventDispatcher, grammar: &'static grammar::Grammar) -> Self {
        Self {
            ged,
            grammar,
            rule_infos: serialize_rule_infos(grammar),
            terminal_infos: serialize_terminal_infos(grammar),
            host: "127.0.0.1",
            port: 8080,
        }
    }

    fn ged(&self) -> &runtime::GlobalEventDispatcher {
        &self.ged
    }
}

impl WebPreviewInterface {
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

    pub fn run(&self) -> runtime::RuntimeResult {
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
        let self_clone = self.clone();
        let server = rouille::Server::new(addr, move |request| {
            let mut path = request.raw_url().trim_start_matches('/').to_string();

            if path.starts_with("api/") {
                return resolve_api_request(&self_clone, &path, request);
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

        let (handler, sender_to_stop) = server.stoppable();

        ctrlc::set_handler(move || {
            cprintln!("Stopping web preview server...");
            sender_to_stop.send(()).unwrap();
        })
        .map_err(|e| runtime::RuntimeError::InvalidRequest {
            message: e.to_string(),
        })?;

        let _ = handler.join();

        Ok(runtime::RuntimeSignal::Ack)
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
    use crate::scheme::layers::{NodePath, ParseTreeQuery};

    fn path_to_json(path: &NodePath) -> serde_json::Value {
        serde_json::Value::Array(
            path.0
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
            Command::SetRoot { id } => Some(serde_json::json!({ "type": "setRoot", "id": id })),
            // Message / Allocator queries are backend-only — skip them.
            _ => None,
        })
        .collect();

    serde_json::Value::Array(items)
}

fn resolve_api_request(
    this: &WebPreviewInterface,
    path: &str,
    request: &rouille::Request,
) -> rouille::Response {
    match path {
        "api/action" => {
            let body: WebAction = rouille::try_or_400!(rouille::input::json_input(request));
            match body {
                WebAction::ApplyTextEdit { span, text } => this
                    .input_till(span.start, span.end, &text, TREE_LAYER())
                    .map(|transaction| rouille::Response::json(&commands_to_web_json(&transaction)))
                    .unwrap_or_else(|e| rouille::Response::json(&e).with_status_code(500)),
                WebAction::GetSource => {
                    match this.query_source_text(None, utils::Span::new(0, usize::MAX)) {
                        Ok(source) => rouille::Response::json(&source),
                        Err(e) => rouille::Response::json(&e).with_status_code(500),
                    }
                }
                WebAction::GetTree => match build_tree_snapshot(this, None) {
                    Ok(commands) => rouille::Response::json(&commands),
                    Err(e) => rouille::Response::json(&e).with_status_code(500),
                },
                WebAction::Shutdown => match this.shutdown() {
                    Ok(_) => rouille::Response::json(&serde_json::json!({})),
                    Err(e) => rouille::Response::json(&e).with_status_code(500),
                },
            }
        }
        "api/rules" => rouille::Response::json(&this.rule_infos),
        "api/terminals" => rouille::Response::json(&this.terminal_infos),
        _ => rouille::Response::empty_404(),
    }
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

fn build_tree_snapshot(
    this: &WebPreviewInterface,
    revision: Option<runtime::RevisionId>,
) -> runtime::RuntimeResult<serde_json::Value> {
    let source = this.query_source_text(revision, utils::Span::new(0, usize::MAX))?;

    let mut parser = crate::parsec::Parser::new(this.grammar);
    let crate::parsec::Result { root, .. } = parser.parse_text(&source);
    let commands = crate::scheme::passes::delta::generate_commands_for_full_tree(
        &parser.alloc,
        root.green,
        &source,
    );

    Ok(commands_to_web_json(&commands))
}
