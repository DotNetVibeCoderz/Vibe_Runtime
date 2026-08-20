//! Column layout for every metadata table.
//!
//! Row sizes in ECMA-335 are not fixed: an index column is 2 bytes when the
//! target table has fewer than 2^16 rows and 4 bytes otherwise, and heap
//! indexes widen based on flags in the `#~` header. Rather than hard-coding 45
//! row-size formulas we describe each table as a list of typed columns and
//! compute sizes from the actual image.

use crate::token::{CodedIndex, TableId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Column {
    /// Fixed-width scalar.
    U8,
    U16,
    U32,
    /// Index into a heap; width depends on the `#~` heap-size flags.
    String,
    Blob,
    Guid,
    /// Simple index into one table; width depends on that table's row count.
    Table(TableId),
    /// Coded index across several tables.
    Coded(CodedIndex),
}

impl TableId {
    /// The column layout of this table, in on-disk order.
    pub const fn columns(self) -> &'static [Column] {
        use CodedIndex as C;
        use Column::*;
        use TableId as T;
        match self {
            T::Module => &[U16, String, Guid, Guid, Guid],
            T::TypeRef => &[Coded(C::ResolutionScope), String, String],
            T::TypeDef => &[
                U32,
                String,
                String,
                Coded(C::TypeDefOrRef),
                Table(T::Field),
                Table(T::MethodDef),
            ],
            T::FieldPtr => &[Table(T::Field)],
            T::Field => &[U16, String, Blob],
            T::MethodPtr => &[Table(T::MethodDef)],
            T::MethodDef => &[U32, U16, U16, String, Blob, Table(T::Param)],
            T::ParamPtr => &[Table(T::Param)],
            T::Param => &[U16, U16, String],
            T::InterfaceImpl => &[Table(T::TypeDef), Coded(C::TypeDefOrRef)],
            T::MemberRef => &[Coded(C::MemberRefParent), String, Blob],
            T::Constant => &[U8, U8, Coded(C::HasConstant), Blob],
            T::CustomAttribute => &[
                Coded(C::HasCustomAttribute),
                Coded(C::CustomAttributeType),
                Blob,
            ],
            T::FieldMarshal => &[Coded(C::HasFieldMarshal), Blob],
            T::DeclSecurity => &[U16, Coded(C::HasDeclSecurity), Blob],
            T::ClassLayout => &[U16, U32, Table(T::TypeDef)],
            T::FieldLayout => &[U32, Table(T::Field)],
            T::StandAloneSig => &[Blob],
            T::EventMap => &[Table(T::TypeDef), Table(T::Event)],
            T::EventPtr => &[Table(T::Event)],
            T::Event => &[U16, String, Coded(C::TypeDefOrRef)],
            T::PropertyMap => &[Table(T::TypeDef), Table(T::Property)],
            T::PropertyPtr => &[Table(T::Property)],
            T::Property => &[U16, String, Blob],
            T::MethodSemantics => &[U16, Table(T::MethodDef), Coded(C::HasSemantics)],
            T::MethodImpl => &[
                Table(T::TypeDef),
                Coded(C::MethodDefOrRef),
                Coded(C::MethodDefOrRef),
            ],
            T::ModuleRef => &[String],
            T::TypeSpec => &[Blob],
            T::ImplMap => &[U16, Coded(C::MemberForwarded), String, Table(T::ModuleRef)],
            T::FieldRva => &[U32, Table(T::Field)],
            T::EncLog => &[U32, U32],
            T::EncMap => &[U32],
            T::Assembly => &[U32, U16, U16, U16, U16, U32, Blob, String, String],
            T::AssemblyProcessor => &[U32],
            T::AssemblyOs => &[U32, U32, U32],
            T::AssemblyRef => &[U16, U16, U16, U16, U32, Blob, String, String, Blob],
            T::AssemblyRefProcessor => &[U32, Table(T::AssemblyRef)],
            T::AssemblyRefOs => &[U32, U32, U32, Table(T::AssemblyRef)],
            T::File => &[U32, String, Blob],
            T::ExportedType => &[U32, U32, String, String, Coded(C::Implementation)],
            T::ManifestResource => &[U32, U32, String, Coded(C::Implementation)],
            T::NestedClass => &[Table(T::TypeDef), Table(T::TypeDef)],
            T::GenericParam => &[U16, U16, Coded(C::TypeOrMethodDef), String],
            T::MethodSpec => &[Coded(C::MethodDefOrRef), Blob],
            T::GenericParamConstraint => &[Table(T::GenericParam), Coded(C::TypeDefOrRef)],
        }
    }
}
