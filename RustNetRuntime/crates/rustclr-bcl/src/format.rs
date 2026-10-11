//! .NET format strings: numbers and dates, rendered the way the invariant
//! culture renders them.
//!
//! `ToString("N2")`, `$"{price:C}"`, `date.ToString("dd MMM yyyy")` are in
//! almost every business program, and a runtime that ignores the specifier
//! prints a number nobody can read. Everything here is a pure function of the
//! value and the format string, so it is unit-tested without an interpreter.
//!
//! # Culture
//!
//! Always invariant: `.` for decimals, `,` for groups, English month and day
//! names, `¤` for currency. A program that passes a `CultureInfo` gets the
//! invariant rendering, which is what `CultureInfo.InvariantCulture` asks for
//! and what most server and browser code uses for data anyway.
//!
//! # Rounding
//!
//! .NET has two rules, and both were measured on .NET 10 rather than assumed:
//!
//! * **Standard formats** (`F`, `N`, `P`, `C`, `E`) round the exact binary
//!   value half to even and keep the sign of a negative value that rounds to
//!   zero: `0.125` → `"0.12"`, `0.375` → `"0.38"`, `-0.125.ToString("F0")` →
//!   `"-0"`. That is exactly what Rust's `{:.N}` does, so [`fixed`] uses it.
//! * **Custom formats** (`0.00`, `#,##0`) round the *shortest round-trip*
//!   digits half away from zero: `2.675` (stored as 2.67499…) → `"2.68"`,
//!   `0.125` → `"0.13"`. [`fixed_custom`] does that.

#[allow(unused_imports)]
use crate::prelude::*;

// ── numbers ──────────────────────────────────────────────────────────────────

/// Formats a double with a .NET format string. `None` means the format is
/// not one this understands, and the caller should fall back to the default.
pub fn format_f64(value: f64, format: &str) -> Option<String> {
    if format.is_empty() {
        return Some(crate::support::format_double(value));
    }
    if !value.is_finite() {
        return Some(crate::support::format_double(value));
    }
    if let Some((kind, precision)) = standard(format) {
        return Some(match kind {
            'F' => fixed(value, precision.unwrap_or(2)),
            'N' => group(&fixed(value, precision.unwrap_or(2))),
            'P' => format!("{} %", group(&fixed(value * 100.0, precision.unwrap_or(2)))),
            'C' => {
                let body = group(&fixed(value.abs(), precision.unwrap_or(2)));
                if value < 0.0 && body.bytes().any(|b| b.is_ascii_digit() && b != b'0') {
                    format!("-¤{body}")
                } else {
                    format!("¤{body}")
                }
            }
            'E' | 'e' => scientific(value, precision.unwrap_or(6), kind == 'E'),
            'D' => {
                // `D` is integer-only in .NET; a double throws there. Render
                // the integer part rather than refuse.
                pad_integer(value as i64, precision.unwrap_or(0))
            }
            'G' | 'R' => match precision {
                Some(p) if p > 0 => general(value, p),
                _ => crate::support::format_double(value),
            },
            'X' | 'x' => return None,
            _ => return None,
        });
    }
    Some(custom(value, format))
}

/// Formats an integer with a .NET format string.
pub fn format_i64(value: i64, format: &str, unsigned_bits: Option<u32>) -> Option<String> {
    if format.is_empty() {
        return Some(value.to_string());
    }
    if let Some((kind, precision)) = standard(format) {
        return Some(match kind {
            'D' => pad_integer(value, precision.unwrap_or(0)),
            'X' | 'x' => {
                // Hex shows the two's-complement bits of the declared width.
                let bits = unsigned_bits.unwrap_or(if value < i32::MIN as i64 || value > u32::MAX as i64 { 64 } else { 32 });
                let raw = if bits >= 64 { value as u64 } else { (value as u64) & ((1u64 << bits) - 1) };
                let text = if kind == 'X' { format!("{raw:X}") } else { format!("{raw:x}") };
                let width = precision.unwrap_or(0);
                if text.len() < width { format!("{}{text}", "0".repeat(width - text.len())) } else { text }
            }
            'N' => group(&fixed(value as f64, precision.unwrap_or(2))),
            'F' => fixed(value as f64, precision.unwrap_or(2)),
            'G' | 'R' => value.to_string(),
            _ => return format_f64(value as f64, format),
        });
    }
    Some(custom(value as f64, format))
}

/// `"N2"` → `('N', Some(2))`. A standard format is one letter and an
/// optional number of up to two digits; anything else is custom.
fn standard(format: &str) -> Option<(char, Option<usize>)> {
    let mut chars = format.chars();
    let first = chars.next()?;
    if !first.is_ascii_alphabetic() {
        return None;
    }
    let rest: &str = &format[first.len_utf8()..];
    if rest.is_empty() {
        return Some((normalise(first), None));
    }
    if rest.len() <= 2 && rest.bytes().all(|b| b.is_ascii_digit()) {
        return Some((normalise(first), rest.parse().ok()));
    }
    None
}

