//! YourTube debug center: a tiny local collector for YourTube Client incident
//! reports and speed telemetry. Listens on 127.0.0.1 only. The extension's
//! background worker POSTs each incident (player fallback, InnerTube error, Rust
//! panic, boot failure) and each telemetry batch (`kind: "perf"`, `schema: 2`) to
//! `/incident`; reports are appended, one JSON object per line, to the incidents
//! file so they can be investigated. `GET /` shows the latest incidents, `GET /perf`
//! the speed dashboard, `GET /perf.json` raw telemetry rows (see `perf.rs`).
//! Standard library only.

mod json;
mod perf;

use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const ADDR: &str = "127.0.0.1:47321";
const MAX_BODY: usize = 512 * 1024;

fn main() {
    let file = std::env::args().nth(1).map(PathBuf::from).unwrap_or_else(default_file);
    if let Some(dir) = file.parent() {
        let _ = fs::create_dir_all(dir);
    }
    // YTC_DEBUG_CENTER_ADDR: another 127.0.0.1 port (tests); the extension only talks to ADDR.
    let addr = std::env::var("YTC_DEBUG_CENTER_ADDR").ok().filter(|a| a.starts_with("127.0.0.1:")).unwrap_or_else(|| ADDR.to_string());
    let listener = TcpListener::bind(&addr).unwrap_or_else(|e| panic!("cannot listen on {addr}: {e}"));
    eprintln!("YourTube debug center on http://{addr}/ → {}", file.display());
    // One thread per connection: a long dashboard read never holds up an incoming report.
    for stream in listener.incoming().flatten() {
        let file = file.clone();
        std::thread::spawn(move || {
            if let Err(e) = handle(stream, &file) {
                eprintln!("request failed: {e}");
            }
        });
    }
}

/// Where incidents go without an argument: the user's data folder, outside any checkout.
/// macOS `~/Library/Application Support/YourTube/incidents.ndjson`, Windows
/// `%APPDATA%\YourTube\incidents.ndjson`, Linux and others `$XDG_DATA_HOME/yourtube/incidents.ndjson`
/// (default `~/.local/share/yourtube/incidents.ndjson`); `incidents.ndjson` here when none is known.
fn default_file() -> PathBuf {
    let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    let dir = if cfg!(target_os = "macos") {
        env("HOME").map(|h| h.join("Library/Application Support/YourTube"))
    } else if cfg!(windows) {
        env("APPDATA").map(|a| a.join("YourTube"))
    } else {
        env("XDG_DATA_HOME").or_else(|| env("HOME").map(|h| h.join(".local/share"))).map(|d| d.join("yourtube"))
    };
    dir.map_or_else(|| PathBuf::from("incidents.ndjson"), |d| d.join("incidents.ndjson"))
}

fn handle(mut stream: TcpStream, file: &Path) -> std::io::Result<()> {
    stream.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let (method, path) = (parts.next().unwrap_or(""), parts.next().unwrap_or("/"));
    let mut length = 0usize;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header)? == 0 || header == "\r\n" || header == "\n" {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                length = value.trim().parse().unwrap_or(0);
            }
        }
    }
    let route = path.split('?').next().unwrap_or("/");
    match (method, route) {
        ("OPTIONS", _) => respond(&mut stream, "204 No Content", "text/plain", b""),
        ("GET", "/perf") => {
            let page = perf::dashboard(file, &perf::query(path));
            respond(&mut stream, "200 OK", "text/html; charset=utf-8", page.as_bytes())
        }
        ("GET", "/perf.json") => {
            let rows = perf::rows_json(file, &perf::query(path));
            respond(&mut stream, "200 OK", "application/json", rows.as_bytes())
        }
        ("GET", "/health") => respond(&mut stream, "200 OK", "text/plain", b"ok"),
        ("POST", "/incident") => {
            if length == 0 || length > MAX_BODY {
                return respond(&mut stream, "413 Payload Too Large", "text/plain", b"size");
            }
            let mut body = vec![0u8; length];
            reader.read_exact(&mut body)?;
            let text = String::from_utf8_lossy(&body);
            let one_line = text.replace(['\n', '\r'], " ");
            let received = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
            let mut out = OpenOptions::new().create(true).append(true).open(file)?;
            // One write per line (appends from parallel connections never interleave).
            out.write_all(format!("{{\"received\":{received},\"report\":{one_line}}}\n").as_bytes())?;
            eprintln!("{} stored ({length} bytes)", if one_line.contains("\"schema\":2") { "telemetry batch" } else { "incident" });
            respond(&mut stream, "200 OK", "text/plain", b"stored")
        }
        ("GET", "/") => {
            let page = overview(file);
            respond(&mut stream, "200 OK", "text/html; charset=utf-8", page.as_bytes())
        }
        _ => respond(&mut stream, "404 Not Found", "text/plain", b"not found"),
    }
}

fn respond(stream: &mut TcpStream, status: &str, kind: &str, body: &[u8]) -> std::io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: POST, GET, OPTIONS\r\nAccess-Control-Allow-Headers: content-type\r\nAccess-Control-Allow-Private-Network: true\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Latest 30 incidents (telemetry batches only counted), newest first, raw JSON per entry.
/// The file is streamed: only the last 30 incident lines are kept.
fn overview(file: &Path) -> String {
    let mut last: std::collections::VecDeque<String> = std::collections::VecDeque::new();
    let (mut incidents, mut batches) = (0usize, 0usize);
    if let Ok(f) = fs::File::open(file) {
        for line in BufReader::with_capacity(1 << 20, f).lines().map_while(Result::ok) {
            if line.contains("\"schema\":2") && line.contains("\"kind\":\"perf\"") {
                batches += 1;
                continue;
            }
            incidents += 1;
            last.push_back(line);
            if last.len() > 30 {
                last.pop_front();
            }
        }
    }
    let mut page = String::from("<!doctype html><meta charset=utf-8><title>YourTube debug center</title><style>body{background:#111;color:#ddd;font:13px ui-monospace,monospace;margin:20px}h1{font:600 18px system-ui;color:#b7ebca}a{color:#9cc8ff}pre{white-space:pre-wrap;word-break:break-all;background:#1a1a1a;padding:10px;border-radius:8px}</style>");
    page.push_str(&format!("<h1>YourTube debug center</h1><p>{incidents} incident(s) and {batches} telemetry batch(es) stored in {} · <a href=/perf>speed dashboard</a></p>", escape(&file.display().to_string())));
    for l in last.iter().rev() {
        page.push_str("<pre>");
        page.push_str(&escape(l));
        page.push_str("</pre>");
    }
    page
}
