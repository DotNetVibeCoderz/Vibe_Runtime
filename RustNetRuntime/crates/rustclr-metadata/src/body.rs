//! IL method bodies (ECMA-335 II.25.4): tiny/fat headers and EH clauses.

use crate::error::{MetadataError, Result};
use crate::reader::Reader;
use crate::token::Token;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandlerKind {
    /// `catch` with a type filter given by a token.
    Catch(Token),
    /// `filter` block, with the IL offset where the filter code starts.
    Filter(u32),
    Finally,
    Fault,
}

#[derive(Debug, Clone, Copy)]
pub struct ExceptionClause {
    pub kind: HandlerKind,
    pub try_offset: u32,
    pub try_length: u32,
    pub handler_offset: u32,
    pub handler_length: u32,
}

impl ExceptionClause {
    pub const fn try_end(&self) -> u32 {
        self.try_offset + self.try_length
    }
    pub const fn handler_end(&self) -> u32 {
        self.handler_offset + self.handler_length
    }
    pub const fn try_contains(&self, offset: u32) -> bool {
        offset >= self.try_offset && offset < self.try_offset + self.try_length
    }
    pub const fn handler_contains(&self, offset: u32) -> bool {
        offset >= self.handler_offset && offset < self.handler_offset + self.handler_length
    }
}

/// A decoded method body: IL bytes plus the metadata the interpreter needs.
#[derive(Debug, Clone)]
pub struct MethodBody<'a> {
    pub il: &'a [u8],
    pub max_stack: u16,
    /// Token of the `StandAloneSig` row describing locals, or null.
    pub local_var_sig_token: Token,
    pub init_locals: bool,
    pub exception_clauses: Vec<ExceptionClause>,
}

const CORILMETHOD_TINYFORMAT: u8 = 0x02;
const CORILMETHOD_FATFORMAT: u8 = 0x03;
const CORILMETHOD_MORESECTS: u8 = 0x08;
const CORILMETHOD_INITLOCALS: u8 = 0x10;

const SECT_EHTABLE: u8 = 0x01;
const SECT_FATFORMAT: u8 = 0x40;
const SECT_MORESECTS: u8 = 0x80;

const COR_ILEXCEPTION_CLAUSE_EXCEPTION: u32 = 0x0000;
const COR_ILEXCEPTION_CLAUSE_FILTER: u32 = 0x0001;
const COR_ILEXCEPTION_CLAUSE_FINALLY: u32 = 0x0002;
const COR_ILEXCEPTION_CLAUSE_FAULT: u32 = 0x0004;

impl<'a> MethodBody<'a> {
    /// Parses a method body starting at the beginning of `data`.
    ///
    /// `data` should extend to at least the end of the body; extra trailing
    /// bytes are ignored.
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        let first = r.u8()?;

        if first & 0x03 == CORILMETHOD_TINYFORMAT {
            // Tiny header: the high 6 bits are the code size. No locals, no EH.
            let code_size = (first >> 2) as usize;
            let il = r.bytes(code_size)?;
            return Ok(MethodBody {
                il,
                max_stack: 8,
                local_var_sig_token: Token::NULL,
                init_locals: false,
                exception_clauses: Vec::new(),
            });
        }

        if first & 0x03 != CORILMETHOD_FATFORMAT {
            return Err(MetadataError::BadSignature("unrecognised method body header"));
        }

        // Fat header is 12 bytes: flags+size (2), maxstack (2), codesize (4),
        // localvarsigtok (4).
        let second = r.u8()?;
        let flags = (first as u16) | ((second as u16) << 8);
        let header_words = (second >> 4) as usize;
        if header_words < 3 {
            return Err(MetadataError::BadSignature("fat header too small"));
        }
        let max_stack = r.u16()?;
        let code_size = r.u32()? as usize;
        let local_var_sig_token = Token(r.u32()?);