fn normalise(c: char) -> char {
    match c {
        'x' | 'e' => c,
        other => other.to_ascii_uppercase(),
    }
}

fn pad_integer(value: i64, width: usize) -> String {
    let digits = value.unsigned_abs().to_string();
    let padded = if digits.len() < width { format!("{}{digits}", "0".repeat(width - digits.len())) } else { digits };
    if value < 0 { format!("-{padded}") } else { padded }
}

/// Fixed-point for the standard formats: exact value, half to even.
pub fn fixed(value: f64, decimals: usize) -> String {
    format!("{:.*}", decimals, value)
}

/// Fixed-point for custom formats: the shortest round-trip digits, rounded
/// half away from zero. A result that rounds to zero loses its sign.
pub fn fixed_custom(value: f64, decimals: usize) -> String {
    let negative = value < 0.0;
    // Rust's `Display` for f64 is the shortest round-trip form, never
    // scientific, which is the digit string .NET rounds.
    let shortest = format!("{}", value.abs());
    let (int_part, frac_part) = shortest.split_once('.').unwrap_or((&shortest, ""));
    let mut digits: Vec<u8> = int_part.bytes().chain(frac_part.bytes().chain(core::iter::repeat(b'0')).take(decimals)).collect();
    let next = frac_part.as_bytes().get(decimals).copied().unwrap_or(b'0');
    if next >= b'5' {
        let mut i = digits.len();
        loop {
            if i == 0 {
                digits.insert(0, b'1');
                break;
            }
            i -= 1;
            if digits[i] == b'9' {
                digits[i] = b'0';
            } else {
                digits[i] += 1;
                break;
            }
        }
    }
    let int_len = digits.len() - decimals;
    let mut out = String::with_capacity(digits.len() + 2);
    if negative && digits.iter().any(|d| *d != b'0') {
        out.push('-');
    }
    out.push_str(core::str::from_utf8(&digits[..int_len]).unwrap_or("0"));
    if decimals > 0 {
        out.push('.');
        out.push_str(core::str::from_utf8(&digits[int_len..]).unwrap_or(""));
    }
    out
}

/// Inserts `,` every three digits of the integer part.
pub fn group(fixed: &str) -> String {
    let (sign, body) = match fixed.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", fixed),
    };
    let (int_part, frac) = match body.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (body, None),
    };
    let mut grouped = String::with_capacity(int_part.len() + int_part.len() / 3);
    for (i, c) in int_part.chars().enumerate() {
        if i > 0 && (int_part.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }
    match frac {
        Some(f) => format!("{sign}{grouped}.{f}"),
        None => format!("{sign}{grouped}"),
    }
}

fn scientific(value: f64, decimals: usize, upper: bool) -> String {
    let text = format!("{:.*e}", decimals, value);
    let (mantissa, exponent) = text.split_once('e').unwrap_or((&text, "0"));
    let exp: i32 = exponent.parse().unwrap_or(0);
    let sign = if exp < 0 { '-' } else { '+' };
    let e = if upper { 'E' } else { 'e' };
    format!("{mantissa}{e}{sign}{:03}", exp.abs())
}

fn general(value: f64, significant: usize) -> String {
    if value == 0.0 {
        return "0".into();
    }
    let magnitude = crate::fmath::floor(crate::fmath::log10(value.abs())) as i32;
    if magnitude < -5 || magnitude >= significant as i32 {
        let s = scientific(value, significant.saturating_sub(1), true);
        return s;
    }
    let decimals = (significant as i32 - 1 - magnitude).max(0) as usize;
    let text = fixed(value, decimals);
    if text.contains('.') { text.trim_end_matches('0').trim_end_matches('.').to_string() } else { text }
}

