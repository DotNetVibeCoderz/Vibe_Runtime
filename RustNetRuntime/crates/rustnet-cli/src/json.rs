//! A JSON reader and writer, sized for the Debug Adapter Protocol.
//!
//! Hand-written for the same reason the PE and metadata readers are: this
//! workspace has no external dependencies, and DAP messages are flat objects of
//! strings, numbers, booleans and short arrays. Pulling in a general-purpose
//! JSON library to carry them would be a larger change to the project than the
//! feature it serves.
//!
//! It is deliberately not a general JSON implementation. Numbers are `f64`
//! because that is what JSON says they are; there is no streaming, no borrowing
//! parser, and no attempt at speed. A debug adapter exchanges a few hundred
//! small messages over the life of a session.

use std::collections::BTreeMap;
use std::fmt::Write as _;

#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    /// Ordered, so a written message is byte-stable and a test can compare one.
    Obj(BTreeMap<String, Json>),
}

impl Json {
    pub fn obj() -> Self {
        Json::Obj(BTreeMap::new())
    }

    pub fn set(mut self, key: &str, value: Json) -> Self {
        if let Json::Obj(map) = &mut self {
            map.insert(key.to_string(), value);
        }
        self
    }

    pub fn str(value: impl Into<String>) -> Self {
        Json::Str(value.into())
    }

    pub fn num(value: impl Into<f64>) -> Self {
        Json::Num(value.into())
    }

    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(map) => map.get(key),
            _ => None,
        }
    }

    /// `a.b.c`, for reaching into a DAP request's arguments without a chain of
    /// `get` calls that each need unwrapping.
    pub fn path(&self, path: &str) -> Option<&Json> {
        let mut current = self;
        for part in path.split('.') {
            current = current.get(part)?;
        }
        Some(current)
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Json::Num(n) => Some(*n as i64),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Arr(items) => Some(items),
            _ => None,
        }
    }

    pub fn write(&self) -> String {
        let mut out = String::new();
        self.write_into(&mut out);
        out
    }

    fn write_into(&self, out: &mut String) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(true) => out.push_str("true"),
            Json::Bool(false) => out.push_str("false"),
            Json::Num(n) => {
                // Integral values are written without a fractional part: DAP
                // line numbers and sequence numbers are integers, and a client
                // reading `"seq": 3.0` is within its rights to object.
                if n.fract() == 0.0 && n.abs() < 9e15 {
                    let _ = write!(out, "{}", *n as i64);
                } else {
                    let _ = write!(out, "{n}");
                }
            }
            Json::Str(s) => write_string(s, out),
            Json::Arr(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    item.write_into(out);
                }
                out.push(']');
            }
            Json::Obj(map) => {
                out.push('{');
                for (i, (key, value)) in map.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_string(key, out);
                    out.push(':');
                    value.write_into(out);
                }
                out.push('}');
            }
        }
    }
}

fn write_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // Everything below a space has to be escaped or the document is
            // invalid; everything else, including non-ASCII, goes out as UTF-8.
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Parses one JSON value. `None` on anything malformed — a debug adapter that
/// guessed at a broken message would be worse than one that said so.
pub fn parse(text: &str) -> Option<Json> {
    let bytes: Vec<char> = text.chars().collect();
    let mut at = 0usize;
    let value = parse_value(&bytes, &mut at)?;
    skip_space(&bytes, &mut at);
    if at == bytes.len() {
        Some(value)
    } else {
        None
    }
}

fn skip_space(b: &[char], at: &mut usize) {
    while *at < b.len() && b[*at].is_whitespace() {
        *at += 1;
    }
}

