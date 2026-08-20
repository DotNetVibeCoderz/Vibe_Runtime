//! Metadata signature blobs (ECMA-335 II.23.2).

#[allow(unused_imports)]
use crate::prelude::*;

use crate::error::{MetadataError, Result};
use crate::reader::Reader;
use crate::token::{CodedIndex, TableId, Token};

/// ELEMENT_TYPE_* constants.
pub mod element_type {
    pub const END: u8 = 0x00;
    pub const VOID: u8 = 0x01;
    pub const BOOLEAN: u8 = 0x02;
    pub const CHAR: u8 = 0x03;
    pub const I1: u8 = 0x04;
    pub const U1: u8 = 0x05;
    pub const I2: u8 = 0x06;
    pub const U2: u8 = 0x07;
    pub const I4: u8 = 0x08;
    pub const U4: u8 = 0x09;
    pub const I8: u8 = 0x0A;
    pub const U8: u8 = 0x0B;
    pub const R4: u8 = 0x0C;
    pub const R8: u8 = 0x0D;
    pub const STRING: u8 = 0x0E;
    pub const PTR: u8 = 0x0F;
    pub const BYREF: u8 = 0x10;
    pub const VALUETYPE: u8 = 0x11;
    pub const CLASS: u8 = 0x12;
    pub const VAR: u8 = 0x13;
    pub const ARRAY: u8 = 0x14;
    pub const GENERICINST: u8 = 0x15;
    pub const TYPEDBYREF: u8 = 0x16;
    pub const I: u8 = 0x18;
    pub const U: u8 = 0x19;
    pub const FNPTR: u8 = 0x1B;
    pub const OBJECT: u8 = 0x1C;
    pub const SZARRAY: u8 = 0x1D;
    pub const MVAR: u8 = 0x1E;
    pub const CMOD_REQD: u8 = 0x1F;
    pub const CMOD_OPT: u8 = 0x20;
    pub const INTERNAL: u8 = 0x21;
    pub const SENTINEL: u8 = 0x41;
    pub const PINNED: u8 = 0x45;
}

/// Calling-convention bits in the first byte of a method signature.
pub mod calling_convention {
    pub const DEFAULT: u8 = 0x00;
    pub const C: u8 = 0x01;
    pub const STDCALL: u8 = 0x02;
    pub const THISCALL: u8 = 0x03;
    pub const FASTCALL: u8 = 0x04;
    pub const VARARG: u8 = 0x05;
    pub const FIELD: u8 = 0x06;
    pub const LOCAL_SIG: u8 = 0x07;
    pub const PROPERTY: u8 = 0x08;
    pub const GENERIC_INST: u8 = 0x0A;
    pub const MASK: u8 = 0x0F;

    pub const HAS_THIS: u8 = 0x20;
    pub const EXPLICIT_THIS: u8 = 0x40;
    pub const GENERIC: u8 = 0x10;
}

/// A decoded type reference from a signature blob.
#[derive(Debug, Clone, PartialEq)]
pub enum TypeSig {
    Void,
    Boolean,
    Char,
    I1,
    U1,
    I2,
    U2,
    I4,
    U4,
    I8,
    U8,
    R4,
    R8,
    String,
    /// `native int` / `native uint`.
    IntPtr,
    UIntPtr,
    Object,
    TypedByRef,
    /// A value type, by token.
    ValueType(Token),
    /// A reference type, by token.
    Class(Token),
    /// Unmanaged pointer.
    Ptr(Box<TypeSig>),
    /// Managed reference (`ref`/`out`).
    ByRef(Box<TypeSig>),
    /// Single-dimension zero-based array.
    SzArray(Box<TypeSig>),
    /// Multi-dimensional array with rank, sizes and lower bounds.
    Array {
        element: Box<TypeSig>,
        rank: u32,
        sizes: Vec<u32>,
        lo_bounds: Vec<i32>,
    },
    /// Generic instantiation, e.g. `List<int>`.
    GenericInst {
        definition: Token,
        is_value_type: bool,
        args: Vec<TypeSig>,
    },
    /// Generic parameter of the enclosing type (`!0`).
    Var(u32),
    /// Generic parameter of the enclosing method (`!!0`).
    MVar(u32),
    /// Function pointer.
    FnPtr(Box<MethodSig>),
    /// A modifier we preserve but do not interpret.
    Modified {
        required: bool,
        modifier: Token,
        inner: Box<TypeSig>,
    },
    Pinned(Box<TypeSig>),
}

