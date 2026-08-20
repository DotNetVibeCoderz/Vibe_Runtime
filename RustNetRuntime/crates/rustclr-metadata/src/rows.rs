//! Typed views over the metadata rows the runtime actually consumes.

#[allow(unused_imports)]
use crate::prelude::*;

use crate::error::Result;
use crate::tables::Metadata;
use crate::token::{CodedIndex, TableId, Token};

/// Type attribute flags (ECMA-335 II.23.1.15).
pub mod type_attributes {
    pub const VISIBILITY_MASK: u32 = 0x0000_0007;
    pub const NOT_PUBLIC: u32 = 0x0000_0000;
    pub const PUBLIC: u32 = 0x0000_0001;
    pub const NESTED_PUBLIC: u32 = 0x0000_0002;

    pub const LAYOUT_MASK: u32 = 0x0000_0018;
    pub const AUTO_LAYOUT: u32 = 0x0000_0000;
    pub const SEQUENTIAL_LAYOUT: u32 = 0x0000_0008;
    pub const EXPLICIT_LAYOUT: u32 = 0x0000_0010;

    pub const CLASS_SEMANTICS_MASK: u32 = 0x0000_0020;
    pub const INTERFACE: u32 = 0x0000_0020;

    pub const ABSTRACT: u32 = 0x0000_0080;
    pub const SEALED: u32 = 0x0000_0100;
    pub const SPECIAL_NAME: u32 = 0x0000_0400;

    pub const STRING_FORMAT_MASK: u32 = 0x0003_0000;
    pub const ANSI_CLASS: u32 = 0x0000_0000;
    pub const UNICODE_CLASS: u32 = 0x0001_0000;

    pub const BEFORE_FIELD_INIT: u32 = 0x0010_0000;
}

/// Method attribute flags (ECMA-335 II.23.1.10).
pub mod method_attributes {
    pub const MEMBER_ACCESS_MASK: u16 = 0x0007;
    pub const PRIVATE: u16 = 0x0001;
    pub const PUBLIC: u16 = 0x0006;

    pub const STATIC: u16 = 0x0010;
    pub const FINAL: u16 = 0x0020;
    pub const VIRTUAL: u16 = 0x0040;
    pub const HIDE_BY_SIG: u16 = 0x0080;
    pub const NEW_SLOT: u16 = 0x0100;
    pub const ABSTRACT: u16 = 0x0400;
    pub const SPECIAL_NAME: u16 = 0x0800;
    pub const PINVOKE_IMPL: u16 = 0x2000;
    pub const RT_SPECIAL_NAME: u16 = 0x1000;
}

/// Method implementation flags (ECMA-335 II.23.1.11).
pub mod method_impl_attributes {
    pub const CODE_TYPE_MASK: u16 = 0x0003;
    pub const IL: u16 = 0x0000;
    pub const NATIVE: u16 = 0x0001;
    pub const RUNTIME: u16 = 0x0003;

    pub const MANAGED_MASK: u16 = 0x0004;
    pub const UNMANAGED: u16 = 0x0004;

    pub const FORWARD_REF: u16 = 0x0010;
    pub const PRESERVE_SIG: u16 = 0x0080;
    pub const INTERNAL_CALL: u16 = 0x1000;
    pub const SYNCHRONIZED: u16 = 0x0020;
    pub const NO_INLINING: u16 = 0x0008;
    pub const AGGRESSIVE_INLINING: u16 = 0x0100;
}

/// Field attribute flags (ECMA-335 II.23.1.5).
pub mod field_attributes {
    pub const FIELD_ACCESS_MASK: u16 = 0x0007;
    pub const PUBLIC: u16 = 0x0006;

    pub const STATIC: u16 = 0x0010;
    pub const INIT_ONLY: u16 = 0x0020;
    pub const LITERAL: u16 = 0x0040;
    pub const NOT_SERIALIZED: u16 = 0x0080;
    pub const HAS_FIELD_RVA: u16 = 0x0100;
    pub const SPECIAL_NAME: u16 = 0x0200;
    pub const HAS_DEFAULT: u16 = 0x8000;
}

