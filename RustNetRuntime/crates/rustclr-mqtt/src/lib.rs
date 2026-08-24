//! MQTT 3.1.1 packet codec.
//!
//! Only the wire format: what a packet is, how it is written, how it is read
//! back. No sockets, no client state machine, no `std`.
//!
//! It lives apart from [`rustclr_net::mqtt`]'s client for one reason. That
//! client speaks over a `TcpStream`, which a bare-metal port does not have —
//! the K210 reaches its broker by handing bytes to an ESP8285 over AT
//! commands, one `AT+CIPSEND` at a time. The packets are identical either way,
//! and a second implementation of them would be a second set of off-by-ones in
//! the remaining-length varint. So the bytes live here, where both callers and
//! the host test run can reach them.

#![no_std]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Packet {
    Connect {
        client_id: String,
        keep_alive_s: u16,
        username: Option<String>,
        password: Option<String>,
    },
    ConnAck { session_present: bool, code: u8 },
    Publish { topic: String, payload: Vec<u8>, qos: u8, packet_id: Option<u16> },
    PubAck { packet_id: u16 },
    Subscribe { packet_id: u16, topics: Vec<(String, u8)> },
    SubAck { packet_id: u16, codes: Vec<u8> },
    PingReq,
    PingResp,
    Disconnect,
}

fn encode_remaining_length(mut len: usize, out: &mut Vec<u8>) {
    loop {
        let mut byte = (len % 128) as u8;
        len /= 128;
        if len > 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if len == 0 {
            break;
        }
    }
}

fn decode_remaining_length(data: &[u8]) -> Option<(usize, usize)> {
    let mut value = 0usize;
    let mut multiplier = 1usize;
    for (i, byte) in data.iter().enumerate().take(4) {
        value += ((byte & 0x7F) as usize) * multiplier;
        if byte & 0x80 == 0 {
            return Some((value, i + 1));
        }
        multiplier *= 128;
    }
    None
}

fn put_str(s: &str, out: &mut Vec<u8>) {
    out.extend_from_slice(&(s.len() as u16).to_be_bytes());
    out.extend_from_slice(s.as_bytes());
}

fn get_str(data: &[u8], pos: &mut usize) -> Option<String> {
    if *pos + 2 > data.len() {
        return None;
    }
    let len = u16::from_be_bytes([data[*pos], data[*pos + 1]]) as usize;
    *pos += 2;
    if *pos + len > data.len() {
        return None;
    }
    let s = String::from_utf8_lossy(&data[*pos..*pos + len]).to_string();
    *pos += len;
    Some(s)
}

impl Packet {
    pub fn encode(&self) -> Vec<u8> {
        let (header, var): (u8, Vec<u8>) = match self {
            Packet::Connect { client_id, keep_alive_s, username, password } => {
                let mut v = Vec::new();
                put_str("MQTT", &mut v);
                v.push(4); // protocol level 3.1.1
                let mut flags = 0b0000_0010u8; // clean session
                if username.is_some() {
                    flags |= 0b1000_0000;
                }
                if password.is_some() {
                    flags |= 0b0100_0000;
                }
                v.push(flags);
                v.extend_from_slice(&keep_alive_s.to_be_bytes());
                put_str(client_id, &mut v);
                if let Some(u) = username {
                    put_str(u, &mut v);
                }
                if let Some(p) = password {
                    put_str(p, &mut v);
                }
                (0x10, v)
            }
            Packet::ConnAck { session_present, code } => {
                (0x20, vec![*session_present as u8, *code])
            }
            Packet::Publish { topic, payload, qos, packet_id } => {
                let mut v = Vec::new();
                put_str(topic, &mut v);
                if *qos > 0 {
                    v.extend_from_slice(&packet_id.unwrap_or(1).to_be_bytes());
                }
                v.extend_from_slice(payload);
                (0x30 | (qos << 1), v)
            }
            Packet::PubAck { packet_id } => (0x40, packet_id.to_be_bytes().to_vec()),
            Packet::Subscribe { packet_id, topics } => {
                let mut v = packet_id.to_be_bytes().to_vec();
                for (t, qos) in topics {
                    put_str(t, &mut v);
                    v.push(*qos);
                }
                (0x82, v)
            }
            Packet::SubAck { packet_id, codes } => {
                let mut v = packet_id.to_be_bytes().to_vec();
                v.extend_from_slice(codes);
                (0x90, v)
            }
            Packet::PingReq => (0xC0, Vec::new()),
            Packet::PingResp => (0xD0, Vec::new()),
            Packet::Disconnect => (0xE0, Vec::new()),
        };
        let mut out = vec![header];
        encode_remaining_length(var.len(), &mut out);
        out.extend_from_slice(&var);
        out
    }