/// A custom numeric format: `0.00`, `#,##0`, `#,##0.00;(#,##0.00)`, `0%`.
fn custom(value: f64, format: &str) -> String {
    // `positive;negative;zero` sections.
    let sections: Vec<&str> = split_sections(format);
    let (section, value) = match sections.len() {
        2 | 3 if value < 0.0 => (sections[1], -value),
        3 if value == 0.0 => (sections[2], value),
        _ => (sections[0], value),
    };
    let explicit_negative = sections.len() >= 2 && section != sections[0];

    // Scientific notation: `0.###e+0`, `0.00E+00`, `##0.0e0`.
    if let Some((mantissa_spec, marker, always_sign, min_digits, suffix)) = split_exponent(section) {
        let int_places = mantissa_spec.split('.').next().unwrap_or("").chars().filter(|c| *c == '0' || *c == '#').count().max(1) as i32;
        let magnitude = value.abs();
        let mut exponent = if magnitude == 0.0 { 0 } else { crate::fmath::floor(crate::fmath::log10(magnitude)) as i32 - (int_places - 1) };
        let mut mantissa_text = custom(magnitude / pow10(exponent), &mantissa_spec);
        // Rounding can carry into another digit (9.9995 -> 10.00); shift once.
        let int_digits = mantissa_text.split('.').next().unwrap_or("").chars().filter(|c| c.is_ascii_digit()).count() as i32;
        if magnitude != 0.0 && int_digits > int_places {
            exponent += 1;
            mantissa_text = custom(magnitude / pow10(exponent), &mantissa_spec);
        }
        let mut out = String::new();
        if value < 0.0 && !explicit_negative {
            out.push('-');
        }
        out.push_str(&mantissa_text);
        out.push(marker);
        if exponent < 0 {
            out.push('-');
        } else if always_sign {
            out.push('+');
        }
        let digits = exponent.unsigned_abs().to_string();
        for _ in digits.len()..min_digits {
            out.push('0');
        }
        out.push_str(&digits);
        out.push_str(&suffix);
        return out;
    }

    let mut scaled = value;
    let mut in_quote = None;
    let mut escape = false;
    for c in section.chars() {
        if escape {
            escape = false;
            continue;
        }
        match (in_quote, c) {
            (None, '\\') => escape = true,
            (None, '\'' | '"') => in_quote = Some(c),
            (Some(q), c) if c == q => in_quote = None,
            (None, '%') => scaled *= 100.0,
            (None, '‰') => scaled *= 1000.0,
            _ => {}
        }
    }

    // Find the digit placeholders and the decimal point.
    let placeholder_text: String = strip_literals(section);
    let (int_spec, frac_spec) = match placeholder_text.split_once('.') {
        Some((i, f)) => (i.to_string(), f.to_string()),
        None => (placeholder_text.clone(), String::new()),
    };
    let grouping = int_spec.contains(',') && int_spec.trim_end_matches(',').contains(',');
    // Trailing commas before the point divide by 1000 each.
    let mut divisor_commas = 0;
    for c in int_spec.chars().rev() {
        if c == ',' {
            divisor_commas += 1;
        } else {
            break;
        }
    }
    for _ in 0..divisor_commas {
        scaled /= 1000.0;
    }
    let min_int = int_spec.chars().filter(|c| *c == '0').count();
    let min_frac = frac_spec.chars().filter(|c| *c == '0').count();
    let max_frac = frac_spec.chars().filter(|c| *c == '0' || *c == '#').count();

    // .NET Core keeps the sign of a negative value that rounds to zero ("-0.00").
    let negative = scaled < 0.0;
    let number = fixed_custom(scaled.abs(), max_frac);
    let (mut int_digits, mut frac_digits) = match number.split_once('.') {
        Some((i, f)) => (i.to_string(), f.to_string()),
        None => (number.clone(), String::new()),
    };
    while frac_digits.len() > min_frac && frac_digits.ends_with('0') {
        frac_digits.pop();
    }
    if int_digits == "0" && min_int == 0 {
        int_digits.clear();
    }
    while int_digits.len() < min_int {
        int_digits.insert(0, '0');
    }
    if grouping && !int_digits.is_empty() {
        int_digits = group(&int_digits);
    }
    let mut digits = int_digits;
    if !frac_digits.is_empty() {
        digits.push('.');
        digits.push_str(&frac_digits);
    }

    // Rebuild: literals around the first and after the last placeholder stay
    // where they were; the placeholders become the digits.
    let mut out = String::new();
    let mut placed = false;
    let mut in_quote = None;
    let mut escape = false;
    for c in section.chars() {
        if escape {
            out.push(c);
            escape = false;
            continue;
        }
        if let Some(q) = in_quote {
            if c == q { in_quote = None } else { out.push(c) }
            continue;
        }
        match c {
            '\\' => escape = true,
            '\'' | '"' => in_quote = Some(c),
            '0' | '#' | '.' | ',' => {
                if !placed {
                    out.push_str(&digits);
                    placed = true;
                }
            }
            other => out.push(other),
        }
    }
    if !placed {
        out.push_str(&digits);
    }
    if negative && !explicit_negative {
        out.insert(0, '-');
    }
    out
}

fn pow10(exponent: i32) -> f64 {
    let mut r = 1.0;
    for _ in 0..exponent.unsigned_abs() {
        r *= 10.0;
    }
    if exponent < 0 { 1.0 / r } else { r }
}

/// Splits `0.##e+00 kg` into the mantissa format, the `e`/`E` marker, whether
/// a `+` is always shown, the minimum exponent digits, and what follows.
fn split_exponent(section: &str) -> Option<(String, char, bool, usize, String)> {
    let chars: Vec<char> = section.chars().collect();
    let mut in_quote = None;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if let Some(q) = in_quote {
            if c == q {
                in_quote = None;
            }
        } else if c == '\\' {
            i += 1;
        } else if c == '\'' || c == '"' {
            in_quote = Some(c);
        } else if c == 'e' || c == 'E' {
            let mut j = i + 1;
            let mut always_sign = false;
            if j < chars.len() && (chars[j] == '+' || chars[j] == '-') {
                always_sign = chars[j] == '+';
                j += 1;
            }
            let start = j;
            while j < chars.len() && chars[j] == '0' {
                j += 1;
            }
            if j > start {
                let mantissa: String = chars[..i].iter().collect();
                let suffix: String = chars[j..].iter().collect();
                return Some((mantissa, c, always_sign, j - start, suffix));
            }
        }
        i += 1;
    }
    None
}