#[derive(Debug, Clone)]
pub struct ModuleRow<'a> {
    pub generation: u16,
    pub name: &'a str,
    pub mvid: [u8; 16],
}

#[derive(Debug, Clone)]
pub struct TypeRefRow<'a> {
    pub resolution_scope: Token,
    pub name: &'a str,
    pub namespace: &'a str,
}

impl<'a> TypeRefRow<'a> {
    pub fn full_name(&self) -> String {
        if self.namespace.is_empty() {
            self.name.to_string()
        } else {
            format!("{}.{}", self.namespace, self.name)
        }
    }
}

#[derive(Debug, Clone)]
pub struct TypeDefRow<'a> {
    pub flags: u32,
    pub name: &'a str,
    pub namespace: &'a str,
    pub extends: Token,
    /// 1-based index of the first row in `Field` owned by this type.
    pub field_list: u32,
    /// 1-based index of the first row in `MethodDef` owned by this type.
    pub method_list: u32,
}

impl<'a> TypeDefRow<'a> {
    pub fn full_name(&self) -> String {
        if self.namespace.is_empty() {
            self.name.to_string()
        } else {
            format!("{}.{}", self.namespace, self.name)
        }
    }

    pub fn is_interface(&self) -> bool {
        self.flags & type_attributes::CLASS_SEMANTICS_MASK == type_attributes::INTERFACE
    }

    pub fn is_abstract(&self) -> bool {
        self.flags & type_attributes::ABSTRACT != 0
    }

    pub fn is_sealed(&self) -> bool {
        self.flags & type_attributes::SEALED != 0
    }

    pub fn is_explicit_layout(&self) -> bool {
        self.flags & type_attributes::LAYOUT_MASK == type_attributes::EXPLICIT_LAYOUT
    }

    pub fn is_sequential_layout(&self) -> bool {
        self.flags & type_attributes::LAYOUT_MASK == type_attributes::SEQUENTIAL_LAYOUT
    }
}

#[derive(Debug, Clone)]
pub struct FieldRow<'a> {
    pub flags: u16,
    pub name: &'a str,
    pub signature: &'a [u8],
}

impl<'a> FieldRow<'a> {
    pub fn is_static(&self) -> bool {
        self.flags & field_attributes::STATIC != 0
    }
    pub fn is_literal(&self) -> bool {
        self.flags & field_attributes::LITERAL != 0
    }
    pub fn has_rva(&self) -> bool {
        self.flags & field_attributes::HAS_FIELD_RVA != 0
    }
}

#[derive(Debug, Clone)]
pub struct MethodDefRow<'a> {
    pub rva: u32,
    pub impl_flags: u16,
    pub flags: u16,
    pub name: &'a str,
    pub signature: &'a [u8],
    /// 1-based index of the first row in `Param` owned by this method.
    pub param_list: u32,
}

impl<'a> MethodDefRow<'a> {
    pub fn is_static(&self) -> bool {
        self.flags & method_attributes::STATIC != 0
    }
    pub fn is_virtual(&self) -> bool {
        self.flags & method_attributes::VIRTUAL != 0
    }
    pub fn is_abstract(&self) -> bool {
        self.flags & method_attributes::ABSTRACT != 0
    }
    pub fn is_pinvoke(&self) -> bool {
        self.flags & method_attributes::PINVOKE_IMPL != 0
    }
    pub fn is_internal_call(&self) -> bool {
        self.impl_flags & method_impl_attributes::INTERNAL_CALL != 0
    }
    pub fn is_runtime_provided(&self) -> bool {
        self.impl_flags & method_impl_attributes::CODE_TYPE_MASK == method_impl_attributes::RUNTIME
    }
    /// True when the method has no IL body of its own.
    pub fn has_body(&self) -> bool {
        self.rva != 0 && !self.is_abstract()
    }
    pub fn is_constructor(&self) -> bool {
        self.name == ".ctor" || self.name == ".cctor"
    }
}

