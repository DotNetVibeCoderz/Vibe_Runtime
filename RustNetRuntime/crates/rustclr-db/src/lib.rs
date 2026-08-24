//! Embedded SQL database for RustNet devices.
//!
//! A self-contained, dependency-free engine speaking a practical subset of
//! SQLite's SQL: CREATE/DROP TABLE, INSERT (multi-row, `?` parameters),
//! SELECT with WHERE / ORDER BY / LIMIT and COUNT/SUM/AVG/MIN/MAX,
//! UPDATE, DELETE, and CREATE/DROP INDEX. Storage is either purely in-memory
//! or a snapshot file on any mounted filesystem (internal flash `/data`, SD
//! card `/sd`, USB drive `/usb`) written through the [`Storage`] trait.
//!
//! **Persistence** is either a full snapshot rewritten after every mutation, or
//! — when the backend implements the WAL methods — a **write-ahead log**: each
//! mutating statement is appended to the log and folded into a fresh snapshot at
//! a periodic checkpoint (and on open, after replay). WAL keeps per-write cost
//! bounded (one appended record) instead of rewriting the whole database.
//!
//! **Secondary indexes** (`CREATE INDEX name ON table(column)`) accelerate
//! equality lookups (`WHERE col = value`, including `?` params and one conjunct
//! of an `AND`): the planner serves candidate rows from the index instead of a
//! full scan, then re-checks the full predicate. Index maps are rebuilt after
//! each mutation (row positions are otherwise unstable) and their definitions
//! persist in the snapshot (format v2).

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

#[allow(unused_imports)]
use alloc::{boxed::Box, format, string::{String, ToString}, vec, vec::Vec};

mod sql;

pub use sql::{ColType, Stmt};
use sql::{eval, Agg, BinOp, Expr, Parser, SelItem};
use alloc::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Int(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

impl Value {
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Null => "null",
            Value::Int(_) => "integer",
            Value::Real(_) => "real",
            Value::Text(_) => "text",
            Value::Blob(_) => "blob",
        }
    }
}

/// Where snapshot bytes go. The firmware bridges this to its VFS so the
/// same database code serves flash, SD card and USB storage.
///
/// A backend may additionally support a **write-ahead log**: instead of
/// rewriting the whole snapshot on every mutation, the engine appends a compact
/// record of each mutating statement and periodically checkpoints (a full
/// snapshot + WAL truncate). The default methods make WAL opt-in — a backend
/// that only implements `load`/`save` keeps the simple full-snapshot behaviour.
pub trait Storage: Send {
    fn load(&mut self) -> Option<Vec<u8>>;
    fn save(&mut self, bytes: &[u8]) -> Result<(), String>;
    /// Whether this backend has a working WAL (append/read/truncate).
    fn supports_wal(&self) -> bool {
        false
    }
    fn append_wal(&mut self, _record: &[u8]) -> Result<(), String> {
        Ok(())
    }
    fn read_wal(&mut self) -> Vec<u8> {
        Vec::new()
    }
    fn truncate_wal(&mut self) -> Result<(), String> {
        Ok(())
    }
}

/// Full snapshot written after this many WAL-logged mutations.
const WAL_CHECKPOINT: usize = 64;

#[derive(Debug, Clone)]
struct Table {
    name: String,
    columns: Vec<(String, ColType)>,
    rows: Vec<Vec<Value>>,
}

impl Table {
    fn column_names(&self) -> Vec<String> {
        self.columns.iter().map(|(n, _)| n.clone()).collect()
    }

    fn col_index(&self, name: &str) -> Result<usize, String> {
        self.columns
            .iter()
            .position(|(n, _)| n.eq_ignore_ascii_case(name))
            .ok_or_else(|| format!("no such column: {name}"))
    }
}

/// A secondary index over one column: encoded column value -> the positions of
/// the rows carrying it. Rebuilt after any mutation of its table (positions are
/// otherwise unstable), so lookups are always consistent with the table.
#[derive(Debug, Clone)]
struct Index {
    name: String,
    table: String,
    column: String,
    map: BTreeMap<Vec<u8>, Vec<usize>>,
}

/// Result of one statement.
#[derive(Debug, Clone, PartialEq)]
pub enum ExecResult {
    Rows { columns: Vec<String>, rows: Vec<Vec<Value>> },
    Affected(usize),
}

pub struct Database {
    tables: Vec<Table>,
    indexes: Vec<Index>,
    storage: Option<Box<dyn Storage>>,
    /// Mutations appended to the WAL since the last checkpoint.
    wal_count: usize,
    /// True while replaying the WAL on open — mutations apply in memory but are
    /// not re-logged.
    replaying: bool,
}

impl Database {
    pub fn in_memory() -> Self {
        Database {
            tables: Vec::new(),
            indexes: Vec::new(),
            storage: None,
            wal_count: 0,
            replaying: false,
        }
    }

