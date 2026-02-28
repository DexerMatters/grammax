use color_print::cprintln;
use crossbeam::channel;
use rust_embed::Embed;

use crate::{interface::Interface, runtime};

#[derive(Embed)]
#[folder = "frontend/static/"]
#[include = "**/*"]
struct Asset;

pub struct WebPreviewInterface {
    sender: channel::Sender<runtime::RuntimeRequest>,
    host: &'static str,
    port: u16,
}

impl Interface for WebPreviewInterface {
    fn new(sender: channel::Sender<runtime::RuntimeRequest>) -> Self {
        Self {
            sender,
            host: "localhost",
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
        let server = rouille::Server::new(self.addr(), move |request| {
            let mut path = request.raw_url().trim_start_matches('/').to_string();

            // Serve index.html for root path
            if path.is_empty() {
                path = "index.html".to_string();
            }

            let file = Asset::get(path.as_str()).or_else(|| Asset::get("index.html"));

            eprintln!("Received request for path: {}", path);

            match file {
                Some(content) => {
                    let mime = mime_guess::from_path(&path)
                        .first_or_octet_stream()
                        .to_string();
                    rouille::Response::from_data(mime, content.data.into_owned())
                }
                None => rouille::Response::empty_404(),
            }
        });

        if let Err(e) = server {
            return Err(runtime::RuntimeError::GeneralError(e));
        }

        let server = server.unwrap();

        cprintln!(
            "Web preview server running at <green>{}</green>.",
            self.url()
        );
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