        // Skip any extra header dwords a future format might add.
        r.skip((header_words - 3) * 4)?;

        let il = r.bytes(code_size)?;

        let mut exception_clauses = Vec::new();
        if first & CORILMETHOD_MORESECTS != 0 {
            // Data sections start on the next 4-byte boundary after the IL.
            r.align4_from(0)?;
            loop {
                let kind = r.u8()?;
                if kind & SECT_EHTABLE == 0 {
                    break;
                }
                let fat = kind & SECT_FATFORMAT != 0;
                let data_size = if fat {
                    let b = r.bytes(3)?;
                    u32::from_le_bytes([b[0], b[1], b[2], 0]) as usize
                } else {
                    let n = r.u8()? as usize;
                    r.skip(2)?; // padding
                    n
                };

                let clause_size = if fat { 24 } else { 12 };
                // data_size counts the 4-byte section header too.
                let count = data_size.saturating_sub(4) / clause_size;
                for _ in 0..count {
                    exception_clauses.push(Self::read_clause(&mut r, fat)?);
                }

                if kind & SECT_MORESECTS == 0 {
                    break;
                }
                r.align4_from(0)?;
            }
        }

        Ok(MethodBody {
            il,
            max_stack,
            local_var_sig_token,
            init_locals: flags & (CORILMETHOD_INITLOCALS as u16) != 0,
            exception_clauses,
        })
    }

    fn read_clause(r: &mut Reader<'a>, fat: bool) -> Result<ExceptionClause> {
        let (flags, try_offset, try_length, handler_offset, handler_length) = if fat {
            (r.u32()?, r.u32()?, r.u32()?, r.u32()?, r.u32()?)
        } else {
            let flags = r.u16()? as u32;
            let try_offset = r.u16()? as u32;
            let try_length = r.u8()? as u32;
            let handler_offset = r.u16()? as u32;
            let handler_length = r.u8()? as u32;
            (flags, try_offset, try_length, handler_offset, handler_length)
        };
        let extra = r.u32()?; // class token or filter offset

        let kind = match flags {
            COR_ILEXCEPTION_CLAUSE_EXCEPTION => HandlerKind::Catch(Token(extra)),
            COR_ILEXCEPTION_CLAUSE_FILTER => HandlerKind::Filter(extra),
            COR_ILEXCEPTION_CLAUSE_FINALLY => HandlerKind::Finally,
            COR_ILEXCEPTION_CLAUSE_FAULT => HandlerKind::Fault,
            _ => return Err(MetadataError::BadSignature("unknown EH clause flags")),
        };

        Ok(ExceptionClause {
            kind,
            try_offset,
            try_length,
            handler_offset,
            handler_length,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_tiny_body() {
        // Tiny header, code size 2: `ldc.i4.1; ret`
        let body = MethodBody::parse(&[(2 << 2) | 0x02, 0x17, 0x2A]).unwrap();
        assert_eq!(body.il, &[0x17, 0x2A]);
        assert_eq!(body.max_stack, 8);
        assert!(body.local_var_sig_token.is_null());
    }

    #[test]
    fn parses_a_fat_body_with_locals() {
        let mut raw = vec![
            0x13, 0x30, // flags = fat | initlocals, header size 3 words
            0x04, 0x00, // max stack 4
            0x02, 0x00, 0x00, 0x00, // code size 2
            0x01, 0x00, 0x00, 0x11, // local var sig token
        ];
        raw.extend_from_slice(&[0x16, 0x2A]);
        let body = MethodBody::parse(&raw).unwrap();
        assert_eq!(body.max_stack, 4);
        assert_eq!(body.il, &[0x16, 0x2A]);
        assert!(body.init_locals);
        assert_eq!(body.local_var_sig_token.raw(), 0x1100_0001);
    }

    #[test]
    fn rejects_a_truncated_body() {
        assert!(MethodBody::parse(&[(8 << 2) | 0x02, 0x00]).is_err());
    }
}
