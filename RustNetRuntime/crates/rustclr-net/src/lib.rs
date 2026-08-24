//! Networking stack for RustNet.
//!
//! On the host and on chips with an OS-level socket API (ESP32 lwIP) the
//! std TCP/UDP types back these modules directly; other chips provide a
//! socket shim. TLS plugs in behind [`tls::TlsProvider`] so hardware
//! implementations (mbedTLS on ESP32) and rustls on the host share the
//! same call sites.

pub mod http;
pub mod modbus;
pub mod mqtt;
pub mod tls;
pub mod webserver;
