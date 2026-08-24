//! MQTT 3.1.1 client: a blocking client over TCP.
//!
//! The packet codec moved to [`rustclr_mqtt`] so that a bare-metal port
//! without a `TcpStream` — the K210, which reaches its broker through an
//! ESP8285 over AT commands — encodes the same bytes rather than growing a
//! second implementation of them. `Packet` is re-exported here, so callers of
//! this module see no change.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

pub use rustclr_mqtt::Packet;

/// Blocking MQTT client (QoS 0/1 publish, subscribe, poll).
pub struct MqttClient {
    stream: TcpStream,
    rx_buf: Vec<u8>,
    next_packet_id: u16,
}

#[derive(Debug)]
pub enum MqttError {
    Io(String),
    Refused(u8),
    Protocol(String),
}

impl core::fmt::Display for MqttError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            MqttError::Io(e) => write!(f, "{e}"),
            // The broker's own return code, named. "connection refused (5)"
            // sends somebody to the credentials; a bare 5 sends them nowhere.
            MqttError::Refused(code) => {
                let reason = match code {
                    1 => "the broker does not speak MQTT 3.1.1",
                    2 => "the client id was rejected",
                    3 => "the broker is unavailable",
                    4 => "bad username or password",
                    5 => "not authorised",
                    _ => "refused",
                };
                write!(f, "{reason} ({code})")
            }
            MqttError::Protocol(what) => write!(f, "protocol error: {what}"),
        }
    }
}

impl std::error::Error for MqttError {}

impl From<std::io::Error> for MqttError {
    fn from(e: std::io::Error) -> Self {
        MqttError::Io(e.to_string())
    }
}

impl MqttClient {
    pub fn connect(addr: &str, client_id: &str) -> Result<MqttClient, MqttError> {
        Self::connect_auth(addr, client_id, None, None)
    }

    /// Connect with optional username/password — the auth shape cloud IoT
    /// hubs (Azure SAS, GCP JWT, AWS custom auth) expect.
    pub fn connect_auth(
        addr: &str,
        client_id: &str,
        username: Option<&str>,
        password: Option<&str>,
    ) -> Result<MqttClient, MqttError> {
        let mut stream = TcpStream::connect(addr)?;
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;
        stream.write_all(
            &Packet::Connect {
                client_id: client_id.into(),
                keep_alive_s: 60,
                username: username.map(String::from),
                password: password.map(String::from),
            }
            .encode(),
        )?;
        let mut client = MqttClient { stream, rx_buf: Vec::new(), next_packet_id: 1 };
        match client.read_packet()? {
            Packet::ConnAck { code: 0, .. } => Ok(client),
            Packet::ConnAck { code, .. } => Err(MqttError::Refused(code)),
            other => Err(MqttError::Protocol(format!("expected CONNACK, got {other:?}"))),
        }
    }

    pub fn publish(&mut self, topic: &str, payload: &[u8], qos: u8) -> Result<(), MqttError> {
        let packet_id = if qos > 0 {
            let id = self.next_packet_id;
            self.next_packet_id = self.next_packet_id.wrapping_add(1).max(1);
            Some(id)
        } else {
            None
        };
        self.stream.write_all(
            &Packet::Publish { topic: topic.into(), payload: payload.to_vec(), qos, packet_id }
                .encode(),
        )?;
        if qos > 0 {
            match self.read_packet()? {
                Packet::PubAck { .. } => {}
                other => return Err(MqttError::Protocol(format!("expected PUBACK, got {other:?}"))),
            }
        }
        Ok(())
    }

    pub fn subscribe(&mut self, topic: &str) -> Result<(), MqttError> {
        let id = self.next_packet_id;
        self.next_packet_id = self.next_packet_id.wrapping_add(1).max(1);
        self.stream
            .write_all(&Packet::Subscribe { packet_id: id, topics: vec![(topic.into(), 0)] }.encode())?;
        match self.read_packet()? {
            Packet::SubAck { .. } => Ok(()),
            other => Err(MqttError::Protocol(format!("expected SUBACK, got {other:?}"))),
        }
    }