fn split_sections(format: &str) -> Vec<&str> {
    let mut sections = Vec::new();
    let mut start = 0;
    let mut quote = None;
    let bytes = format.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        match quote {
            Some(q) if b == q => quote = None,
            Some(_) => {}
            None => match b {
                b'\'' | b'"' => quote = Some(b),
                b'\\' => i += 1,
                b';' => {
                    sections.push(&format[start..i]);
                    start = i + 1;
                }
                _ => {}
            },
        }
        i += 1;
    }
    sections.push(&format[start..]);
    sections
}

fn strip_literals(section: &str) -> String {
    let mut out = String::new();
    let mut quote = None;
    let mut escape = false;
    for c in section.chars() {
        if escape {
            escape = false;
            continue;
        }
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '\\' => escape = true,
            '\'' | '"' => quote = Some(c),
            '0' | '#' | '.' | ',' => out.push(c),
            _ => {}
        }
    }
    out
}

// ── calendar ─────────────────────────────────────────────────────────────────

pub const TICKS_PER_MS: i64 = 10_000;
pub const TICKS_PER_SECOND: i64 = 10_000_000;
pub const TICKS_PER_MINUTE: i64 = 60 * TICKS_PER_SECOND;
pub const TICKS_PER_HOUR: i64 = 60 * TICKS_PER_MINUTE;
pub const TICKS_PER_DAY: i64 = 24 * TICKS_PER_HOUR;

/// Ticks between 0001-01-01 and the Unix epoch.
pub const UNIX_EPOCH_TICKS: i64 = 621_355_968_000_000_000;

pub const MONTHS: [&str; 12] = [
    "January", "February", "March", "April", "May", "June", "July", "August", "September", "October",
    "November", "December",
];
pub const DAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];

pub fn is_leap(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

pub fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => if is_leap(year) { 29 } else { 28 },
    }
}

/// Days from 0001-01-01 to the given date (proleptic Gregorian).
pub fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = year - 1;
    let mut days = y * 365 + y / 4 - y / 100 + y / 400;
    for m in 1..month {
        days += days_in_month(year, m);
    }
    days + day - 1
}

/// (year, month, day) for a day number counted from 0001-01-01.
pub fn civil_from_days(days: i64) -> (i64, i64, i64) {
    // 400-year cycles of 146,097 days, then centuries, 4-year runs and years.
    let mut n = days;
    let y400 = n / 146_097;
    n %= 146_097;
    let mut y100 = n / 36_524;
    if y100 == 4 {
        y100 = 3;
    }
    n -= y100 * 36_524;
    let y4 = n / 1_461;
    n %= 1_461;
    let mut y1 = n / 365;
    if y1 == 4 {
        y1 = 3;
    }
    n -= y1 * 365;
    let year = y400 * 400 + y100 * 100 + y4 * 4 + y1 + 1;
    let mut month = 1;
    while n >= days_in_month(year, month) {
        n -= days_in_month(year, month);
        month += 1;
    }
    (year, month, n + 1)
}

/// The parts of a tick count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Parts {
    pub year: i64,
    pub month: i64,
    pub day: i64,
    pub hour: i64,
    pub minute: i64,
    pub second: i64,
    pub millisecond: i64,
    pub ticks_in_second: i64,
    pub day_of_week: i64,
    pub day_of_year: i64,
}

pub fn parts(ticks: i64) -> Parts {
    let days = ticks.div_euclid(TICKS_PER_DAY);
    let time = ticks.rem_euclid(TICKS_PER_DAY);
    let (year, month, day) = civil_from_days(days);
    Parts {
        year,
        month,
        day,
        hour: time / TICKS_PER_HOUR,
        minute: time % TICKS_PER_HOUR / TICKS_PER_MINUTE,
        second: time % TICKS_PER_MINUTE / TICKS_PER_SECOND,
        millisecond: time % TICKS_PER_SECOND / TICKS_PER_MS,
        ticks_in_second: time % TICKS_PER_SECOND,
        // 0001-01-01 was a Monday.
        day_of_week: (days + 1).rem_euclid(7),
        day_of_year: days - days_from_civil(year, 1, 1) + 1,
    }
}

pub fn ticks_from(year: i64, month: i64, day: i64, hour: i64, minute: i64, second: i64, ms: i64) -> i64 {
    days_from_civil(year, month, day) * TICKS_PER_DAY
        + hour * TICKS_PER_HOUR
        + minute * TICKS_PER_MINUTE
        + second * TICKS_PER_SECOND
        + ms * TICKS_PER_MS
}

