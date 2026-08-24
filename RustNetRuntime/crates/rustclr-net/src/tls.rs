//! TLS integration point.
//!
//! Chips bring their own stack (mbedTLS on ESP32, host uses rustls via the
//! `rustclr-firmware` `tls-rustls` feature); everything above talks to
//! [`TlsProvider`]. `PlainTextProvider` passes bytes through so code paths
//! can be exercised without certificates.

use std::io::{Read, Write};

pub trait TlsSession: Read + Write + Send {}

pub trait TlsProvider: Send {
    /// Wrap an established TCP stream in a TLS session (client handshake).
    fn client(
        &self,
        stream: std::net::TcpStream,
        server_name: &str,
    ) -> std::io::Result<Box<dyn TlsSession>>;
}

/// Pass-through provider for development and tests.
pub struct PlainTextProvider;

struct PlainSession(std::net::TcpStream);

impl Read for PlainSession {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(buf)
    }
}

impl Write for PlainSession {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}

impl TlsSession for PlainSession {}

impl TlsProvider for PlainTextProvider {
    fn client(
        &self,
        stream: std::net::TcpStream,
        _server_name: &str,
    ) -> std::io::Result<Box<dyn TlsSession>> {
        Ok(Box::new(PlainSession(stream)))
    }
}