    /// Open over a storage backend; loads the existing snapshot if any, then
    /// replays and folds in the write-ahead log when the backend has one.
    pub fn open(mut storage: Box<dyn Storage>) -> Result<Self, String> {
        let (tables, index_defs) = match storage.load() {
            Some(bytes) => decode(&bytes)?,
            None => (Vec::new(), Vec::new()),
        };
        let mut db = Database {
            tables,
            indexes: Vec::new(),
            storage: Some(storage),
            wal_count: 0,
            replaying: false,
        };
        // Rebuild each persisted index's lookup map from the loaded rows.
        for (name, table, column) in index_defs {
            db.indexes.push(Index { name, table, column, map: BTreeMap::new() });
        }
        let table_names = db.table_names();
        for t in table_names {
            db.rebuild_indexes_for(&t);
        }
        // Replay the WAL on top of the snapshot, then checkpoint to fold it in.
        if db.storage.as_ref().map(|s| s.supports_wal()).unwrap_or(false) {
            let wal = db.storage.as_mut().unwrap().read_wal();
            if !wal.is_empty() {
                db.replay_wal(&wal)?;
                db.checkpoint()?;
            }
        }
        Ok(db)
    }

    fn build_index_map(table: &Table, col: usize) -> BTreeMap<Vec<u8>, Vec<usize>> {
        let mut map: BTreeMap<Vec<u8>, Vec<usize>> = BTreeMap::new();
        for (i, row) in table.rows.iter().enumerate() {
            map.entry(index_key(&row[col])).or_default().push(i);
        }
        map
    }

    /// Rebuild the lookup maps of every index on `table_name` from its rows.
    fn rebuild_indexes_for(&mut self, table_name: &str) {
        let targets: Vec<usize> = self
            .indexes
            .iter()
            .enumerate()
            .filter(|(_, ix)| ix.table.eq_ignore_ascii_case(table_name))
            .map(|(i, _)| i)
            .collect();
        for i in targets {
            let (tbl, col) = (self.indexes[i].table.clone(), self.indexes[i].column.clone());
            let new_map = match self.table(&tbl) {
                Ok(t) => t.col_index(&col).ok().map(|ci| Self::build_index_map(t, ci)),
                Err(_) => None,
            };
            if let Some(m) = new_map {
                self.indexes[i].map = m;
            }
        }
    }

    /// Candidate row positions when the WHERE clause has an equality on an
    /// indexed column; `None` means "no usable index, scan every row". The full
    /// WHERE is still evaluated on each candidate, so a superset is safe.
    fn indexed_candidates(
        &self,
        table: &str,
        where_: &Option<Expr>,
        params: &[Value],
    ) -> Option<Vec<usize>> {
        let w = where_.as_ref()?;
        let t = self.table(table).ok()?;
        let (col, val) = find_indexable_eq(w, params)?;
        let ci = t.col_index(&col).ok()?;
        let coerced = coerce(val, t.columns[ci].1).ok()?;
        let ix = self.indexes.iter().find(|ix| {
            ix.table.eq_ignore_ascii_case(table) && ix.column.eq_ignore_ascii_case(&col)
        })?;
        Some(ix.map.get(&index_key(&coerced)).cloned().unwrap_or_default())
    }

    fn index_defs(&self) -> Vec<(String, String, String)> {
        self.indexes
            .iter()
            .map(|ix| (ix.name.clone(), ix.table.clone(), ix.column.clone()))
            .collect()
    }

    fn table(&self, name: &str) -> Result<&Table, String> {
        self.tables
            .iter()
            .find(|t| t.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| format!("no such table: {name}"))
    }

    fn table_mut(&mut self, name: &str) -> Result<&mut Table, String> {
        self.tables
            .iter_mut()
            .find(|t| t.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| format!("no such table: {name}"))
    }

    fn persist(&mut self) -> Result<(), String> {
        let defs = self.index_defs();
        if let Some(storage) = &mut self.storage {
            let bytes = encode(&self.tables, &defs);
            storage.save(&bytes)?;
        }
        Ok(())
    }

    /// Persist one mutating statement: append it to the WAL (checkpointing when
    /// the log grows) if the backend supports one, else rewrite the snapshot.
    /// A no-op while replaying the WAL.
    fn persist_mutation(&mut self, sql: &str, params: &[Value]) -> Result<(), String> {
        if self.replaying {
            return Ok(());
        }
        let wal = self.storage.as_ref().map(|s| s.supports_wal()).unwrap_or(false);
        if !wal {
            return self.persist();
        }
        let record = encode_wal_record(sql, params);
        self.storage.as_mut().unwrap().append_wal(&record)?;
        self.wal_count += 1;
        if self.wal_count >= WAL_CHECKPOINT {
            self.checkpoint()?;
        }
        Ok(())
    }