/// `AddMonths`, clamping the day to the target month's length as .NET does.
pub fn add_months(ticks: i64, months: i64) -> i64 {
    let p = parts(ticks);
    let total = p.year * 12 + (p.month - 1) + months;
    let (year, month) = (total.div_euclid(12), total.rem_euclid(12) + 1);
    let day = p.day.min(days_in_month(year, month));
    days_from_civil(year, month, day) * TICKS_PER_DAY + ticks.rem_euclid(TICKS_PER_DAY)
}

// ── date formatting ──────────────────────────────────────────────────────────

/// Expands a one-letter standard date format to its invariant pattern.
pub fn date_pattern(format: &str) -> &str {
    match format {
        "" | "G" => "MM/dd/yyyy HH:mm:ss",
        "d" => "MM/dd/yyyy",
        "D" => "dddd, dd MMMM yyyy",
        "f" => "dddd, dd MMMM yyyy HH:mm",
        "F" => "dddd, dd MMMM yyyy HH:mm:ss",
        "g" => "MM/dd/yyyy HH:mm",
        "t" => "HH:mm",
        "T" => "HH:mm:ss",
        "M" | "m" => "MMMM dd",
        "Y" | "y" => "yyyy MMMM",
        "s" => "yyyy'-'MM'-'dd'T'HH':'mm':'ss",
        "u" => "yyyy'-'MM'-'dd HH':'mm':'ss'Z'",
        "o" | "O" => "yyyy'-'MM'-'dd'T'HH':'mm':'ss'.'fffffff",
        "r" | "R" => "ddd, dd MMM yyyy HH':'mm':'ss 'GMT'",
        other => other,
    }
}

pub fn format_date(ticks: i64, format: &str) -> String {
    let p = parts(ticks);
    let pattern = date_pattern(format);
    let chars: Vec<char> = pattern.chars().collect();
    let mut out = String::with_capacity(pattern.len() + 8);
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let mut run = 1;
        while i + run < chars.len() && chars[i + run] == c {
            run += 1;
        }
        match c {
            'y' => {
                if run <= 2 {
                    out.push_str(&format!("{:0width$}", p.year % 100, width = run));
                } else {
                    out.push_str(&format!("{:0width$}", p.year, width = run));
                }
            }
            'M' => match run {
                1 => out.push_str(&p.month.to_string()),
                2 => out.push_str(&format!("{:02}", p.month)),
                3 => out.push_str(&MONTHS[(p.month - 1) as usize][..3]),
                _ => out.push_str(MONTHS[(p.month - 1) as usize]),
            },
            'd' => match run {
                1 => out.push_str(&p.day.to_string()),
                2 => out.push_str(&format!("{:02}", p.day)),
                3 => out.push_str(&DAYS[p.day_of_week as usize][..3]),
                _ => out.push_str(DAYS[p.day_of_week as usize]),
            },
            'H' => out.push_str(&pad(p.hour, run)),
            'h' => {
                let h = if p.hour % 12 == 0 { 12 } else { p.hour % 12 };
                out.push_str(&pad(h, run));
            }
            'm' => out.push_str(&pad(p.minute, run)),
            's' => out.push_str(&pad(p.second, run)),
            'f' | 'F' => {
                let digits = format!("{:07}", p.ticks_in_second);
                let take = run.min(7);
                let mut part = digits[..take].to_string();
                if c == 'F' {
                    part = part.trim_end_matches('0').to_string();
                }
                out.push_str(&part);
            }
            't' => {
                let am = if p.hour < 12 { "AM" } else { "PM" };
                out.push_str(if run == 1 { &am[..1] } else { am });
            }
            'z' | 'K' => {
                if c == 'z' {
                    out.push_str(if run >= 3 { "+00:00" } else if run == 2 { "+00" } else { "+0" });
                }
            }
            '\'' | '"' => {
                // Quoted literal, up to the matching quote.
                let mut j = i + 1;
                while j < chars.len() && chars[j] != c {
                    out.push(chars[j]);
                    j += 1;
                }
                i = j + 1;
                continue;
            }
            '\\' => {
                if i + 1 < chars.len() {
                    out.push(chars[i + 1]);
                }
                i += 2;
                continue;
            }
            '%' => {
                i += 1;
                continue;
            }
            other => {
                for _ in 0..run {
                    out.push(other);
                }
            }
        }
        i += run;
    }
    out
}

