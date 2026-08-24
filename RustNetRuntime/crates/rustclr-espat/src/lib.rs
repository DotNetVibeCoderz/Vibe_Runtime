//! Espressif's AT command set, as far as it can be understood without a UART.
//!
//! A companion radio spoken to over a serial line is a text protocol, and the
//! text half of it has no business being tied to a chip. This crate holds that
//! half — building command lines, deciding when a reply has finished, pulling
//! the useful field out of one — so that it is covered by the ordinary host
//! test run rather than only by whatever happens to be on a desk. The port
//! supplies the bytes.
//!
//! The commands are Espressif's, cross-checked against MaixPy's `esp8285.c`.
//! Two behaviours of the module drive most of what is here.
//!
//! **A reply ends with a terminator line, not with silence.** `AT+CWJAP`
//! answers `WIFI DISCONNECT`, then says nothing at all for several seconds
//! while it associates, and only then finishes. Treating the first quiet
//! moment as the end of the reply reports a failure for a join that was still
//! in progress — and it looks exactly like a wrong password, which is a long
//! way to go down the wrong road.
//!
//! **The module's own status lines are interleaved with command replies.**
//! `WIFI CONNECTED`, `WIFI GOT IP` and `+IPD` arrive unbidden, so a parser that
//! assumes reply line one belongs to the command it just sent will misread the
//! first one that does not.

#![no_std]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Has the module finished answering?
///
/// AT replies end with a line of their own — `OK`, `ERROR`, `FAIL`, or `busy`
/// — and everything before it is progress reporting. Whole lines, not
/// substrings: `AT+CIFSR` prints addresses and network names, and one that
/// happened to contain `OK` would otherwise end the read early.
pub fn is_terminated(reply: &str) -> bool {
    reply.lines().map(str::trim).any(|line| {
        line == "OK" || line == "ERROR" || line.starts_with("FAIL") || line.starts_with("busy")
    })
}

/// How a finished reply turned out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Ok,
    Failed,
    /// Nothing conclusive arrived — the budget ran out mid-reply.
    Incomplete,
}

/// Read the outcome out of a reply.
///
/// Failure is checked before success on purpose. A failed join produces a
/// reply that contains `FAIL` and, depending on what was left in the module's
/// buffer, can also contain a trailing `OK` from something earlier. Looking
/// for `OK` first turns that into a successful join with no address.
pub fn outcome(reply: &str) -> Outcome {
    let mut lines = reply.lines().map(str::trim);
    if lines
        .clone()
        .any(|l| l == "ERROR" || l.starts_with("FAIL") || l.starts_with("+CWJAP:"))
    {
        return Outcome::Failed;
    }
    if lines.any(|l| l == "OK") {
        return Outcome::Ok;
    }
    Outcome::Incomplete
}

/// The station address out of an `AT+CIFSR` reply.
///
/// The reply carries several lines — the soft-AP's address as well as the
/// station's, plus both MAC addresses — and the soft-AP's comes *first*.
/// Taking line one reports an address that is real, belongs to the module, and
/// routes nowhere.
pub fn station_ip(reply: &str) -> Option<String> {
    for line in reply.lines().map(str::trim) {
        let Some(rest) = line.strip_prefix("+CIFSR:STAIP,") else {
            continue;
        };
        let ip = rest.trim().trim_matches('"');
        // An associated but unaddressed module reports 0.0.0.0, which is not an
        // address anyone can use and must not read as a completed join.
        if !ip.is_empty() && ip != "0.0.0.0" {
            return Some(ip.to_string());
        }
    }
    None
}

