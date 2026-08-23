//! Portable PDB: source lines and local-variable names.
//!
//! A portable PDB is not a separate format so much as a separate *file* in the
//! format this crate already reads. It begins with the same `BSJB` signature,
//! has the same streams and heaps, and its tables are declared in
//! [`crate::schema`] beside every other table. Everything here is therefore
//! about the two things that genuinely differ: the **document name blob**, and
//! the **sequence points blob**, both of which are compressed encodings with
//! no counterpart on the assembly side.
//!
//! # What it is for
//!
//! Two questions a debugger cannot answer without it:
//!
//! * *Which source line is this IL offset?* — and its inverse, which is what a
//!   breakpoint set in an editor actually needs.
//! * *What is local 3 called?* — the assembly stores locals by index; only the
//!   PDB names them.
//!
//! # What it deliberately does not do
//!
//! Nothing here guesses. A method with no debug information has no lines, and
//! is reported as having none rather than being given the nearest ones; a
//! hidden sequence point is kept as hidden rather than folded into its
//! neighbour, because "this IL belongs to no source line" is a real answer and
//! a debugger that stepped onto it should say so.

use crate::heaps::BlobHeap;
use crate::prelude::*;
use crate::reader::Reader;
use crate::tables::Metadata;
use crate::token::TableId;
use crate::MetadataError;

type Result<T> = core::result::Result<T, MetadataError>;

/// A source file named by a PDB.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    /// Reassembled path, with the separator the PDB chose.
    pub name: String,
}

/// One sequence point: a span of IL that corresponds to a span of source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SequencePoint {
    /// Row index into [`Pdb::documents`], 1-based as metadata rows are.
    pub document: u32,
    pub il_offset: u32,
    /// `None` for a *hidden* sequence point: IL the compiler generated that
    /// belongs to no source line. Kept distinct rather than dropped, because a
    /// debugger stepping onto one should say "no source here" rather than
    /// silently show the previous line.
    pub start_line: Option<u32>,
    pub start_column: u32,
    pub end_line: u32,
    pub end_column: u32,
}

/// A local variable's name and slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalName {
    /// Slot index in the method's locals signature.
    pub index: u16,
    pub name: String,
}

/// Everything one method's debug information says.
#[derive(Debug, Clone, Default)]
pub struct MethodDebug {
    pub sequence_points: Vec<SequencePoint>,
    pub locals: Vec<LocalName>,
}

impl MethodDebug {
    /// The source line for an IL offset, or `None` if there is none.
    ///
    /// The sequence point *covering* the offset: the last one at or before it.
    /// A hidden point covers its range too and answers `None`, which is the
    /// honest answer rather than the previous visible line.
    pub fn line_for(&self, il_offset: u32) -> Option<(u32, u32)> {
        let point = self
            .sequence_points
            .iter()
            .take_while(|p| p.il_offset <= il_offset)
            .last()?;
        point.start_line.map(|line| (point.document, line))
    }

    /// The first IL offset belonging to `line`, which is where a breakpoint
    /// set on that line goes.
    ///
    /// `None` when the line has no code — a blank line, a comment, or a
    /// declaration the compiler emitted nothing for. An editor asking for one
    /// should be told so rather than given the nearest line that does.
    pub fn offset_for_line(&self, document: u32, line: u32) -> Option<u32> {
        self.sequence_points
            .iter()
            .filter(|p| p.document == document && p.start_line == Some(line))
            .map(|p| p.il_offset)
            .min()
    }

    /// The name of local `index`, if the PDB records one.
    pub fn local_name(&self, index: u16) -> Option<&str> {
        self.locals.iter().find(|l| l.index == index).map(|l| l.name.as_str())
    }
}

/// A parsed portable PDB.
pub struct Pdb {
    /// Documents, in row order. Index with a [`SequencePoint::document`] minus
    /// one, or use [`Pdb::document`].
    pub documents: Vec<Document>,
    /// Debug information per MethodDef row, indexed by row number minus one.
    /// A method with no debug information has an empty entry.
    methods: Vec<MethodDebug>,
}