/// `DateTimeOffset.ToString`: the clock time in its offset, with `z` and `K`
/// naming that offset. `u` and `r` print the UTC instant, as .NET does.
pub fn format_date_offset(clock: i64, offset_minutes: i64, format: &str) -> String {
    let utc = clock - offset_minutes * TICKS_PER_MINUTE;
    let pattern = match format {
        "" | "G" => "MM/dd/yyyy HH:mm:ss zzz",
        "o" | "O" => "yyyy'-'MM'-'dd'T'HH':'mm':'ss'.'fffffffzzz",
        "u" | "r" | "R" => return format_date(utc, format),
        other => date_pattern(other),
    };
    let sign = if offset_minutes < 0 { '-' } else { '+' };
    let (h, m) = (offset_minutes.abs() / 60, offset_minutes.abs() % 60);
    let mut out = String::with_capacity(pattern.len() + 8);
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\'' || c == '"' {
            // Copy a quoted literal through untouched.
            let end = chars[i + 1..].iter().position(|x| *x == c).map(|p| i + 1 + p).unwrap_or(chars.len() - 1);
            out.extend(&chars[i..=end]);
            i = end + 1;
            continue;
        }
        if c == '\\' && i + 1 < chars.len() {
            out.push(c);
            out.push(chars[i + 1]);
            i += 2;
            continue;
        }
        let mut run = 1;
        while i + run < chars.len() && chars[i + run] == c {
            run += 1;
        }
        let literal = match c {
            'z' if run >= 3 => Some(format!("{sign}{h:02}:{m:02}")),
            'z' if run == 2 => Some(format!("{sign}{h:02}")),
            'z' => Some(format!("{sign}{h}")),
            'K' => Some(format!("{sign}{h:02}:{m:02}")),
            _ => None,
        };
        match literal {
            Some(text) => {
                out.push('\'');
                out.push_str(&text);
                out.push('\'');
            }
            None => out.extend(core::iter::repeat(c).take(run)),
        }
        i += run;
    }
    format_date(clock, &out)
}

/// Splits a trailing `Z` or `±hh[:mm]` off a date string: the clock text and
/// the offset in minutes, or `None` when the text names no offset.
pub fn split_offset(text: &str) -> (&str, Option<i64>) {
    let text = text.trim();
    if let Some(rest) = text.strip_suffix('Z') {
        return (rest, Some(0));
    }
    // An offset only follows a time, so look after the date/time separator.
    let Some(time_at) = text.find(|c| c == 'T' || c == ' ') else { return (text, None) };
    let Some(at) = text[time_at..].rfind(|c| c == '+' || c == '-').map(|p| time_at + p) else { return (text, None) };
    let (clock, offset) = text.split_at(at);
    let negative = offset.starts_with('-');
    let digits: String = offset[1..].chars().filter(|c| *c != ':').collect();
    let (h, m) = match digits.len() {
        1 | 2 => (digits.parse::<i64>().ok(), Some(0)),
        4 => (digits[..2].parse::<i64>().ok(), digits[2..].parse::<i64>().ok()),
        _ => (None, None),
    };
    match (h, m) {
        (Some(h), Some(m)) if h <= 14 && m < 60 => {
            let minutes = h * 60 + m;
            (clock.trim_end(), Some(if negative { -minutes } else { minutes }))
        }
        _ => (text, None),
    }
}

fn pad(n: i64, width: usize) -> String {
    if width >= 2 { format!("{n:02}") } else { n.to_string() }
}

/// `TimeSpan.ToString()` and its custom formats.
pub fn format_timespan(ticks: i64, format: &str) -> String {
    let negative = ticks < 0;
    let t = ticks.unsigned_abs() as i64;
    let days = t / TICKS_PER_DAY;
    let hours = t % TICKS_PER_DAY / TICKS_PER_HOUR;
    let minutes = t % TICKS_PER_HOUR / TICKS_PER_MINUTE;
    let seconds = t % TICKS_PER_MINUTE / TICKS_PER_SECOND;
    let fraction = t % TICKS_PER_SECOND;
    if format.is_empty() || format == "c" || format == "t" || format == "T" || format == "g" || format == "G" {
        let mut out = String::new();
        if negative {
            out.push('-');
        }
        if days > 0 {
            out.push_str(&format!("{days}."));
        }
        out.push_str(&format!("{hours:02}:{minutes:02}:{seconds:02}"));
        if fraction > 0 {
            out.push_str(&format!(".{fraction:07}"));
        }
        return out;
    }
    let chars: Vec<char> = format.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let mut run = 1;
        while i + run < chars.len() && chars[i + run] == c {
            run += 1;
        }
        match c {
            'd' => out.push_str(&format!("{:0width$}", days, width = run)),
            'h' => out.push_str(&pad(hours, run)),
            'm' => out.push_str(&pad(minutes, run)),
            's' => out.push_str(&pad(seconds, run)),
            'f' | 'F' => out.push_str(&format!("{fraction:07}")[..run.min(7)]),
            '\\' => {
                if i + 1 < chars.len() {
                    out.push(chars[i + 1]);
                }
                i += 2;
                continue;
            }
            '\'' | '"' => {
                let mut j = i + 1;
                while j < chars.len() && chars[j] != c {
                    out.push(chars[j]);
                    j += 1;
                }
                i = j + 1;
                continue;
            }
            other => {
                for _ in 0..run {
                    out.push(other);
                }
            }
        }
        i += run;
    }
    out
}

