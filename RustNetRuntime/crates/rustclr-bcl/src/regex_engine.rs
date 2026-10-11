//! Regular expressions with .NET semantics, behind `System.Text.RegularExpressions`.
//!
//! A backtracking matcher over UTF-16 code units, as .NET's interpreter is: a
//! match's `Index` and `Length` count code units, `.` matches one unit, and a
//! character outside the BMP is two units. What it supports is what programs
//! actually write:
//!
//! * literals and escapes (`\t \n \r \f \v \e \a \0 \xHH \uHHHH \cX`),
//! * classes `\d \w \s` and their negations, `\p{..}` / `\P{..}` for the
//!   general categories and the common named blocks, `[...]` sets with ranges,
//!   negation and subtraction (`[a-z-[aeiou]]`),
//! * anchors `^ $ \A \z \Z \b \B \G`,
//! * groups: capturing, `(?:...)`, named `(?<n>...)` / `(?'n'...)`, lookahead
//!   and lookbehind, atomic `(?>...)`, inline options `(?imnsx-imnsx)` and
//!   `(?imnsx-imnsx:...)`, comments `(?#...)`,
//! * quantifiers `* + ? {n} {n,} {n,m}`, greedy and lazy,
//! * backreferences `\1` and `\k<name>`, and alternation.
//!
//! Group numbering follows .NET: unnamed groups first, left to right, then
//! named groups after them. `RightToLeft` is refused at construction rather
//! than silently matching left to right.
//!
//! Unicode categories are answered from Rust's `char` predicates plus a few
//! ranges, which is exact for letters, digits, case and white space and close
//! for punctuation and symbols. `docs/limitations.md` says so.

use crate::prelude::*;

pub const IGNORE_CASE: i32 = 1;
pub const MULTILINE: i32 = 2;
pub const EXPLICIT_CAPTURE: i32 = 4;
pub const SINGLELINE: i32 = 16;
pub const IGNORE_PATTERN_WHITESPACE: i32 = 32;
pub const RIGHT_TO_LEFT: i32 = 64;
pub const ECMASCRIPT: i32 = 256;

/// More backtracking steps than this and a match gives up, as a .NET regex
/// with a timeout would, rather than hanging the page.
const STEP_LIMIT: u64 = 50_000_000;

#[derive(Clone, Debug, PartialEq)]
enum Cat {
    L,
    Lu,
    Ll,
    Lt,
    Lm,
    Lo,
    M,
    N,
    Nd,
    Nl,
    No,
    P,
    Pc,
    Pd,
    Ps,
    Pe,
    Pi,
    Pf,
    Po,
    S,
    Sm,
    Sc,
    Sk,
    So,
    Z,
    Zs,
    Zl,
    Zp,
    C,
    Cc,
    Cf,
    Cs,
    Never,
    Block(u32, u32),
}

#[derive(Clone, Debug)]
enum Item {
    Range(u32, u32),
    Digit(bool),
    Word(bool),
    Space(bool),
    Category(Cat, bool),
}

#[derive(Clone, Debug)]
struct Set {
    items: Vec<Item>,
    negate: bool,
    subtract: Option<Box<Set>>,
    ecma: bool,
}

#[derive(Clone, Debug)]
enum Node {
    Empty,
    Lit(u16, bool),
    Any(bool),
    Set(Box<Set>, bool),
    Bol(bool),
    Eol(bool),
    Start,
    End,
    EndZ,
    Boundary(bool, bool),
    PrevEnd,
    Group(Box<Node>, usize),
    NamedGroup(Box<Node>, String),
    Concat(Vec<Node>),
    Alt(Vec<Node>),
    Repeat(Box<Node>, u32, u32, bool),
    Look(Box<Node>, bool, bool),
    Atomic(Box<Node>),
    Backref(usize, bool),
    NamedBackref(String, bool),
}

/// A compiled pattern.
#[derive(Clone, Debug)]
pub struct Regex {
    root: Node,
    /// Group count including group 0, the whole match.
    pub group_count: usize,
    /// Every group's name, by number: digits for unnamed groups, as .NET reports.
    pub names: Vec<String>,
    pub options: i32,
    anchored: bool,
    first: Option<u16>,
}

/// A match: a span per group, `None` where a group did not take part.
pub type Captures = Vec<Option<(usize, usize)>>;

#[derive(Debug, PartialEq)]
pub enum RegexError {
    Parse(String),
    Timeout,
}

impl Regex {
    pub fn new(pattern: &str, options: i32) -> Result<Regex, RegexError> {
        if options & RIGHT_TO_LEFT != 0 {
            return Err(RegexError::Parse("RegexOptions.RightToLeft is not supported by this runtime.".into()));
        }
        let chars: Vec<char> = pattern.chars().collect();
        let mut p = Parser { s: &chars, i: 0, opts: options, unnamed: 0, named: Vec::new(), pattern };
        let mut root = p.parse_alt()?;
        if p.i < chars.len() {
            return Err(p.error("Too many )'s"));
        }
        // Named groups are numbered after every unnamed one. A numeric name
        // (`(?<3>...)`) takes that number.
        let unnamed = p.unnamed;
        let mut names: Vec<String> = (0..=unnamed).map(|n| n.to_string()).collect();
        let mut numbers: Vec<(String, usize)> = Vec::new();
        for name in &p.named {
            if numbers.iter().any(|(n, _)| n == name) {
                continue;
            }
            let number = match name.parse::<usize>() {
                Ok(n) => n,
                Err(_) => {
                    names.push(name.clone());
                    names.len() - 1
                }
            };
            while names.len() <= number {
                names.push(names.len().to_string());
            }
            numbers.push((name.clone(), number));
        }
        resolve(&mut root, &numbers, pattern)?;
        let group_count = names.len();
        check_backrefs(&root, group_count, pattern)?;
        let anchored = starts_anchored(&root);
        let first = first_literal(&root);
        Ok(Regex { root, group_count, names, options, anchored, first })
    }

    /// The first match at or after `start`. `prev_end` is where `\G` matches.
    pub fn find_at(&self, input: &[u16], start: usize, prev_end: usize) -> Result<Option<Captures>, RegexError> {
        let mut ctx = Ctx { input, caps: vec![None; self.group_count], prev_end, steps: 0, timed_out: false };
        let mut s = start;
        while s <= input.len() {
            if let Some(f) = self.first {
                match input[s..].iter().position(|u| *u == f) {
                    Some(off) => s += off,
                    None => return Ok(None),
                }
            }
            for c in ctx.caps.iter_mut() {
                *c = None;
            }
            let found = self.m(&self.root, s, &mut ctx, &mut |q, c| {
                c.caps[0] = Some((s, q));
                true
            });
            if ctx.timed_out {
                return Err(RegexError::Timeout);
            }
            if found {
                return Ok(Some(ctx.caps));
            }
            if self.anchored {
                break;
            }
            s += 1;
        }
        Ok(None)
    }

    /// Every non-overlapping match, as `Regex.Matches` enumerates them.
    pub fn find_all(&self, input: &[u16], start: usize) -> Result<Vec<Captures>, RegexError> {
        let mut out = Vec::new();
        let mut at = start;
        let mut prev = start;
        while at <= input.len() {
            let Some(caps) = self.find_at(input, at, prev)? else { break };
            let (s, e) = caps[0].unwrap_or((at, at));
            prev = e;
            at = if e == s { e + 1 } else { e };
            out.push(caps);
        }
        Ok(out)
    }

    pub fn group_number(&self, name: &str) -> Option<usize> {
        self.names.iter().position(|n| n == name)
    }

