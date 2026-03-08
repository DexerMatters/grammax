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
    ApplyTextEdit {
        span: utils::Span,
        text: String,
        completion: Option<runtime::CompletionPolicy>,
    },
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

fn signal_to_response(signal: &runtime::RuntimeSignal) -> serde_json::Value {
    match signal {
        runtime::RuntimeSignal::Event { event } => event.payload.clone(),
        runtime::RuntimeSignal::QueryResult { value, .. } => value.clone(),
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
                WebAction::ApplyTextEdit {
                    span,
                    text,
                    completion,
                } => {
                    let request = runtime::RuntimeRequest::ApplyTextEdit {
                        span,
                        text,
                        completion: completion.unwrap_or(runtime::CompletionPolicy::Settled),
                    };

                    match this.request(request) {
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
            layer: runtime::LayerName::root(),
            index: serde_json::to_value(span).unwrap_or(serde_json::json!({})),
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

    serde_json::to_value(commands).map_err(|err| runtime::RuntimeError::InvalidRequest {
        message: format!("failed to encode tree snapshot commands: {err}"),
    })
}

fn extract_source_text(signal: &runtime::RuntimeSignal) -> runtime::RuntimeResult<String> {
    match signal {
        runtime::RuntimeSignal::QueryResult { value, .. } => {
            serde_json::from_value::<String>(value.clone()).map_err(|err| {
                runtime::RuntimeError::InvalidRequest {
                    message: format!("failed to decode source text query result: {err}"),
                }
            })
        }
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
