//! The metadata root, its streams, and random access into the `#~` tables.

#[allow(unused_imports)]
use crate::prelude::*;

use crate::error::{MetadataError, Result};
use crate::heaps::{BlobHeap, GuidHeap, StringHeap, UserStringHeap};
use crate::reader::Reader;
use crate::schema::Column;
use crate::token::{CodedIndex, TableId, Token, TABLE_COUNT};

/// One entry of the metadata stream directory.
#[derive(Debug, Clone)]
pub struct StreamHeader {
    pub offset: u32,
    pub size: u32,
    pub name: String,
}

/// Per-table geometry computed once at load time.
#[derive(Debug, Clone, Copy, Default)]
pub struct TableInfo {
    pub row_count: u32,
    /// Byte width of one row.
    pub row_size: u32,
    /// Offset of row 1 within the `#~` stream data.
    pub base: u32,
}

/// A fully parsed metadata section, ready for random access.
#[derive(Debug, Clone)]
pub struct Metadata<'a> {
    pub version: String,
    pub strings: StringHeap<'a>,
    pub blobs: BlobHeap<'a>,
    pub guids: GuidHeap<'a>,
    pub user_strings: UserStringHeap<'a>,

    /// Raw bytes of the `#~` (or `#-`) table stream.
    table_data: &'a [u8],
    tables: [TableInfo; TABLE_COUNT],

    wide_string: bool,
    wide_guid: bool,
    wide_blob: bool,
    /// Set for the uncompressed `#-` stream, which permits pointer tables.
    pub uncompressed: bool,
    pub sorted_mask: u64,
}

impl<'a> Metadata<'a> {
    /// Parses the metadata root (`BSJB`) located at `data`.
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        let sig = r.u32()?;
        if sig != 0x424A_5342 {
            return Err(MetadataError::BadMagic { what: "BSJB", found: sig });
        }
        r.skip(8)?; // major, minor, reserved
        let version_len = r.u32()? as usize;
        let version_bytes = r.bytes(version_len)?;
        let version = core::str::from_utf8(version_bytes)
            .unwrap_or("")
            .trim_end_matches('\0')
            .to_string();
        r.skip(2)?; // flags
        let stream_count = r.u16()? as usize;

        let mut headers = Vec::with_capacity(stream_count);
        for _ in 0..stream_count {
            let offset = r.u32()?;
            let size = r.u32()?;
            let name_start = r.position();
            let name = core::str::from_utf8(r.cstr()?).unwrap_or("").to_string();
            // Stream names are padded to a 4-byte boundary from the name start.
            let consumed = r.position() - name_start;
            let pad = (4 - (consumed % 4)) % 4;
            r.skip(pad)?;
            headers.push(StreamHeader { offset, size, name });
        }

