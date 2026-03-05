use std::net::{SocketAddr, TcpListener};

use color_print::cprintln;
use crossbeam::channel;
use rust_embed::Embed;

use crate::{grammar, interface::Interface, runtime};

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

#[derive(Clone)]
pub struct WebPreviewInterface {
    sender: channel::Sender<runtime::RuntimeRequest>,
    rule_infos: &'static Vec<RuleInfo>,
    terminal_infos: &'static Vec<TerminalInfo>,
    host: &'static str,
    port: u16,
}

impl Interface for WebPreviewInterface {
    fn new(
        sender: channel::Sender<runtime::RuntimeRequest>,
        grammar: &'static grammar::Grammar,
    ) -> Self {
        let rule_infos = serialize_rule_infos(grammar);
        let terminal_infos = serialize_terminal_infos(grammar);
        Self {
            sender,
            rule_infos,
            terminal_infos,
            host: "127.0.0.1",
            port: 8080,
        }
    }

    fn sender(&self) -> &channel::Sender<runtime::RuntimeRequest> {
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
        self.request(runtime::Action::Run)?;

        // Find a free port
        let mut port = self.port;
        while !is_port_free(self.host, port) {
            cprintln!(
                "<yellow>Port {} is in use, trying {}...</yellow>",
                port,
                port + 1
            );
            port += 1;
        }

        // Create server on the free port
        let addr = format!("{}:{}", self.host, port);
        let self_clone = self.clone();
        let server = rouille::Server::new(addr, move |request| {
            let mut path = request.raw_url().trim_start_matches('/').to_string();

            // API
            if path.starts_with("api/") {
                return resolve_api_request(&self_clone, &path, request);
            }

            // Serve index.html for root path
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
        .map_err(|e| runtime::RuntimeError::GeneralError(e))?;

        let url = format!("http://{}:{}/", self.host, port);
        cprintln!("Web preview server running at <green>{}</green>.", url);
        cprintln!("Press Ctrl+C to stop the server.");

        let (handler, sender_to_stop) = server.stoppable();

        ctrlc::set_handler(move || {
            cprintln!("Stopping web preview server...");
            sender_to_stop.send(()).unwrap();
        })
        .map_err(|e| runtime::RuntimeError::GeneralError(Box::new(e)))?;

        handler.join().unwrap(); // Keep the server running until it's stopped

        Ok(None)
    }
}

fn is_port_free(host: &str, port: u16) -> bool {
    // Use 127.0.0.1 for 127.0.0.1 to avoid DNS resolution issues
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

fn resolve_api_request(
    this: &WebPreviewInterface,
    path: &str,
    request: &rouille::Request,
) -> rouille::Response {
    match path {
        /* POST */
        "api/action" => {
            let body: runtime::Action = rouille::try_or_400!(rouille::input::json_input(request));
            match this.request(body) {
                Ok(Some(response)) => rouille::Response::json(&response),
                Ok(None) => rouille::Response::empty_204(),
                Err(e) => rouille::Response::json(&e).with_status_code(500),
            }
        }

        /* GET */
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
