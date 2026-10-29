//! Linux implementation of the Figma Agent (local font helper).
//!
//! Protocol reverse-engineered from the macOS agent (com.figma.agent 126.10.13):
//!   HTTP  127.0.0.1:44950   (the macOS agent also serves HTTPS on 44960; browsers use HTTP)
//!   GET /figma/version                -> {"version": <int>, "package": "<agent version>"}
//!   GET /figma/font-files             -> {"fontFiles": {path: [FontInfo]}, "version", "package",
//!                                         "modified_at", "modified_fonts"}
//!   GET /figma/font-file?file=<path>  -> raw font bytes
//!   GET /figma/font-preview?file=&family=&style=&postscript=&font_size= -> image/svg+xml
//!   GET /figma/health                 -> 200
//! Every response carries CORS headers for figma.com, and preflights get
//! Access-Control-Allow-Private-Network so Chrome lets a public page reach localhost.
//! Requests must name the agent by a loopback Host, which blocks DNS-rebinding pages.

mod fonts;
mod preview;

use std::collections::HashMap;
use std::io::Cursor;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;
use std::time::Instant;

use serde_json::{json, Value};
use tiny_http::{Header, Method, Request, Response, Server};

use fonts::FontIndex;

const PACKAGE: &str = env!("CARGO_PKG_VERSION"); // CFBundleShortVersionString of the macOS agent
const VERSION: u32 = 23; // protocol version reported by the official agents
const WORKERS: usize = 4;

type Reply = Response<Cursor<Vec<u8>>>;

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("valid header")
}

fn reply(status: u16, body: impl Into<Vec<u8>>, content_type: &str) -> Reply {
    Response::from_data(body.into())
        .with_status_code(status)
        .with_header(header("Content-Type", content_type))
}

fn json_reply(value: Value) -> Reply {
    reply(200, value.to_string(), "application/json")
}

fn not_found(message: &str) -> Reply {
    reply(404, json!({ "error": message }).to_string(), "application/json")
}

fn is_figma_origin(origin: &str) -> bool {
    origin.strip_prefix("https://").is_some_and(|host| {
        (host == "figma.com" || host.ends_with(".figma.com"))
            && host.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-')
    })
}

fn request_header(req: &Request, name: &'static str) -> Option<String> {
    req.headers()
        .iter()
        .find(|h| h.field.equiv(name))
        .map(|h| h.value.as_str().to_string())
}

fn preflight(req: &Request) -> Reply {
    let allow_headers = request_header(req, "Access-Control-Request-Headers").unwrap_or_else(|| "*".into());
    let mut resp = reply(204, Vec::new(), "text/plain")
        .with_header(header("Access-Control-Allow-Methods", "GET, POST, OPTIONS"))
        .with_header(header("Access-Control-Allow-Headers", &allow_headers))
        .with_header(header("Access-Control-Max-Age", "600"));
    if request_header(req, "Access-Control-Request-Private-Network").as_deref() == Some("true") {
        resp = resp.with_header(header("Access-Control-Allow-Private-Network", "true"));
    }
    resp
}

fn serve_font(index: &FontIndex, path: &str) -> Reply {
    if !index.contains(path) {
        index.rescan_if_stale();
    }
    if !index.contains(path) {
        return not_found("Invalid path");
    }
    let Ok(data) = fonts::load(path) else { return not_found("Not found") };
    let content_type = fonts::content_type(&data);
    reply(200, data, content_type)
}

fn serve_preview(index: &FontIndex, q: &HashMap<String, String>) -> Reply {
    let param = |k: &str| q.get(k).map(String::as_str).unwrap_or("");
    let path = param("file");
    if !index.contains(path) {
        return not_found("Invalid path");
    }
    let Ok(data) = fonts::load(path) else { return not_found("Not found") };
    let text = if param("family").is_empty() { param("postscript") } else { param("family") };
    let size = param("font_size").parse().unwrap_or(12.0);
    match preview::render(&data, param("postscript"), text, size) {
        Some(svg) => reply(200, svg, "image/svg+xml"),
        None => not_found("Not found"),
    }
}