impl Pdb {
    /// Parses a portable PDB from its bytes.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let md = Metadata::parse(bytes)?;

        let mut documents = Vec::new();
        for row in 1..=md.row_count(TableId::Document) {
            let mut cursor = md.row(TableId::Document, row)?;
            let name_blob = cursor.raw()?;
            documents.push(Document {
                name: document_name(&md.blobs, name_blob)?,
            });
        }

        let method_count = md.row_count(TableId::MethodDebugInformation) as usize;
        let mut methods = vec![MethodDebug::default(); method_count];
        for row in 1..=md.row_count(TableId::MethodDebugInformation) {
            let mut cursor = md.row(TableId::MethodDebugInformation, row)?;
            let document = cursor.raw()?;
            let points_blob = cursor.raw()?;
            if points_blob == 0 {
                continue;
            }
            let blob = md.blobs.get(points_blob)?;
            methods[row as usize - 1].sequence_points = sequence_points(blob, document)?;
        }

        // Local names hang off `LocalScope`, which spans a range of a method's
        // IL and names a run of `LocalVariable` rows. The runtime only needs
        // "what is slot N called", so the scopes are flattened per method —
        // shadowing across scopes is rare enough that the first name wins, and
        // saying which scope a name came from would be a bigger interface for
        // no gain here.
        let scope_count = md.row_count(TableId::LocalScope);
        for row in 1..=scope_count {
            let mut cursor = md.row(TableId::LocalScope, row)?;
            let method = cursor.raw()?;
            let _import_scope = cursor.raw()?;
            let first_variable = cursor.raw()?;
            let _first_constant = cursor.raw()?;

            // A table's list column runs to the next row's value, or to the
            // end of the target table for the last row — the same rule
            // `TypeDef`'s field and method lists follow.
            let next_variable = if row < scope_count {
                let mut next = md.row(TableId::LocalScope, row + 1)?;
                next.raw()?;
                next.raw()?;
                next.raw()?
            } else {
                md.row_count(TableId::LocalVariable) + 1
            };

            if method == 0 || method as usize > methods.len() {
                continue;
            }
            for variable in first_variable..next_variable {
                if variable == 0 || variable > md.row_count(TableId::LocalVariable) {
                    break;
                }
                let mut v = md.row(TableId::LocalVariable, variable)?;
                let _attributes = v.u16()?;
                let index = v.u16()?;
                let name = v.raw()?;
                let name = md.strings.get(name)?;
                let entry = &mut methods[method as usize - 1];
                if !entry.locals.iter().any(|l| l.index == index) {
                    entry.locals.push(LocalName { index, name: String::from(name) });
                }
            }
        }

        Ok(Self { documents, methods })
    }

    /// Debug information for a MethodDef row, if the PDB has any.
    ///
    /// The `MethodDebugInformation` table is parallel to `MethodDef` — row *n*
    /// describes method *n* — which is what makes this a lookup rather than a
    /// search.
    pub fn method(&self, method_def_row: u32) -> Option<&MethodDebug> {
        let entry = self.methods.get(method_def_row.checked_sub(1)? as usize)?;
        if entry.sequence_points.is_empty() && entry.locals.is_empty() {
            return None;
        }
        Some(entry)
    }

    /// A document by the 1-based row a sequence point names.
    pub fn document(&self, row: u32) -> Option<&Document> {
        self.documents.get(row.checked_sub(1)? as usize)
    }

    /// The document row whose name ends with `suffix`, for resolving the path
    /// an editor sends against the absolute path the compiler recorded.
    ///
    /// Suffix rather than equality because an editor's path and the build
    /// machine's rarely match, and a case-insensitive tail comparison is what
    /// every other debugger does. Ambiguity returns `None`: two documents
    /// ending the same way is a question this cannot answer, and picking one
    /// would put a breakpoint in a file the user did not name.
    pub fn document_by_suffix(&self, suffix: &str) -> Option<u32> {
        let wanted = normalise(suffix);
        let mut found = None;
        for (index, document) in self.documents.iter().enumerate() {
            if normalise(&document.name).ends_with(&wanted) {
                if found.is_some() {
                    return None;
                }
                found = Some(index as u32 + 1);
            }
        }
        found
    }
}