        let find = |want: &str| -> Option<&StreamHeader> {
            headers.iter().find(|h| h.name == want)
        };
        let slice = |h: &StreamHeader| -> &'a [u8] {
            let start = h.offset as usize;
            let end = (start + h.size as usize).min(data.len());
            data.get(start..end).unwrap_or(&[])
        };

        let strings = find("#Strings").map(|h| StringHeap(slice(h))).unwrap_or_default();
        let blobs = find("#Blob").map(|h| BlobHeap(slice(h))).unwrap_or_default();
        let guids = find("#GUID").map(|h| GuidHeap(slice(h))).unwrap_or_default();
        let user_strings = find("#US").map(|h| UserStringHeap(slice(h))).unwrap_or_default();

        let (table_stream, uncompressed) = match find("#~") {
            Some(h) => (slice(h), false),
            None => match find("#-") {
                Some(h) => (slice(h), true),
                None => return Err(MetadataError::MissingHeap("#~")),
            },
        };

        // --- `#~` header -----------------------------------------------------
        let mut t = Reader::new(table_stream);
        t.skip(6)?; // reserved, major, minor
        let heap_sizes = t.u8()?;
        t.skip(1)?; // reserved
        let valid = t.u64()?;
        let sorted_mask = t.u64()?;

        let wide_string = heap_sizes & 0x01 != 0;
        let wide_guid = heap_sizes & 0x02 != 0;
        let wide_blob = heap_sizes & 0x04 != 0;

        let mut tables = [TableInfo::default(); TABLE_COUNT];
        for i in 0..TABLE_COUNT {
            if valid & (1u64 << i) != 0 {
                tables[i].row_count = t.u32()?;
            }
        }

        let mut md = Metadata {
            version,
            strings,
            blobs,
            guids,
            user_strings,
            table_data: table_stream,
            tables,
            wide_string,
            wide_guid,
            wide_blob,
            uncompressed,
            sorted_mask,
        };

        // Row sizes depend on row counts, so they can only be computed now.
        let mut cursor = t.position() as u32;
        for i in 0..TABLE_COUNT {
            if md.tables[i].row_count == 0 {
                continue;
            }
            let Some(id) = TableId::from_raw(i as u8) else { continue };
            let size = md.compute_row_size(id);
            md.tables[i].row_size = size;
            md.tables[i].base = cursor;
            cursor += size * md.tables[i].row_count;
        }

        Ok(md)
    }

    fn heap_index_width(&self, c: Column) -> u32 {
        match c {
            Column::String => if self.wide_string { 4 } else { 2 },
            Column::Guid => if self.wide_guid { 4 } else { 2 },
            Column::Blob => if self.wide_blob { 4 } else { 2 },
            _ => unreachable!(),
        }
    }

    fn column_width(&self, c: Column) -> u32 {
        match c {
            Column::U8 => 1,
            Column::U16 => 2,
            Column::U32 => 4,
            Column::String | Column::Guid | Column::Blob => self.heap_index_width(c),
            Column::Table(t) => {
                if self.row_count(t) < (1 << 16) { 2 } else { 4 }
            }
            Column::Coded(ci) => {
                let bits = ci.tag_bits();
                let max_rows = ci
                    .tables()
                    .iter()
                    .filter_map(|t| *t)
                    .map(|t| self.row_count(t))
                    .max()
                    .unwrap_or(0);
                // A 2-byte coded index must fit both the tag and the row index.
                if (max_rows as u64) < (1u64 << (16 - bits)) { 2 } else { 4 }
            }
        }
    }

    fn compute_row_size(&self, id: TableId) -> u32 {
        id.columns().iter().map(|c| self.column_width(*c)).sum()
    }

    #[inline]
    pub fn row_count(&self, id: TableId) -> u32 {
        self.tables[id as usize].row_count
    }

    #[inline]
    pub fn info(&self, id: TableId) -> TableInfo {
        self.tables[id as usize]
    }

    #[inline]
    pub fn is_sorted(&self, id: TableId) -> bool {
        self.sorted_mask & (1u64 << (id as u32)) != 0
    }

    /// Returns a cursor over one table row. `index` is 1-based.
    pub fn row(&self, id: TableId, index: u32) -> Result<RowCursor<'_, 'a>> {
        let info = self.tables[id as usize];
        if index == 0 || index > info.row_count {
            return Err(MetadataError::RowOutOfRange { table: id.name(), index });
        }
        let offset = (info.base + (index - 1) * info.row_size) as usize;
        Ok(RowCursor {
            md: self,
            reader: Reader::at(self.table_data, offset),
            table: id,
            column: 0,
        })
    }

    /// Iterates every row of a table as a cursor.
    pub fn rows(&self, id: TableId) -> impl Iterator<Item = Result<RowCursor<'_, 'a>>> + '_ {
        (1..=self.row_count(id)).map(move |i| self.row(id, i))
    }

    /// Decodes a coded index value into a token.
    pub fn decode_coded(&self, kind: CodedIndex, value: u32) -> Result<Token> {
        let bits = kind.tag_bits();
        let tag = value & ((1 << bits) - 1);
        let row = value >> bits;
        match kind.tables().get(tag as usize).copied().flatten() {
            Some(table) => Ok(Token::new(table, row)),
            None => Err(MetadataError::BadCodedIndex { kind: kind.kind_name(), tag }),
        }
    }

    pub fn string(&self, offset: u32) -> Result<&'a str> {
        self.strings.get(offset)
    }

    pub fn blob(&self, offset: u32) -> Result<&'a [u8]> {
        self.blobs.get(offset)
    }

    pub fn user_string(&self, offset: u32) -> Result<String> {
        self.user_strings.get(offset)
    }
}

/// A cursor positioned at the start of a row, reading columns in order.
///
/// Column widths vary per image, so callers must read columns sequentially
/// rather than at fixed offsets. The cursor tracks which column comes next and
/// consults the schema for its width.
#[derive(Debug, Clone)]
pub struct RowCursor<'m, 'a> {
    md: &'m Metadata<'a>,
    reader: Reader<'a>,
    table: TableId,
    column: usize,
}

impl<'m, 'a> RowCursor<'m, 'a> {
    fn next_column(&mut self) -> Result<Column> {
        let cols = self.table.columns();
        let c = cols
            .get(self.column)
            .copied()
            .ok_or(MetadataError::BadSignature("read past end of row"))?;
        self.column += 1;
        Ok(c)
    }

    /// Reads the next column as a raw integer, whatever its declared width.
    pub fn raw(&mut self) -> Result<u32> {
        let c = self.next_column()?;
        match self.md.column_width(c) {
            1 => Ok(self.reader.u8()? as u32),
            2 => Ok(self.reader.u16()? as u32),
            _ => self.reader.u32(),
        }
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.raw()? as u8)
    }

    pub fn u16(&mut self) -> Result<u16> {
        Ok(self.raw()? as u16)
    }

    pub fn u32(&mut self) -> Result<u32> {
        self.raw()
    }

    /// Reads a `#Strings` index and resolves it.
    pub fn string(&mut self) -> Result<&'a str> {
        let off = self.raw()?;
        self.md.string(off)
    }

    /// Reads a `#Blob` index and resolves it.
    pub fn blob(&mut self) -> Result<&'a [u8]> {
        let off = self.raw()?;
        self.md.blob(off)
    }

    /// Reads a `#GUID` index and resolves it.
    pub fn guid(&mut self) -> Result<[u8; 16]> {
        let idx = self.raw()?;
        self.md.guids.get(idx)
    }

    /// Reads a simple table index as a token into `target`.
    pub fn table_index(&mut self, target: TableId) -> Result<Token> {
        let row = self.raw()?;
        Ok(Token::new(target, row))
    }

    /// Reads and decodes a coded index.
    pub fn coded(&mut self, kind: CodedIndex) -> Result<Token> {
        let v = self.raw()?;
        self.md.decode_coded(kind, v)
    }

    /// Skips `n` columns.
    pub fn skip(&mut self, n: usize) -> Result<()> {
        for _ in 0..n {
            self.raw()?;
        }
        Ok(())
    }
}
