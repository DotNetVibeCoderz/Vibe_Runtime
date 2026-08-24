//! RNDP — the RustNet Device Protocol.
//!
//! ```text
//! 0x52 0x4E | u8 code | u32 len (LE) | payload[len] | u16 crc (LE)
//! ```
//!
//! CRC-16/CCITT-FALSE over code + length + payload. A request carries a
//! command code; a reply carries a status — [`ST_OK`], or [`ST_ERR`] with a
//! UTF-8 message.
//!
//! # This contract is implemented twice, on purpose
//!
//! Here, and in `tools/RustNet.Deploy/Rndp.cs`. One of them is `no_std` Rust
//! running on a microcontroller and the other is C# on a desktop, so there is
//! no shared implementation to be had — only a shared *specification*, which
//! is this module comment and the tests at the bottom of the file. Those tests
//! pin the wire format with literal bytes rather than round-tripping through
//! this same code, because a round trip passes just as happily when both
//! halves change together and the other implementation cannot follow.
//!
//! # Sharing a wire with a program's console
//!
//! A device has one UART, and the program running on it is already writing to
//! it. So a decoder must expect to find console text between frames, and both
//! ends resynchronise the same way: skip forward to the next `RN`, try again,
//! and treat a bad checksum as "that was not a frame" rather than "the link is
//! broken". [`Decoder`] does that; the C# client's `SkipToMagic` is its twin.
//!
//! The checksum is what makes this safe rather than hopeful. Console output
//! *will* eventually contain the two bytes `RN` — `WARN` alone is enough — and
//! [`MAX_PAYLOAD`] plus the CRC are what stop such a coincidence being
//! executed as a command.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

pub const MAGIC: [u8; 2] = [0x52, 0x4E];
pub const PROTOCOL_VERSION: u8 = 1;

/// Bytes before the payload: magic, code, length.
const HEADER: usize = 2 + 1 + 4;
/// Bytes after it.
const TRAILER: usize = 2;
/// The smallest thing that can possibly be a frame.
pub const MIN_FRAME: usize = HEADER + TRAILER;

/// Largest payload a frame may declare.
///
/// 8 MB, matching `RndpFrame.MaxPayload` on the C# side — the two bounds have
/// to agree, or one end rejects what the other happily sends. It is also far
/// more than any board here can hold, which is the point: the bound exists to
/// tell a frame from a coincidence in log text, not to size a buffer.
pub const MAX_PAYLOAD: usize = 8 * 1024 * 1024;

// ── command codes ───────────────────────────────────────────────────────────
pub const CMD_PING: u8 = 0x01;
pub const CMD_INFO: u8 = 0x02;
pub const CMD_PROVISION_KEY: u8 = 0x03;
pub const CMD_LIST_APPS: u8 = 0x10;
pub const CMD_FLASH_APP: u8 = 0x11;
pub const CMD_ERASE_APP: u8 = 0x12;
pub const CMD_START_APP: u8 = 0x13;
pub const CMD_STOP_APP: u8 = 0x14;
pub const CMD_SET_AUTOSTART: u8 = 0x15;
pub const CMD_FLASH_DATA: u8 = 0x20;
pub const CMD_READ_DATA: u8 = 0x21;
pub const CMD_SET_CONFIG: u8 = 0x30;
pub const CMD_GET_CONFIG: u8 = 0x31;
pub const CMD_WIFI_CONFIG: u8 = 0x32;
pub const CMD_GET_LOGS: u8 = 0x40;
pub const CMD_GET_PERF: u8 = 0x41;
pub const CMD_SET_BOOT_IMAGE: u8 = 0x50;
pub const CMD_GET_BOOT_IMAGE: u8 = 0x51;
pub const CMD_GET_DISPLAY: u8 = 0x52;
pub const CMD_IO_STATE: u8 = 0x53;
pub const CMD_OTA_BEGIN: u8 = 0x60;
pub const CMD_OTA_DATA: u8 = 0x61;
pub const CMD_OTA_END: u8 = 0x62;
pub const CMD_OTA_CONFIRM: u8 = 0x63;
pub const CMD_OTA_ROLLBACK: u8 = 0x64;
pub const CMD_DEBUG_SET_BP: u8 = 0x70;
pub const CMD_DEBUG_CONTINUE: u8 = 0x71;
pub const CMD_DEBUG_STEP: u8 = 0x72;
pub const CMD_DEBUG_STACK: u8 = 0x73;
pub const CMD_DEBUG_CLEAR_BP: u8 = 0x74;
pub const CMD_DEBUG_STATE: u8 = 0x75;
pub const CMD_DEBUG_LOCALS: u8 = 0x76;
pub const CMD_REBOOT: u8 = 0x7F;

