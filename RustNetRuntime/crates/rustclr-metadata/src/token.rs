//! Metadata tokens and the table identifiers they encode.

#[allow(unused_imports)]
use crate::prelude::*;

use core::fmt;

/// The 45 metadata tables of ECMA-335 II.22, identified by their table number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum TableId {
    Module = 0x00,
    TypeRef = 0x01,
    TypeDef = 0x02,
    FieldPtr = 0x03,
    Field = 0x04,
    MethodPtr = 0x05,
    MethodDef = 0x06,
    ParamPtr = 0x07,
    Param = 0x08,
    InterfaceImpl = 0x09,
    MemberRef = 0x0A,
    Constant = 0x0B,
    CustomAttribute = 0x0C,
    FieldMarshal = 0x0D,
    DeclSecurity = 0x0E,
    ClassLayout = 0x0F,
    FieldLayout = 0x10,
    StandAloneSig = 0x11,
    EventMap = 0x12,
    EventPtr = 0x13,
    Event = 0x14,
    PropertyMap = 0x15,
    PropertyPtr = 0x16,
    Property = 0x17,
    MethodSemantics = 0x18,
    MethodImpl = 0x19,
    ModuleRef = 0x1A,
    TypeSpec = 0x1B,
    ImplMap = 0x1C,
    FieldRva = 0x1D,
    EncLog = 0x1E,
    EncMap = 0x1F,
    Assembly = 0x20,
    AssemblyProcessor = 0x21,
    AssemblyOs = 0x22,
    AssemblyRef = 0x23,
    AssemblyRefProcessor = 0x24,
    AssemblyRefOs = 0x25,
    File = 0x26,
    ExportedType = 0x27,
    ManifestResource = 0x28,
    NestedClass = 0x29,
    GenericParam = 0x2A,
    MethodSpec = 0x2B,
    GenericParamConstraint = 0x2C,
}

pub const TABLE_COUNT: usize = 64;

impl TableId {
    pub fn from_raw(v: u8) -> Option<Self> {
        use TableId::*;
        Some(match v {
            0x00 => Module,
            0x01 => TypeRef,
            0x02 => TypeDef,
            0x03 => FieldPtr,
            0x04 => Field,
            0x05 => MethodPtr,
            0x06 => MethodDef,
            0x07 => ParamPtr,
            0x08 => Param,
            0x09 => InterfaceImpl,
            0x0A => MemberRef,
            0x0B => Constant,
            0x0C => CustomAttribute,
            0x0D => FieldMarshal,
            0x0E => DeclSecurity,
            0x0F => ClassLayout,
            0x10 => FieldLayout,
            0x11 => StandAloneSig,
            0x12 => EventMap,
            0x13 => EventPtr,
            0x14 => Event,
            0x15 => PropertyMap,
            0x16 => PropertyPtr,
            0x17 => Property,
            0x18 => MethodSemantics,
            0x19 => MethodImpl,
            0x1A => ModuleRef,
            0x1B => TypeSpec,
            0x1C => ImplMap,
            0x1D => FieldRva,
            0x1E => EncLog,
            0x1F => EncMap,
            0x20 => Assembly,
            0x21 => AssemblyProcessor,
            0x22 => AssemblyOs,
            0x23 => AssemblyRef,
            0x24 => AssemblyRefProcessor,
            0x25 => AssemblyRefOs,
            0x26 => File,
            0x27 => ExportedType,
            0x28 => ManifestResource,
            0x29 => NestedClass,
            0x2A => GenericParam,
            0x2B => MethodSpec,
            0x2C => GenericParamConstraint,
            _ => return None,
        })
    }

    pub const fn name(self) -> &'static str {
        use TableId::*;
        match self {
            Module => "Module",
            TypeRef => "TypeRef",
            TypeDef => "TypeDef",
            FieldPtr => "FieldPtr",
            Field => "Field",
            MethodPtr => "MethodPtr",
            MethodDef => "MethodDef",
            ParamPtr => "ParamPtr",
            Param => "Param",
            InterfaceImpl => "InterfaceImpl",
            MemberRef => "MemberRef",
            Constant => "Constant",
            CustomAttribute => "CustomAttribute",
            FieldMarshal => "FieldMarshal",
            DeclSecurity => "DeclSecurity",
            ClassLayout => "ClassLayout",
            FieldLayout => "FieldLayout",
            StandAloneSig => "StandAloneSig",
            EventMap => "EventMap",
            EventPtr => "EventPtr",
            Event => "Event",
            PropertyMap => "PropertyMap",
            PropertyPtr => "PropertyPtr",
            Property => "Property",
            MethodSemantics => "MethodSemantics",
            MethodImpl => "MethodImpl",
            ModuleRef => "ModuleRef",
            TypeSpec => "TypeSpec",
            ImplMap => "ImplMap",
            FieldRva => "FieldRVA",
            EncLog => "ENCLog",
            EncMap => "ENCMap",
            Assembly => "Assembly",
            AssemblyProcessor => "AssemblyProcessor",
            AssemblyOs => "AssemblyOS",
            AssemblyRef => "AssemblyRef",
            AssemblyRefProcessor => "AssemblyRefProcessor",
            AssemblyRefOs => "AssemblyRefOS",
            File => "File",
            ExportedType => "ExportedType",
            ManifestResource => "ManifestResource",
            NestedClass => "NestedClass",
            GenericParam => "GenericParam",
            MethodSpec => "MethodSpec",
            GenericParamConstraint => "GenericParamConstraint",
        }
    }
}

/// A metadata token: one byte of table id plus a 24-bit 1-based row index.
///
/// Row index 0 is the "null token" and means *no row*.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Token(pub u32);

impl Token {
    pub const NULL: Token = Token(0);

