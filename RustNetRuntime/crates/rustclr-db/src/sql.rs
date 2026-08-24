//! SQL tokenizer, parser and expression evaluator for the embedded engine.

#[allow(unused_imports)]
use alloc::{boxed::Box, format, string::{String, ToString}, vec, vec::Vec};

use crate::Value;

// ----------------------------------------------------------- tokens --

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    Ident(String),
    Kw(String), // uppercased keyword-shaped ident
    Int(i64),
    Real(f64),
    Str(String),
    Sym(char),
    Le,
    Ge,
    Ne,
    Question,
}

const KEYWORDS: &[&str] = &[
    "CREATE", "TABLE", "IF", "NOT", "EXISTS", "DROP", "INSERT", "INTO", "VALUES", "SELECT",
    "FROM", "WHERE", "ORDER", "BY", "ASC", "DESC", "LIMIT", "UPDATE", "SET", "DELETE", "AND",
    "OR", "NULL", "IS", "LIKE", "COUNT", "SUM", "AVG", "MIN", "MAX", "INTEGER", "INT", "REAL",
    "TEXT", "BLOB", "PRIMARY", "KEY", "AUTOINCREMENT", "INDEX", "ON",
];

pub fn tokenize(sql: &str) -> Result<Vec<Tok>, String> {
    let chars: Vec<char> = sql.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            ' ' | '\t' | '\r' | '\n' => i += 1,
            '\'' => {
                let mut s = String::new();
                i += 1;
                loop {
                    match chars.get(i) {
                        Some('\'') if chars.get(i + 1) == Some(&'\'') => {
                            s.push('\'');
                            i += 2;
                        }
                        Some('\'') => {
                            i += 1;
                            break;
                        }
                        Some(ch) => {
                            s.push(*ch);
                            i += 1;
                        }
                        None => return Err("unterminated string literal".into()),
                    }
                }
                out.push(Tok::Str(s));
            }
            '?' => {
                out.push(Tok::Question);
                i += 1;
            }
            '<' if chars.get(i + 1) == Some(&'=') => {
                out.push(Tok::Le);
                i += 2;
            }
            '>' if chars.get(i + 1) == Some(&'=') => {
                out.push(Tok::Ge);
                i += 2;
            }
            '<' if chars.get(i + 1) == Some(&'>') => {
                out.push(Tok::Ne);
                i += 2;
            }
            '!' if chars.get(i + 1) == Some(&'=') => {
                out.push(Tok::Ne);
                i += 2;
            }
            '(' | ')' | ',' | '*' | '=' | '<' | '>' | '+' | '-' | '.' | ';' => {
                out.push(Tok::Sym(c));
                i += 1;
            }
            _ if c.is_ascii_digit() => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                    i += 1;
                }
                let text: String = chars[start..i].iter().collect();
                if text.contains('.') {
                    out.push(Tok::Real(text.parse().map_err(|_| format!("bad number {text}"))?));
                } else {
                    out.push(Tok::Int(text.parse().map_err(|_| format!("bad number {text}"))?));
                }
            }
            _ if c.is_alphabetic() || c == '_' => {
                let start = i;
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                let word: String = chars[start..i].iter().collect();
                let upper = word.to_ascii_uppercase();
                if KEYWORDS.contains(&upper.as_str()) {
                    out.push(Tok::Kw(upper));
                } else {
                    out.push(Tok::Ident(word));
                }
            }
            _ => return Err(format!("unexpected character '{c}'")),
        }
    }
    Ok(out)
}

// -------------------------------------------------------------- AST --

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColType {
    Integer,
    Real,
    Text,
    Blob,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    Add,
    Sub,
}