    fn m(&self, n: &Node, p: usize, c: &mut Ctx, k: &mut dyn FnMut(usize, &mut Ctx) -> bool) -> bool {
        c.steps += 1;
        if c.steps > STEP_LIMIT {
            c.timed_out = true;
        }
        if c.timed_out {
            return false;
        }
        let input = c.input;
        match n {
            Node::Empty => k(p, c),
            Node::Lit(..) | Node::Any(_) | Node::Set(..) => {
                if p < input.len() && single(n, input[p]) {
                    k(p + 1, c)
                } else {
                    false
                }
            }
            Node::Bol(multi) => (p == 0 || (*multi && input[p - 1] == b'\n' as u16)) && k(p, c),
            Node::Eol(multi) => {
                let ok = p == input.len()
                    || (*multi && input[p] == b'\n' as u16)
                    || (!*multi && p + 1 == input.len() && input[p] == b'\n' as u16);
                ok && k(p, c)
            }
            Node::Start => p == 0 && k(p, c),
            Node::End => p == input.len() && k(p, c),
            Node::EndZ => (p == input.len() || (p + 1 == input.len() && input[p] == b'\n' as u16)) && k(p, c),
            Node::Boundary(negate, ecma) => {
                let before = p > 0 && is_word(input[p - 1], *ecma);
                let after = p < input.len() && is_word(input[p], *ecma);
                ((before != after) != *negate) && k(p, c)
            }
            Node::PrevEnd => p == c.prev_end && k(p, c),
            Node::Group(inner, slot) => {
                let slot = *slot;
                self.m(inner, p, c, &mut |q, c| {
                    let saved = c.caps[slot];
                    c.caps[slot] = Some((p, q));
                    if k(q, c) {
                        true
                    } else {
                        c.caps[slot] = saved;
                        false
                    }
                })
            }
            Node::NamedGroup(..) | Node::NamedBackref(..) => false,
            Node::Concat(nodes) => self.seq(nodes, p, c, k),
            Node::Alt(options) => {
                for o in options {
                    if self.m(o, p, c, k) {
                        return true;
                    }
                    if c.timed_out {
                        return false;
                    }
                }
                false
            }
            Node::Repeat(inner, min, max, greedy) => {
                if is_single(inner) {
                    // One unit per iteration: count the run, then try lengths
                    // without recursing once per character.
                    let limit = (*max as usize).min(input.len() - p);
                    let mut run = 0;
                    while run < limit && single(inner, input[p + run]) {
                        run += 1;
                    }
                    let min = *min as usize;
                    if run < min {
                        return false;
                    }
                    if *greedy {
                        for n in (min..=run).rev() {
                            if k(p + n, c) {
                                return true;
                            }
                            if c.timed_out {
                                return false;
                            }
                        }
                    } else {
                        for n in min..=run {
                            if k(p + n, c) {
                                return true;
                            }
                            if c.timed_out {
                                return false;
                            }
                        }
                    }
                    return false;
                }
                self.rep(inner, *min, *max, *greedy, 0, p, c, k)
            }
            Node::Look(inner, behind, negate) => {
                let saved = c.caps.clone();
                let matched = if *behind {
                    let mut hit = false;
                    for s in (0..=p).rev() {
                        if self.m(inner, s, c, &mut |q, _| q == p) {
                            hit = true;
                            break;
                        }
                        if c.timed_out {
                            return false;
                        }
                    }
                    hit
                } else {
                    self.m(inner, p, c, &mut |_, _| true)
                };
                if *negate {
                    c.caps = saved;
                    !matched && k(p, c)
                } else if matched {
                    if k(p, c) {
                        true
                    } else {
                        c.caps = saved;
                        false
                    }
                } else {
                    c.caps = saved;
                    false
                }
            }
            Node::Atomic(inner) => {
                let saved = c.caps.clone();
                let mut end = None;
                if !self.m(inner, p, c, &mut |q, _| {
                    end = Some(q);
                    true
                }) {
                    return false;
                }
                let end = end.unwrap_or(p);
                if k(end, c) {
                    true
                } else {
                    c.caps = saved;
                    false
                }
            }
            Node::Backref(slot, ci) => match c.caps.get(*slot).copied().flatten() {
                Some((s, e)) => {
                    let len = e - s;
                    if p + len > input.len() {
                        return false;
                    }
                    let same = (0..len).all(|i| {
                        let (a, b) = (input[s + i], input[p + i]);
                        a == b || (*ci && fold(a) == fold(b))
                    });
                    same && k(p + len, c)
                }
                // .NET: a reference to a group that has not matched fails.
                None => self.options & ECMASCRIPT != 0 && k(p, c),
            },
        }
    }