    /// Write a full snapshot and truncate the WAL.
    fn checkpoint(&mut self) -> Result<(), String> {
        self.persist()?;
        if let Some(storage) = &mut self.storage {
            storage.truncate_wal()?;
        }
        self.wal_count = 0;
        Ok(())
    }

    /// Re-apply logged statements (in memory only) after loading the snapshot.
    fn replay_wal(&mut self, wal: &[u8]) -> Result<(), String> {
        self.replaying = true;
        let mut r = Reader { bytes: wal, pos: 0 };
        let mut result = Ok(());
        while r.pos < wal.len() {
            match decode_wal_record(&mut r).and_then(|(sql, params)| {
                self.execute(&sql, &params).map(|_| ())
            }) {
                Ok(()) => {}
                Err(e) => {
                    result = Err(e);
                    break;
                }
            }
        }
        self.replaying = false;
        result
    }

    pub fn execute(&mut self, sql_text: &str, params: &[Value]) -> Result<ExecResult, String> {
        let mut parser = Parser::new(sql_text)?;
        let stmt = parser.parse_stmt()?;
        if parser.params_used > params.len() {
            return Err(format!(
                "statement uses {} parameters, {} supplied",
                parser.params_used,
                params.len()
            ));
        }
        match stmt {
            Stmt::CreateTable { name, if_not_exists, columns } => {
                if self.table(&name).is_ok() {
                    if if_not_exists {
                        return Ok(ExecResult::Affected(0));
                    }
                    return Err(format!("table {name} already exists"));
                }
                self.tables.push(Table { name, columns, rows: Vec::new() });
                self.persist_mutation(sql_text, params)?;
                Ok(ExecResult::Affected(0))
            }
            Stmt::DropTable { name, if_exists } => {
                let before = self.tables.len();
                self.tables.retain(|t| !t.name.eq_ignore_ascii_case(&name));
                if self.tables.len() == before && !if_exists {
                    return Err(format!("no such table: {name}"));
                }
                self.indexes.retain(|ix| !ix.table.eq_ignore_ascii_case(&name));
                self.persist_mutation(sql_text, params)?;
                Ok(ExecResult::Affected(before - self.tables.len()))
            }
            Stmt::Insert { table, columns, rows } => {
                let empty: Vec<String> = Vec::new();
                let mut count = 0;
                for exprs in &rows {
                    let t = self.table(&table)?;
                    let targets: Vec<usize> = if columns.is_empty() {
                        (0..t.columns.len()).collect()
                    } else {
                        columns
                            .iter()
                            .map(|c| t.col_index(c))
                            .collect::<Result<_, _>>()?
                    };
                    if exprs.len() != targets.len() {
                        return Err(format!(
                            "{} values for {} columns",
                            exprs.len(),
                            targets.len()
                        ));
                    }
                    let mut row = vec![Value::Null; t.columns.len()];
                    for (slot, e) in targets.iter().zip(exprs) {
                        row[*slot] = coerce(eval(e, &empty, &[], params)?, t.columns[*slot].1)?;
                    }
                    self.table_mut(&table)?.rows.push(row);
                    count += 1;
                }
                self.rebuild_indexes_for(&table);
                self.persist_mutation(sql_text, params)?;
                Ok(ExecResult::Affected(count))
            }
            Stmt::Select { table, items, where_, order_by, limit } => {
                let candidates = self.indexed_candidates(&table, &where_, params);
                let t = self.table(&table)?;
                let names = t.column_names();
                let positions: Vec<usize> =
                    candidates.unwrap_or_else(|| (0..t.rows.len()).collect());
                let mut selected: Vec<&Vec<Value>> = Vec::new();
                for &pos in &positions {
                    let Some(row) = t.rows.get(pos) else { continue };
                    if let Some(w) = &where_ {
                        if !sql::truthy(&eval(w, &names, row, params)?) {
                            continue;
                        }
                    }
                    selected.push(row);
                }
                if let Some((col, desc)) = &order_by {
                    let idx = t.col_index(col)?;
                    selected.sort_by(|a, b| {
                        let ord = sql::compare(&a[idx], &b[idx])
                            .unwrap_or(core::cmp::Ordering::Equal);
                        if *desc {
                            ord.reverse()
                        } else {
                            ord
                        }
                    });
                }
                if let Some(n) = limit {
                    selected.truncate(n);
                }
                // Aggregate query when any item is an aggregate.
                if items.iter().any(|i| matches!(i, SelItem::Agg(..))) {
                    let mut cols = Vec::new();
                    let mut row = Vec::new();
                    for item in &items {
                        match item {
                            SelItem::Agg(a, col) => {
                                let (name, v) = aggregate(t, *a, col.as_deref(), &selected)?;
                                cols.push(name);
                                row.push(v);
                            }
                            _ => return Err("cannot mix aggregates and columns".into()),
                        }
                    }
                    return Ok(ExecResult::Rows { columns: cols, rows: vec![row] });
                }
                let proj: Vec<usize> = {
                    let mut out = Vec::new();
                    for item in &items {
                        match item {
                            SelItem::Star => out.extend(0..t.columns.len()),
                            SelItem::Col(c) => out.push(t.col_index(c)?),
                            SelItem::Agg(..) => unreachable!(),
                        }
                    }
                    out
                };
                let columns = proj.iter().map(|i| names[*i].clone()).collect();
                let rows = selected
                    .iter()
                    .map(|row| proj.iter().map(|i| row[*i].clone()).collect())
                    .collect();
                Ok(ExecResult::Rows { columns, rows })
            }
            Stmt::Update { table, sets, where_ } => {
                let (names, set_idx): (Vec<String>, Vec<usize>) = {
                    let t = self.table(&table)?;
                    let names = t.column_names();
                    let idx = sets
                        .iter()
                        .map(|(c, _)| t.col_index(c))
                        .collect::<Result<Vec<_>, _>>()?;
                    (names, idx)
                };
                let col_types: Vec<ColType> =
                    self.table(&table)?.columns.iter().map(|(_, t)| *t).collect();
                let candidates = self.indexed_candidates(&table, &where_, params);
                let t = self.table_mut(&table)?;
                let positions: Vec<usize> =
                    candidates.unwrap_or_else(|| (0..t.rows.len()).collect());
                let mut affected = 0;
                for pos in positions {
                    let Some(row) = t.rows.get_mut(pos) else { continue };
                    if let Some(w) = &where_ {
                        if !sql::truthy(&eval(w, &names, row, params)?) {
                            continue;
                        }
                    }
                    for (slot, (_, e)) in set_idx.iter().zip(&sets) {
                        row[*slot] = coerce(eval(e, &names, row, params)?, col_types[*slot])?;
                    }
                    affected += 1;
                }
                self.rebuild_indexes_for(&table);
                self.persist_mutation(sql_text, params)?;
                Ok(ExecResult::Affected(affected))
            }
            Stmt::Delete { table, where_ } => {
                let names = self.table(&table)?.column_names();
                let t = self.table_mut(&table)?;
                let before = t.rows.len();
                let mut err = None;
                t.rows.retain(|row| match &where_ {
                    None => false,
                    Some(w) => match eval(w, &names, row, params) {
                        Ok(v) => !sql::truthy(&v),
                        Err(e) => {
                            err = Some(e);
                            true
                        }
                    },
                });
                if let Some(e) = err {
                    return Err(e);
                }
                let affected = before - t.rows.len();
                self.rebuild_indexes_for(&table);
                self.persist_mutation(sql_text, params)?;
                Ok(ExecResult::Affected(affected))
            }
            Stmt::CreateIndex { name, table, column, if_not_exists } => {
                if self.indexes.iter().any(|ix| ix.name.eq_ignore_ascii_case(&name)) {
                    if if_not_exists {
                        return Ok(ExecResult::Affected(0));
                    }
                    return Err(format!("index {name} already exists"));
                }
                // Validate the table and column, then build the lookup map.
                let ci = self.table(&table)?.col_index(&column)?;
                let map = Self::build_index_map(self.table(&table)?, ci);
                self.indexes.push(Index { name, table, column, map });
                self.persist_mutation(sql_text, params)?;
                Ok(ExecResult::Affected(0))
            }
            Stmt::DropIndex { name, if_exists } => {
                let before = self.indexes.len();
                self.indexes.retain(|ix| !ix.name.eq_ignore_ascii_case(&name));
                if self.indexes.len() == before && !if_exists {
                    return Err(format!("no such index: {name}"));
                }
                self.persist_mutation(sql_text, params)?;
                Ok(ExecResult::Affected(before - self.indexes.len()))
            }
        }
    }