    #[inline]
    pub const fn new(table: TableId, row: u32) -> Self {
        Token(((table as u32) << 24) | (row & 0x00FF_FFFF))
    }

    #[inline]
    pub const fn raw(self) -> u32 {
        self.0
    }

    #[inline]
    pub const fn table_byte(self) -> u8 {
        (self.0 >> 24) as u8
    }

    #[inline]
    pub fn table(self) -> Option<TableId> {
        TableId::from_raw(self.table_byte())
    }

    /// 1-based row index within the table.
    #[inline]
    pub const fn row(self) -> u32 {
        self.0 & 0x00FF_FFFF
    }

    #[inline]
    pub const fn is_null(self) -> bool {
        self.row() == 0
    }

    /// A token in the 0x70 "UserString" pseudo-table addresses the `#US` heap.
    #[inline]
    pub const fn is_user_string(self) -> bool {
        self.table_byte() == 0x70
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.table() {
            Some(t) => write!(f, "{}({})", t.name(), self.row()),
            None if self.is_user_string() => write!(f, "UserString(0x{:06x})", self.row()),
            None => write!(f, "Token(0x{:08x})", self.0),
        }
    }
}

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x{:08x}", self.0)
    }
}

/// The coded-index kinds of ECMA-335 II.24.2.6.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodedIndex {
    TypeDefOrRef,
    HasConstant,
    HasCustomAttribute,
    HasFieldMarshal,
    HasDeclSecurity,
    MemberRefParent,
    HasSemantics,
    MethodDefOrRef,
    MemberForwarded,
    Implementation,
    CustomAttributeType,
    ResolutionScope,
    TypeOrMethodDef,
}

impl CodedIndex {
    /// The tables a tag can select, in tag order. `None` marks a reserved tag.
    pub const fn tables(self) -> &'static [Option<TableId>] {
        use TableId::*;
        match self {
            Self::TypeDefOrRef => &[Some(TypeDef), Some(TypeRef), Some(TypeSpec)],
            Self::HasConstant => &[Some(Field), Some(Param), Some(Property)],
            Self::HasCustomAttribute => &[
                Some(MethodDef),
                Some(Field),
                Some(TypeRef),
                Some(TypeDef),
                Some(Param),
                Some(InterfaceImpl),
                Some(MemberRef),
                Some(Module),
                Some(DeclSecurity),
                Some(Property),
                Some(Event),
                Some(StandAloneSig),
                Some(ModuleRef),
                Some(TypeSpec),
                Some(Assembly),
                Some(AssemblyRef),
                Some(File),
                Some(ExportedType),
                Some(ManifestResource),
                Some(GenericParam),
                Some(GenericParamConstraint),
                Some(MethodSpec),
            ],
            Self::HasFieldMarshal => &[Some(Field), Some(Param)],
            Self::HasDeclSecurity => &[Some(TypeDef), Some(MethodDef), Some(Assembly)],
            Self::MemberRefParent => &[
                Some(TypeDef),
                Some(TypeRef),
                Some(ModuleRef),
                Some(MethodDef),
                Some(TypeSpec),
            ],
            Self::HasSemantics => &[Some(Event), Some(Property)],
            Self::MethodDefOrRef => &[Some(MethodDef), Some(MemberRef)],
            Self::MemberForwarded => &[Some(Field), Some(MethodDef)],
            Self::Implementation => &[Some(File), Some(AssemblyRef), Some(ExportedType)],
            Self::CustomAttributeType => {
                &[None, None, Some(MethodDef), Some(MemberRef), None]
            }
            Self::ResolutionScope => {
                &[Some(Module), Some(ModuleRef), Some(AssemblyRef), Some(TypeRef)]
            }
            Self::TypeOrMethodDef => &[Some(TypeDef), Some(MethodDef)],
        }
    }

    /// Number of low bits consumed by the tag.
    pub const fn tag_bits(self) -> u32 {
        let n = self.tables().len() as u32;
        // ceil(log2(n))
        let mut bits = 0;
        while (1u32 << bits) < n {
            bits += 1;
        }
        bits
    }

    pub const fn kind_name(self) -> &'static str {
        match self {
            Self::TypeDefOrRef => "TypeDefOrRef",
            Self::HasConstant => "HasConstant",
            Self::HasCustomAttribute => "HasCustomAttribute",
            Self::HasFieldMarshal => "HasFieldMarshal",
            Self::HasDeclSecurity => "HasDeclSecurity",
            Self::MemberRefParent => "MemberRefParent",
            Self::HasSemantics => "HasSemantics",
            Self::MethodDefOrRef => "MethodDefOrRef",
            Self::MemberForwarded => "MemberForwarded",
            Self::Implementation => "Implementation",
            Self::CustomAttributeType => "CustomAttributeType",
            Self::ResolutionScope => "ResolutionScope",
            Self::TypeOrMethodDef => "TypeOrMethodDef",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_widths_match_the_spec() {
        assert_eq!(CodedIndex::TypeDefOrRef.tag_bits(), 2);
        assert_eq!(CodedIndex::HasConstant.tag_bits(), 2);
        assert_eq!(CodedIndex::HasCustomAttribute.tag_bits(), 5);
        assert_eq!(CodedIndex::MethodDefOrRef.tag_bits(), 1);
        assert_eq!(CodedIndex::ResolutionScope.tag_bits(), 2);
        assert_eq!(CodedIndex::MemberRefParent.tag_bits(), 3);
    }

    #[test]
    fn token_round_trips() {
        let t = Token::new(TableId::MethodDef, 42);
        assert_eq!(t.table(), Some(TableId::MethodDef));
        assert_eq!(t.row(), 42);
        assert_eq!(t.raw(), 0x0600_002A);
    }
}
