//! Modbus master: RTU (CRC-16 over serial) and TCP (MBAP) framing with a
//! shared PDU layer. Encoding writes into caller buffers — no per-frame
//! heap traffic on the request path — so RTU on a slow UART costs only the
//! wire time.
//!
//! A [`SimSlave`] backs the virtual device and the tests; real slaves are
//! reached through whatever byte pipe the firmware provides.

/// Modbus CRC-16 (poly 0xA001 reflected, init 0xFFFF), appended LSB first.
pub fn crc16(data: &[u8]) -> u16 {
    let mut crc = 0xFFFFu16;
    for byte in data {
        crc ^= *byte as u16;
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xA001;
            } else {
                crc >>= 1;
            }
        }
    }
    crc
}

/// Wrap a PDU into an RTU ADU: `unit | pdu | crc_lo crc_hi`.
pub fn rtu_encode(unit: u8, pdu: &[u8], out: &mut Vec<u8>) {
    out.clear();
    out.push(unit);
    out.extend_from_slice(pdu);
    let crc = crc16(out);
    out.push((crc & 0xFF) as u8);
    out.push((crc >> 8) as u8);
}

/// Validate CRC and split an RTU ADU into (unit, pdu).
pub fn rtu_decode(adu: &[u8]) -> Result<(u8, &[u8]), String> {
    if adu.len() < 4 {
        return Err("RTU frame too short".into());
    }
    let (body, crc_bytes) = adu.split_at(adu.len() - 2);
    let want = u16::from_le_bytes([crc_bytes[0], crc_bytes[1]]);
    let got = crc16(body);
    if want != got {
        return Err(format!("RTU CRC mismatch: got {got:04X}, want {want:04X}"));
    }
    Ok((body[0], &body[1..]))
}

/// Wrap a PDU into a Modbus-TCP ADU (MBAP header, no CRC).
pub fn tcp_encode(txn: u16, unit: u8, pdu: &[u8], out: &mut Vec<u8>) {
    out.clear();
    out.extend_from_slice(&txn.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes()); // protocol id
    out.extend_from_slice(&((pdu.len() + 1) as u16).to_be_bytes());
    out.push(unit);
    out.extend_from_slice(pdu);
}

/// Split a Modbus-TCP ADU into (txn, unit, pdu).
pub fn tcp_decode(adu: &[u8]) -> Result<(u16, u8, &[u8]), String> {
    if adu.len() < 8 {
        return Err("MBAP frame too short".into());
    }
    let txn = u16::from_be_bytes([adu[0], adu[1]]);
    let len = u16::from_be_bytes([adu[4], adu[5]]) as usize;
    if adu.len() < 6 + len || len < 1 {
        return Err("MBAP length mismatch".into());
    }
    Ok((txn, adu[6], &adu[7..6 + len]))
}

// ------------------------------------------------------------- PDUs --

pub mod fc {
    pub const READ_COILS: u8 = 0x01;
    pub const READ_DISCRETE: u8 = 0x02;
    pub const READ_HOLDING: u8 = 0x03;
    pub const READ_INPUT: u8 = 0x04;
    pub const WRITE_COIL: u8 = 0x05;
    pub const WRITE_REGISTER: u8 = 0x06;
    pub const WRITE_COILS: u8 = 0x0F;
    pub const WRITE_REGISTERS: u8 = 0x10;
}

pub fn build_read(function: u8, addr: u16, count: u16, pdu: &mut Vec<u8>) {
    pdu.clear();
    pdu.push(function);
    pdu.extend_from_slice(&addr.to_be_bytes());
    pdu.extend_from_slice(&count.to_be_bytes());
}

pub fn build_write_coil(addr: u16, on: bool, pdu: &mut Vec<u8>) {
    pdu.clear();
    pdu.push(fc::WRITE_COIL);
    pdu.extend_from_slice(&addr.to_be_bytes());
    pdu.extend_from_slice(if on { &[0xFF, 0x00] } else { &[0x00, 0x00] });
}

pub fn build_write_register(addr: u16, value: u16, pdu: &mut Vec<u8>) {
    pdu.clear();
    pdu.push(fc::WRITE_REGISTER);
    pdu.extend_from_slice(&addr.to_be_bytes());
    pdu.extend_from_slice(&value.to_be_bytes());
}

pub fn build_write_registers(addr: u16, values: &[u16], pdu: &mut Vec<u8>) {
    pdu.clear();
    pdu.push(fc::WRITE_REGISTERS);
    pdu.extend_from_slice(&addr.to_be_bytes());
    pdu.extend_from_slice(&(values.len() as u16).to_be_bytes());
    pdu.push((values.len() * 2) as u8);
    for v in values {
        pdu.extend_from_slice(&v.to_be_bytes());
    }
}