    pub fn table_names(&self) -> Vec<String> {
        self.tables.iter().map(|t| t.name.clone()).collect()
    }
}

fn aggregate(
    t: &Table,
    agg: Agg,
    col: Option<&str>,
    rows: &[&Vec<Value>],
) -> Result<(String, Value), String> {
    let idx = match col {
        Some(c) => Some(t.col_index(c)?),
        None => None,
    };
    let name = format!(
        "{}({})",
        match agg {
            Agg::Count => "count",
            Agg::Sum => "sum",
            Agg::Avg => "avg",
            Agg::Min => "min",
            Agg::Max => "max",
        },
        col.unwrap_or("*")
    );
    if agg == Agg::Count {
        let n = match idx {
            None => rows.len(),
            Some(i) => rows.iter().filter(|r| !matches!(r[i], Value::Null)).count(),
        };
        return Ok((name, Value::Int(n as i64)));
    }
    let i = idx.ok_or("aggregate needs a column")?;
    let nums: Vec<f64> = rows
        .iter()
        .filter_map(|r| match &r[i] {
            Value::Int(n) => Some(*n as f64),
            Value::Real(f) => Some(*f),
            _ => None,
        })
        .collect();
    let v = match agg {
        Agg::Sum => Value::Real(nums.iter().sum()),
        Agg::Avg if nums.is_empty() => Value::Null,
        Agg::Avg => Value::Real(nums.iter().sum::<f64>() / nums.len() as f64),
        Agg::Min => nums
            .iter()
            .cloned()
            .fold(None::<f64>, |m, x| Some(m.map_or(x, |m| m.min(x))))
            .map(Value::Real)
            .unwrap_or(Value::Null),
        Agg::Max => nums
            .iter()
            .cloned()
            .fold(None::<f64>, |m, x| Some(m.map_or(x, |m| m.max(x))))
            .map(Value::Real)
            .unwrap_or(Value::Null),
        Agg::Count => unreachable!(),
    };
    Ok((name, v))
}