fn parse_value(b: &[char], at: &mut usize) -> Option<Json> {
    skip_space(b, at);
    match *b.get(*at)? {
        'n' => literal(b, at, "null", Json::Null),
        't' => literal(b, at, "true", Json::Bool(true)),
        'f' => literal(b, at, "false", Json::Bool(false)),
        '"' => parse_string(b, at).map(Json::Str),
        '[' => {
            *at += 1;
            let mut items = Vec::new();
            skip_space(b, at);
            if *b.get(*at)? == ']' {
                *at += 1;
                return Some(Json::Arr(items));
            }
            loop {
                items.push(parse_value(b, at)?);
                skip_space(b, at);
                match *b.get(*at)? {
                    ',' => *at += 1,
                    ']' => {
                        *at += 1;
                        return Some(Json::Arr(items));
                    }
                    _ => return None,
                }
            }
        }
        '{' => {
            *at += 1;
            let mut map = BTreeMap::new();
            skip_space(b, at);
            if *b.get(*at)? == '}' {
                *at += 1;
                return Some(Json::Obj(map));
            }
            loop {
                skip_space(b, at);
                let key = parse_string(b, at)?;
                skip_space(b, at);
                if *b.get(*at)? != ':' {
                    return None;
                }
                *at += 1;
                map.insert(key, parse_value(b, at)?);
                skip_space(b, at);
                match *b.get(*at)? {
                    ',' => *at += 1,
                    '}' => {
                        *at += 1;
                        return Some(Json::Obj(map));
                    }
                    _ => return None,
                }
            }
        }
        _ => parse_number(b, at),
    }
}

fn literal(b: &[char], at: &mut usize, word: &str, value: Json) -> Option<Json> {
    for (i, c) in word.chars().enumerate() {
        if *b.get(*at + i)? != c {
            return None;
        }
    }
    *at += word.len();
    Some(value)
}

fn parse_string(b: &[char], at: &mut usize) -> Option<String> {
    if *b.get(*at)? != '"' {
        return None;
    }
    *at += 1;
    let mut out = String::new();
    loop {
        let c = *b.get(*at)?;
        *at += 1;
        match c {
            '"' => return Some(out),
            '\\' => {
                let escape = *b.get(*at)?;
                *at += 1;
                out.push(match escape {
                    '"' => '"',
                    '\\' => '\\',
                    '/' => '/',
                    'b' => '\u{8}',
                    'f' => '\u{c}',
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    'u' => {
                        let mut code = 0u32;
                        for _ in 0..4 {
                            code = code * 16 + b.get(*at)?.to_digit(16)?;
                            *at += 1;
                        }
                        // A lone surrogate is not a character. Replacing it
                        // keeps a path name readable instead of failing the
                        // whole message over one byte.
                        char::from_u32(code).unwrap_or('\u{fffd}')
                    }
                    _ => return None,
                });
            }
            c => out.push(c),
        }
    }
}

fn parse_number(b: &[char], at: &mut usize) -> Option<Json> {
    let start = *at;
    if *b.get(*at)? == '-' {
        *at += 1;
    }
    while b
        .get(*at)
        .is_some_and(|c| c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '+' | '-'))
    {
        *at += 1;
    }
    if *at == start {
        return None;
    }
    b[start..*at].iter().collect::<String>().parse().ok().map(Json::Num)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_a_dap_shaped_message() {
        let text = r#"{"seq":3,"type":"request","command":"setBreakpoints",
            "arguments":{"source":{"path":"C:\\src\\Program.cs"},
            "breakpoints":[{"line":12},{"line":40}]}}"#;
        let value = parse(text).expect("parses");
        assert_eq!(value.path("arguments.source.path").and_then(Json::as_str),
                   Some("C:\\src\\Program.cs"));
        assert_eq!(value.get("seq").and_then(Json::as_i64), Some(3));
        let lines: Vec<i64> = value
            .path("arguments.breakpoints")
            .and_then(Json::as_array)
            .unwrap()
            .iter()
            .filter_map(|b| b.get("line").and_then(Json::as_i64))
            .collect();
        assert_eq!(lines, vec![12, 40]);
        // And what is written parses back to the same thing.
        assert_eq!(parse(&value.write()), Some(value));
    }

    #[test]
    fn escapes_what_has_to_be_escaped() {
        let value = Json::str("a\"b\\c\nd\te");
        let written = value.write();
        assert_eq!(written, r#""a\"b\\c\nd\te""#);
        assert_eq!(parse(&written), Some(value));
    }

    #[test]
    fn integers_are_written_without_a_fractional_part() {
        // A client reading `"line": 12.0` is within its rights to object.
        assert_eq!(Json::num(12i32).write(), "12");
        assert_eq!(Json::num(-1i32).write(), "-1");
    }

    #[test]
    fn malformed_input_is_refused_rather_than_guessed() {
        for bad in [r#"{"a":}"#, "[1,", r#""unterminated"#, "{", "tru", ""] {
            assert_eq!(parse(bad), None, "`{bad}` should not parse");
        }
    }
}