/// Escape the characters Espressif's parser treats specially inside a quoted
/// argument.
///
/// Real: an SSID or PSK containing a quote, a backslash or a comma has to
/// arrive escaped, or the module parses the command short and answers `ERROR`
/// — which reads as a wrong password rather than as a quoting bug.
pub fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        if c == '"' || c == '\\' || c == ',' {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// `AT+CWJAP`, quoted and escaped.
pub fn join_command(ssid: &str, psk: &str) -> String {
    let mut cmd = String::from("AT+CWJAP=\"");
    cmd.push_str(&escape(ssid));
    cmd.push_str("\",\"");
    cmd.push_str(&escape(psk));
    cmd.push('"');
    cmd
}

/// A reply trimmed to something that fits in a log line, with the echo of the
/// command dropped — it is never the interesting part.
pub fn summarise(reply: &str, limit: usize) -> String {
    let mut out = String::new();
    for line in reply.lines().map(str::trim) {
        if line.is_empty() || line.starts_with("AT") {
            continue;
        }
        if !out.is_empty() {
            out.push_str("; ");
        }
        out.push_str(line);
    }
    if out.len() > limit {
        // Truncate on a character boundary: an AT reply is normally ASCII, but
        // a network name need not be, and slicing mid-character panics.
        let mut end = limit;
        while end > 0 && !out.is_char_boundary(end) {
            end -= 1;
        }
        out.truncate(end);
    }
    out
}

/// Pull the payload bytes out of a stream containing `+IPD` announcements.
///
/// The module reports received data as `+IPD,<len>:` followed by exactly
/// `<len>` bytes, and those announcements are interleaved with command replies
/// rather than answering them — a broker can publish while an `AT+CIPSEND` is
/// still being acknowledged. So this walks the whole buffer and takes every
/// payload it finds, in order, ignoring everything else.
///
/// The declared length is obeyed rather than trusted to line endings. Payloads
/// are binary here (MQTT packets), so they contain `\r`, `\n`, and the text
/// `+IPD` itself as often as chance allows; scanning for a terminator instead
/// of counting would truncate a packet the first time its bytes looked like
/// punctuation.
pub fn extract_ipd(stream: &[u8]) -> Vec<u8> {
    const MARKER: &[u8] = b"+IPD,";
    let mut out = Vec::new();
    let mut at = 0usize;
    while at + MARKER.len() <= stream.len() {
        if &stream[at..at + MARKER.len()] != MARKER {
            at += 1;
            continue;
        }
        let mut cursor = at + MARKER.len();
        let mut len = 0usize;
        let mut digits = 0;
        while cursor < stream.len() && stream[cursor].is_ascii_digit() {
            len = len * 10 + (stream[cursor] - b'0') as usize;
            cursor += 1;
            digits += 1;
        }
        // A marker with no length, or one whose payload has not all arrived,
        // is not an error — it is a chunk boundary. Leave it and let the next
        // read see the whole thing.
        if digits == 0 || cursor >= stream.len() || stream[cursor] != b':' {
            at += MARKER.len();
            continue;
        }
        cursor += 1;
        let end = cursor + len;
        if end > stream.len() {
            break;
        }
        out.extend_from_slice(&stream[cursor..end]);
        at = end;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The failure this crate exists to prevent: a join goes quiet mid-reply,
    /// and a read that stops there reports a failure for a join in progress.
    #[test]
    fn a_join_is_not_over_until_it_terminates() {
        assert!(!is_terminated("WIFI DISCONNECT\r\n"));
        assert!(!is_terminated("WIFI CONNECTED\r\nWIFI GOT IP\r\n"));
        assert!(is_terminated("WIFI CONNECTED\r\nWIFI GOT IP\r\n\r\nOK\r\n"));
        assert!(is_terminated("ERROR\r\n"));
        assert!(is_terminated("+CWJAP:1\r\nFAIL\r\n"));
        assert!(is_terminated("busy p...\r\n"));
    }

    /// Whole lines, not substrings. A network called `OKNET` in a `CIFSR`
    /// reply must not look like a terminator.
    #[test]
    fn a_terminator_is_a_line_of_its_own() {
        assert!(!is_terminated("+CIFSR:STAIP,\"10.0.0.5\" OKNET\r\n"));
    }

    /// A stale `OK` left in the module's buffer sits in front of a real
    /// failure. Checking success first would call that join a success.
    #[test]
    fn a_failure_outranks_a_leftover_ok() {
        assert_eq!(outcome("OK\r\n+CWJAP:3\r\nFAIL\r\n"), Outcome::Failed);
        assert_eq!(outcome("WIFI GOT IP\r\n\r\nOK\r\n"), Outcome::Ok);
        assert_eq!(outcome("WIFI DISCONNECT\r\n"), Outcome::Incomplete);
    }

    /// `AT+CIFSR` lists the soft-AP's address first. Taking it reports an
    /// address that exists, is the module's, and routes nowhere.
    #[test]
    fn the_station_address_is_not_the_first_one_listed() {
        let reply = "+CIFSR:APIP,\"192.168.4.1\"\r\n\
                     +CIFSR:APMAC,\"1a:fe:34:a5:8d:c6\"\r\n\
                     +CIFSR:STAIP,\"192.168.1.57\"\r\n\
                     +CIFSR:STAMAC,\"18:fe:34:a5:8d:c6\"\r\nOK";
        assert_eq!(station_ip(reply).as_deref(), Some("192.168.1.57"));
    }

    /// Associated but not addressed. Not a usable address, and not a join.
    #[test]
    fn an_unassigned_address_is_not_an_address() {
        assert_eq!(station_ip("+CIFSR:STAIP,\"0.0.0.0\"\r\nOK"), None);
    }

    /// A PSK with a quote in it has to reach the module escaped, or its parser
    /// ends the argument early and answers ERROR — which looks like a wrong
    /// password.
    #[test]
    fn quotes_commas_and_backslashes_survive_the_command_line() {
        assert_eq!(escape(r#"pa"ss,word\1"#), r#"pa\"ss\,word\\1"#);
        assert_eq!(escape("ordinary"), "ordinary");
        assert_eq!(
            join_command("my net", "s3cret"),
            "AT+CWJAP=\"my net\",\"s3cret\""
        );
    }

    /// A network name need not be ASCII, and truncating mid-character panics.
    #[test]
    fn summarising_does_not_split_a_character() {
        let reply = "AT+CWJAP\r\n+CWJAP:\"café-réseau-très-long\"\r\nOK\r\n";
        let short = summarise(reply, 20);
        assert!(short.len() <= 20);
        assert!(reply.contains(&short));
    }

    /// Payloads are MQTT packets, so they contain CR, LF and `+IPD` as often
    /// as chance allows. The declared length is what says where one ends;
    /// scanning for a line ending would truncate the first packet whose bytes
    /// happened to look like punctuation.
    #[test]
    fn an_ipd_payload_is_counted_not_scanned() {
        let mut stream = Vec::from(&b"\r\nOK\r\n+IPD,6:"[..]);
        stream.extend_from_slice(b"\r\n+IPD");
        stream.extend_from_slice(b"\r\nSEND OK\r\n");
        assert_eq!(extract_ipd(&stream), b"\r\n+IPD");
    }

    /// Two announcements in one chunk. Taking only the first loses whatever
    /// the broker published in the same breath.
    #[test]
    fn every_announcement_in_the_buffer_is_taken() {
        let stream = b"+IPD,3:abc\r\n+IPD,2:de\r\nOK\r\n";
        assert_eq!(extract_ipd(stream), b"abcde");
    }

    /// A payload cut off by a chunk boundary is not a short payload. Returning
    /// the fragment would hand a truncated MQTT packet to the decoder and
    /// desynchronise the stream for good.
    #[test]
    fn a_truncated_payload_waits_rather_than_arriving_short() {
        assert_eq!(extract_ipd(b"+IPD,10:abc"), b"");
        // ...and the marker itself can be split, which must not consume it.
        assert_eq!(extract_ipd(b"OK\r\n+IP"), b"");
        assert_eq!(extract_ipd(b"+IPD,"), b"");
    }

    /// Nothing but command traffic: no payload, and no panic walking it.
    #[test]
    fn a_stream_with_no_data_yields_none() {
        assert_eq!(extract_ipd(b"AT+CIPSEND=12\r\n\r\nOK\r\n> "), b"");
        assert_eq!(extract_ipd(b""), b"");
    }

    /// The echo is dropped and the informative lines are kept.
    #[test]
    fn the_echo_is_not_the_interesting_part() {
        assert_eq!(summarise("AT+CWMODE=1\r\nERROR\r\n", 120), "ERROR");
    }
}