    /// Decode one packet; returns (packet, bytes consumed).
    pub fn decode(data: &[u8]) -> Option<(Packet, usize)> {
        if data.is_empty() {
            return None;
        }
        let header = data[0];
        let (len, len_bytes) = decode_remaining_length(&data[1..])?;
        let start = 1 + len_bytes;
        if data.len() < start + len {
            return None;
        }
        let var = &data[start..start + len];
        let total = start + len;
        let packet = match header >> 4 {
            1 => {
                let mut pos = 0;
                let _proto = get_str(var, &mut pos)?;
                let _level = *var.get(pos)?;
                let flags = *var.get(pos + 1)?;
                pos += 2; // level + flags
                let keep_alive_s = u16::from_be_bytes([*var.get(pos)?, *var.get(pos + 1)?]);
                pos += 2;
                let client_id = get_str(var, &mut pos)?;
                let username = if flags & 0b1000_0000 != 0 { Some(get_str(var, &mut pos)?) } else { None };
                let password = if flags & 0b0100_0000 != 0 { Some(get_str(var, &mut pos)?) } else { None };
                Packet::Connect { client_id, keep_alive_s, username, password }
            }
            2 => Packet::ConnAck { session_present: var[0] & 1 != 0, code: var[1] },
            3 => {
                let qos = (header >> 1) & 0x03;
                let mut pos = 0;
                let topic = get_str(var, &mut pos)?;
                let packet_id = if qos > 0 {
                    let id = u16::from_be_bytes([*var.get(pos)?, *var.get(pos + 1)?]);
                    pos += 2;
                    Some(id)
                } else {
                    None
                };
                Packet::Publish { topic, payload: var[pos..].to_vec(), qos, packet_id }
            }
            4 => Packet::PubAck { packet_id: u16::from_be_bytes([var[0], var[1]]) },
            8 => {
                let packet_id = u16::from_be_bytes([var[0], var[1]]);
                let mut topics = Vec::new();
                let mut pos = 2;
                while pos < var.len() {
                    let t = get_str(var, &mut pos)?;
                    let qos = *var.get(pos)?;
                    pos += 1;
                    topics.push((t, qos));
                }
                Packet::Subscribe { packet_id, topics }
            }
            9 => Packet::SubAck {
                packet_id: u16::from_be_bytes([var[0], var[1]]),
                codes: var[2..].to_vec(),
            },
            12 => Packet::PingReq,
            13 => Packet::PingResp,
            14 => Packet::Disconnect,
            _ => return None,
        };
        Some((packet, total))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every packet this codec writes, read back. The encode and decode halves
    /// share no code, so a round trip catches a change to either.
    #[test]
    fn every_packet_round_trips() {
        let packets = vec![
            Packet::Connect {
                client_id: "dev-01".to_string(),
                keep_alive_s: 30,
                username: None,
                password: None,
            },
            Packet::Connect {
                client_id: "dev-02".to_string(),
                keep_alive_s: 60,
                username: Some("user".to_string()),
                password: Some("pass".to_string()),
            },
            Packet::ConnAck { session_present: false, code: 0 },
            Packet::Publish {
                topic: "sensors/temp".to_string(),
                payload: b"25.5".to_vec(),
                qos: 0,
                packet_id: None,
            },
            Packet::Publish {
                topic: "cmd".to_string(),
                payload: b"on".to_vec(),
                qos: 1,
                packet_id: Some(7),
            },
            Packet::PubAck { packet_id: 7 },
            Packet::Subscribe {
                packet_id: 2,
                topics: vec![("a/b".to_string(), 0), ("c/#".to_string(), 1)],
            },
            Packet::SubAck { packet_id: 2, codes: vec![0, 1] },
            Packet::PingReq,
            Packet::PingResp,
            Packet::Disconnect,
        ];
        for p in packets {
            let bytes = p.encode();
            let (decoded, used) = Packet::decode(&bytes).expect("decode");
            assert_eq!(used, bytes.len(), "{p:?} left bytes unconsumed");
            assert_eq!(decoded, p);
        }
    }

    /// The remaining-length field is a base-128 varint, and its boundaries are
    /// where a hand-written codec goes wrong. A payload of 127 bytes encodes in
    /// one byte and 128 needs two; get that edge wrong and small messages work
    /// while slightly larger ones desynchronise the stream for good.
    #[test]
    fn the_length_varint_spans_its_boundaries() {
        for payload_len in [0usize, 1, 126, 127, 128, 129, 16_383, 16_384] {
            let p = Packet::Publish {
                topic: "t".to_string(),
                payload: vec![0xAB; payload_len],
                qos: 0,
                packet_id: None,
            };
            let bytes = p.encode();
            let (decoded, used) = Packet::decode(&bytes).expect("decode");
            assert_eq!(used, bytes.len(), "{payload_len} bytes");
            assert_eq!(decoded, p, "{payload_len} bytes");
        }
    }

    /// A packet arriving in pieces — which is the normal case over a serial
    /// link, where `+IPD` hands over whatever fitted in one chunk — must read
    /// as "not yet", never as a short packet.
    #[test]
    fn a_partial_packet_is_not_a_packet() {
        let whole = Packet::Publish {
            topic: "sensors/humidity".to_string(),
            payload: b"48".to_vec(),
            qos: 0,
            packet_id: None,
        }
        .encode();
        for cut in 1..whole.len() {
            assert!(
                Packet::decode(&whole[..cut]).is_none(),
                "{cut} of {} bytes decoded as a whole packet",
                whole.len()
            );
        }
        assert!(Packet::decode(&whole).is_some());
    }

    /// Two packets in one buffer: `decode` reports how much it used so the
    /// caller can take the next one. Reading only the first and discarding the
    /// rest loses messages that arrived in the same chunk.
    #[test]
    fn decoding_reports_what_it_consumed() {
        let mut stream = Packet::PingResp.encode();
        let second = Packet::Publish {
            topic: "a".to_string(),
            payload: b"1".to_vec(),
            qos: 0,
            packet_id: None,
        };
        stream.extend_from_slice(&second.encode());

        let (first, used) = Packet::decode(&stream).expect("first");
        assert_eq!(first, Packet::PingResp);
        let (rest, _) = Packet::decode(&stream[used..]).expect("second");
        assert_eq!(rest, second);
    }
}