/// Store values under the column's declared affinity, SQLite-style.
fn coerce(v: Value, ty: ColType) -> Result<Value, String> {
    Ok(match (ty, v) {
        (_, Value::Null) => Value::Null,
        (ColType::Integer, Value::Int(n)) => Value::Int(n),
        (ColType::Integer, Value::Real(f)) => Value::Int(f as i64),
        (ColType::Real, Value::Int(n)) => Value::Real(n as f64),
        (ColType::Real, Value::Real(f)) => Value::Real(f),
        (ColType::Text, Value::Text(s)) => Value::Text(s),
        (ColType::Text, Value::Int(n)) => Value::Text(n.to_string()),
        (ColType::Text, Value::Real(f)) => Value::Text(f.to_string()),
        (ColType::Blob, Value::Blob(b)) => Value::Blob(b),
        (ColType::Blob, Value::Text(s)) => Value::Blob(s.into_bytes()),
        (ty, v) => return Err(format!("cannot store {} in {ty:?} column", v.type_name())),
    })
}

/// A stable, order-preserving byte key for a column value — used as the
/// secondary-index map key. Values in a column are coerced to the column type,
/// so equality lookups compute the same key from a coerced query value.
fn index_key(v: &Value) -> Vec<u8> {
    let mut k = Vec::new();
    match v {
        Value::Null => k.push(0),
        Value::Int(n) => {
            k.push(1);
            k.extend_from_slice(&n.to_le_bytes());
        }
        Value::Real(f) => {
            k.push(2);
            k.extend_from_slice(&f.to_le_bytes());
        }
        Value::Text(s) => {
            k.push(3);
            k.extend_from_slice(s.as_bytes());
        }
        Value::Blob(b) => {
            k.push(4);
            k.extend_from_slice(b);
        }
    }
    k
}

/// Extract a `column = value` equality usable by an index from a WHERE tree
/// (descending through `AND`), resolving parameters. Returns (column, value).
fn find_indexable_eq(e: &Expr, params: &[Value]) -> Option<(String, Value)> {
    match e {
        Expr::Bin(BinOp::Eq, l, r) => match (l.as_ref(), r.as_ref()) {
            (Expr::Col(c), Expr::Lit(v)) | (Expr::Lit(v), Expr::Col(c)) => {
                Some((c.clone(), v.clone()))
            }
            (Expr::Col(c), Expr::Param(i)) | (Expr::Param(i), Expr::Col(c)) => {
                params.get(*i).map(|v| (c.clone(), v.clone()))
            }
            _ => None,
        },
        Expr::Bin(BinOp::And, l, r) => {
            find_indexable_eq(l, params).or_else(|| find_indexable_eq(r, params))
        }
        _ => None,
    }
}

// ------------------------------------------------------- persistence --

const MAGIC: &[u8; 4] = b"RNDB";
const VERSION: u16 = 2;

fn encode(tables: &[Table], indexes: &[(String, String, String)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&(tables.len() as u32).to_le_bytes());
    for t in tables {
        put_str(&mut out, &t.name);
        out.extend_from_slice(&(t.columns.len() as u32).to_le_bytes());
        for (name, ty) in &t.columns {
            put_str(&mut out, name);
            out.push(*ty as u8);
        }
        out.extend_from_slice(&(t.rows.len() as u32).to_le_bytes());
        for row in &t.rows {
            for v in row {
                encode_value(&mut out, v);
            }
        }
    }
    // Index definitions (v2): name, table, column.
    out.extend_from_slice(&(indexes.len() as u32).to_le_bytes());
    for (name, table, column) in indexes {
        put_str(&mut out, name);
        put_str(&mut out, table);
        put_str(&mut out, column);
    }
    out
}

type Decoded = (Vec<Table>, Vec<(String, String, String)>);

