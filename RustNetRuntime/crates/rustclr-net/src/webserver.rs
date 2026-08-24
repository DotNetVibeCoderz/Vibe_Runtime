//! Micro web server: routed HTTP/1.1 handler on a background thread.

use crate::http::{read_body, read_head, HttpRequest, HttpResponse};
use std::collections::BTreeMap;
use std::io::{BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

type Handler = Box<dyn Fn(&HttpRequest) -> HttpResponse + Send + Sync>;

#[derive(Default)]
pub struct Router {
    routes: Vec<(String, String, Handler)>,
}

impl Router {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn route(
        mut self,
        method: &str,
        path: &str,
        handler: impl Fn(&HttpRequest) -> HttpResponse + Send + Sync + 'static,
    ) -> Self {
        self.routes.push((method.to_uppercase(), path.to_string(), Box::new(handler)));
        self
    }

    pub fn get(
        self,
        path: &str,
        handler: impl Fn(&HttpRequest) -> HttpResponse + Send + Sync + 'static,
    ) -> Self {
        self.route("GET", path, handler)
    }

    pub fn post(
        self,
        path: &str,
        handler: impl Fn(&HttpRequest) -> HttpResponse + Send + Sync + 'static,
    ) -> Self {
        self.route("POST", path, handler)
    }

    fn dispatch(&self, req: &HttpRequest) -> HttpResponse {
        for (method, path, handler) in &self.routes {
            if *method == req.method && *path == req.path {
                return handler(req);
            }
        }
        HttpResponse::not_found()
    }
}

pub struct WebServer {
    pub port: u16,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl WebServer {
    /// Bind to 127.0.0.1:port (0 = ephemeral) and serve on a background
    /// thread until dropped.
    pub fn start(port: u16, router: Router) -> std::io::Result<WebServer> {
        let listener = TcpListener::bind(("127.0.0.1", port))?;
        let actual_port = listener.local_addr()?.port();
        listener.set_nonblocking(true)?;
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let router = Arc::new(router);
        let handle = std::thread::spawn(move || {
            while !stop2.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let router = router.clone();
                        let _ = handle_conn(stream, &router);
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(WebServer { port: actual_port, stop, handle: Some(handle) })
    }
}

fn handle_conn(stream: TcpStream, router: &Router) -> std::io::Result<()> {
    stream.set_nonblocking(false)?;
    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream);
    let (request_line, headers) = read_head(&mut reader)?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("/").to_string();
    let body = read_body(&mut reader, &headers)?;
    let req = HttpRequest { method, path, headers: BTreeMap::new(), body };
    let resp = router.dispatch(&req);
    writer.write_all(&resp.serialize())
}

impl Drop for WebServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http;

    #[test]
    fn serves_routes_and_404() {
        let router = Router::new()
            .get("/", |_| HttpResponse::ok("RustNet Micro Web Server"))
            .get("/status", |_| {
                HttpResponse::ok(r#"{"uptime":1}"#).header("Content-Type", "application/json")
            })
            .post("/echo", |req| HttpResponse::ok(req.body.clone()));
        let server = WebServer::start(0, router).unwrap();
        let addr = format!("127.0.0.1:{}", server.port);

        let resp = http::get(&addr, "/").unwrap();
        assert_eq!(resp.status, 200);
        assert_eq!(resp.body_text(), "RustNet Micro Web Server");

        let resp = http::get(&addr, "/status").unwrap();
        assert_eq!(resp.headers.get("content-type").map(String::as_str), Some("application/json"));

        let resp = http::post(&addr, "/echo", b"ping-pong").unwrap();
        assert_eq!(resp.body, b"ping-pong");

        let resp = http::get(&addr, "/missing").unwrap();
        assert_eq!(resp.status, 404);
    }
}
