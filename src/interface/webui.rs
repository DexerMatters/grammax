use std::net::{SocketAddr, TcpListener};

use color_print::cprintln;
use crossbeam::channel;
use rust_embed::Embed;

use crate::{grammar, interface::Interface, runtime, utils};

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

#[derive(Clone)]
pub struct WebPreviewInterface {
    sender: channel::Sender<runtime::RuntimeEnvelope>,
    grammar: &'static grammar::Grammar,
    rule_infos: &'static Vec<RuleInfo>,
    terminal_infos: &'static Vec<TerminalInfo>,
    host: &'static str,
    port: u16,
}

impl Interface for WebPreviewInterface {
    fn new(
        sender: channel::Sender<runtime::RuntimeEnvelope>,
        grammar: &'static grammar::Grammar,
    ) -> Self {
        Self {
            sender,
            grammar,
            rule_infos: serialize_rule_infos(grammar),
            terminal_infos: serialize_terminal_infos(grammar),
            host: "127.0.0.1",
            port: 8080,
        }
    }

    fn sender(&self) -> &channel::Sender<runtime::RuntimeEnvelope> {
        &self.sender
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

/// Convert a slice of parse-tree commands to the JSON wire format the frontend
/// `normalizeCommands` function understands.
///
/// The key difference from a plain `serde_json::to_value` call is that
/// `ParseTreeQuery::Path(NodePath)` is an internally-tagged serde enum whose
/// inner type is a sequence (`Vec<usize>`).  Serde cannot add a tag key to a
/// JSON sequence, so serialisation silently returns `null`.  Here we manually
/// flatten `Path([…])` → `[…]` so the frontend receives the expected
/// `{ "index": [0, 1, …] }` shape.
fn commands_to_web_json(commands: &[runtime::Command]) -> serde_json::Value {
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
            } => {
                Some(serde_json::json!({ "type": "insert", "index": path_to_json(path), "id": id }))
            }
            Command::Delete {
                index: ParseTreeQuery::Path(path),
            } => Some(serde_json::json!({ "type": "delete", "index": path_to_json(path) })),
            Command::Replace {
                index: ParseTreeQuery::Path(path),
                id,
            } => Some(
                serde_json::json!({ "type": "replace", "index": path_to_json(path), "id": id }),
            ),
            Command::SetRoot { id } => Some(serde_json::json!({ "type": "setRoot", "id": id })),
            // Message / Allocator queries are backend-only — skip them.
            _ => None,
        })
        .collect();

    serde_json::Value::Array(items)
}

fn signal_to_response(signal: &runtime::RuntimeSignal) -> serde_json::Value {
    match signal {
        runtime::RuntimeSignal::Event { event } => {
            if let Some(commands) = event.payload.downcast_ref::<Vec<runtime::Command>>() {
                return commands_to_web_json(commands);
            }
            event.payload.to_json()
        }
        runtime::RuntimeSignal::QueryResult { value, .. } => value.to_json(),
        runtime::RuntimeSignal::Accepted { .. } | runtime::RuntimeSignal::Ack => {
            serde_json::json!({})
        }
    }
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
                WebAction::ApplyTextEdit { span, text } => {
                    let request = runtime::RuntimeRequest::ApplyTextEdit {
                        span,
                        text: text.clone(),
                    };

                    let result = this.request(request);
                    match result {
                        Ok(signal) => rouille::Response::json(&signal_to_response(&signal)),
                        Err(e) => rouille::Response::json(&e).with_status_code(500),
                    }
                }
                WebAction::GetSource => match query_source_text(this) {
                    Ok(signal) => rouille::Response::json(&signal_to_response(&signal)),
                    Err(e) => rouille::Response::json(&e).with_status_code(500),
                },
                WebAction::GetTree => match build_tree_snapshot(this) {
                    Ok(commands) => rouille::Response::json(&commands),
                    Err(e) => rouille::Response::json(&e).with_status_code(500),
                },
                WebAction::Shutdown => {
                    let request = runtime::RuntimeRequest::Shutdown;
                    match this.request(request) {
                        Ok(signal) => rouille::Response::json(&signal_to_response(&signal)),
                        Err(e) => rouille::Response::json(&e).with_status_code(500),
                    }
                }
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

fn query_source_text(this: &WebPreviewInterface) -> runtime::RuntimeResult<runtime::RuntimeSignal> {
    let request_for_span = |span: utils::Span| -> runtime::RuntimeRequest {
        runtime::RuntimeRequest::QueryLayer {
            layer_path: runtime::RuntimePath::root(),
            index: runtime::Payload::new(span),
        }
    };

    match this.request(request_for_span(utils::Span::new(0, usize::MAX))) {
        Ok(signal) => Ok(signal),
        Err(runtime::RuntimeError::InvalidRequest { message }) => {
            let Some(source_len) = extract_text_length(&message) else {
                return Err(runtime::RuntimeError::InvalidRequest { message });
            };
            this.request(request_for_span(utils::Span::new(0, source_len)))
        }
        Err(err) => Err(err),
    }
}

fn build_tree_snapshot(this: &WebPreviewInterface) -> runtime::RuntimeResult<serde_json::Value> {
    let source_signal = query_source_text(this)?;
    let source = extract_source_text(&source_signal)?;

    // Directly parse the source with a fresh parser so we always get a root
    // (even for empty source the parser produces an error-recovery tree).
    // This avoids the incremental-no-op problem where submitting "" to a temp
    // compiler produces an empty CST delta, leaving the CST without a root.
    let mut parser = crate::parsec::Parser::new(this.grammar);
    let crate::parsec::Result { root, .. } = parser.parse_text(&source);
    let commands = crate::scheme::passes::delta::generate_commands_for_full_tree(
        &parser.alloc,
        root.green,
        &source,
    );

    Ok(commands_to_web_json(&commands))
}

fn extract_source_text(signal: &runtime::RuntimeSignal) -> runtime::RuntimeResult<String> {
    match signal {
        runtime::RuntimeSignal::QueryResult { value, .. } => value
            .downcast_ref::<String>()
            .cloned()
            .ok_or_else(|| runtime::RuntimeError::InvalidRequest {
                message: "source text query result was not a String".to_string(),
            }),
        other => Err(runtime::RuntimeError::InvalidRequest {
            message: format!("unexpected signal for source query: {other:?}"),
        }),
    }
}

fn extract_text_length(message: &str) -> Option<usize> {
    parse_usize_after(message, "text length ").or_else(|| parse_usize_after(message, "text_len:"))
}

fn parse_usize_after(message: &str, marker: &str) -> Option<usize> {
    let start = message.find(marker)? + marker.len();
    let mut value = String::new();

    for ch in message[start..].chars() {
        if ch.is_ascii_digit() {
            value.push(ch);
            continue;
        }

        if !value.is_empty() {
            break;
        }
    }

    if value.is_empty() {
        None
    } else {
        value.parse::<usize>().ok()
    }
}