fn decode(bytes: &[u8]) -> Result<Decoded, String> {
    let mut r = Reader { bytes, pos: 0 };
    if r.take(4)? != MAGIC {
        return Err("not a RustNet database file".into());
    }
    let version = u16::from_le_bytes(r.take(2)?.try_into().unwrap());
    if version == 0 || version > VERSION {
        return Err(format!("unsupported database version {version}"));
    }
    let ntables = r.u32()? as usize;
    let mut tables = Vec::with_capacity(ntables);
    for _ in 0..ntables {
        let name = r.string()?;
        let ncols = r.u32()? as usize;
        let mut columns = Vec::with_capacity(ncols);
        for _ in 0..ncols {
            let cname = r.string()?;
            let ty = match r.u8()? {
                0 => ColType::Integer,
                1 => ColType::Real,
                2 => ColType::Text,
                3 => ColType::Blob,
                other => return Err(format!("bad column type {other}")),
            };
            columns.push((cname, ty));
        }
        let nrows = r.u32()? as usize;
        let mut rows = Vec::with_capacity(nrows);
        for _ in 0..nrows {
            let mut row = Vec::with_capacity(ncols);
            for _ in 0..ncols {
                row.push(decode_value(&mut r)?);
            }
            rows.push(row);
        }
        tables.push(Table { name, columns, rows });
    }
    // Index definitions (v2+); older snapshots simply have none.
    let mut indexes = Vec::new();
    if version >= 2 {
        let nidx = r.u32()? as usize;
        for _ in 0..nidx {
            let name = r.string()?;
            let table = r.string()?;
            let column = r.string()?;
            indexes.push((name, table, column));
        }
    }
    Ok((tables, indexes))
}

fn encode_value(out: &mut Vec<u8>, v: &Value) {
    match v {
        Value::Null => out.push(0),
        Value::Int(n) => {
            out.push(1);
            out.extend_from_slice(&n.to_le_bytes());
        }
        Value::Real(f) => {
            out.push(2);
            out.extend_from_slice(&f.to_le_bytes());
        }
        Value::Text(s) => {
            out.push(3);
            put_str(out, s);
        }
        Value::Blob(b) => {
            out.push(4);
            out.extend_from_slice(&(b.len() as u32).to_le_bytes());
            out.extend_from_slice(b);
        }
    }
}

fn decode_value(r: &mut Reader) -> Result<Value, String> {
    Ok(match r.u8()? {
        0 => Value::Null,
        1 => Value::Int(i64::from_le_bytes(r.take(8)?.try_into().unwrap())),
        2 => Value::Real(f64::from_le_bytes(r.take(8)?.try_into().unwrap())),
        3 => Value::Text(r.string()?),
        4 => {
            let n = r.u32()? as usize;
            Value::Blob(r.take(n)?.to_vec())
        }
        other => return Err(format!("bad value tag {other}")),
    })
}

/// A WAL entry: the mutating statement's SQL text plus its bound parameters,
/// enough to re-run it verbatim during replay.
fn encode_wal_record(sql: &str, params: &[Value]) -> Vec<u8> {
    let mut out = Vec::new();
    put_str(&mut out, sql);
    out.extend_from_slice(&(params.len() as u32).to_le_bytes());
    for v in params {
        encode_value(&mut out, v);
    }
    out
}

fn decode_wal_record(r: &mut Reader) -> Result<(String, Vec<Value>), String> {
    let sql = r.string()?;
    let n = r.u32()? as usize;
    let mut params = Vec::with_capacity(n);
    for _ in 0..n {
        params.push(decode_value(r)?);
    }
    Ok((sql, params))
}