fn route(index: &FontIndex, req: &Request) -> Reply {
    let url = req.url();
    let (path, query) = url.split_once('?').unwrap_or((url, ""));
    let q: HashMap<String, String> = form_urlencoded::parse(query.as_bytes()).into_owned().collect();
    match (req.method(), path.trim_end_matches('/')) {
        (Method::Options, _) => preflight(req),
        (Method::Get | Method::Head, "/figma/version") => {
            json_reply(json!({ "version": VERSION, "package": PACKAGE }))
        }
        (Method::Get | Method::Head, "/figma/health") => reply(200, "ok", "text/plain"),
        (Method::Get | Method::Head, "/figma/font-files") => json_reply(json!({
            "fontFiles": index.scan(),
            "version": VERSION,
            "package": PACKAGE,
            "modified_at": null,
            "modified_fonts": null,
        })),
        (Method::Get | Method::Head, "/figma/font-file") => {
            serve_font(index, q.get("file").map(String::as_str).unwrap_or(""))
        }
        (Method::Get | Method::Head, "/figma/font-preview") => serve_preview(index, &q),
        // Spellcheck and desktop-app endpoints exist on macOS but are optional for fonts.
        _ => not_found("Not found"),
    }
}

fn handle(index: &FontIndex, hosts: &[String], req: Request) {
    let origin = request_header(&req, "Origin").filter(|o| is_figma_origin(o));
    let host = request_header(&req, "Host").unwrap_or_default().to_ascii_lowercase();
    let mut resp = if !hosts.contains(&host) {
        reply(403, json!({ "error": "Forbidden host" }).to_string(), "application/json")
    } else {
        catch_unwind(AssertUnwindSafe(|| route(index, &req)))
            .unwrap_or_else(|_| reply(500, json!({ "error": "internal error" }).to_string(), "application/json"))
    };
    if let Some(origin) = origin {
        resp = resp
            .with_header(header("Access-Control-Allow-Origin", &origin))
            .with_header(header("Vary", "Origin"));
    }
    eprintln!("{} {} {}", req.method(), req.url(), resp.status_code().0);
    let _ = req.respond(resp);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--version" | "-V") => {
            println!("figma-agent {PACKAGE}");
            return;
        }
        // Show what Figma would see for one font file, to debug a font that does not appear.
        Some("--inspect") => {
            for path in &args[1..] {
                match fonts::describe_file(path, 0) {
                    Ok(faces) => println!("{}", json!({ path.as_str(): faces })),
                    Err(e) => eprintln!("{path}: {e}"),
                }
            }
            return;
        }
        Some(other) => {
            eprintln!("usage: figma-agent [--version | --inspect FONT_FILE...]\nunknown argument: {other}");
            std::process::exit(2);
        }
        None => {}
    }
    let host = std::env::var("FIGMA_AGENT_HOST").unwrap_or_else(|_| "127.0.0.1".into());
    let port: u16 = std::env::var("FIGMA_AGENT_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(44950);

    // Host header values we answer to; anything else is a DNS-rebinding attempt.
    let hosts: Arc<Vec<String>> = Arc::new(
        ["127.0.0.1", "localhost", "[::1]", host.as_str()]
            .iter()
            .map(|name| format!("{}:{port}", name.to_ascii_lowercase()))
            .collect(),
    );
    let index = Arc::new(FontIndex::default());
    let started = Instant::now();
    let count = index.scan().len();
    eprintln!("indexed {count} font files in {:.1}s", started.elapsed().as_secs_f32());

    let server = match Server::http((host.as_str(), port)) {
        Ok(server) => Arc::new(server),
        Err(e) => {
            eprintln!("cannot listen on {host}:{port}: {e}");
            std::process::exit(1);
        }
    };
    eprintln!("starting figma agent {PACKAGE}, listening on http://{host}:{port}");

    let workers: Vec<_> = (0..WORKERS)
        .map(|_| {
            let (server, index, hosts) = (server.clone(), index.clone(), hosts.clone());
            std::thread::spawn(move || {
                for req in server.incoming_requests() {
                    handle(&index, &hosts, req);
                }
            })
        })
        .collect();
    for worker in workers {
        let _ = worker.join();
    }
}