impl TypeSig {
    /// True for types whose values live directly in a stack slot.
    pub fn is_primitive(&self) -> bool {
        matches!(
            self,
            Self::Boolean
                | Self::Char
                | Self::I1
                | Self::U1
                | Self::I2
                | Self::U2
                | Self::I4
                | Self::U4
                | Self::I8
                | Self::U8
                | Self::R4
                | Self::R8
                | Self::IntPtr
                | Self::UIntPtr
        )
    }

    /// True for types tracked by the GC as object references.
    pub fn is_gc_reference(&self) -> bool {
        matches!(
            self,
            Self::String | Self::Object | Self::Class(_) | Self::SzArray(_) | Self::Array { .. }
        )
    }

    /// Strips modifiers and pinning to reach the underlying type.
    pub fn unwrap_modifiers(&self) -> &TypeSig {
        match self {
            Self::Modified { inner, .. } | Self::Pinned(inner) => inner.unwrap_modifiers(),
            other => other,
        }
    }
}

/// A decoded method signature.
#[derive(Debug, Clone, PartialEq)]
pub struct MethodSig {
    pub calling_convention: u8,
    pub has_this: bool,
    pub explicit_this: bool,
    pub generic_param_count: u32,
    pub return_type: TypeSig,
    pub params: Vec<TypeSig>,
    /// Index into `params` where varargs begin, if a sentinel was present.
    pub sentinel_at: Option<usize>,
}

impl MethodSig {
    /// Number of stack slots the callee pops, including `this`.
    pub fn arg_count(&self) -> usize {
        self.params.len() + usize::from(self.has_this)
    }
}

/// A decoded local-variable signature.
#[derive(Debug, Clone, PartialEq)]
pub struct LocalVarSig {
    pub locals: Vec<TypeSig>,
}

/// Parses signature blobs.
pub struct SignatureParser<'a> {
    reader: Reader<'a>,
}

impl<'a> SignatureParser<'a> {
    pub fn new(blob: &'a [u8]) -> Self {
        Self { reader: Reader::new(blob) }
    }

    /// Parses a `Field` signature: `FIELD Type`.
    pub fn parse_field(&mut self) -> Result<TypeSig> {
        let cc = self.reader.u8()?;
        if cc & calling_convention::MASK != calling_convention::FIELD {
            return Err(MetadataError::BadSignature("expected FIELD calling convention"));
        }
        self.parse_type()
    }

    /// Parses a `Property` signature, returning the property type.
    pub fn parse_property(&mut self) -> Result<(TypeSig, Vec<TypeSig>)> {
        let cc = self.reader.u8()?;
        if cc & calling_convention::MASK != calling_convention::PROPERTY {
            return Err(MetadataError::BadSignature("expected PROPERTY calling convention"));
        }
        let count = self.reader.compressed_u32()? as usize;
        let ty = self.parse_type()?;
        let mut params = Vec::with_capacity(count);
        for _ in 0..count {
            params.push(self.parse_type()?);
        }
        Ok((ty, params))
    }

    /// Parses a `MethodDef` / `MethodRef` signature.
    pub fn parse_method(&mut self) -> Result<MethodSig> {
        let cc = self.reader.u8()?;
        let kind = cc & calling_convention::MASK;
        let has_this = cc & calling_convention::HAS_THIS != 0;
        let explicit_this = cc & calling_convention::EXPLICIT_THIS != 0;
        let generic_param_count = if cc & calling_convention::GENERIC != 0 {
            self.reader.compressed_u32()?
        } else {
            0
        };
        let param_count = self.reader.compressed_u32()? as usize;
        let return_type = self.parse_return_type()?;

        let mut params = Vec::with_capacity(param_count);
        let mut sentinel_at = None;
        for _ in 0..param_count {
            if self.peek()? == element_type::SENTINEL {
                self.reader.u8()?;
                sentinel_at = Some(params.len());
            }
            params.push(self.parse_type()?);
        }

        Ok(MethodSig {
            calling_convention: kind,
            has_this,
            explicit_this,
            generic_param_count,
            return_type,
            params,
            sentinel_at,
        })
    }

