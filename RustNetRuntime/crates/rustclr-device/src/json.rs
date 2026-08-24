//! Just enough JSON to write [`crate::DeviceInfo`].
//!
//! Hand-written, like the debug adapter's: this workspace has no external
//! dependencies, and the only value ever serialised here is one flat struct
//! with known field names. A serialiser would be more code than the thing it
//! serialises.
//!
//! Reading JSON is not needed and is not here. The Workbench parses the
//! device's answer; the device never parses the Workbench's.

use alloc::format;
use alloc::string::String;

/// Escape a string for a JSON double-quoted literal.
///
/// A board name comes from a firmware and an application name comes off the
/// wire, so neither is guaranteed free of a quote or a backslash. One of those
/// unescaped turns the whole readout into a parse error, and the Workbench
/// then falls back to showing raw text instead of the fields — which looks
/// like a device answering nonsense rather than a missing backslash.
pub fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // Control characters are not legal raw in a JSON string, and a
            // device name read out of flash can contain anything at all.
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_text_is_unchanged() {
        assert_eq!(escape("M5Stack Tough"), "M5Stack Tough");
    }

    #[test]
    fn quotes_and_backslashes_are_escaped() {
        assert_eq!(escape("a\"b"), "a\\\"b");
        assert_eq!(escape("a\\b"), "a\\\\b");
    }

    #[test]
    fn control_characters_do_not_survive_raw() {
        assert_eq!(escape("a\nb"), "a\\nb");
        assert_eq!(escape("a\tb"), "a\\tb");
        assert_eq!(escape("a\u{1}b"), "a\\u0001b");
        // Nothing below 0x20 may appear literally in the output.
        assert!(!escape("x\u{7}y").chars().any(|c| (c as u32) < 0x20));
    }
}