// ── parsing ──────────────────────────────────────────────────────────────────

/// Parses the date shapes programs and browsers actually produce:
/// `2026-03-09`, `2026-03-09T14:05[:07[.123]][Z]`, `2026-03-09 14:05`,
/// `03/09/2026 [14:05[:07]]` and `2026/03/09`.
pub fn parse_date(text: &str) -> Option<i64> {
    let text = text.trim().trim_end_matches('Z');
    let (date, time) = match text.find(|c| c == 'T' || c == ' ') {
        Some(at) => (&text[..at], text[at + 1..].trim()),
        None => (text, ""),
    };
    let fields: Vec<&str> = date.split(|c| c == '-' || c == '/' || c == '.').collect();
    if fields.len() != 3 {
        return None;
    }
    let nums: Vec<i64> = fields.iter().map(|f| f.parse::<i64>().ok()).collect::<Option<Vec<_>>>()?;
    let (year, month, day) = if fields[0].len() == 4 { (nums[0], nums[1], nums[2]) } else { (nums[2], nums[0], nums[1]) };
    if !(1..=12).contains(&month) || day < 1 || day > days_in_month(year, month) || !(1..=9999).contains(&year) {
        return None;
    }
    let (mut hour, mut minute, mut second, mut ms) = (0, 0, 0, 0);
    if !time.is_empty() {
        // Drop a trailing offset; times are taken as given.
        let time = time.split(|c| c == '+').next().unwrap_or(time);
        let pieces: Vec<&str> = time.split(':').collect();
        hour = pieces.first()?.parse().ok()?;
        minute = pieces.get(1).map(|m| m.parse().ok()).unwrap_or(Some(0))?;
        if let Some(s) = pieces.get(2) {
            let (whole, frac) = s.split_once('.').unwrap_or((s, ""));
            second = whole.parse().ok()?;
            if !frac.is_empty() {
                let three: String = frac.chars().chain(core::iter::repeat('0')).take(3).collect();
                ms = three.parse().ok()?;
            }
        }
        if hour > 23 || minute > 59 || second > 59 {
            return None;
        }
    }
    Some(ticks_from(year, month, day, hour, minute, second, ms))
}

/// Parses `TimeSpan.ToString()` output and the shapes people type:
/// `[-][d.]hh:mm[:ss[.fffffff]]`, or a bare whole number of days.
pub fn parse_timespan(text: &str) -> Option<i64> {
    let text = text.trim();
    let (negative, text) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    if text.is_empty() {
        return None;
    }
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let ticks = if !text.contains(':') {
        if !digits(text) {
            return None;
        }
        text.parse::<i64>().ok()? * TICKS_PER_DAY
    } else {
        let (days, clock) = match text.split_once('.') {
            Some((d, rest)) if digits(d) && rest.contains(':') => (d.parse::<i64>().ok()?, rest),
            _ => (0, text),
        };
        let parts: Vec<&str> = clock.split(':').collect();
        if parts.len() < 2 || parts.len() > 3 {
            return None;
        }
        let (h, m) = (parts[0], parts[1]);
        if !digits(h) || !digits(m) {
            return None;
        }
        let (hours, minutes) = (h.parse::<i64>().ok()?, m.parse::<i64>().ok()?);
        let (mut seconds, mut fraction) = (0, 0);
        if let Some(sec) = parts.get(2) {
            let (whole, frac) = sec.split_once('.').unwrap_or((sec, ""));
            if !digits(whole) || !(frac.is_empty() || digits(frac)) || frac.len() > 7 {
                return None;
            }
            seconds = whole.parse::<i64>().ok()?;
            if !frac.is_empty() {
                let seven: String = frac.chars().chain(core::iter::repeat('0')).take(7).collect();
                fraction = seven.parse::<i64>().ok()?;
            }
        }
        if hours > 23 || minutes > 59 || seconds > 59 {
            return None;
        }
        days * TICKS_PER_DAY + hours * TICKS_PER_HOUR + minutes * TICKS_PER_MINUTE + seconds * TICKS_PER_SECOND + fraction
    };
    Some(if negative { -ticks } else { ticks })
}