// ── status codes ────────────────────────────────────────────────────────────
pub const ST_OK: u8 = 0x00;
pub const ST_ERR: u8 = 0x01;

/// The name a command goes by, for logs and error messages.
///
/// A bare number in a diagnostic is a number somebody has to go and look up.
/// An unrecognised code answers `None` rather than inventing a name for it.
pub fn command_name(code: u8) -> Option<&'static str> {
    Some(match code {
        CMD_PING => "PING",
        CMD_INFO => "INFO",
        CMD_PROVISION_KEY => "PROVISION_KEY",
        CMD_LIST_APPS => "LIST_APPS",
        CMD_FLASH_APP => "FLASH_APP",
        CMD_ERASE_APP => "ERASE_APP",
        CMD_START_APP => "START_APP",
        CMD_STOP_APP => "STOP_APP",
        CMD_SET_AUTOSTART => "SET_AUTOSTART",
        CMD_FLASH_DATA => "FLASH_DATA",
        CMD_READ_DATA => "READ_DATA",
        CMD_SET_CONFIG => "SET_CONFIG",
        CMD_GET_CONFIG => "GET_CONFIG",
        CMD_WIFI_CONFIG => "WIFI_CONFIG",
        CMD_GET_LOGS => "GET_LOGS",
        CMD_GET_PERF => "GET_PERF",
        CMD_SET_BOOT_IMAGE => "SET_BOOT_IMAGE",
        CMD_GET_BOOT_IMAGE => "GET_BOOT_IMAGE",
        CMD_GET_DISPLAY => "GET_DISPLAY",
        CMD_IO_STATE => "IO_STATE",
        CMD_OTA_BEGIN => "OTA_BEGIN",
        CMD_OTA_DATA => "OTA_DATA",
        CMD_OTA_END => "OTA_END",
        CMD_OTA_CONFIRM => "OTA_CONFIRM",
        CMD_OTA_ROLLBACK => "OTA_ROLLBACK",
        CMD_DEBUG_SET_BP => "DEBUG_SET_BP",
        CMD_DEBUG_CONTINUE => "DEBUG_CONTINUE",
        CMD_DEBUG_STEP => "DEBUG_STEP",
        CMD_DEBUG_STACK => "DEBUG_STACK",
        CMD_DEBUG_CLEAR_BP => "DEBUG_CLEAR_BP",
        CMD_DEBUG_STATE => "DEBUG_STATE",
        CMD_DEBUG_LOCALS => "DEBUG_LOCALS",
        CMD_REBOOT => "REBOOT",
        _ => return None,
    })
}

/// CRC-16/CCITT-FALSE (polynomial 0x1021, initial value 0xFFFF).
pub fn crc16(data: &[u8]) -> u16 {
    let mut crc = 0xFFFFu16;
    for &b in data {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x1021 } else { crc << 1 };
        }
    }
    crc
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub code: u8,
    pub payload: Vec<u8>,
}

impl Frame {
    pub fn new(code: u8, payload: impl Into<Vec<u8>>) -> Self {
        Self { code, payload: payload.into() }
    }

    pub fn ok(payload: impl Into<Vec<u8>>) -> Self {
        Self::new(ST_OK, payload)
    }

    pub fn err(message: impl AsRef<str>) -> Self {
        Self::new(ST_ERR, message.as_ref().as_bytes().to_vec())
    }

    pub fn is_ok(&self) -> bool {
        self.code == ST_OK
    }

    /// The payload as text, for the many commands whose answer is a string.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.payload).into_owned()
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(MIN_FRAME + self.payload.len());
        out.extend_from_slice(&MAGIC);
        out.push(self.code);
        out.extend_from_slice(&(self.payload.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.payload);
        // The checksum covers code, length and payload — everything but the
        // magic, which is a marker rather than content.
        let crc = crc16(&out[MAGIC.len()..]);
        out.extend_from_slice(&crc.to_le_bytes());
        out
    }
}

/// Why a buffer did not yield a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// The buffer does not start with [`MAGIC`].
    BadMagic,
    /// The declared length exceeds [`MAX_PAYLOAD`].
    ImplausibleLength(u32),
    /// The checksum did not match. Both values are reported because a
    /// transposition and a truncation look different in the two numbers.
    BadCrc { got: u16, want: u16 },
}