pub fn build_write_coils(addr: u16, bits: &[bool], pdu: &mut Vec<u8>) {
    pdu.clear();
    pdu.push(fc::WRITE_COILS);
    pdu.extend_from_slice(&addr.to_be_bytes());
    pdu.extend_from_slice(&(bits.len() as u16).to_be_bytes());
    let nbytes = bits.len().div_ceil(8);
    pdu.push(nbytes as u8);
    for chunk in bits.chunks(8) {
        let mut b = 0u8;
        for (i, bit) in chunk.iter().enumerate() {
            if *bit {
                b |= 1 << i;
            }
        }
        pdu.push(b);
    }
}

/// Check for a Modbus exception response; returns the response payload
/// (after the function code) on success.
pub fn parse_response<'a>(request_fn: u8, pdu: &'a [u8]) -> Result<&'a [u8], String> {
    if pdu.is_empty() {
        return Err("empty response PDU".into());
    }
    if pdu[0] == request_fn | 0x80 {
        let code = pdu.get(1).copied().unwrap_or(0);
        let name = match code {
            1 => "illegal function",
            2 => "illegal data address",
            3 => "illegal data value",
            4 => "slave device failure",
            _ => "exception",
        };
        return Err(format!("modbus exception {code}: {name}"));
    }
    if pdu[0] != request_fn {
        return Err(format!("unexpected function {:#04X}", pdu[0]));
    }
    Ok(&pdu[1..])
}

/// Decode a read-bits response into booleans.
pub fn parse_bits(payload: &[u8], count: usize) -> Result<Vec<bool>, String> {
    if payload.is_empty() || payload.len() < 1 + payload[0] as usize {
        return Err("short bit response".into());
    }
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let byte = payload[1 + i / 8];
        out.push(byte & (1 << (i % 8)) != 0);
    }
    Ok(out)
}

/// Decode a read-registers response into u16 words.
pub fn parse_registers(payload: &[u8]) -> Result<Vec<u16>, String> {
    if payload.is_empty() || payload.len() < 1 + payload[0] as usize || payload[0] % 2 != 0 {
        return Err("short register response".into());
    }
    Ok(payload[1..1 + payload[0] as usize]
        .chunks_exact(2)
        .map(|c| u16::from_be_bytes([c[0], c[1]]))
        .collect())
}

// -------------------------------------------------------- simulation --

/// In-memory Modbus slave: 10k coils/discretes and 10k holding/input
/// registers. Drives tests and the virtual device.
pub struct SimSlave {
    pub unit: u8,
    pub coils: Vec<bool>,
    pub discrete: Vec<bool>,
    pub holding: Vec<u16>,
    pub input: Vec<u16>,
}

impl SimSlave {
    pub fn new(unit: u8) -> Self {
        SimSlave {
            unit,
            coils: vec![false; 10_000],
            discrete: vec![false; 10_000],
            holding: vec![0; 10_000],
            input: vec![0; 10_000],
        }
    }

    /// Process one request PDU, producing the response PDU.
    pub fn handle_pdu(&mut self, pdu: &[u8]) -> Vec<u8> {
        let err = |f: u8, code: u8| vec![f | 0x80, code];
        if pdu.len() < 5 {
            return err(pdu.first().copied().unwrap_or(0), 3);
        }
        let f = pdu[0];
        let addr = u16::from_be_bytes([pdu[1], pdu[2]]) as usize;
        let val = u16::from_be_bytes([pdu[3], pdu[4]]);
        match f {
            fc::READ_COILS | fc::READ_DISCRETE => {
                let bits = if f == fc::READ_COILS { &self.coils } else { &self.discrete };
                let count = val as usize;
                if addr + count > bits.len() {
                    return err(f, 2);
                }
                let mut out = vec![f, count.div_ceil(8) as u8];
                for chunk in bits[addr..addr + count].chunks(8) {
                    let mut b = 0u8;
                    for (i, bit) in chunk.iter().enumerate() {
                        if *bit {
                            b |= 1 << i;
                        }
                    }
                    out.push(b);
                }
                out
            }
            fc::READ_HOLDING | fc::READ_INPUT => {
                let regs = if f == fc::READ_HOLDING { &self.holding } else { &self.input };
                let count = val as usize;
                if addr + count > regs.len() {
                    return err(f, 2);
                }
                let mut out = vec![f, (count * 2) as u8];
                for r in &regs[addr..addr + count] {
                    out.extend_from_slice(&r.to_be_bytes());
                }
                out
            }
            fc::WRITE_COIL => {
                if addr >= self.coils.len() {
                    return err(f, 2);
                }
                self.coils[addr] = val == 0xFF00;
                pdu[..5].to_vec() // echo
            }
            fc::WRITE_REGISTER => {
                if addr >= self.holding.len() {
                    return err(f, 2);
                }
                self.holding[addr] = val;
                pdu[..5].to_vec()
            }
            fc::WRITE_REGISTERS => {
                let count = val as usize;
                if pdu.len() < 6 + count * 2 || addr + count > self.holding.len() {
                    return err(f, 2);
                }
                for i in 0..count {
                    self.holding[addr + i] =
                        u16::from_be_bytes([pdu[6 + i * 2], pdu[7 + i * 2]]);
                }
                let mut out = vec![f];
                out.extend_from_slice(&pdu[1..5]);
                out
            }
            fc::WRITE_COILS => {
                let count = val as usize;
                if pdu.len() < 6 || addr + count > self.coils.len() {
                    return err(f, 2);
                }
                for i in 0..count {
                    self.coils[addr + i] = pdu[6 + i / 8] & (1 << (i % 8)) != 0;
                }
                let mut out = vec![f];
                out.extend_from_slice(&pdu[1..5]);
                out
            }
            _ => err(f, 1),
        }
    }