    fn seq(&self, nodes: &[Node], p: usize, c: &mut Ctx, k: &mut dyn FnMut(usize, &mut Ctx) -> bool) -> bool {
        match nodes.split_first() {
            None => k(p, c),
            Some((head, rest)) => self.m(head, p, c, &mut |q, c| self.seq(rest, q, c, k)),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn rep(&self, n: &Node, min: u32, max: u32, greedy: bool, count: u32, p: usize, c: &mut Ctx, k: &mut dyn FnMut(usize, &mut Ctx) -> bool) -> bool {
        if greedy {
            if count < max
                && self.m(n, p, c, &mut |q, c| {
                    // An empty iteration past the minimum cannot make progress.
                    if q == p && count >= min {
                        return false;
                    }
                    self.rep(n, min, max, greedy, count + 1, q, c, k)
                })
            {
                return true;
            }
            count >= min && !c.timed_out && k(p, c)
        } else {
            if count >= min && k(p, c) {
                return true;
            }
            count < max
                && !c.timed_out
                && self.m(n, p, c, &mut |q, c| {
                    if q == p && count >= min {
                        return false;
                    }
                    self.rep(n, min, max, greedy, count + 1, q, c, k)
                })
        }
    }
}

struct Ctx<'a> {
    input: &'a [u16],
    caps: Captures,
    prev_end: usize,
    steps: u64,
    timed_out: bool,
}

fn is_single(n: &Node) -> bool {
    matches!(n, Node::Lit(..) | Node::Any(_) | Node::Set(..))
}

fn single(n: &Node, u: u16) -> bool {
    match n {
        Node::Lit(ch, ci) => u == *ch || (*ci && fold(u) == fold(*ch)),
        Node::Any(singleline) => *singleline || u != b'\n' as u16,
        Node::Set(set, ci) => set.contains(u) || (*ci && (set.contains(fold(u)) || set.contains(upper(u)))),
        _ => false,
    }
}

fn as_char(u: u16) -> Option<char> {
    char::from_u32(u as u32)
}

/// Lower-case folding for a code unit, when it folds to one unit.
pub fn fold(u: u16) -> u16 {
    match as_char(u) {
        Some(c) if c.is_ascii() => c.to_ascii_lowercase() as u16,
        Some(c) => {
            let mut l = c.to_lowercase();
            match (l.next(), l.next()) {
                (Some(x), None) if (x as u32) < 0x10000 => x as u16,
                _ => u,
            }
        }
        None => u,
    }
}

fn upper(u: u16) -> u16 {
    match as_char(u) {
        Some(c) => {
            let mut l = c.to_uppercase();
            match (l.next(), l.next()) {
                (Some(x), None) if (x as u32) < 0x10000 => x as u16,
                _ => u,
            }
        }
        None => u,
    }
}

fn is_word(u: u16, ecma: bool) -> bool {
    if ecma {
        return as_char(u).map(|c| c.is_ascii_alphanumeric() || c == '_').unwrap_or(false);
    }
    match as_char(u) {
        Some(c) => c.is_alphanumeric() || c == '_' || is_mark(c as u32) || matches!(c, '\u{203F}' | '\u{2040}'),
        None => false,
    }
}

fn is_digit(u: u16, ecma: bool) -> bool {
    match as_char(u) {
        Some(c) if ecma => c.is_ascii_digit(),
        Some(c) => c.is_ascii_digit() || (c.is_numeric() && !c.is_ascii() && decimal_digit(c as u32)),
        None => false,
    }
}

/// Unicode `Nd` outside ASCII: the decimal digit blocks, by their zero.
fn decimal_digit(cp: u32) -> bool {
    const ZEROS: &[u32] = &[
        0x0660, 0x06F0, 0x07C0, 0x0966, 0x09E6, 0x0A66, 0x0AE6, 0x0B66, 0x0BE6, 0x0C66, 0x0CE6, 0x0D66, 0x0DE6,
        0x0E50, 0x0ED0, 0x0F20, 0x1040, 0x1090, 0x17E0, 0x1810, 0x1946, 0x19D0, 0x1A80, 0x1A90, 0x1B50, 0x1BB0,
        0x1C40, 0x1C50, 0xA620, 0xA8D0, 0xA900, 0xA9D0, 0xA9F0, 0xAA50, 0xABF0, 0xFF10,
    ];
    ZEROS.iter().any(|z| cp >= *z && cp < z + 10)
}

fn is_space(u: u16, ecma: bool) -> bool {
    match as_char(u) {
        Some(c) if ecma => matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{B}' | '\u{C}'),
        Some(c) => c.is_whitespace(),
        None => false,
    }
}

fn is_mark(cp: u32) -> bool {
    matches!(cp, 0x0300..=0x036F | 0x0483..=0x0489 | 0x0591..=0x05BD | 0x0610..=0x061A | 0x064B..=0x065F
        | 0x0900..=0x0903 | 0x093A..=0x094F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE00..=0xFE0F | 0xFE20..=0xFE2F)
}

fn punct_kind(cp: u32) -> Option<Cat> {
    let c = char::from_u32(cp)?;
    Some(match c {
        '_' | '\u{203F}' | '\u{2040}' | '\u{2054}' | '\u{FE33}' | '\u{FE34}' | '\u{FE4D}'..='\u{FE4F}' | '\u{FF3F}' => Cat::Pc,
        '-' | '\u{058A}' | '\u{05BE}' | '\u{2010}'..='\u{2015}' | '\u{2E17}' | '\u{301C}' | '\u{30A0}' | '\u{FE58}' | '\u{FE63}' | '\u{FF0D}' => Cat::Pd,
        '(' | '[' | '{' | '\u{2045}' | '\u{207D}' | '\u{208D}' | '\u{2329}' | '\u{3008}' | '\u{300A}' | '\u{300C}' | '\u{300E}' | '\u{3010}' | '\u{FF08}' | '\u{FF3B}' | '\u{FF5B}' => Cat::Ps,
        ')' | ']' | '}' | '\u{2046}' | '\u{207E}' | '\u{208E}' | '\u{232A}' | '\u{3009}' | '\u{300B}' | '\u{300D}' | '\u{300F}' | '\u{3011}' | '\u{FF09}' | '\u{FF3D}' | '\u{FF5D}' => Cat::Pe,
        '\u{AB}' | '\u{2018}' | '\u{201B}' | '\u{201C}' | '\u{201F}' | '\u{2039}' => Cat::Pi,
        '\u{BB}' | '\u{2019}' | '\u{201D}' | '\u{203A}' => Cat::Pf,
        '!' | '"' | '#' | '%' | '&' | '\'' | '*' | ',' | '.' | '/' | ':' | ';' | '?' | '@' | '\\' | '\u{A1}' | '\u{A7}' | '\u{B6}' | '\u{B7}' | '\u{BF}'
        | '\u{2016}' | '\u{2017}' | '\u{2020}'..='\u{2027}' | '\u{2030}'..='\u{2038}' | '\u{203B}'..='\u{203E}' | '\u{3001}'..='\u{3003}'
        | '\u{FF01}'..='\u{FF03}' | '\u{FF05}'..='\u{FF07}' | '\u{FF0A}' | '\u{FF0C}' | '\u{FF0E}' | '\u{FF0F}' | '\u{FF1A}' | '\u{FF1B}' | '\u{FF1F}' | '\u{FF20}'
        | '\u{060C}' | '\u{061B}' | '\u{061F}' | '\u{0964}' | '\u{0965}' => Cat::Po,
        _ => return None,
    })
}

fn symbol_kind(cp: u32) -> Option<Cat> {
    let c = char::from_u32(cp)?;
    Some(match c {
        '$' | '\u{A2}'..='\u{A5}' | '\u{058F}' | '\u{060B}' | '\u{09F2}' | '\u{09F3}' | '\u{0E3F}' | '\u{20A0}'..='\u{20CF}' | '\u{FDFC}' | '\u{FF04}' | '\u{FFE0}' | '\u{FFE1}' | '\u{FFE5}' | '\u{FFE6}' => Cat::Sc,
        '+' | '<' | '=' | '>' | '|' | '~' | '\u{AC}' | '\u{B1}' | '\u{D7}' | '\u{F7}' | '\u{03F6}' | '\u{2044}' | '\u{2052}' | '\u{207A}'..='\u{207C}'
        | '\u{208A}'..='\u{208C}' | '\u{2118}' | '\u{2140}'..='\u{2144}' | '\u{2190}'..='\u{2194}' | '\u{2200}'..='\u{22FF}' | '\u{2308}'..='\u{230B}'
        | '\u{27C0}'..='\u{27FF}' | '\u{2900}'..='\u{2AFF}' | '\u{FF0B}' | '\u{FF1C}'..='\u{FF1E}' | '\u{FF5C}' | '\u{FF5E}' => Cat::Sm,
        '^' | '`' | '\u{A8}' | '\u{AF}' | '\u{B4}' | '\u{B8}' | '\u{02C2}'..='\u{02C5}' | '\u{02D2}'..='\u{02DF}' | '\u{FF3E}' | '\u{FF40}' => Cat::Sk,
        '\u{A6}' | '\u{A9}' | '\u{AE}' | '\u{B0}' | '\u{2100}'..='\u{2117}' | '\u{2119}'..='\u{213F}' | '\u{2195}'..='\u{21FF}' | '\u{2300}'..='\u{2307}'
        | '\u{230C}'..='\u{23FF}' | '\u{2400}'..='\u{27BF}' | '\u{2B00}'..='\u{2BFF}' | '\u{3004}' | '\u{3012}' | '\u{3013}' | '\u{3020}' => Cat::So,
        _ => return None,
    })
}

fn in_category(u: u16, cat: &Cat) -> bool {
    let cp = u as u32;
    if let Cat::Block(lo, hi) = cat {
        return cp >= *lo && cp <= *hi;
    }
    if let Cat::Cs = cat {
        return (0xD800..=0xDFFF).contains(&cp);
    }
    let Some(c) = as_char(u) else { return matches!(cat, Cat::C) };
    let letter = c.is_alphabetic() && !c.is_numeric() && !is_mark(cp);
    match cat {
        Cat::L => letter,
        Cat::Lu => c.is_uppercase(),
        Cat::Ll => c.is_lowercase(),
        Cat::Lt => matches!(cp, 0x01C5 | 0x01C8 | 0x01CB | 0x01F2 | 0x1F88..=0x1F8F | 0x1F98..=0x1F9F | 0x1FA8..=0x1FAF | 0x1FBC | 0x1FCC | 0x1FFC),
        Cat::Lm => matches!(cp, 0x02B0..=0x02C1 | 0x02C6..=0x02D1 | 0x02E0..=0x02E4 | 0x02EC | 0x02EE | 0x3005 | 0x309D | 0x309E | 0x30FC..=0x30FE | 0xFF70),
        Cat::Lo => letter && !c.is_uppercase() && !c.is_lowercase() && !in_category(u, &Cat::Lm),
        Cat::M => is_mark(cp),
        Cat::N => c.is_numeric(),
        Cat::Nd => is_digit(u, false),
        Cat::Nl => matches!(cp, 0x16EE..=0x16F0 | 0x2160..=0x2188 | 0x3007 | 0x3021..=0x3029 | 0x3038..=0x303A),
        Cat::No => c.is_numeric() && !is_digit(u, false) && !in_category(u, &Cat::Nl),
        Cat::P => punct_kind(cp).is_some(),
        Cat::Pc | Cat::Pd | Cat::Ps | Cat::Pe | Cat::Pi | Cat::Pf | Cat::Po => punct_kind(cp).as_ref() == Some(cat),
        Cat::S => symbol_kind(cp).is_some(),
        Cat::Sm | Cat::Sc | Cat::Sk | Cat::So => symbol_kind(cp).as_ref() == Some(cat),
        Cat::Z => matches!(cp, 0x20 | 0xA0 | 0x1680 | 0x2000..=0x200A | 0x2028 | 0x2029 | 0x202F | 0x205F | 0x3000),
        Cat::Zs => matches!(cp, 0x20 | 0xA0 | 0x1680 | 0x2000..=0x200A | 0x202F | 0x205F | 0x3000),
        Cat::Zl => cp == 0x2028,
        Cat::Zp => cp == 0x2029,
        Cat::C => c.is_control() || in_category(u, &Cat::Cf),
        Cat::Cc => c.is_control(),
        Cat::Cf => matches!(cp, 0xAD | 0x0600..=0x0605 | 0x061C | 0x06DD | 0x180E | 0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x2064 | 0x2066..=0x206F | 0xFEFF),
        Cat::Never => false,
        Cat::Cs | Cat::Block(..) => false,
    }
}

fn category(name: &str) -> Option<Cat> {
    Some(match name {
        "L" => Cat::L,
        "Lu" => Cat::Lu,
        "Ll" => Cat::Ll,
        "Lt" => Cat::Lt,
        "Lm" => Cat::Lm,
        "Lo" => Cat::Lo,
        "M" | "Mn" | "Mc" | "Me" => Cat::M,
        "N" => Cat::N,
        "Nd" => Cat::Nd,
        "Nl" => Cat::Nl,
        "No" => Cat::No,
        "P" => Cat::P,
        "Pc" => Cat::Pc,
        "Pd" => Cat::Pd,
        "Ps" => Cat::Ps,
        "Pe" => Cat::Pe,
        "Pi" => Cat::Pi,
        "Pf" => Cat::Pf,
        "Po" => Cat::Po,
        "S" => Cat::S,
        "Sm" => Cat::Sm,
        "Sc" => Cat::Sc,
        "Sk" => Cat::Sk,
        "So" => Cat::So,
        "Z" => Cat::Z,
        "Zs" => Cat::Zs,
        "Zl" => Cat::Zl,
        "Zp" => Cat::Zp,
        "C" => Cat::C,
        "Cc" => Cat::Cc,
        "Cf" => Cat::Cf,
        "Cs" => Cat::Cs,
        "Co" | "Cn" => Cat::Never,
        "IsBasicLatin" => Cat::Block(0x0000, 0x007F),
        "IsLatin-1Supplement" => Cat::Block(0x0080, 0x00FF),
        "IsLatinExtended-A" => Cat::Block(0x0100, 0x017F),
        "IsLatinExtended-B" => Cat::Block(0x0180, 0x024F),
        "IsIPAExtensions" => Cat::Block(0x0250, 0x02AF),
        "IsCombiningDiacriticalMarks" => Cat::Block(0x0300, 0x036F),
        "IsGreek" | "IsGreekandCoptic" => Cat::Block(0x0370, 0x03FF),
        "IsCyrillic" => Cat::Block(0x0400, 0x04FF),
        "IsArmenian" => Cat::Block(0x0530, 0x058F),
        "IsHebrew" => Cat::Block(0x0590, 0x05FF),
        "IsArabic" => Cat::Block(0x0600, 0x06FF),
        "IsDevanagari" => Cat::Block(0x0900, 0x097F),
        "IsThai" => Cat::Block(0x0E00, 0x0E7F),
        "IsGeorgian" => Cat::Block(0x10A0, 0x10FF),
        "IsHangulJamo" => Cat::Block(0x1100, 0x11FF),
        "IsLatinExtendedAdditional" => Cat::Block(0x1E00, 0x1EFF),
        "IsGreekExtended" => Cat::Block(0x1F00, 0x1FFF),
        "IsGeneralPunctuation" => Cat::Block(0x2000, 0x206F),
        "IsCurrencySymbols" => Cat::Block(0x20A0, 0x20CF),
        "IsLetterlikeSymbols" => Cat::Block(0x2100, 0x214F),
        "IsArrows" => Cat::Block(0x2190, 0x21FF),
        "IsMathematicalOperators" => Cat::Block(0x2200, 0x22FF),
        "IsBoxDrawing" => Cat::Block(0x2500, 0x257F),
        "IsGeometricShapes" => Cat::Block(0x25A0, 0x25FF),
        "IsMiscellaneousSymbols" => Cat::Block(0x2600, 0x26FF),
        "IsDingbats" => Cat::Block(0x2700, 0x27BF),
        "IsCJKSymbolsandPunctuation" => Cat::Block(0x3000, 0x303F),
        "IsHiragana" => Cat::Block(0x3040, 0x309F),
        "IsKatakana" => Cat::Block(0x30A0, 0x30FF),
        "IsCJKUnifiedIdeographs" => Cat::Block(0x4E00, 0x9FFF),
        "IsHangulSyllables" => Cat::Block(0xAC00, 0xD7AF),
        "IsHighSurrogates" => Cat::Block(0xD800, 0xDB7F),
        "IsLowSurrogates" => Cat::Block(0xDC00, 0xDFFF),
        "IsPrivateUse" | "IsPrivateUseArea" => Cat::Block(0xE000, 0xF8FF),
        "IsHalfwidthandFullwidthForms" => Cat::Block(0xFF00, 0xFFEF),
        "IsSpecials" => Cat::Block(0xFFF0, 0xFFFF),
        _ => return None,
    })
}

impl Set {
    fn contains(&self, u: u16) -> bool {
        let mut hit = self.items.iter().any(|item| match item {
            Item::Range(lo, hi) => (*lo..=*hi).contains(&(u as u32)),
            Item::Digit(neg) => is_digit(u, self.ecma) != *neg,
            Item::Word(neg) => is_word(u, self.ecma) != *neg,
            Item::Space(neg) => is_space(u, self.ecma) != *neg,
            Item::Category(cat, neg) => in_category(u, cat) != *neg,
        });
        if self.negate {
            hit = !hit;
        }
        if hit {
            if let Some(sub) = &self.subtract {
                if sub.contains(u) {
                    return false;
                }
            }
        }
        hit
    }
}

struct Parser<'a> {
    s: &'a [char],
    i: usize,
    opts: i32,
    unnamed: usize,
    named: Vec<String>,
    pattern: &'a str,
}