#[derive(Debug, Clone)]
pub struct ParamRow<'a> {
    pub flags: u16,
    /// 0 is the return value; parameters are 1-based.
    pub sequence: u16,
    pub name: &'a str,
}

#[derive(Debug, Clone)]
pub struct MemberRefRow<'a> {
    pub class: Token,
    pub name: &'a str,
    pub signature: &'a [u8],
}

#[derive(Debug, Clone)]
pub struct AssemblyRefRow<'a> {
    pub major: u16,
    pub minor: u16,
    pub build: u16,
    pub revision: u16,
    pub flags: u32,
    pub public_key_or_token: &'a [u8],
    pub name: &'a str,
    pub culture: &'a str,
    pub hash_value: &'a [u8],
}

impl<'a> AssemblyRefRow<'a> {
    pub fn version_string(&self) -> String {
        format!("{}.{}.{}.{}", self.major, self.minor, self.build, self.revision)
    }
}

#[derive(Debug, Clone)]
pub struct AssemblyRow<'a> {
    pub hash_alg_id: u32,
    pub major: u16,
    pub minor: u16,
    pub build: u16,
    pub revision: u16,
    pub flags: u32,
    pub public_key: &'a [u8],
    pub name: &'a str,
    pub culture: &'a str,
}

impl<'a> AssemblyRow<'a> {
    pub fn version_string(&self) -> String {
        format!("{}.{}.{}.{}", self.major, self.minor, self.build, self.revision)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct InterfaceImplRow {
    pub class: Token,
    pub interface: Token,
}

#[derive(Debug, Clone, Copy)]
pub struct NestedClassRow {
    pub nested: Token,
    pub enclosing: Token,
}

#[derive(Debug, Clone, Copy)]
pub struct ClassLayoutRow {
    pub packing_size: u16,
    pub class_size: u32,
    pub parent: Token,
}

#[derive(Debug, Clone, Copy)]
pub struct FieldLayoutRow {
    pub offset: u32,
    pub field: Token,
}

#[derive(Debug, Clone, Copy)]
pub struct FieldRvaRow {
    pub rva: u32,
    pub field: Token,
}

#[derive(Debug, Clone)]
pub struct ConstantRow<'a> {
    pub element_type: u8,
    pub parent: Token,
    pub value: &'a [u8],
}

#[derive(Debug, Clone)]
pub struct CustomAttributeRow<'a> {
    pub parent: Token,
    pub constructor: Token,
    pub value: &'a [u8],
}

#[derive(Debug, Clone)]
pub struct ImplMapRow<'a> {
    pub mapping_flags: u16,
    pub member_forwarded: Token,
    pub import_name: &'a str,
    pub import_scope: Token,
}

#[derive(Debug, Clone)]
pub struct ModuleRefRow<'a> {
    pub name: &'a str,
}

#[derive(Debug, Clone)]
pub struct PropertyRow<'a> {
    pub flags: u16,
    pub name: &'a str,
    pub signature: &'a [u8],
}

#[derive(Debug, Clone, Copy)]
pub struct PropertyMapRow {
    pub parent: Token,
    pub property_list: u32,
}

#[derive(Debug, Clone)]
pub struct EventRow<'a> {
    pub flags: u16,
    pub name: &'a str,
    pub event_type: Token,
}

#[derive(Debug, Clone, Copy)]
pub struct EventMapRow {
    pub parent: Token,
    pub event_list: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct MethodSemanticsRow {
    pub semantics: u16,
    pub method: Token,
    pub association: Token,
}

pub mod method_semantics {
    pub const SETTER: u16 = 0x0001;
    pub const GETTER: u16 = 0x0002;
    pub const OTHER: u16 = 0x0004;
    pub const ADD_ON: u16 = 0x0008;
    pub const REMOVE_ON: u16 = 0x0010;
    pub const FIRE: u16 = 0x0020;
}

#[derive(Debug, Clone, Copy)]
pub struct MethodImplRow {
    pub class: Token,
    pub body: Token,
    pub declaration: Token,
}

#[derive(Debug, Clone)]
pub struct GenericParamRow<'a> {
    pub number: u16,
    pub flags: u16,
    pub owner: Token,
    pub name: &'a str,
}