    /// Block until the next PUBLISH arrives (answers pings transparently).
    pub fn poll_message(&mut self) -> Result<(String, Vec<u8>), MqttError> {
        loop {
            match self.read_packet()? {
                Packet::Publish { topic, payload, qos, packet_id } => {
                    if qos > 0 {
                        if let Some(id) = packet_id {
                            self.stream.write_all(&Packet::PubAck { packet_id: id }.encode())?;
                        }
                    }
                    return Ok((topic, payload));
                }
                Packet::PingReq => self.stream.write_all(&Packet::PingResp.encode())?,
                _ => {}
            }
        }
    }

    pub fn ping(&mut self) -> Result<(), MqttError> {
        self.stream.write_all(&Packet::PingReq.encode())?;
        match self.read_packet()? {
            Packet::PingResp => Ok(()),
            other => Err(MqttError::Protocol(format!("expected PINGRESP, got {other:?}"))),
        }
    }

    pub fn disconnect(mut self) -> Result<(), MqttError> {
        self.stream.write_all(&Packet::Disconnect.encode())?;
        Ok(())
    }

    fn read_packet(&mut self) -> Result<Packet, MqttError> {
        loop {
            if let Some((packet, used)) = Packet::decode(&self.rx_buf) {
                self.rx_buf.drain(..used);
                return Ok(packet);
            }
            let mut chunk = [0u8; 1024];
            let n = self.stream.read(&mut chunk)?;
            if n == 0 {
                return Err(MqttError::Io("connection closed".into()));
            }
            self.rx_buf.extend_from_slice(&chunk[..n]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    /// Tiny in-process broker good enough to exercise the client.
    fn mini_broker(listener: TcpListener) {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = Vec::new();
        let mut retained: Option<(String, Vec<u8>)> = None;
        let mut subscribed = false;
        loop {
            let mut chunk = [0u8; 1024];
            let n = match stream.read(&mut chunk) {
                Ok(0) | Err(_) => return,
                Ok(n) => n,
            };
            buf.extend_from_slice(&chunk[..n]);
            while let Some((packet, used)) = Packet::decode(&buf) {
                buf.drain(..used);
                match packet {
                    Packet::Connect { .. } => {
                        stream
                            .write_all(&Packet::ConnAck { session_present: false, code: 0 }.encode())
                            .unwrap();
                    }
                    Packet::Subscribe { packet_id, topics } => {
                        subscribed = true;
                        stream
                            .write_all(
                                &Packet::SubAck { packet_id, codes: vec![0; topics.len()] }.encode(),
                            )
                            .unwrap();
                        if let Some((t, p)) = retained.take() {
                            stream
                                .write_all(
                                    &Packet::Publish { topic: t, payload: p, qos: 0, packet_id: None }
                                        .encode(),
                                )
                                .unwrap();
                        }
                    }
                    Packet::Publish { topic, payload, qos, packet_id } => {
                        if qos > 0 {
                            stream
                                .write_all(
                                    &Packet::PubAck { packet_id: packet_id.unwrap() }.encode(),
                                )
                                .unwrap();
                        }
                        if subscribed {
                            stream
                                .write_all(
                                    &Packet::Publish { topic, payload, qos: 0, packet_id: None }
                                        .encode(),
                                )
                                .unwrap();
                        } else {
                            retained = Some((topic, payload));
                        }
                    }
                    Packet::PingReq => stream.write_all(&Packet::PingResp.encode()).unwrap(),
                    Packet::Disconnect => return,
                    _ => {}
                }
            }
        }
    }

    #[test]
    fn client_against_mini_broker() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = format!("127.0.0.1:{}", listener.local_addr().unwrap().port());
        let broker = std::thread::spawn(move || mini_broker(listener));

        let mut client = MqttClient::connect(&addr, "test-device").unwrap();
        client.ping().unwrap();
        client.publish("telemetry/temp", b"26.1", 1).unwrap();
        client.subscribe("telemetry/#").unwrap();
        let (topic, payload) = client.poll_message().unwrap();
        assert_eq!(topic, "telemetry/temp");
        assert_eq!(payload, b"26.1");
        client.disconnect().unwrap();
        broker.join().unwrap();
    }
}