    /// Full RTU round trip: validate the request ADU, run the PDU, frame
    /// the response. Returns None when the unit id does not match
    /// (another slave's traffic).
    pub fn handle_rtu(&mut self, adu: &[u8]) -> Option<Vec<u8>> {
        let (unit, pdu) = rtu_decode(adu).ok()?;
        if unit != self.unit && unit != 0 {
            return None;
        }
        let resp = self.handle_pdu(pdu);
        let mut out = Vec::with_capacity(resp.len() + 3);
        rtu_encode(self.unit, &resp, &mut out);
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc16_test_vector() {
        // Classic vector: 01 03 00 00 00 0A -> CRC C5 CD
        assert_eq!(crc16(&[0x01, 0x03, 0x00, 0x00, 0x00, 0x0A]), 0xCDC5);
    }

    #[test]
    fn rtu_roundtrip_and_registers() {
        let mut slave = SimSlave::new(1);
        slave.holding[100] = 1234;
        slave.holding[101] = 0xBEEF;

        let mut pdu = Vec::new();
        build_read(fc::READ_HOLDING, 100, 2, &mut pdu);
        let mut adu = Vec::new();
        rtu_encode(1, &pdu, &mut adu);
        let resp_adu = slave.handle_rtu(&adu).expect("unit match");
        let (unit, resp_pdu) = rtu_decode(&resp_adu).unwrap();
        assert_eq!(unit, 1);
        let payload = parse_response(fc::READ_HOLDING, resp_pdu).unwrap();
        assert_eq!(parse_registers(payload).unwrap(), vec![1234, 0xBEEF]);
    }

    #[test]
    fn write_then_read_coils() {
        let mut slave = SimSlave::new(9);
        let mut pdu = Vec::new();
        build_write_coils(10, &[true, false, true, true], &mut pdu);
        let resp = slave.handle_pdu(&pdu);
        parse_response(fc::WRITE_COILS, &resp).unwrap();
        build_read(fc::READ_COILS, 10, 4, &mut pdu);
        let resp = slave.handle_pdu(&pdu);
        let payload = parse_response(fc::READ_COILS, &resp).unwrap();
        assert_eq!(parse_bits(payload, 4).unwrap(), vec![true, false, true, true]);
    }

    #[test]
    fn tcp_framing_and_exception() {
        let mut pdu = Vec::new();
        build_read(fc::READ_HOLDING, 60_000, 100, &mut pdu); // out of range
        let mut adu = Vec::new();
        tcp_encode(7, 1, &pdu, &mut adu);
        let (txn, unit, got_pdu) = tcp_decode(&adu).unwrap();
        assert_eq!((txn, unit), (7, 1));
        let mut slave = SimSlave::new(1);
        let resp = slave.handle_pdu(got_pdu);
        let e = parse_response(fc::READ_HOLDING, &resp).unwrap_err();
        assert!(e.contains("illegal data address"), "{e}");
    }

    #[test]
    fn corrupted_rtu_frame_is_rejected() {
        let mut pdu = Vec::new();
        build_read(fc::READ_HOLDING, 0, 1, &mut pdu);
        let mut adu = Vec::new();
        rtu_encode(1, &pdu, &mut adu);
        adu[2] ^= 0xFF;
        assert!(rtu_decode(&adu).is_err());
    }
}