#[derive(Debug, Clone)]
pub struct MethodSpecRow<'a> {
    pub method: Token,
    pub instantiation: &'a [u8],
}

#[derive(Debug, Clone)]
pub struct TypeSpecRow<'a> {
    pub signature: &'a [u8],
}

#[derive(Debug, Clone)]
pub struct StandAloneSigRow<'a> {
    pub signature: &'a [u8],
}

// ---------------------------------------------------------------------------
// Row readers
// ---------------------------------------------------------------------------

impl<'a> Metadata<'a> {
    pub fn module(&self, index: u32) -> Result<ModuleRow<'a>> {
        let mut r = self.row(TableId::Module, index)?;
        Ok(ModuleRow {
            generation: r.u16()?,
            name: r.string()?,
            mvid: r.guid()?,
        })
    }

    pub fn type_ref(&self, index: u32) -> Result<TypeRefRow<'a>> {
        let mut r = self.row(TableId::TypeRef, index)?;
        Ok(TypeRefRow {
            resolution_scope: r.coded(CodedIndex::ResolutionScope)?,
            name: r.string()?,
            namespace: r.string()?,
        })
    }

    pub fn type_def(&self, index: u32) -> Result<TypeDefRow<'a>> {
        let mut r = self.row(TableId::TypeDef, index)?;
        Ok(TypeDefRow {
            flags: r.u32()?,
            name: r.string()?,
            namespace: r.string()?,
            extends: r.coded(CodedIndex::TypeDefOrRef)?,
            field_list: r.u32()?,
            method_list: r.u32()?,
        })
    }

    pub fn field(&self, index: u32) -> Result<FieldRow<'a>> {
        let mut r = self.row(TableId::Field, index)?;
        Ok(FieldRow {
            flags: r.u16()?,
            name: r.string()?,
            signature: r.blob()?,
        })
    }

    pub fn method_def(&self, index: u32) -> Result<MethodDefRow<'a>> {
        let mut r = self.row(TableId::MethodDef, index)?;
        Ok(MethodDefRow {
            rva: r.u32()?,
            impl_flags: r.u16()?,
            flags: r.u16()?,
            name: r.string()?,
            signature: r.blob()?,
            param_list: r.u32()?,
        })
    }

    pub fn param(&self, index: u32) -> Result<ParamRow<'a>> {
        let mut r = self.row(TableId::Param, index)?;
        Ok(ParamRow {
            flags: r.u16()?,
            sequence: r.u16()?,
            name: r.string()?,
        })
    }

    pub fn member_ref(&self, index: u32) -> Result<MemberRefRow<'a>> {
        let mut r = self.row(TableId::MemberRef, index)?;
        Ok(MemberRefRow {
            class: r.coded(CodedIndex::MemberRefParent)?,
            name: r.string()?,
            signature: r.blob()?,
        })
    }

    pub fn assembly(&self, index: u32) -> Result<AssemblyRow<'a>> {
        let mut r = self.row(TableId::Assembly, index)?;
        Ok(AssemblyRow {
            hash_alg_id: r.u32()?,
            major: r.u16()?,
            minor: r.u16()?,
            build: r.u16()?,
            revision: r.u16()?,
            flags: r.u32()?,
            public_key: r.blob()?,
            name: r.string()?,
            culture: r.string()?,
        })
    }

    pub fn assembly_ref(&self, index: u32) -> Result<AssemblyRefRow<'a>> {
        let mut r = self.row(TableId::AssemblyRef, index)?;
        Ok(AssemblyRefRow {
            major: r.u16()?,
            minor: r.u16()?,
            build: r.u16()?,
            revision: r.u16()?,
            flags: r.u32()?,
            public_key_or_token: r.blob()?,
            name: r.string()?,
            culture: r.string()?,
            hash_value: r.blob()?,
        })
    }

    pub fn interface_impl(&self, index: u32) -> Result<InterfaceImplRow> {
        let mut r = self.row(TableId::InterfaceImpl, index)?;
        Ok(InterfaceImplRow {
            class: r.table_index(TableId::TypeDef)?,
            interface: r.coded(CodedIndex::TypeDefOrRef)?,
        })
    }

    pub fn nested_class(&self, index: u32) -> Result<NestedClassRow> {
        let mut r = self.row(TableId::NestedClass, index)?;
        Ok(NestedClassRow {
            nested: r.table_index(TableId::TypeDef)?,
            enclosing: r.table_index(TableId::TypeDef)?,
        })
    }

    pub fn class_layout(&self, index: u32) -> Result<ClassLayoutRow> {
        let mut r = self.row(TableId::ClassLayout, index)?;
        Ok(ClassLayoutRow {
            packing_size: r.u16()?,
            class_size: r.u32()?,
            parent: r.table_index(TableId::TypeDef)?,
        })
    }

    pub fn field_layout(&self, index: u32) -> Result<FieldLayoutRow> {
        let mut r = self.row(TableId::FieldLayout, index)?;
        Ok(FieldLayoutRow {
            offset: r.u32()?,
            field: r.table_index(TableId::Field)?,
        })
    }

    pub fn field_rva(&self, index: u32) -> Result<FieldRvaRow> {
        let mut r = self.row(TableId::FieldRva, index)?;
        Ok(FieldRvaRow {
            rva: r.u32()?,
            field: r.table_index(TableId::Field)?,
        })
    }

    pub fn constant(&self, index: u32) -> Result<ConstantRow<'a>> {
        let mut r = self.row(TableId::Constant, index)?;
        let element_type = r.u8()?;
        r.skip(1)?; // padding byte
        Ok(ConstantRow {
            element_type,
            parent: r.coded(CodedIndex::HasConstant)?,
            value: r.blob()?,
        })
    }

    pub fn custom_attribute(&self, index: u32) -> Result<CustomAttributeRow<'a>> {
        let mut r = self.row(TableId::CustomAttribute, index)?;
        Ok(CustomAttributeRow {
            parent: r.coded(CodedIndex::HasCustomAttribute)?,
            constructor: r.coded(CodedIndex::CustomAttributeType)?,
            value: r.blob()?,
        })
    }

    pub fn impl_map(&self, index: u32) -> Result<ImplMapRow<'a>> {
        let mut r = self.row(TableId::ImplMap, index)?;
        Ok(ImplMapRow {
            mapping_flags: r.u16()?,
            member_forwarded: r.coded(CodedIndex::MemberForwarded)?,
            import_name: r.string()?,
            import_scope: r.table_index(TableId::ModuleRef)?,
        })
    }

    pub fn module_ref(&self, index: u32) -> Result<ModuleRefRow<'a>> {
        let mut r = self.row(TableId::ModuleRef, index)?;
        Ok(ModuleRefRow { name: r.string()? })
    }

    pub fn property(&self, index: u32) -> Result<PropertyRow<'a>> {
        let mut r = self.row(TableId::Property, index)?;
        Ok(PropertyRow {
            flags: r.u16()?,
            name: r.string()?,
            signature: r.blob()?,
        })
    }

    pub fn property_map(&self, index: u32) -> Result<PropertyMapRow> {
        let mut r = self.row(TableId::PropertyMap, index)?;
        Ok(PropertyMapRow {
            parent: r.table_index(TableId::TypeDef)?,
            property_list: r.u32()?,
        })
    }

    pub fn event(&self, index: u32) -> Result<EventRow<'a>> {
        let mut r = self.row(TableId::Event, index)?;
        Ok(EventRow {
            flags: r.u16()?,
            name: r.string()?,
            event_type: r.coded(CodedIndex::TypeDefOrRef)?,
        })
    }

    pub fn event_map(&self, index: u32) -> Result<EventMapRow> {
        let mut r = self.row(TableId::EventMap, index)?;
        Ok(EventMapRow {
            parent: r.table_index(TableId::TypeDef)?,
            event_list: r.u32()?,
        })
    }

    pub fn method_semantics(&self, index: u32) -> Result<MethodSemanticsRow> {
        let mut r = self.row(TableId::MethodSemantics, index)?;
        Ok(MethodSemanticsRow {
            semantics: r.u16()?,
            method: r.table_index(TableId::MethodDef)?,
            association: r.coded(CodedIndex::HasSemantics)?,
        })
    }

    pub fn method_impl(&self, index: u32) -> Result<MethodImplRow> {
        let mut r = self.row(TableId::MethodImpl, index)?;
        Ok(MethodImplRow {
            class: r.table_index(TableId::TypeDef)?,
            body: r.coded(CodedIndex::MethodDefOrRef)?,
            declaration: r.coded(CodedIndex::MethodDefOrRef)?,
        })
    }

    pub fn generic_param(&self, index: u32) -> Result<GenericParamRow<'a>> {
        let mut r = self.row(TableId::GenericParam, index)?;
        Ok(GenericParamRow {
            number: r.u16()?,
            flags: r.u16()?,
            owner: r.coded(CodedIndex::TypeOrMethodDef)?,
            name: r.string()?,
        })
    }

    pub fn method_spec(&self, index: u32) -> Result<MethodSpecRow<'a>> {
        let mut r = self.row(TableId::MethodSpec, index)?;
        Ok(MethodSpecRow {
            method: r.coded(CodedIndex::MethodDefOrRef)?,
            instantiation: r.blob()?,
        })
    }

    pub fn type_spec(&self, index: u32) -> Result<TypeSpecRow<'a>> {
        let mut r = self.row(TableId::TypeSpec, index)?;
        Ok(TypeSpecRow { signature: r.blob()? })
    }

    pub fn stand_alone_sig(&self, index: u32) -> Result<StandAloneSigRow<'a>> {
        let mut r = self.row(TableId::StandAloneSig, index)?;
        Ok(StandAloneSigRow { signature: r.blob()? })
    }

    // -- range helpers ------------------------------------------------------

    /// Resolves an owner-list range. Metadata stores only the start index of a
    /// child list; the end is the next owner's start, or the table's row count
    /// for the last owner.
    pub fn child_range(
        &self,
        owner_table: TableId,
        owner_index: u32,
        child_table: TableId,
        start: u32,
    ) -> Result<core::ops::Range<u32>> {
        let owner_rows = self.row_count(owner_table);
        let end = if owner_index >= owner_rows {
            self.row_count(child_table) + 1
        } else {
            // Read the same column from the next owner row.
            let next_start = match owner_table {
                TableId::TypeDef if child_table == TableId::Field => {
                    self.type_def(owner_index + 1)?.field_list
                }
                TableId::TypeDef if child_table == TableId::MethodDef => {
                    self.type_def(owner_index + 1)?.method_list
                }
                TableId::MethodDef => self.method_def(owner_index + 1)?.param_list,
                TableId::PropertyMap => self.property_map(owner_index + 1)?.property_list,
                TableId::EventMap => self.event_map(owner_index + 1)?.event_list,
                _ => self.row_count(child_table) + 1,
            };
            next_start
        };
        Ok(start..end.max(start))
    }

    /// Fields owned by TypeDef row `index`.
    pub fn fields_of(&self, index: u32) -> Result<core::ops::Range<u32>> {
        let start = self.type_def(index)?.field_list;
        self.child_range(TableId::TypeDef, index, TableId::Field, start)
    }

    /// Methods owned by TypeDef row `index`.
    pub fn methods_of(&self, index: u32) -> Result<core::ops::Range<u32>> {
        let start = self.type_def(index)?.method_list;
        self.child_range(TableId::TypeDef, index, TableId::MethodDef, start)
    }

    /// Parameters owned by MethodDef row `index`.
    pub fn params_of(&self, index: u32) -> Result<core::ops::Range<u32>> {
        let start = self.method_def(index)?.param_list;
        self.child_range(TableId::MethodDef, index, TableId::Param, start)
    }
}