/// Try to read one frame from the front of `buf`.
///
/// `Ok(None)` means "not yet" — the buffer holds a prefix of a frame, and more
/// bytes may complete it. That is deliberately distinct from an error: a
/// caller must *keep* a partial frame and must *discard* a corrupt one.
pub fn decode(buf: &[u8]) -> Result<Option<(Frame, usize)>, DecodeError> {
    if buf.len() < MIN_FRAME {
        return Ok(None);
    }
    if buf[0..2] != MAGIC {
        return Err(DecodeError::BadMagic);
    }
    let code = buf[2];
    let len = u32::from_le_bytes([buf[3], buf[4], buf[5], buf[6]]);
    if len as usize > MAX_PAYLOAD {
        // Checked before it is used as a length: on a 32-bit target a value
        // near `u32::MAX` would otherwise become an allocation request no
        // board can meet, from what is almost certainly a stray "RN" in log
        // output rather than a frame at all.
        return Err(DecodeError::ImplausibleLength(len));
    }
    let len = len as usize;
    let total = HEADER + len + TRAILER;
    if buf.len() < total {
        return Ok(None);
    }
    let want = crc16(&buf[MAGIC.len()..HEADER + len]);
    let got = u16::from_le_bytes([buf[HEADER + len], buf[HEADER + len + 1]]);
    if got != want {
        return Err(DecodeError::BadCrc { got, want });
    }
    Ok(Some((Frame { code, payload: buf[HEADER..HEADER + len].to_vec() }, total)))
}

/// A resynchronising byte-stream decoder.
///
/// Feed it whatever arrived; take whatever frames are complete. Anything that
/// is not a frame — a boot banner, the running program's console output, line
/// noise on a serial link — is skipped rather than treated as an error,
/// because on a device that shares one UART between a console and this
/// protocol, non-frame bytes are the normal case rather than a fault.
#[derive(Debug, Default)]
pub struct Decoder {
    buffer: Vec<u8>,
}