fn put_str(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(&(s.len() as u32).to_le_bytes());
    out.extend_from_slice(s.as_bytes());
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        if self.pos + n > self.bytes.len() {
            return Err("truncated database file".into());
        }
        let s = &self.bytes[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn string(&mut self) -> Result<String, String> {
        let n = self.u32()? as usize;
        String::from_utf8(self.take(n)?.to_vec()).map_err(|_| "bad utf8 in db".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn rows(r: ExecResult) -> Vec<Vec<Value>> {
        match r {
            ExecResult::Rows { rows, .. } => rows,
            other => panic!("expected rows, got {other:?}"),
        }
    }

    #[test]
    fn create_insert_select() {
        let mut db = Database::in_memory();
        db.execute("CREATE TABLE sensors (id INTEGER, name TEXT, temp REAL)", &[]).unwrap();
        db.execute(
            "INSERT INTO sensors VALUES (1, 'kitchen', 21.5), (2, 'garage', 16.0), (3, 'attic', 28.9)",
            &[],
        )
        .unwrap();
        let r = rows(
            db.execute("SELECT name FROM sensors WHERE temp > 20 ORDER BY temp DESC", &[])
                .unwrap(),
        );
        assert_eq!(
            r,
            vec![vec![Value::Text("attic".into())], vec![Value::Text("kitchen".into())]]
        );
    }

    #[test]
    fn params_update_delete() {
        let mut db = Database::in_memory();
        db.execute("CREATE TABLE kv (k TEXT, v INTEGER)", &[]).unwrap();
        db.execute(
            "INSERT INTO kv (k, v) VALUES (?, ?)",
            &[Value::Text("boots".into()), Value::Int(7)],
        )
        .unwrap();
        db.execute("UPDATE kv SET v = v + 1 WHERE k = ?", &[Value::Text("boots".into())])
            .unwrap();
        let r = rows(db.execute("SELECT v FROM kv WHERE k = 'boots'", &[]).unwrap());
        assert_eq!(r, vec![vec![Value::Int(8)]]);
        let del = db.execute("DELETE FROM kv WHERE v >= 8", &[]).unwrap();
        assert_eq!(del, ExecResult::Affected(1));
        assert_eq!(rows(db.execute("SELECT * FROM kv", &[]).unwrap()).len(), 0);
    }

    #[test]
    fn aggregates_and_like() {
        let mut db = Database::in_memory();
        db.execute("CREATE TABLE t (name TEXT, x INTEGER)", &[]).unwrap();
        db.execute("INSERT INTO t VALUES ('alpha', 1), ('beta', 2), ('alps', 3)", &[]).unwrap();
        let r = rows(db.execute("SELECT COUNT(*), SUM(x), AVG(x) FROM t", &[]).unwrap());
        assert_eq!(r[0][0], Value::Int(3));
        assert_eq!(r[0][1], Value::Real(6.0));
        assert_eq!(r[0][2], Value::Real(2.0));
        let r = rows(db.execute("SELECT name FROM t WHERE name LIKE 'al%'", &[]).unwrap());
        assert_eq!(r.len(), 2);
    }

    struct MemStorage(Arc<Mutex<Option<Vec<u8>>>>);
    impl Storage for MemStorage {
        fn load(&mut self) -> Option<Vec<u8>> {
            self.0.lock().unwrap().clone()
        }
        fn save(&mut self, bytes: &[u8]) -> Result<(), String> {
            *self.0.lock().unwrap() = Some(bytes.to_vec());
            Ok(())
        }
    }

    #[test]
    fn persistence_roundtrip() {
        let shared = Arc::new(Mutex::new(None));
        {
            let mut db = Database::open(Box::new(MemStorage(shared.clone()))).unwrap();
            db.execute("CREATE TABLE logs (msg TEXT, sev INTEGER)", &[]).unwrap();
            db.execute("INSERT INTO logs VALUES ('boot ok', 0), ('temp high', 2)", &[]).unwrap();
        }
        // Reopen from the saved snapshot.
        let mut db = Database::open(Box::new(MemStorage(shared))).unwrap();
        let r = rows(db.execute("SELECT msg FROM logs WHERE sev >= 2", &[]).unwrap());
        assert_eq!(r, vec![vec![Value::Text("temp high".into())]]);
    }

    #[test]
    fn secondary_index_equality_lookup() {
        let mut db = Database::in_memory();
        db.execute("CREATE TABLE users (id INTEGER, city TEXT)", &[]).unwrap();
        db.execute(
            "INSERT INTO users VALUES (1,'oslo'),(2,'riga'),(3,'oslo'),(4,'riga'),(5,'oslo')",
            &[],
        )
        .unwrap();
        db.execute("CREATE INDEX idx_city ON users (city)", &[]).unwrap();

        // Index-served equality returns exactly the matching rows.
        let r = rows(db.execute("SELECT id FROM users WHERE city = 'oslo'", &[]).unwrap());
        assert_eq!(r, vec![vec![Value::Int(1)], vec![Value::Int(3)], vec![Value::Int(5)]]);
        // Parameterised, and combined with a non-indexed conjunct.
        let r = rows(
            db.execute(
                "SELECT id FROM users WHERE city = ? AND id > 2",
                &[Value::Text("oslo".into())],
            )
            .unwrap(),
        );
        assert_eq!(r, vec![vec![Value::Int(3)], vec![Value::Int(5)]]);

        // Mutations keep the index consistent.
        db.execute("UPDATE users SET city = 'riga' WHERE id = 1", &[]).unwrap();
        db.execute("DELETE FROM users WHERE id = 5", &[]).unwrap();
        let r = rows(db.execute("SELECT id FROM users WHERE city = 'oslo'", &[]).unwrap());
        assert_eq!(r, vec![vec![Value::Int(3)]]);

        db.execute("DROP INDEX idx_city", &[]).unwrap();
        // Still correct after dropping the index (falls back to a scan).
        let r = rows(db.execute("SELECT id FROM users WHERE city = 'riga'", &[]).unwrap());
        assert_eq!(r, vec![vec![Value::Int(1)], vec![Value::Int(2)], vec![Value::Int(4)]]);
    }

    #[derive(Default)]
    struct WalBacking {
        snap: Option<Vec<u8>>,
        wal: Vec<u8>,
    }
    struct WalStorage(Arc<Mutex<WalBacking>>);
    impl Storage for WalStorage {
        fn load(&mut self) -> Option<Vec<u8>> {
            self.0.lock().unwrap().snap.clone()
        }
        fn save(&mut self, bytes: &[u8]) -> Result<(), String> {
            self.0.lock().unwrap().snap = Some(bytes.to_vec());
            Ok(())
        }
        fn supports_wal(&self) -> bool {
            true
        }
        fn append_wal(&mut self, record: &[u8]) -> Result<(), String> {
            self.0.lock().unwrap().wal.extend_from_slice(record);
            Ok(())
        }
        fn read_wal(&mut self) -> Vec<u8> {
            self.0.lock().unwrap().wal.clone()
        }
        fn truncate_wal(&mut self) -> Result<(), String> {
            self.0.lock().unwrap().wal.clear();
            Ok(())
        }
    }

    #[test]
    fn wal_incremental_persistence() {
        let backing = Arc::new(Mutex::new(WalBacking::default()));
        {
            let mut db = Database::open(Box::new(WalStorage(backing.clone()))).unwrap();
            db.execute("CREATE TABLE t (id INTEGER, v TEXT)", &[]).unwrap();
            db.execute("INSERT INTO t VALUES (1, 'a'), (2, 'b')", &[]).unwrap();
            db.execute(
                "UPDATE t SET v = ? WHERE id = ?",
                &[Value::Text("z".into()), Value::Int(2)],
            )
            .unwrap();
        }
        // A handful of mutations stay in the WAL (below the checkpoint threshold)
        // rather than each rewriting a full snapshot.
        assert!(!backing.lock().unwrap().wal.is_empty(), "mutations logged to WAL");

        // Reopen: snapshot + WAL replay reconstruct the data; the reopen
        // checkpoint folds the WAL into a snapshot and clears it.
        let mut db = Database::open(Box::new(WalStorage(backing.clone()))).unwrap();
        let r = rows(db.execute("SELECT v FROM t WHERE id = 2", &[]).unwrap());
        assert_eq!(r, vec![vec![Value::Text("z".into())]]);
        {
            let g = backing.lock().unwrap();
            assert!(g.wal.is_empty(), "reopen checkpoint truncates the WAL");
            assert!(g.snap.is_some(), "reopen wrote a folded snapshot");
        }

        // A mutation after the fold still persists and replays on the next open.
        db.execute("DELETE FROM t WHERE id = 1", &[]).unwrap();
        drop(db);
        let mut db = Database::open(Box::new(WalStorage(backing))).unwrap();
        assert_eq!(rows(db.execute("SELECT * FROM t", &[]).unwrap()).len(), 1);
    }

    #[test]
    fn index_survives_persistence() {
        let shared = Arc::new(Mutex::new(None));
        {
            let mut db = Database::open(Box::new(MemStorage(shared.clone()))).unwrap();
            db.execute("CREATE TABLE p (k TEXT, v INTEGER)", &[]).unwrap();
            db.execute("INSERT INTO p VALUES ('a',1),('b',2),('a',3)", &[]).unwrap();
            db.execute("CREATE INDEX idx_k ON p (k)", &[]).unwrap();
        }
        // Reopen: the index definition is reloaded and its map rebuilt.
        let mut db = Database::open(Box::new(MemStorage(shared))).unwrap();
        db.execute("INSERT INTO p VALUES ('a',4)", &[]).unwrap();
        let r = rows(db.execute("SELECT v FROM p WHERE k = 'a'", &[]).unwrap());
        assert_eq!(
            r,
            vec![vec![Value::Int(1)], vec![Value::Int(3)], vec![Value::Int(4)]]
        );
    }

    #[test]
    fn errors_are_clear() {
        let mut db = Database::in_memory();
        assert!(db.execute("SELECT * FROM nope", &[]).unwrap_err().contains("no such table"));
        db.execute("CREATE TABLE t (x INTEGER)", &[]).unwrap();
        assert!(db
            .execute("SELECT y FROM t WHERE y = 1", &[])
            .unwrap_err()
            .contains("no such column"));
        assert!(db.execute("INSERT INTO t VALUES (?)", &[]).unwrap_err().contains("parameter"));
    }

    #[test]
    fn null_and_ordering() {
        let mut db = Database::in_memory();
        db.execute("CREATE TABLE t (x INTEGER)", &[]).unwrap();
        db.execute("INSERT INTO t VALUES (NULL), (3), (1)", &[]).unwrap();
        let r = rows(db.execute("SELECT COUNT(x) FROM t", &[]).unwrap());
        assert_eq!(r[0][0], Value::Int(2)); // COUNT(col) skips NULLs
        let r = rows(db.execute("SELECT x FROM t WHERE x IS NOT NULL ORDER BY x", &[]).unwrap());
        assert_eq!(r, vec![vec![Value::Int(1)], vec![Value::Int(3)]]);
    }
}