impl Parser<'_> {
    fn error(&self, what: &str) -> RegexError {
        RegexError::Parse(format!("Invalid pattern '{}' at offset {}. {}.", self.pattern, self.i, what))
    }

    fn peek(&self) -> Option<char> {
        self.s.get(self.i).copied()
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn has(&self, flag: i32) -> bool {
        self.opts & flag != 0
    }

    fn parse_alt(&mut self) -> Result<Node, RegexError> {
        let mut options = vec![self.parse_seq()?];
        while self.eat('|') {
            options.push(self.parse_seq()?);
        }
        Ok(if options.len() == 1 { options.pop().unwrap() } else { Node::Alt(options) })
    }

    fn parse_seq(&mut self) -> Result<Node, RegexError> {
        let mut nodes = Vec::new();
        while let Some(c) = self.peek() {
            if c == '|' || c == ')' {
                break;
            }
            if self.has(IGNORE_PATTERN_WHITESPACE) {
                if c.is_whitespace() {
                    self.i += 1;
                    continue;
                }
                if c == '#' {
                    while let Some(c) = self.peek() {
                        self.i += 1;
                        if c == '\n' {
                            break;
                        }
                    }
                    continue;
                }
            }
            let atom = self.parse_atom()?;
            let atom = self.parse_quantifier(atom)?;
            match atom {
                Node::Empty => {}
                Node::Concat(mut inner) => nodes.append(&mut inner),
                other => nodes.push(other),
            }
        }
        Ok(match nodes.len() {
            0 => Node::Empty,
            1 => nodes.pop().unwrap(),
            _ => Node::Concat(nodes),
        })
    }

    fn parse_quantifier(&mut self, atom: Node) -> Result<Node, RegexError> {
        let (min, max) = match self.peek() {
            Some('*') => {
                self.i += 1;
                (0, u32::MAX)
            }
            Some('+') => {
                self.i += 1;
                (1, u32::MAX)
            }
            Some('?') => {
                self.i += 1;
                (0, 1)
            }
            Some('{') => match self.braces() {
                Some(range) => range,
                None => return Ok(atom),
            },
            _ => return Ok(atom),
        };
        if matches!(atom, Node::Empty | Node::Bol(_) | Node::Eol(_) | Node::Start | Node::End | Node::EndZ) && min == 0 && max == u32::MAX {
            return Err(self.error("Quantifier following nothing"));
        }
        let greedy = !self.eat('?');
        if matches!(self.peek(), Some('*' | '+' | '?')) || (self.peek() == Some('{') && self.lookahead_braces()) {
            return Err(self.error("Nested quantifier"));
        }
        // A character outside the BMP is two units, and .NET quantifies only
        // the second: `😀+` repeats the low surrogate.
        if let Node::Concat(units) = &atom {
            if let [Node::Lit(high, _), Node::Lit(..)] = units.as_slice() {
                if (0xD800..=0xDBFF).contains(high) {
                    let Node::Concat(mut units) = atom else { unreachable!() };
                    let last = units.pop().unwrap();
                    units.push(Node::Repeat(Box::new(last), min, max, greedy));
                    return Ok(Node::Concat(units));
                }
            }
        }
        Ok(Node::Repeat(Box::new(atom), min, max, greedy))
    }

    fn lookahead_braces(&mut self) -> bool {
        let at = self.i;
        let ok = self.braces().is_some();
        self.i = at;
        ok
    }

    /// `{n}`, `{n,}` or `{n,m}`; anything else leaves `{` a literal.
    fn braces(&mut self) -> Option<(u32, u32)> {
        let start = self.i;
        self.i += 1;
        let min = self.number();
        let result = match (min, self.peek()) {
            (Some(n), Some('}')) => {
                self.i += 1;
                Some((n, n))
            }
            (Some(n), Some(',')) => {
                self.i += 1;
                let max = self.number();
                if self.eat('}') {
                    Some((n, max.unwrap_or(u32::MAX)))
                } else {
                    None
                }
            }
            _ => None,
        };
        if result.is_none() {
            self.i = start;
        }
        result
    }

    fn number(&mut self) -> Option<u32> {
        let start = self.i;
        while matches!(self.peek(), Some('0'..='9')) {
            self.i += 1;
        }
        if start == self.i {
            return None;
        }
        self.s[start..self.i].iter().collect::<String>().parse().ok()
    }

    fn lit(&self, c: char) -> Node {
        let ci = self.has(IGNORE_CASE);
        let mut buf = [0u16; 2];
        let units = c.encode_utf16(&mut buf);
        if units.len() == 1 {
            Node::Lit(units[0], ci)
        } else {
            Node::Concat(units.iter().map(|u| Node::Lit(*u, false)).collect())
        }
    }

    fn parse_atom(&mut self) -> Result<Node, RegexError> {
        let c = self.peek().unwrap();
        self.i += 1;
        Ok(match c {
            '.' => Node::Any(self.has(SINGLELINE)),
            '^' => Node::Bol(self.has(MULTILINE)),
            '$' => Node::Eol(self.has(MULTILINE)),
            '[' => Node::Set(Box::new(self.parse_set()?), self.has(IGNORE_CASE)),
            '(' => self.parse_group()?,
            '\\' => self.parse_escape()?,
            '*' | '+' | '?' => return Err(self.error("Quantifier following nothing")),
            '{' if self.i >= 1 && {
                self.i -= 1;
                let q = self.lookahead_braces();
                self.i += 1;
                q
            } =>
            {
                return Err(self.error("Quantifier following nothing"))
            }
            other => self.lit(other),
        })
    }

    fn parse_group(&mut self) -> Result<Node, RegexError> {
        let saved = self.opts;
        let node = if self.eat('?') {
            match self.peek() {
                Some(':') => {
                    self.i += 1;
                    Node::Concat(vec![self.parse_alt()?])
                }
                Some('=') | Some('!') => {
                    let negate = self.peek() == Some('!');
                    self.i += 1;
                    Node::Look(Box::new(self.parse_alt()?), false, negate)
                }
                Some('>') => {
                    self.i += 1;
                    Node::Atomic(Box::new(self.parse_alt()?))
                }
                Some('#') => {
                    while let Some(c) = self.peek() {
                        self.i += 1;
                        if c == ')' {
                            return Ok(Node::Empty);
                        }
                    }
                    return Err(self.error("Unterminated (?#...) comment"));
                }
                Some('<') if matches!(self.s.get(self.i + 1), Some('=') | Some('!')) => {
                    let negate = self.s[self.i + 1] == '!';
                    self.i += 2;
                    Node::Look(Box::new(self.parse_alt()?), true, negate)
                }
                Some('<') | Some('\'') => {
                    let close = if self.peek() == Some('<') { '>' } else { '\'' };
                    self.i += 1;
                    let start = self.i;
                    while matches!(self.peek(), Some(c) if c != close) {
                        self.i += 1;
                    }
                    let name: String = self.s[start..self.i].iter().collect();
                    if !self.eat(close) || name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                        return Err(self.error("Invalid group name"));
                    }
                    self.named.push(name.clone());
                    let inner = self.parse_alt()?;
                    Node::NamedGroup(Box::new(inner), name)
                }
                _ => {
                    // Inline options: (?imnsx-imnsx) or (?imnsx-imnsx:...).
                    let mut on = true;
                    let mut opts = self.opts;
                    loop {
                        let flag = match self.peek() {
                            Some('i') => IGNORE_CASE,
                            Some('m') => MULTILINE,
                            Some('n') => EXPLICIT_CAPTURE,
                            Some('s') => SINGLELINE,
                            Some('x') => IGNORE_PATTERN_WHITESPACE,
                            Some('-') => {
                                on = false;
                                self.i += 1;
                                continue;
                            }
                            Some(')') => {
                                self.i += 1;
                                // Applies to the rest of the enclosing group.
                                self.opts = opts;
                                return Ok(Node::Empty);
                            }
                            Some(':') => {
                                self.i += 1;
                                self.opts = opts;
                                let inner = self.parse_alt()?;
                                self.opts = saved;
                                if !self.eat(')') {
                                    return Err(self.error("Not enough )'s"));
                                }
                                return Ok(Node::Concat(vec![inner]));
                            }
                            _ => return Err(self.error("Unrecognized grouping construct")),
                        };
                        self.i += 1;
                        if on {
                            opts |= flag;
                        } else {
                            opts &= !flag;
                        }
                    }
                }
            }
        } else if self.has(EXPLICIT_CAPTURE) {
            Node::Concat(vec![self.parse_alt()?])
        } else {
            self.unnamed += 1;
            let slot = self.unnamed;
            Node::Group(Box::new(self.parse_alt()?), slot)
        };
        self.opts = saved;
        if !self.eat(')') {
            return Err(self.error("Not enough )'s"));
        }
        Ok(node)
    }

    fn parse_escape(&mut self) -> Result<Node, RegexError> {
        let Some(c) = self.peek() else { return Err(self.error("Illegal \\ at end of pattern")) };
        self.i += 1;
        let ecma = self.has(ECMASCRIPT);
        let class = |item: Item| Node::Set(Box::new(Set { items: vec![item], negate: false, subtract: None, ecma }), false);
        Ok(match c {
            'd' => class(Item::Digit(false)),
            'D' => class(Item::Digit(true)),
            'w' => class(Item::Word(false)),
            'W' => class(Item::Word(true)),
            's' => class(Item::Space(false)),
            'S' => class(Item::Space(true)),
            'p' | 'P' => class(Item::Category(self.property()?, c == 'P')),
            'b' => Node::Boundary(false, ecma),
            'B' => Node::Boundary(true, ecma),
            'A' => Node::Start,
            'z' => Node::End,
            'Z' => Node::EndZ,
            'G' => Node::PrevEnd,
            'k' => {
                let close = match self.peek() {
                    Some('<') => '>',
                    Some('\'') => '\'',
                    _ => return Err(self.error("Malformed \\k<...> named back reference")),
                };
                self.i += 1;
                let start = self.i;
                while matches!(self.peek(), Some(c) if c != close) {
                    self.i += 1;
                }
                let name: String = self.s[start..self.i].iter().collect();
                if !self.eat(close) {
                    return Err(self.error("Malformed \\k<...> named back reference"));
                }
                match name.parse::<usize>() {
                    Ok(n) => Node::Backref(n, self.has(IGNORE_CASE)),
                    Err(_) => Node::NamedBackref(name, self.has(IGNORE_CASE)),
                }
            }
            '1'..='9' => {
                self.i -= 1;
                let n = self.number().unwrap_or(0) as usize;
                Node::Backref(n, self.has(IGNORE_CASE))
            }
            other => {
                let ch = self.escaped_char(other)?;
                self.lit(ch)
            }
        })
    }

    /// The character a simple escape stands for, after its backslash.
    fn escaped_char(&mut self, c: char) -> Result<char, RegexError> {
        Ok(match c {
            't' => '\t',
            'n' => '\n',
            'r' => '\r',
            'f' => '\u{C}',
            'v' => '\u{B}',
            'e' => '\u{1B}',
            'a' => '\u{7}',
            '0' => {
                // Up to two more octal digits.
                let mut v = 0u32;
                for _ in 0..2 {
                    match self.peek() {
                        Some(d @ '0'..='7') => {
                            v = v * 8 + (d as u32 - '0' as u32);
                            self.i += 1;
                        }
                        _ => break,
                    }
                }
                char::from_u32(v).unwrap_or('\0')
            }
            'x' => self.hex(2)?,
            'u' => self.hex(4)?,
            'c' => match self.peek() {
                Some(l) if l.is_ascii_alphabetic() => {
                    self.i += 1;
                    char::from_u32(l.to_ascii_uppercase() as u32 - 64).unwrap_or('\0')
                }
                _ => return Err(self.error("Missing control character")),
            },
            c if c.is_ascii_alphanumeric() => return Err(self.error(&format!("Unrecognized escape sequence \\{c}"))),
            other => other,
        })
    }

    fn hex(&mut self, n: usize) -> Result<char, RegexError> {
        let digits: String = self.s.get(self.i..self.i + n).map(|d| d.iter().collect()).unwrap_or_default();
        let v = u32::from_str_radix(&digits, 16).map_err(|_| self.error("Insufficient hexadecimal digits"))?;
        self.i += n;
        char::from_u32(v).ok_or_else(|| self.error("Invalid hexadecimal value"))
    }

    fn property(&mut self) -> Result<Cat, RegexError> {
        if !self.eat('{') {
            return Err(self.error("Incomplete \\p{X} character escape"));
        }
        let start = self.i;
        while matches!(self.peek(), Some(c) if c != '}') {
            self.i += 1;
        }
        let name: String = self.s[start..self.i].iter().collect();
        if !self.eat('}') {
            return Err(self.error("Incomplete \\p{X} character escape"));
        }
        category(&name).ok_or_else(|| self.error(&format!("Unknown property '{name}'")))
    }

    fn parse_set(&mut self) -> Result<Set, RegexError> {
        let ecma = self.has(ECMASCRIPT);
        let mut set = Set { items: Vec::new(), negate: self.eat('^'), subtract: None, ecma };
        let mut first = true;
        loop {
            let Some(c) = self.peek() else { return Err(self.error("Unterminated [] set")) };
            if c == ']' && !first {
                self.i += 1;
                break;
            }
            first = false;
            // Subtraction: [a-z-[aeiou]] must be the last thing in the set.
            if c == '-' && self.s.get(self.i + 1) == Some(&'[') {
                self.i += 2;
                set.subtract = Some(Box::new(self.parse_set()?));
                if !self.eat(']') {
                    return Err(self.error("A subtraction must be the last element in a character class"));
                }
                break;
            }
            self.i += 1;
            let low = if c == '\\' {
                let Some(e) = self.peek() else { return Err(self.error("Illegal \\ at end of pattern")) };
                self.i += 1;
                match e {
                    'd' => { set.items.push(Item::Digit(false)); continue; }
                    'D' => { set.items.push(Item::Digit(true)); continue; }
                    'w' => { set.items.push(Item::Word(false)); continue; }
                    'W' => { set.items.push(Item::Word(true)); continue; }
                    's' => { set.items.push(Item::Space(false)); continue; }
                    'S' => { set.items.push(Item::Space(true)); continue; }
                    'p' | 'P' => {
                        let cat = self.property()?;
                        set.items.push(Item::Category(cat, e == 'P'));
                        continue;
                    }
                    'b' => '\u{8}',
                    other => self.escaped_char(other)?,
                }
            } else {
                c
            };
            // A range, unless the `-` is last or starts a subtraction.
            if self.peek() == Some('-') && !matches!(self.s.get(self.i + 1), Some(']') | Some('[') | None) {
                self.i += 1;
                let h = self.peek().unwrap();
                self.i += 1;
                let high = if h == '\\' {
                    let e = self.peek().ok_or_else(|| self.error("Illegal \\ at end of pattern"))?;
                    self.i += 1;
                    if e == 'b' { '\u{8}' } else { self.escaped_char(e)? }
                } else {
                    h
                };
                if (high as u32) < (low as u32) {
                    return Err(self.error("[x-y] range in reverse order"));
                }
                set.items.push(Item::Range(low as u32, high as u32));
            } else {
                set.items.push(Item::Range(low as u32, low as u32));
            }
        }
        Ok(set)
    }
}

