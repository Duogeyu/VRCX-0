//! Minimal read-only static file server used to serve overlay plugin pages.
//!
//! Serving plugins over `http://127.0.0.1:<port>/` instead of `file://` keeps
//! the page origin a normal HTTP origin, so cross-origin calls to the VRCC API
//! (which echoes the request origin) keep working.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

/// A tiny HTTP server bound to a loopback port.
pub struct StaticServer {
    addr: std::net::SocketAddr,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl StaticServer {
    /// Bind to an ephemeral loopback port and serve `root` in the background.
    pub fn spawn(root: PathBuf) -> Result<Self, String> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .map_err(|error| format!("bind plugin server: {error}"))?;
        listener
            .set_nonblocking(true)
            .map_err(|error| format!("configure plugin server: {error}"))?;
        let addr = listener
            .local_addr()
            .map_err(|error| format!("read plugin server address: {error}"))?;
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let stop = Arc::clone(&stop);
            let root = root.clone();
            thread::Builder::new()
                .name("vrcx-web-plugins".to_string())
                .spawn(move || run(listener, root, stop))
                .map_err(|error| format!("spawn plugin server: {error}"))?
        };
        Ok(Self {
            addr,
            stop,
            thread: Some(thread),
        })
    }

    /// URL of the server root (ends with `/`).
    pub fn base_url(&self) -> String {
        format!("http://{}/", self.addr)
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for StaticServer {
    fn drop(&mut self) {
        self.stop();
    }
}

fn run(listener: TcpListener, root: PathBuf, stop: Arc<AtomicBool>) {
    while !stop.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, _peer)) => {
                let root = root.clone();
                let stop = Arc::clone(&stop);
                let _ = thread::Builder::new()
                    .name("vrcx-web-plugins-conn".to_string())
                    .spawn(move || {
                        let _ = handle(stream, &root);
                        drop(stop);
                    });
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(25));
            }
            Err(_) => thread::sleep(Duration::from_millis(50)),
        }
    }
}

fn handle(mut stream: TcpStream, root: &Path) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    // drain headers
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 || line == "\r\n" || line == "\n" {
            break;
        }
    }

    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("/");
    if method != "GET" && method != "HEAD" {
        return respond(&mut stream, 405, "text/plain", b"method not allowed");
    }

    let path = target.split('?').next().unwrap_or("/");
    let path = percent_decode(path);
    let path = path.trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    let file = sanitize(root, path);
    let Some(file) = file else {
        return respond(&mut stream, 403, "text/plain", b"forbidden");
    };
    let body = std::fs::read(&file)
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::NotFound, "not found"));
    match body {
        Ok(body) => {
            let mime = mime_for(&file);
            if method == "HEAD" {
                respond_head(&mut stream, 200, mime, body.len())
            } else {
                respond(&mut stream, 200, mime, &body)
            }
        }
        Err(_) => respond(&mut stream, 404, "text/plain", b"not found"),
    }
}

fn sanitize(root: &Path, relative: &str) -> Option<PathBuf> {
    let candidate = Path::new(relative);
    if candidate.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return None;
    }
    let full = root.join(candidate);
    let canonical_root = std::fs::canonicalize(root).ok()?;
    let canonical = std::fs::canonicalize(&full).ok()?;
    canonical.starts_with(&canonical_root).then_some(canonical)
}

fn respond(stream: &mut TcpStream, status: u16, mime: &str, body: &[u8]) -> std::io::Result<()> {
    respond_head(stream, status, mime, body.len())?;
    stream.write_all(body)?;
    stream.flush()
}

fn respond_head(
    stream: &mut TcpStream,
    status: u16,
    mime: &str,
    len: usize,
) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "OK",
    };
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {mime}\r\nContent-Length: {len}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n"
    )?;
    Ok(())
}

fn mime_for(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        // Work on the byte slice, not on `input`: slicing a `&str` by byte
        // offsets panics whenever the request path contains non-ASCII text
        // (plugin pages with Chinese asset names, for instance).
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Some(value) = hex_byte(bytes[i + 1], bytes[i + 2]) {
                out.push(value);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_byte(high: u8, low: u8) -> Option<u8> {
    let high = (high as char).to_digit(16)?;
    let low = (low as char).to_digit(16)?;
    Some((high as u8) << 4 | low as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_parent_traversal() {
        let root = std::env::temp_dir().join("vrcx-web-plugins-test");
        let _ = std::fs::create_dir_all(&root);
        assert!(sanitize(&root, "../secret.txt").is_none());
        assert!(sanitize(&root, "a/../../secret.txt").is_none());
    }

    #[test]
    fn decodes_percent_escapes() {
        assert_eq!(percent_decode("/a%20b"), "/a b");
        assert_eq!(percent_decode("/%E4%B8%AD%E6%96%87"), "/中文");
    }

    #[test]
    fn leaves_non_ascii_request_paths_intact() {
        // Regression: slicing the &str by byte offset used to panic here.
        assert_eq!(percent_decode("/地图/排名.json"), "/地图/排名.json");
        assert_eq!(percent_decode("/%星"), "/%星");
        assert_eq!(percent_decode("/100%"), "/100%");
    }
}