fn normalise(path: &str) -> String {
    path.replace('\\', "/").to_lowercase()
}

/// Reassembles a document name blob.
///
/// The encoding is one separator byte, then a sequence of compressed blob-heap
/// indexes naming the parts. A zero index is an empty part, which is how a
/// leading separator — an absolute Unix path — is expressed.
fn document_name(blobs: &BlobHeap<'_>, offset: u32) -> Result<String> {
    let blob = blobs.get(offset)?;
    let mut reader = Reader::new(blob);
    let separator = reader.u8()? as char;
    let mut parts: Vec<String> = Vec::new();
    while !reader.is_empty() {
        let part = reader.compressed_u32()?;
        if part == 0 {
            parts.push(String::new());
        } else {
            parts.push(String::from(core::str::from_utf8(blobs.get(part)?).map_err(
                |_| MetadataError::BadSignature("document name part is not UTF-8"),
            )?));
        }
    }
    let mut name = String::new();
    for (index, part) in parts.iter().enumerate() {
        if index > 0 {
            name.push(separator);
        }
        name.push_str(part);
    }
    Ok(name)
}

/// Decodes the sequence points blob.
///
/// Every field after the first is a *delta*, which is what makes this a decoder
/// rather than a struct read: line and column are relative to the previous
/// point, and the IL offset is relative to the previous offset. A zero IL delta
/// on a non-first record is not a zero-length point — it introduces a new
/// document, which is how one method spans several files.
fn sequence_points(blob: &[u8], initial_document: u32) -> Result<Vec<SequencePoint>> {
    let mut reader = Reader::new(blob);
    let _local_signature = reader.compressed_u32()?;

    let mut document = initial_document;
    if document == 0 {
        document = reader.compressed_u32()?;
    }

    let mut points = Vec::new();
    let mut il_offset = 0u32;
    let mut line = 0i64;
    let mut column = 0i64;
    // Two separate firsts, and conflating them is a real bug rather than a
    // tidiness point. The IL offset is absolute in the first *record*; the line
    // and column are absolute in the first *non-hidden* one. A method whose
    // first record is hidden — which the C# compiler emits routinely — would
    // otherwise have its first real line read as a delta from zero, and the
    // whole method's lines drift from there.
    let mut first_record = true;
    let mut first_visible = true;

    while !reader.is_empty() {
        let delta = reader.compressed_u32()?;
        if !first_record && delta == 0 {
            // A document record, not a point.
            document = reader.compressed_u32()?;
            continue;
        }
        il_offset = if first_record { delta } else { il_offset + delta };
        first_record = false;

        let delta_lines = reader.compressed_u32()?;
        let delta_columns = if delta_lines == 0 {
            reader.compressed_u32()? as i64
        } else {
            reader.compressed_i32()? as i64
        };

        if delta_lines == 0 && delta_columns == 0 {
            // Hidden: IL the compiler generated for no source construct.
            points.push(SequencePoint {
                document,
                il_offset,
                start_line: None,
                start_column: 0,
                end_line: 0,
                end_column: 0,
            });
            continue;
        }

        if first_visible {
            line = reader.compressed_u32()? as i64;
            column = reader.compressed_u32()? as i64;
        } else {
            line += reader.compressed_i32()? as i64;
            column += reader.compressed_i32()? as i64;
        }
        if line < 0 || column < 0 {
            return Err(MetadataError::BadSignature("negative sequence point"));
        }

        points.push(SequencePoint {
            document,
            il_offset,
            start_line: Some(line as u32),
            start_column: column as u32,
            end_line: line as u32 + delta_lines,
            end_column: (column + delta_columns).max(0) as u32,
        });
        first_visible = false;
    }
    Ok(points)
}