fn resolve(n: &mut Node, numbers: &[(String, usize)], pattern: &str) -> Result<(), RegexError> {
    let lookup = |name: &str| numbers.iter().find(|(n, _)| n == name).map(|(_, v)| *v);
    match n {
        Node::NamedGroup(inner, name) => {
            resolve(inner, numbers, pattern)?;
            let slot = lookup(name).unwrap_or(0);
            let inner = core::mem::replace(inner.as_mut(), Node::Empty);
            *n = Node::Group(Box::new(inner), slot);
        }
        Node::NamedBackref(name, ci) => {
            let slot = lookup(name).ok_or_else(|| RegexError::Parse(format!("Invalid pattern '{pattern}'. Reference to undefined group name '{name}'.")))?;
            *n = Node::Backref(slot, *ci);
        }
        Node::Group(inner, _) | Node::Repeat(inner, ..) | Node::Look(inner, ..) | Node::Atomic(inner) => resolve(inner, numbers, pattern)?,
        Node::Concat(nodes) | Node::Alt(nodes) => {
            for x in nodes {
                resolve(x, numbers, pattern)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn check_backrefs(n: &Node, groups: usize, pattern: &str) -> Result<(), RegexError> {
    match n {
        Node::Backref(slot, _) if *slot >= groups => {
            Err(RegexError::Parse(format!("Invalid pattern '{pattern}'. Reference to undefined group number {slot}.")))
        }
        Node::Group(inner, _) | Node::Repeat(inner, ..) | Node::Look(inner, ..) | Node::Atomic(inner) => check_backrefs(inner, groups, pattern),
        Node::Concat(nodes) | Node::Alt(nodes) => nodes.iter().try_for_each(|x| check_backrefs(x, groups, pattern)),
        _ => Ok(()),
    }
}

/// Whether every match must start at position 0 (`\A`, or `^` without Multiline).
fn starts_anchored(n: &Node) -> bool {
    match n {
        Node::Start | Node::Bol(false) => true,
        Node::Concat(nodes) => nodes.first().map(starts_anchored).unwrap_or(false),
        Node::Group(inner, _) | Node::Atomic(inner) => starts_anchored(inner),
        Node::Alt(options) => options.iter().all(starts_anchored),
        _ => false,
    }
}

/// A case-sensitive literal every match starts with, to skip ahead to.
fn first_literal(n: &Node) -> Option<u16> {
    match n {
        Node::Lit(u, false) => Some(*u),
        Node::Concat(nodes) => nodes.first().and_then(first_literal),
        Node::Group(inner, _) | Node::Atomic(inner) => first_literal(inner),
        Node::Repeat(inner, min, ..) if *min > 0 => first_literal(inner),
        _ => None,
    }
}

/// `Regex.Escape`.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for c in s.chars() {
        match c {
            '\\' | '*' | '+' | '?' | '|' | '{' | '[' | '(' | ')' | '^' | '$' | '.' | '#' => {
                out.push('\\');
                out.push(c);
            }
            ' ' => out.push_str("\\ "),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\u{C}' => out.push_str("\\f"),
            _ => out.push(c),
        }
    }
    out
}

/// `Regex.Unescape`.
pub fn unescape(s: &str) -> Result<String, RegexError> {
    let chars: Vec<char> = s.chars().collect();
    let mut p = Parser { s: &chars, i: 0, opts: 0, unnamed: 0, named: Vec::new(), pattern: s };
    let mut out = String::new();
    while let Some(c) = p.peek() {
        p.i += 1;
        if c != '\\' {
            out.push(c);
            continue;
        }
        let Some(e) = p.peek() else { return Err(p.error("Illegal \\ at end of pattern")) };
        p.i += 1;
        out.push(if e.is_ascii_alphanumeric() { p.escaped_char(e)? } else { e });
    }
    Ok(out)
}

/// One piece of a replacement pattern.
#[derive(Debug, Clone)]
pub enum Piece {
    Text(Vec<u16>),
    Group(usize),
    Before,
    After,
    LastGroup,
    Input,
}

/// Parses `$1`, `${name}`, `$$`, `$&`, `` $` ``, `$'`, `$+` and `$_`. A `$`
/// that names no group stays literal, as in .NET.
pub fn parse_replacement(re: &Regex, replacement: &[u16]) -> Vec<Piece> {
    let mut out = Vec::new();
    let mut text: Vec<u16> = Vec::new();
    let mut i = 0;
    let dollar = b'$' as u16;
    while i < replacement.len() {
        let u = replacement[i];
        if u != dollar || i + 1 == replacement.len() {
            text.push(u);
            i += 1;
            continue;
        }
        let next = replacement[i + 1];
        let piece = match char::from_u32(next as u32) {
            Some('$') => {
                text.push(dollar);
                i += 2;
                continue;
            }
            Some('&') => Some((Piece::Group(0), 2)),
            Some('`') => Some((Piece::Before, 2)),
            Some('\'') => Some((Piece::After, 2)),
            Some('+') => Some((Piece::LastGroup, 2)),
            Some('_') => Some((Piece::Input, 2)),
            Some('{') => {
                let close = replacement[i + 2..].iter().position(|u| *u == b'}' as u16);
                close.and_then(|end| {
                    let name = String::from_utf16_lossy(&replacement[i + 2..i + 2 + end]);
                    let slot = match name.parse::<usize>() {
                        Ok(n) if n < re.group_count => Some(n),
                        Ok(_) => None,
                        Err(_) => re.group_number(&name),
                    };
                    slot.map(|s| (Piece::Group(s), end + 3))
                })
            }
            Some(d) if d.is_ascii_digit() => {
                // The longest run of digits that names a group.
                let mut end = i + 1;
                while end < replacement.len() && (replacement[end] as u8 as char).is_ascii_digit() && replacement[end] < 128 {
                    end += 1;
                }
                let mut found = None;
                while end > i + 1 {
                    let n: usize = String::from_utf16_lossy(&replacement[i + 1..end]).parse().unwrap_or(usize::MAX);
                    if n < re.group_count {
                        found = Some((Piece::Group(n), end - i));
                        break;
                    }
                    end -= 1;
                }
                found
            }
            _ => None,
        };
        match piece {
            Some((p, len)) => {
                if !text.is_empty() {
                    out.push(Piece::Text(core::mem::take(&mut text)));
                }
                out.push(p);
                i += len;
            }
            None => {
                text.push(dollar);
                i += 1;
            }
        }
    }
    if !text.is_empty() {
        out.push(Piece::Text(text));
    }
    out
}

/// Expands a parsed replacement for one match.
pub fn expand(pieces: &[Piece], input: &[u16], caps: &Captures, out: &mut Vec<u16>) {
    for p in pieces {
        match p {
            Piece::Text(t) => out.extend_from_slice(t),
            Piece::Group(n) => {
                if let Some(Some((s, e))) = caps.get(*n) {
                    out.extend_from_slice(&input[*s..*e]);
                }
            }
            Piece::Before => {
                if let Some(Some((s, _))) = caps.first() {
                    out.extend_from_slice(&input[..*s]);
                }
            }
            Piece::After => {
                if let Some(Some((_, e))) = caps.first() {
                    out.extend_from_slice(&input[*e..]);
                }
            }
            Piece::LastGroup => {
                if let Some(Some((s, e))) = caps.iter().skip(1).rev().find(|c| c.is_some()).or(caps.first()) {
                    out.extend_from_slice(&input[*s..*e]);
                }
            }
            Piece::Input => out.extend_from_slice(input),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    fn find(p: &str, s: &str) -> Option<String> {
        find_opt(p, 0, s)
    }

    fn find_opt(p: &str, o: i32, s: &str) -> Option<String> {
        let re = Regex::new(p, o).unwrap();
        let input = u(s);
        re.find_at(&input, 0, 0).unwrap().map(|c| {
            let (a, b) = c[0].unwrap();
            String::from_utf16_lossy(&input[a..b])
        })
    }

    fn groups(p: &str, s: &str) -> Vec<Option<String>> {
        let re = Regex::new(p, 0).unwrap();
        let input = u(s);
        let c = re.find_at(&input, 0, 0).unwrap().unwrap();
        c.iter().map(|g| g.map(|(a, b)| String::from_utf16_lossy(&input[a..b]))).collect()
    }

    #[test]
    fn literals_classes_and_quantifiers() {
        assert_eq!(find(r"\d+", "abc 123 def"), Some("123".into()));
        assert_eq!(find(r"[a-c]+", "xxbcaz"), Some("bca".into()));
        assert_eq!(find(r"colou?r", "the color"), Some("color".into()));
        assert_eq!(find(r"a{2,3}", "aaaa"), Some("aaa".into()));
        assert_eq!(find(r"a{2,3}?", "aaaa"), Some("aa".into()));
        assert_eq!(find(r"<.+?>", "<a><b>"), Some("<a>".into()));
        assert_eq!(find(r"<.+>", "<a><b>"), Some("<a><b>".into()));
        assert_eq!(find(r"x{2}", "x{2}"), None);
        assert_eq!(find(r"\{2\}", "x{2}"), Some("{2}".into()));
        assert_eq!(find(r"[^\s,]+", "  one, two"), Some("one".into()));
        assert_eq!(find(r"[a-z-[aeiou]]+", "aeibcdo"), Some("bcd".into()));
        assert_eq!(find(r"\bcat\b", "concat cat"), Some("cat".into()));
    }

    #[test]
    fn anchors_and_options() {
        assert_eq!(find(r"^b", "a\nb"), None);
        assert_eq!(find_opt(r"^b", MULTILINE, "a\nb"), Some("b".into()));
        assert_eq!(find(r"a$", "a\n"), Some("a".into()));
        assert_eq!(find_opt(r"HELLO", IGNORE_CASE, "say hello"), Some("hello".into()));
        assert_eq!(find(r"(?i)hello", "HeLLo"), Some("HeLLo".into()));
        assert_eq!(find(r"a(?i:B)c", "abc aBc"), Some("abc".into()));
        assert_eq!(find(r"a.c", "a\nc"), None);
        assert_eq!(find_opt(r"a.c", SINGLELINE, "a\nc"), Some("a\nc".into()));
        assert_eq!(find_opt("a b # comment\n c", IGNORE_PATTERN_WHITESPACE, "abc"), Some("abc".into()));
    }

    #[test]
    fn groups_backrefs_and_lookaround() {
        assert_eq!(groups(r"(\w+)@(\w+)\.com", "mail bob@example.com"), vec![Some("bob@example.com".into()), Some("bob".into()), Some("example".into())]);
        let re = Regex::new(r"(?<year>\d{4})-(\d{2})", 0).unwrap();
        assert_eq!(re.names, vec!["0", "1", "year"]);
        assert_eq!(groups(r"(?<year>\d{4})-(\d{2})", "2026-10"), vec![Some("2026-10".into()), Some("10".into()), Some("2026".into())]);
        assert_eq!(find(r"(\w)\1", "abccd"), Some("cc".into()));
        assert_eq!(find(r#"(?<q>['"]).*?\k<q>"#, r#"say "hi" ok"#), Some("\"hi\"".into()));
        assert_eq!(find(r"\d+(?= dollars)", "100 euros 200 dollars"), Some("200".into()));
        assert_eq!(find(r"(?<!\$)\b\d+", "$5 and 7"), Some("7".into()));
        assert_eq!(find(r"(?<=\$)\d+", "cost $42"), Some("42".into()));
        // No boundary inside "20em", so .NET finds the 30.
        assert_eq!(find(r"\d+(?!px)\b", "10px 20em 30 em"), Some("30".into()));
        assert_eq!(find(r"(?>a+)b", "aaab"), Some("aaab".into()));
        assert_eq!(find(r"(?>a+)ab", "aaab"), None);
        assert_eq!(groups(r"(a)|(b)", "b"), vec![Some("b".into()), None, Some("b".into())]);
        assert_eq!(groups(r"(?:(\d)x)+", "1x2x3x"), vec![Some("1x2x3x".into()), Some("3".into())]);
    }

    #[test]
    fn unicode_and_categories() {
        assert_eq!(find(r"\p{Lu}+", "abcÄÖÜdef"), Some("ÄÖÜ".into()));
        assert_eq!(find(r"\w+", "  café!"), Some("café".into()));
        assert_eq!(find(r"\p{IsCyrillic}+", "hi Привет"), Some("Привет".into()));
        assert_eq!(find(r"[\p{P}]+", "hi... ok"), Some("...".into()));
        assert_eq!(find(r"\P{L}+", "ab12cd"), Some("12".into()));
        assert_eq!(find(r"\d+", "٣٤٥"), Some("٣٤٥".into()));
        assert_eq!(find_opt(r"\d+", ECMASCRIPT, "٣٤5"), Some("5".into()));
    }

    #[test]
    fn matches_replacement_and_errors() {
        let re = Regex::new(r"x*", 0).unwrap();
        assert_eq!(re.find_all(&u("abc"), 0).unwrap().len(), 4);
        let re = Regex::new(r"(?<first>\w+) (?<last>\w+)", 0).unwrap();
        let input = u("Ada Lovelace");
        let caps = re.find_at(&input, 0, 0).unwrap().unwrap();
        let mut out = Vec::new();
        expand(&parse_replacement(&re, &u("${last}, $1 $$ $9")), &input, &caps, &mut out);
        assert_eq!(String::from_utf16_lossy(&out), "Lovelace, Ada $ $9");
        assert!(matches!(Regex::new("(abc", 0), Err(RegexError::Parse(_))));
        assert!(matches!(Regex::new("abc)", 0), Err(RegexError::Parse(_))));
        assert!(matches!(Regex::new(r"\k<nope>", 0), Err(RegexError::Parse(_))));
        assert!(matches!(Regex::new(r"a**", 0), Err(RegexError::Parse(_))));
        assert_eq!(escape("1+1=2 (ok?)"), r"1\+1=2\ \(ok\?\)");
        assert_eq!(unescape(r"a\tb\x41").unwrap(), "a\tbA");
    }

    #[test]
    fn long_inputs_do_not_recurse_per_character() {
        let s: String = "a".repeat(200_000);
        assert_eq!(find(r"a*", &s).map(|m| m.len()), Some(200_000));
        assert_eq!(find(r"[a-z]+$", &s).map(|m| m.len()), Some(200_000));
    }

    #[test]
    fn catastrophic_patterns_time_out() {
        let re = Regex::new(r"(a+)+b", 0).unwrap();
        let input = u(&"a".repeat(40));
        assert_eq!(re.find_at(&input, 0, 0), Err(RegexError::Timeout));
    }
}