    /// Parses a `LocalVarSig` (the signature named by a method's LocalVarSigTok).
    pub fn parse_locals(&mut self) -> Result<LocalVarSig> {
        let cc = self.reader.u8()?;
        if cc & calling_convention::MASK != calling_convention::LOCAL_SIG {
            return Err(MetadataError::BadSignature("expected LOCAL_SIG calling convention"));
        }
        let count = self.reader.compressed_u32()? as usize;
        let mut locals = Vec::with_capacity(count);
        for _ in 0..count {
            locals.push(self.parse_type()?);
        }
        Ok(LocalVarSig { locals })
    }

    /// Parses a `MethodSpec` signature: `GENERICINST GenArgCount Type+`.
    pub fn parse_method_spec(&mut self) -> Result<Vec<TypeSig>> {
        let cc = self.reader.u8()?;
        if cc != calling_convention::GENERIC_INST {
            return Err(MetadataError::BadSignature("expected GENERICINST"));
        }
        let count = self.reader.compressed_u32()? as usize;
        let mut args = Vec::with_capacity(count);
        for _ in 0..count {
            args.push(self.parse_type()?);
        }
        Ok(args)
    }

    /// Parses a bare `TypeSpec` blob, which is a single Type.
    pub fn parse_type_spec(&mut self) -> Result<TypeSig> {
        self.parse_type()
    }

    fn peek(&self) -> Result<u8> {
        let mut clone = self.reader.clone();
        clone.u8()
    }

    fn parse_return_type(&mut self) -> Result<TypeSig> {
        // A return type may carry modifiers and BYREF before VOID.
        self.parse_type()
    }

    fn parse_type(&mut self) -> Result<TypeSig> {
        use element_type as et;
        let b = self.reader.u8()?;
        Ok(match b {
            et::VOID => TypeSig::Void,
            et::BOOLEAN => TypeSig::Boolean,
            et::CHAR => TypeSig::Char,
            et::I1 => TypeSig::I1,
            et::U1 => TypeSig::U1,
            et::I2 => TypeSig::I2,
            et::U2 => TypeSig::U2,
            et::I4 => TypeSig::I4,
            et::U4 => TypeSig::U4,
            et::I8 => TypeSig::I8,
            et::U8 => TypeSig::U8,
            et::R4 => TypeSig::R4,
            et::R8 => TypeSig::R8,
            et::STRING => TypeSig::String,
            et::I => TypeSig::IntPtr,
            et::U => TypeSig::UIntPtr,
            et::OBJECT => TypeSig::Object,
            et::TYPEDBYREF => TypeSig::TypedByRef,

            et::VALUETYPE => TypeSig::ValueType(self.type_def_or_ref()?),
            et::CLASS => TypeSig::Class(self.type_def_or_ref()?),

            et::PTR => TypeSig::Ptr(Box::new(self.parse_type()?)),
            et::BYREF => TypeSig::ByRef(Box::new(self.parse_type()?)),
            et::SZARRAY => TypeSig::SzArray(Box::new(self.parse_type()?)),
            et::PINNED => TypeSig::Pinned(Box::new(self.parse_type()?)),

            et::VAR => TypeSig::Var(self.reader.compressed_u32()?),
            et::MVAR => TypeSig::MVar(self.reader.compressed_u32()?),

            et::ARRAY => {
                let element = Box::new(self.parse_type()?);
                let rank = self.reader.compressed_u32()?;
                let num_sizes = self.reader.compressed_u32()? as usize;
                let mut sizes = Vec::with_capacity(num_sizes);
                for _ in 0..num_sizes {
                    sizes.push(self.reader.compressed_u32()?);
                }
                let num_lo = self.reader.compressed_u32()? as usize;
                let mut lo_bounds = Vec::with_capacity(num_lo);
                for _ in 0..num_lo {
                    lo_bounds.push(self.reader.compressed_i32()?);
                }
                TypeSig::Array { element, rank, sizes, lo_bounds }
            }

            et::GENERICINST => {
                let kind = self.reader.u8()?;
                let is_value_type = kind == et::VALUETYPE;
                let definition = self.type_def_or_ref()?;
                let count = self.reader.compressed_u32()? as usize;
                let mut args = Vec::with_capacity(count);
                for _ in 0..count {
                    args.push(self.parse_type()?);
                }
                TypeSig::GenericInst { definition, is_value_type, args }
            }

            et::FNPTR => TypeSig::FnPtr(Box::new(self.parse_method()?)),

            et::CMOD_REQD | et::CMOD_OPT => {
                let modifier = self.type_def_or_ref()?;
                let inner = Box::new(self.parse_type()?);
                TypeSig::Modified { required: b == et::CMOD_REQD, modifier, inner }
            }

            other => {
                let _ = other;
                return Err(MetadataError::BadSignature("unrecognised ELEMENT_TYPE"));
            }
        })
    }