/// Parses a number the invariant way: optional sign, digits, one `.`, an
/// exponent, and `,` group separators.
pub fn parse_number(text: &str) -> Option<f64> {
    let cleaned: String = text.trim().chars().filter(|c| *c != ',').collect();
    if cleaned.is_empty() {
        return None;
    }
    cleaned.parse::<f64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_numeric_formats_match_dotnet() {
        assert_eq!(format_f64(1234567.891, "N2").unwrap(), "1,234,567.89");
        assert_eq!(format_f64(1234567.891, "F1").unwrap(), "1234567.9");
        assert_eq!(format_f64(-3.5, "F0").unwrap(), "-4");
        assert_eq!(format_f64(0.256, "P1").unwrap(), "25.6 %");
        // Measured on .NET 10: standard formats round half to even.
        assert_eq!(format_f64(0.125, "F2").unwrap(), "0.12");
        assert_eq!(format_f64(0.375, "F2").unwrap(), "0.38");
        assert_eq!(format_f64(1187.625, "N2").unwrap(), "1,187.62");
        assert_eq!(format_f64(2.675, "F2").unwrap(), "2.67");
        assert_eq!(format_f64(2.5, "F0").unwrap(), "2");
        assert_eq!(format_f64(-0.125, "F0").unwrap(), "-0");
        assert_eq!(format_i64(42, "D5", None).unwrap(), "00042");
        assert_eq!(format_i64(1234567, "N0", None).unwrap(), "1,234,567");
        assert_eq!(format_i64(255, "X", None).unwrap(), "FF");
        assert_eq!(format_i64(-1, "x", Some(32)).unwrap(), "ffffffff");
        assert_eq!(format_f64(1234.5, "E2").unwrap(), "1.23E+003");
    }

    #[test]
    fn custom_numeric_formats_match_dotnet() {
        assert_eq!(format_f64(3.14159, "0.00").unwrap(), "3.14");
        // ...and custom formats round the shortest digits half away from zero.
        assert_eq!(format_f64(0.125, "0.00").unwrap(), "0.13");
        assert_eq!(format_f64(2.675, "0.00").unwrap(), "2.68");
        assert_eq!(format_f64(-0.125, "0.00").unwrap(), "-0.13");
        assert_eq!(format_f64(1234567.891, "#,##0").unwrap(), "1,234,568");
        assert_eq!(format_f64(1234.5, "#,##0.00").unwrap(), "1,234.50");
        assert_eq!(format_f64(0.5, "0%").unwrap(), "50%");
        assert_eq!(format_f64(-12.0, "#,##0;(#,##0)").unwrap(), "(12)");
        assert_eq!(format_f64(7.0, "000").unwrap(), "007");
        assert_eq!(format_f64(1.5, "#.##").unwrap(), "1.5");
        assert_eq!(format_f64(0.0, "#.##").unwrap(), "");
        assert_eq!(format_f64(42.0, "'Rp' #,##0").unwrap(), "Rp 42");
    }

    #[test]
    fn calendar_round_trips() {
        let t = ticks_from(2026, 3, 9, 14, 5, 7, 0);
        assert_eq!(t, 639_086_619_070_000_000);
        let p = parts(t);
        assert_eq!((p.year, p.month, p.day, p.hour, p.minute, p.second), (2026, 3, 9, 14, 5, 7));
        assert_eq!(DAYS[p.day_of_week as usize], "Monday");
        assert_eq!(p.day_of_year, 68);
        assert_eq!(parts(0).day_of_week, 1);
        assert_eq!(format_date(add_months(ticks_from(2024, 1, 31, 0, 0, 0, 0), 1), "yyyy-MM-dd"), "2024-02-29");
        for days in [0, 59, 365, 1460, 146_096, 738_000, 3_652_058] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days);
        }
    }

    #[test]
    fn date_formats_match_dotnet() {
        let t = ticks_from(2026, 3, 9, 14, 5, 7, 0);
        assert_eq!(format_date(t, "yyyy-MM-dd HH:mm"), "2026-03-09 14:05");
        assert_eq!(format_date(t, "dd MMM yyyy"), "09 Mar 2026");
        assert_eq!(format_date(t, "dddd, MMMM d"), "Monday, March 9");
        assert_eq!(format_date(t, "h:mm tt"), "2:05 PM");
        assert_eq!(format_date(t, ""), "03/09/2026 14:05:07");
        assert_eq!(format_date(t, "s"), "2026-03-09T14:05:07");
        assert_eq!(format_timespan(3725 * TICKS_PER_SECOND, "hh\\:mm\\:ss"), "01:02:05");
        assert_eq!(format_timespan(90 * TICKS_PER_MINUTE, ""), "01:30:00");
        assert_eq!(format_timespan(TICKS_PER_DAY + TICKS_PER_SECOND, ""), "1.00:00:01");
    }

    #[test]
    fn parses_the_common_date_shapes() {
        assert_eq!(parse_date("2026-01-15"), Some(ticks_from(2026, 1, 15, 0, 0, 0, 0)));
        assert_eq!(parse_date("2026-01-15T08:30"), Some(ticks_from(2026, 1, 15, 8, 30, 0, 0)));
        assert_eq!(parse_date("2026-01-15 08:30:05.250Z"), Some(ticks_from(2026, 1, 15, 8, 30, 5, 250)));
        assert_eq!(parse_date("03/09/2026"), Some(ticks_from(2026, 3, 9, 0, 0, 0, 0)));
        assert_eq!(parse_date("2026-02-30"), None);
        assert_eq!(parse_date("nonsense"), None);
    }
}