#[derive(Debug, Clone)]
pub enum Expr {
    Lit(Value),
    Col(String),
    Param(usize),
    Bin(BinOp, Box<Expr>, Box<Expr>),
    Not(Box<Expr>),
    IsNull(Box<Expr>, bool),
    Like(Box<Expr>, Box<Expr>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agg {
    Count,
    Sum,
    Avg,
    Min,
    Max,
}

#[derive(Debug, Clone)]
pub enum SelItem {
    Star,
    Col(String),
    Agg(Agg, Option<String>),
}

#[derive(Debug, Clone)]
pub enum Stmt {
    CreateTable { name: String, if_not_exists: bool, columns: Vec<(String, ColType)> },
    DropTable { name: String, if_exists: bool },
    Insert { table: String, columns: Vec<String>, rows: Vec<Vec<Expr>> },
    Select {
        table: String,
        items: Vec<SelItem>,
        where_: Option<Expr>,
        order_by: Option<(String, bool)>, // (col, descending)
        limit: Option<usize>,
    },
    Update { table: String, sets: Vec<(String, Expr)>, where_: Option<Expr> },
    Delete { table: String, where_: Option<Expr> },
    CreateIndex { name: String, table: String, column: String, if_not_exists: bool },
    DropIndex { name: String, if_exists: bool },
}

// ------------------------------------------------------------ parser --

pub struct Parser {
    toks: Vec<Tok>,
    pos: usize,
    pub params_used: usize,
}

impl Parser {
    pub fn new(sql: &str) -> Result<Self, String> {
        Ok(Parser { toks: tokenize(sql)?, pos: 0, params_used: 0 })
    }

    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn eat_kw(&mut self, kw: &str) -> bool {
        if matches!(self.peek(), Some(Tok::Kw(k)) if k == kw) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect_kw(&mut self, kw: &str) -> Result<(), String> {
        if self.eat_kw(kw) {
            Ok(())
        } else {
            Err(format!("expected {kw}, found {:?}", self.peek()))
        }
    }

    fn eat_sym(&mut self, c: char) -> bool {
        if matches!(self.peek(), Some(Tok::Sym(s)) if *s == c) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect_sym(&mut self, c: char) -> Result<(), String> {
        if self.eat_sym(c) {
            Ok(())
        } else {
            Err(format!("expected '{c}', found {:?}", self.peek()))
        }
    }

    fn ident(&mut self) -> Result<String, String> {
        match self.next() {
            Some(Tok::Ident(s)) => Ok(s),
            Some(Tok::Kw(k)) => Ok(k.to_ascii_lowercase()), // allow keyword-named cols
            other => Err(format!("expected identifier, found {other:?}")),
        }
    }

    pub fn parse_stmt(&mut self) -> Result<Stmt, String> {
        let stmt = if self.eat_kw("CREATE") {
            if self.eat_kw("INDEX") {
                // CREATE INDEX [IF NOT EXISTS] name ON table (column)
                let if_not_exists = if self.eat_kw("IF") {
                    self.expect_kw("NOT")?;
                    self.expect_kw("EXISTS")?;
                    true
                } else {
                    false
                };
                let name = self.ident()?;
                self.expect_kw("ON")?;
                let table = self.ident()?;
                self.expect_sym('(')?;
                let column = self.ident()?;
                self.expect_sym(')')?;
                Stmt::CreateIndex { name, table, column, if_not_exists }
            } else {
                self.expect_kw("TABLE")?;
                let if_not_exists = if self.eat_kw("IF") {
                    self.expect_kw("NOT")?;
                    self.expect_kw("EXISTS")?;
                    true
                } else {
                    false
                };
                let name = self.ident()?;
                self.expect_sym('(')?;
                let mut columns = Vec::new();
                loop {
                    let col = self.ident()?;
                    let ty = match self.next() {
                        Some(Tok::Kw(k)) => match k.as_str() {
                            "INTEGER" | "INT" => ColType::Integer,
                            "REAL" => ColType::Real,
                            "TEXT" => ColType::Text,
                            "BLOB" => ColType::Blob,
                            other => return Err(format!("unknown column type {other}")),
                        },
                        other => return Err(format!("expected column type, found {other:?}")),
                    };
                    // Accept and ignore PRIMARY KEY [AUTOINCREMENT] / NOT NULL.
                    loop {
                        if self.eat_kw("PRIMARY") {
                            self.expect_kw("KEY")?;
                            let _ = self.eat_kw("AUTOINCREMENT");
                        } else if self.eat_kw("NOT") {
                            self.expect_kw("NULL")?;
                        } else {
                            break;
                        }
                    }
                    columns.push((col, ty));
                    if !self.eat_sym(',') {
                        break;
                    }
                }
                self.expect_sym(')')?;
                Stmt::CreateTable { name, if_not_exists, columns }
            }
        } else if self.eat_kw("DROP") {
            if self.eat_kw("INDEX") {
                let if_exists = if self.eat_kw("IF") {
                    self.expect_kw("EXISTS")?;
                    true
                } else {
                    false
                };
                Stmt::DropIndex { name: self.ident()?, if_exists }
            } else {
                self.expect_kw("TABLE")?;
                let if_exists = if self.eat_kw("IF") {
                    self.expect_kw("EXISTS")?;
                    true
                } else {
                    false
                };
                Stmt::DropTable { name: self.ident()?, if_exists }
            }
        } else if self.eat_kw("INSERT") {
            self.expect_kw("INTO")?;
            let table = self.ident()?;
            let mut columns = Vec::new();
            if self.eat_sym('(') {
                loop {
                    columns.push(self.ident()?);
                    if !self.eat_sym(',') {
                        break;
                    }
                }
                self.expect_sym(')')?;
            }
            self.expect_kw("VALUES")?;
            let mut rows = Vec::new();
            loop {
                self.expect_sym('(')?;
                let mut vals = Vec::new();
                loop {
                    vals.push(self.expr()?);
                    if !self.eat_sym(',') {
                        break;
                    }
                }
                self.expect_sym(')')?;
                rows.push(vals);
                if !self.eat_sym(',') {
                    break;
                }
            }
            Stmt::Insert { table, columns, rows }
        } else if self.eat_kw("SELECT") {
            let mut items = Vec::new();
            loop {
                if self.eat_sym('*') {
                    items.push(SelItem::Star);
                } else if let Some(Tok::Kw(k)) = self.peek() {
                    let agg = match k.as_str() {
                        "COUNT" => Some(Agg::Count),
                        "SUM" => Some(Agg::Sum),
                        "AVG" => Some(Agg::Avg),
                        "MIN" => Some(Agg::Min),
                        "MAX" => Some(Agg::Max),
                        _ => None,
                    };
                    match agg {
                        Some(a) => {
                            self.pos += 1;
                            self.expect_sym('(')?;
                            let col = if self.eat_sym('*') { None } else { Some(self.ident()?) };
                            self.expect_sym(')')?;
                            items.push(SelItem::Agg(a, col));
                        }
                        None => items.push(SelItem::Col(self.ident()?)),
                    }
                } else {
                    items.push(SelItem::Col(self.ident()?));
                }
                if !self.eat_sym(',') {
                    break;
                }
            }
            self.expect_kw("FROM")?;
            let table = self.ident()?;
            let where_ = if self.eat_kw("WHERE") { Some(self.expr()?) } else { None };
            let order_by = if self.eat_kw("ORDER") {
                self.expect_kw("BY")?;
                let col = self.ident()?;
                let desc = if self.eat_kw("DESC") { true } else { !self.eat_kw("ASC") && false };
                Some((col, desc))
            } else {
                None
            };
            let limit = if self.eat_kw("LIMIT") {
                match self.next() {
                    Some(Tok::Int(n)) if n >= 0 => Some(n as usize),
                    other => return Err(format!("expected LIMIT count, found {other:?}")),
                }
            } else {
                None
            };
            Stmt::Select { table, items, where_, order_by, limit }
        } else if self.eat_kw("UPDATE") {
            let table = self.ident()?;
            self.expect_kw("SET")?;
            let mut sets = Vec::new();
            loop {
                let col = self.ident()?;
                self.expect_sym('=')?;
                sets.push((col, self.expr()?));
                if !self.eat_sym(',') {
                    break;
                }
            }
            let where_ = if self.eat_kw("WHERE") { Some(self.expr()?) } else { None };
            Stmt::Update { table, sets, where_ }
        } else if self.eat_kw("DELETE") {
            self.expect_kw("FROM")?;
            let table = self.ident()?;
            let where_ = if self.eat_kw("WHERE") { Some(self.expr()?) } else { None };
            Stmt::Delete { table, where_ }
        } else {
            return Err(format!("expected statement, found {:?}", self.peek()));
        };
        let _ = self.eat_sym(';');
        if self.pos != self.toks.len() {
            return Err(format!("trailing tokens after statement: {:?}", self.peek()));
        }
        Ok(stmt)
    }

    // expr := or_term; standard precedence OR < AND < NOT < cmp < add
    fn expr(&mut self) -> Result<Expr, String> {
        let mut left = self.and_term()?;
        while self.eat_kw("OR") {
            let right = self.and_term()?;
            left = Expr::Bin(BinOp::Or, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn and_term(&mut self) -> Result<Expr, String> {
        let mut left = self.not_term()?;
        while self.eat_kw("AND") {
            let right = self.not_term()?;
            left = Expr::Bin(BinOp::And, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn not_term(&mut self) -> Result<Expr, String> {
        if self.eat_kw("NOT") {
            Ok(Expr::Not(Box::new(self.not_term()?)))
        } else {
            self.cmp_term()
        }
    }

    fn cmp_term(&mut self) -> Result<Expr, String> {
        let left = self.add_term()?;
        if self.eat_kw("IS") {
            let negated = self.eat_kw("NOT");
            self.expect_kw("NULL")?;
            return Ok(Expr::IsNull(Box::new(left), !negated));
        }
        if self.eat_kw("LIKE") {
            let pat = self.add_term()?;
            return Ok(Expr::Like(Box::new(left), Box::new(pat)));
        }
        let op = match self.peek() {
            Some(Tok::Sym('=')) => Some(BinOp::Eq),
            Some(Tok::Ne) => Some(BinOp::Ne),
            Some(Tok::Sym('<')) => Some(BinOp::Lt),
            Some(Tok::Le) => Some(BinOp::Le),
            Some(Tok::Sym('>')) => Some(BinOp::Gt),
            Some(Tok::Ge) => Some(BinOp::Ge),
            _ => None,
        };
        match op {
            Some(op) => {
                self.pos += 1;
                let right = self.add_term()?;
                Ok(Expr::Bin(op, Box::new(left), Box::new(right)))
            }
            None => Ok(left),
        }
    }

    fn add_term(&mut self) -> Result<Expr, String> {
        let mut left = self.atom()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Sym('+')) => BinOp::Add,
                Some(Tok::Sym('-')) => BinOp::Sub,
                _ => break,
            };
            self.pos += 1;
            let right = self.atom()?;
            left = Expr::Bin(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn atom(&mut self) -> Result<Expr, String> {
        match self.next() {
            Some(Tok::Int(n)) => Ok(Expr::Lit(Value::Int(n))),
            Some(Tok::Real(f)) => Ok(Expr::Lit(Value::Real(f))),
            Some(Tok::Str(s)) => Ok(Expr::Lit(Value::Text(s))),
            Some(Tok::Kw(k)) if k == "NULL" => Ok(Expr::Lit(Value::Null)),
            Some(Tok::Question) => {
                let idx = self.params_used;
                self.params_used += 1;
                Ok(Expr::Param(idx))
            }
            Some(Tok::Sym('-')) => match self.next() {
                Some(Tok::Int(n)) => Ok(Expr::Lit(Value::Int(-n))),
                Some(Tok::Real(f)) => Ok(Expr::Lit(Value::Real(-f))),
                other => Err(format!("expected number after '-', found {other:?}")),
            },
            Some(Tok::Sym('(')) => {
                let e = self.expr()?;
                self.expect_sym(')')?;
                Ok(e)
            }
            Some(Tok::Ident(name)) => Ok(Expr::Col(name)),
            other => Err(format!("expected expression, found {other:?}")),
        }
    }
}

// -------------------------------------------------------- evaluation --

pub fn eval(
    expr: &Expr,
    columns: &[String],
    row: &[Value],
    params: &[Value],
) -> Result<Value, String> {
    match expr {
        Expr::Lit(v) => Ok(v.clone()),
        Expr::Param(i) => params
            .get(*i)
            .cloned()
            .ok_or_else(|| format!("missing parameter {}", i + 1)),
        Expr::Col(name) => {
            let idx = columns
                .iter()
                .position(|c| c.eq_ignore_ascii_case(name))
                .ok_or_else(|| format!("no such column: {name}"))?;
            Ok(row[idx].clone())
        }
        Expr::Not(e) => Ok(Value::Int(!truthy(&eval(e, columns, row, params)?) as i64)),
        Expr::IsNull(e, want_null) => {
            let v = eval(e, columns, row, params)?;
            Ok(Value::Int((matches!(v, Value::Null) == *want_null) as i64))
        }
        Expr::Like(e, pat) => {
            let v = eval(e, columns, row, params)?;
            let p = eval(pat, columns, row, params)?;
            match (v, p) {
                (Value::Text(s), Value::Text(p)) => Ok(Value::Int(like(&s, &p) as i64)),
                _ => Ok(Value::Int(0)),
            }
        }
        Expr::Bin(op, l, r) => {
            let lv = eval(l, columns, row, params)?;
            match op {
                BinOp::And => {
                    if !truthy(&lv) {
                        return Ok(Value::Int(0));
                    }
                    let rv = eval(r, columns, row, params)?;
                    Ok(Value::Int(truthy(&rv) as i64))
                }
                BinOp::Or => {
                    if truthy(&lv) {
                        return Ok(Value::Int(1));
                    }
                    let rv = eval(r, columns, row, params)?;
                    Ok(Value::Int(truthy(&rv) as i64))
                }
                BinOp::Add | BinOp::Sub => {
                    let rv = eval(r, columns, row, params)?;
                    arith(*op, &lv, &rv)
                }
                _ => {
                    let rv = eval(r, columns, row, params)?;
                    let ord = compare(&lv, &rv);
                    let b = match (op, ord) {
                        (_, None) => false, // NULL comparisons are false
                        (BinOp::Eq, Some(o)) => o == core::cmp::Ordering::Equal,
                        (BinOp::Ne, Some(o)) => o != core::cmp::Ordering::Equal,
                        (BinOp::Lt, Some(o)) => o == core::cmp::Ordering::Less,
                        (BinOp::Le, Some(o)) => o != core::cmp::Ordering::Greater,
                        (BinOp::Gt, Some(o)) => o == core::cmp::Ordering::Greater,
                        (BinOp::Ge, Some(o)) => o != core::cmp::Ordering::Less,
                        _ => unreachable!(),
                    };
                    Ok(Value::Int(b as i64))
                }
            }
        }
    }
}

pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Int(n) => *n != 0,
        Value::Real(f) => *f != 0.0,
        Value::Text(s) => !s.is_empty(),
        Value::Blob(b) => !b.is_empty(),
    }
}

fn arith(op: BinOp, l: &Value, r: &Value) -> Result<Value, String> {
    let as_f = |v: &Value| match v {
        Value::Int(n) => Some(*n as f64),
        Value::Real(f) => Some(*f),
        _ => None,
    };
    match (l, r) {
        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(match op {
            BinOp::Add => a + b,
            _ => a - b,
        })),
        _ => match (as_f(l), as_f(r)) {
            (Some(a), Some(b)) => Ok(Value::Real(match op {
                BinOp::Add => a + b,
                _ => a - b,
            })),
            _ => Err("arithmetic on non-numeric value".into()),
        },
    }
}

/// SQL ordering: NULL sorts first; numbers compare numerically across
/// Int/Real; text lexicographically.
pub fn compare(l: &Value, r: &Value) -> Option<core::cmp::Ordering> {
    use core::cmp::Ordering;
    match (l, r) {
        (Value::Null, _) | (_, Value::Null) => None,
        (Value::Int(a), Value::Int(b)) => Some(a.cmp(b)),
        (Value::Int(a), Value::Real(b)) => (*a as f64).partial_cmp(b),
        (Value::Real(a), Value::Int(b)) => a.partial_cmp(&(*b as f64)),
        (Value::Real(a), Value::Real(b)) => a.partial_cmp(b),
        (Value::Text(a), Value::Text(b)) => Some(a.cmp(b)),
        (Value::Blob(a), Value::Blob(b)) => Some(a.cmp(b)),
        _ => Some(Ordering::Equal), // mixed types: treat as equal (no match)
    }
}

/// SQL LIKE: `%` any run, `_` one char; case-insensitive like SQLite.
fn like(s: &str, pattern: &str) -> bool {
    fn rec(s: &[char], p: &[char]) -> bool {
        match p.first() {
            None => s.is_empty(),
            Some('%') => (0..=s.len()).any(|k| rec(&s[k..], &p[1..])),
            Some('_') => !s.is_empty() && rec(&s[1..], &p[1..]),
            Some(pc) => {
                s.first().is_some_and(|sc| sc.eq_ignore_ascii_case(pc)) && rec(&s[1..], &p[1..])
            }
        }
    }
    let s: Vec<char> = s.chars().collect();
    let p: Vec<char> = pattern.chars().collect();
    rec(&s, &p)
}