    /// Reads a compressed `TypeDefOrRef` coded index.
    fn type_def_or_ref(&mut self) -> Result<Token> {
        let v = self.reader.compressed_u32()?;
        let tag = v & 0x03;
        let row = v >> 2;
        let table = match tag {
            0 => TableId::TypeDef,
            1 => TableId::TypeRef,
            2 => TableId::TypeSpec,
            _ => {
                return Err(MetadataError::BadCodedIndex {
                    kind: CodedIndex::TypeDefOrRef.kind_name(),
                    tag,
                })
            }
        };
        Ok(Token::new(table, row))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_static_void_method_with_no_params() {
        // DEFAULT, 0 params, void
        let mut p = SignatureParser::new(&[0x00, 0x00, 0x01]);
        let sig = p.parse_method().unwrap();
        assert!(!sig.has_this);
        assert_eq!(sig.return_type, TypeSig::Void);
        assert!(sig.params.is_empty());
    }

    #[test]
    fn parses_instance_method_returning_int_taking_string() {
        // HAS_THIS, 1 param, I4 return, STRING param
        let mut p = SignatureParser::new(&[0x20, 0x01, 0x08, 0x0E]);
        let sig = p.parse_method().unwrap();
        assert!(sig.has_this);
        assert_eq!(sig.return_type, TypeSig::I4);
        assert_eq!(sig.params, vec![TypeSig::String]);
        assert_eq!(sig.arg_count(), 2);
    }

    #[test]
    fn parses_szarray_of_string() {
        // FIELD, SZARRAY STRING
        let mut p = SignatureParser::new(&[0x06, 0x1D, 0x0E]);
        assert_eq!(p.parse_field().unwrap(), TypeSig::SzArray(Box::new(TypeSig::String)));
    }

    #[test]
    fn parses_generic_instantiation() {
        // FIELD, GENERICINST CLASS <TypeRef 1> 1 I4
        let mut p = SignatureParser::new(&[0x06, 0x15, 0x12, 0x05, 0x01, 0x08]);
        let ty = p.parse_field().unwrap();
        match ty {
            TypeSig::GenericInst { definition, is_value_type, args } => {
                assert!(!is_value_type);
                assert_eq!(definition, Token::new(TableId::TypeRef, 1));
                assert_eq!(args, vec![TypeSig::I4]);
            }
            other => panic!("expected GenericInst, got {other:?}"),
        }
    }

    #[test]
    fn rejects_a_truncated_signature_without_panicking() {
        let mut p = SignatureParser::new(&[0x00, 0x02, 0x01, 0x0E]);
        assert!(p.parse_method().is_err());
    }
}
