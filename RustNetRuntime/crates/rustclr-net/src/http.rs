//! Minimal HTTP/1.1 client + message types (no external dependencies).

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: String,
    pub path: String,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

impl HttpRequest {
    pub fn new(method: &str, path: &str) -> Self {
        Self {
            method: method.to_uppercase(),
            path: path.to_string(),
            headers: BTreeMap::new(),
            body: Vec::new(),
        }
    }

    pub fn header(mut self, k: &str, v: &str) -> Self {
        self.headers.insert(k.to_string(), v.to_string());
        self
    }

    pub fn body(mut self, data: impl Into<Vec<u8>>) -> Self {
        self.body = data.into();
        self
    }

    pub fn serialize(&self, host: &str) -> Vec<u8> {
        let mut out = format!("{} {} HTTP/1.1\r\nHost: {}\r\n", self.method, self.path, host);
        for (k, v) in &self.headers {
            out.push_str(&format!("{k}: {v}\r\n"));
        }
        out.push_str(&format!("Content-Length: {}\r\nConnection: close\r\n\r\n", self.body.len()));
        let mut bytes = out.into_bytes();
        bytes.extend_from_slice(&self.body);
        bytes
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub reason: String,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn ok(body: impl Into<Vec<u8>>) -> Self {
        Self::with_status(200, "OK", body)
    }

    pub fn not_found() -> Self {
        Self::with_status(404, "Not Found", b"404 Not Found".to_vec())
    }

    pub fn with_status(status: u16, reason: &str, body: impl Into<Vec<u8>>) -> Self {
        Self { status, reason: reason.to_string(), headers: BTreeMap::new(), body: body.into() }
    }

    pub fn header(mut self, k: &str, v: &str) -> Self {
        self.headers.insert(k.to_string(), v.to_string());
        self
    }

    pub fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    pub fn serialize(&self) -> Vec<u8> {
        let mut out = format!("HTTP/1.1 {} {}\r\n", self.status, self.reason);
        for (k, v) in &self.headers {
            out.push_str(&format!("{k}: {v}\r\n"));
        }
        out.push_str(&format!("Content-Length: {}\r\nConnection: close\r\n\r\n", self.body.len()));
        let mut bytes = out.into_bytes();
        bytes.extend_from_slice(&self.body);
        bytes
    }
}

/// Read one HTTP message (request or response head + body) from a stream.
pub(crate) fn read_head<R: BufRead>(reader: &mut R) -> std::io::Result<(String, BTreeMap<String, String>)> {
    let mut first = String::new();
    reader.read_line(&mut first)?;
    let mut headers = BTreeMap::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line)?;
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_lowercase(), v.trim().to_string());
        }
    }
    Ok((first.trim_end().to_string(), headers))
}

pub(crate) fn read_body<R: BufRead>(
    reader: &mut R,
    headers: &BTreeMap<String, String>,
) -> std::io::Result<Vec<u8>> {
    let len: usize = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; len];
    if len > 0 {
        reader.read_exact(&mut body)?;
    }
    Ok(body)
}

/// Blocking HTTP client. `addr` is "host:port".
pub fn request(addr: &str, req: &HttpRequest) -> std::io::Result<HttpResponse> {
    let stream = TcpStream::connect(addr)?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let host = addr.split(':').next().unwrap_or(addr);
    let mut writer = stream.try_clone()?;
    writer.write_all(&req.serialize(host))?;
    let mut reader = BufReader::new(stream);
    let (status_line, headers) = read_head(&mut reader)?;
    let mut parts = status_line.split_whitespace();
    let _version = parts.next();
    let status: u16 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let reason = parts.collect::<Vec<_>>().join(" ");
    let body = read_body(&mut reader, &headers)?;
    Ok(HttpResponse { status, reason, headers, body })
}

pub fn get(addr: &str, path: &str) -> std::io::Result<HttpResponse> {
    request(addr, &HttpRequest::new("GET", path))
}

pub fn post(addr: &str, path: &str, body: &[u8]) -> std::io::Result<HttpResponse> {
    request(addr, &HttpRequest::new("POST", path).body(body.to_vec()))
}