impl Decoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }

    /// Bytes currently held but not yet part of a complete frame.
    pub fn buffered(&self) -> usize {
        self.buffer.len()
    }

    /// Forget everything buffered.
    ///
    /// For a caller that has decided the link is desynchronised beyond what
    /// skipping can fix — after a device reset, say.
    pub fn reset(&mut self) {
        self.buffer.clear();
    }

    /// The next complete frame, if the bytes so far contain one.
    ///
    /// Skips any leading rubbish. Returns `None` when what remains is either
    /// empty or a genuine prefix of a frame still arriving.
    pub fn next_frame(&mut self) -> Option<Frame> {
        loop {
            self.skip_to_magic();
            match decode(&self.buffer) {
                Ok(Some((frame, used))) => {
                    self.buffer.drain(..used);
                    return Some(frame);
                }
                Ok(None) => return None,
                // Not a frame after all: drop the byte the magic started on
                // and look for the next one. One byte, not two, so a real
                // frame beginning at the second byte is still found.
                Err(_) => {
                    self.buffer.remove(0);
                }
            }
        }
    }

    fn skip_to_magic(&mut self) {
        if let Some(at) = self.buffer.windows(2).position(|w| w == MAGIC) {
            self.buffer.drain(..at);
            return;
        }
        // No magic anywhere in the buffer. Everything except a possible first
        // half of one at the very end is rubbish, and keeping it would grow
        // the buffer without bound on a link carrying a chatty console.
        let keep = usize::from(self.buffer.last() == Some(&MAGIC[0]));
        let drop = self.buffer.len() - keep;
        self.buffer.drain(..drop);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// The wire format, as literal bytes.
    ///
    /// Pinned rather than round-tripped, because the other implementation of
    /// this protocol is in C# and cannot follow a change made here.
    #[test]
    fn a_ping_is_exactly_these_nine_bytes() {
        let bytes = Frame::new(CMD_PING, Vec::new()).encode();
        assert_eq!(
            bytes,
            vec![
                0x52, 0x4E, // magic
                0x01, // PING
                0x00, 0x00, 0x00, 0x00, // length 0, little-endian
                0x5D, 0xBB, // CRC-16/CCITT-FALSE over [01 00 00 00 00] = 0xBB5D, little-endian
            ]
        );
    }

    #[test]
    fn crc16_matches_the_published_check_value() {
        // Every CRC-16/CCITT-FALSE specification quotes this one.
        assert_eq!(crc16(b"123456789"), 0x29B1);
    }

    #[test]
    fn a_frame_survives_a_round_trip() {
        let frame = Frame::new(CMD_FLASH_APP, b"\x04demoMZ\x90\x00".to_vec());
        let bytes = frame.encode();
        let (decoded, used) = decode(&bytes).unwrap().unwrap();
        assert_eq!(used, bytes.len());
        assert_eq!(decoded, frame);
    }

    #[test]
    fn a_partial_frame_is_not_an_error() {
        let bytes = Frame::new(CMD_PING, vec![1, 2, 3]).encode();
        for cut in 0..bytes.len() {
            assert_eq!(decode(&bytes[..cut]), Ok(None), "a cut at {cut} should be incomplete");
        }
    }

    #[test]
    fn a_corrupt_checksum_is_rejected() {
        let mut bytes = Frame::new(CMD_PING, vec![1, 2, 3]).encode();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF;
        assert!(matches!(decode(&bytes), Err(DecodeError::BadCrc { .. })));
    }

    #[test]
    fn an_implausible_length_is_rejected_before_it_is_used() {
        let mut bytes = Frame::new(CMD_PING, Vec::new()).encode();
        bytes[3..7].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(decode(&bytes), Err(DecodeError::ImplausibleLength(u32::MAX)));
    }

    /// The case this protocol actually lives in: one UART, a chatty program.
    #[test]
    fn frames_are_found_between_a_programs_console_output() {
        let mut decoder = Decoder::new();
        decoder.push(b"[intro] 92 frames in 5040 ms\n");
        decoder.push(&Frame::new(CMD_PING, Vec::new()).encode());
        decoder.push(b"[3d] 191 frames in 9001 ms\n");
        decoder.push(&Frame::new(CMD_INFO, Vec::new()).encode());

        assert_eq!(decoder.next_frame().map(|f| f.code), Some(CMD_PING));
        assert_eq!(decoder.next_frame().map(|f| f.code), Some(CMD_INFO));
        assert_eq!(decoder.next_frame(), None);
    }

    /// Console text containing the magic bytes must not become a command.
    ///
    /// "RN" is two extremely ordinary letters: `WARN`, `LEARNING`, `TURN` and
    /// `BURNT` all carry it. A device that rebooted because a log line said
    /// `WARN` would be indefensible, and the CRC is what prevents it.
    #[test]
    fn the_letters_rn_in_a_log_line_are_not_a_command() {
        let mut decoder = Decoder::new();
        decoder.push(b"WARN: LEARNING TURN BURNT\n");
        assert_eq!(decoder.next_frame(), None);

        // And a real frame arriving after the near-misses is still found.
        decoder.push(&Frame::new(CMD_REBOOT, Vec::new()).encode());
        assert_eq!(decoder.next_frame().map(|f| f.code), Some(CMD_REBOOT));
    }

    /// A frame split across reads is reassembled rather than lost.
    #[test]
    fn a_frame_arriving_one_byte_at_a_time_is_still_read() {
        let bytes = Frame::new(CMD_GET_LOGS, b"hello".to_vec()).encode();
        let mut decoder = Decoder::new();
        for (i, b) in bytes.iter().enumerate() {
            decoder.push(&[*b]);
            if i + 1 < bytes.len() {
                assert_eq!(decoder.next_frame(), None, "complete after only {} bytes", i + 1);
            }
        }
        assert_eq!(decoder.next_frame().map(|f| f.code), Some(CMD_GET_LOGS));
    }

    /// A console that never sends a frame must not grow the buffer for ever.
    #[test]
    fn console_output_alone_does_not_grow_the_buffer() {
        let mut decoder = Decoder::new();
        for _ in 0..1000 {
            decoder.push(b"scene: gradients\nscene: text\nscene: 3d cube\n");
            decoder.next_frame();
        }
        assert!(
            decoder.buffered() <= 1,
            "buffered {} bytes of pure console output",
            decoder.buffered()
        );
    }

    #[test]
    fn every_command_code_has_a_name() {
        for code in [CMD_PING, CMD_FLASH_APP, CMD_START_APP, CMD_GET_DISPLAY, CMD_REBOOT] {
            assert!(command_name(code).is_some(), "{code:#04x} has no name");
        }
        assert_eq!(command_name(0xAB), None);
    }
}
