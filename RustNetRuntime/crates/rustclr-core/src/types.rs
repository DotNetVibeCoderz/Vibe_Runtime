//! The runtime type system.
//!
//! Metadata describes types; this module turns that description into the
//! layout, vtable and field maps the interpreter needs. Types are interned in
//! arenas and referred to by small integer ids so the hot paths never chase
//! strings or pointers.

use rustclr_metadata::{MethodSig, Token, TypeSig};

#[allow(unused_imports)]
use crate::prelude::*;


macro_rules! newtype_id {
    ($(#[$m:meta])* $name:ident) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(pub u32);

        impl $name {
            pub const INVALID: $name = $name(u32::MAX);
            #[inline]
            pub const fn index(self) -> usize {
                self.0 as usize
            }
            #[inline]
            pub const fn is_valid(self) -> bool {
                self.0 != u32::MAX
            }
        }
    };
}

newtype_id!(/// Index into the [`TypeRegistry`].
    TypeId);
newtype_id!(/// Index into the method arena.
    MethodId);
newtype_id!(/// Index into the field arena.
    FieldId);
newtype_id!(/// Index into the loaded-assembly table.
    AssemblyId);
newtype_id!(/// Index into the property arena.
    PropertyId);

/// A property: a name and the accessors the `MethodSemantics` table pairs
/// with it.
///
/// The runtime does not *execute* properties — C# compiles `p.X` to a call to
/// `get_X`, and that has always worked. This exists so reflection can answer
/// `GetProperties()` and `GetValue()`, which need to know that `get_X` and
/// `set_X` are two halves of one member rather than two unrelated methods.
///
/// Reconstructed from metadata rather than guessed from method names: a method
/// called `get_Total` is not necessarily an accessor, and a property whose
/// accessors were renamed by an obfuscator still pairs correctly here.
#[derive(Debug, Clone)]
pub struct PropertyInfo {
    pub id: PropertyId,
    pub name: String,
    pub declaring_type: TypeId,
    pub getter: Option<MethodId>,
    pub setter: Option<MethodId>,
}

impl PropertyInfo {
    pub fn can_read(&self) -> bool {
        self.getter.is_some()
    }
    pub fn can_write(&self) -> bool {
        self.setter.is_some()
    }
    /// Static when its accessors are — a property has no storage of its own.
    pub fn is_static(&self, registry: &TypeRegistry) -> bool {
        self.getter
            .or(self.setter)
            .is_some_and(|m| registry.method(m).is_static())
    }
}

/// What kind of type this is, which decides copy semantics and layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeKind {
    /// A reference type allocated on the managed heap.
    Class,
    /// A value type copied by value.
    ValueType,
    /// An enum; `underlying` in [`RuntimeType`] gives the storage type.
    Enum,
    Interface,
    /// A single-dimension zero-based array (`T[]`).
    SzArray,
    /// A multi-dimensional array.
    Array,
    /// One of the built-in primitives.
    Primitive,
    /// `System.String`, which has its own storage.
    String,
    /// A delegate, dispatched through its invocation list.
    Delegate,
    /// An unmanaged or managed pointer type.
    Pointer,
}

impl TypeKind {
    /// True when instances live on the evaluation stack rather than the heap.
    pub fn is_value_like(self) -> bool {
        matches!(self, Self::ValueType | Self::Enum | Self::Primitive)
    }
}

/// The primitive types the runtime knows intrinsically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Primitive {
    Boolean,
    Char,
    SByte,
    Byte,
    Int16,
    UInt16,
    Int32,
    UInt32,
    Int64,
    UInt64,
    Single,
    Double,
    IntPtr,
    UIntPtr,
    Void,
}

impl Primitive {
    pub const fn full_name(self) -> &'static str {
        match self {
            Self::Boolean => "System.Boolean",
            Self::Char => "System.Char",
            Self::SByte => "System.SByte",
            Self::Byte => "System.Byte",
            Self::Int16 => "System.Int16",
            Self::UInt16 => "System.UInt16",
            Self::Int32 => "System.Int32",
            Self::UInt32 => "System.UInt32",
            Self::Int64 => "System.Int64",
            Self::UInt64 => "System.UInt64",
            Self::Single => "System.Single",
            Self::Double => "System.Double",
            Self::IntPtr => "System.IntPtr",
            Self::UIntPtr => "System.UIntPtr",
            Self::Void => "System.Void",
        }
    }

    /// Storage width in bytes. Pointer-sized types report the host width.
    pub const fn size(self) -> usize {
        match self {
            Self::Boolean | Self::SByte | Self::Byte => 1,
            Self::Char | Self::Int16 | Self::UInt16 => 2,
            Self::Int32 | Self::UInt32 | Self::Single => 4,
            Self::Int64 | Self::UInt64 | Self::Double => 8,
            Self::IntPtr | Self::UIntPtr => core::mem::size_of::<usize>(),
            Self::Void => 0,
        }
    }

    pub const fn is_signed_integer(self) -> bool {
        matches!(self, Self::SByte | Self::Int16 | Self::Int32 | Self::Int64 | Self::IntPtr)
    }

    pub const fn is_unsigned_integer(self) -> bool {
        matches!(
            self,
            Self::Boolean | Self::Byte | Self::Char | Self::UInt16 | Self::UInt32 | Self::UInt64 | Self::UIntPtr
        )
    }

    pub const fn is_float(self) -> bool {
        matches!(self, Self::Single | Self::Double)
    }
}

/// A field, resolved to a slot within its declaring type.
#[derive(Debug, Clone)]
pub struct FieldInfo {
    pub id: FieldId,
    pub name: String,
    pub declaring_type: TypeId,
    pub token: Token,
    pub signature: TypeSig,
    /// Resolved type of the field, once its type has been loaded.
    pub field_type: TypeId,
    pub is_static: bool,
    pub is_literal: bool,
    /// Index into the instance-field vector, or the static-storage vector.
    pub slot: u32,
    /// Byte offset, for explicit-layout types and interop marshalling.
    pub offset: Option<u32>,
    /// Compile-time constant, for `literal` fields.
    pub constant: Option<crate::value::Value>,
}

/// How a method is implemented.
#[derive(Debug, Clone)]
pub enum MethodKind {
    /// Managed IL, interpreted or jitted.
    Il(Box<IlBody>),
    /// Implemented natively by RustBCL, looked up by `Type::Method` key.
    InternalCall,
    /// Forwarded to a native library through the interop bridge.
    PInvoke { library: String, entry_point: String, flags: u16 },
    /// Supplied by the runtime itself (delegate `Invoke`, array accessors).
    RuntimeProvided,
    /// Declared but not implemented here.
    Abstract,
}

/// A method's IL and the state the interpreter needs to run it.
#[derive(Debug, Clone)]
pub struct IlBody {
    pub il: Vec<u8>,
    pub max_stack: u16,
    pub locals: Vec<TypeSig>,
    pub init_locals: bool,
    pub exception_clauses: Vec<rustclr_metadata::ExceptionClause>,
}

/// A method, resolved from metadata.
#[derive(Debug, Clone)]
pub struct MethodInfo {
    pub id: MethodId,
    pub name: String,
    pub declaring_type: TypeId,
    pub token: Token,
    pub assembly: AssemblyId,
    pub signature: MethodSig,
    pub flags: u16,
    pub impl_flags: u16,
    pub kind: MethodKind,
    /// Slot in the declaring type's vtable, for virtual methods.
    pub vtable_slot: Option<u32>,
    /// Fully qualified `Namespace.Type::Method` key, used for native lookup.
    pub qualified_name: String,
    /// Type arguments, when this is a *constructed* generic method.
    ///
    /// `M<int>` and `M<string>` share one body — generics are erased for
    /// execution — but each `MethodSpec` gets its own entry here, and this is
    /// what that entry knows that the open definition does not. It is what
    /// lets `typeof(T)` inside `M<T>` answer, rather than refusing because the
    /// argument was thrown away.
    ///
    /// Empty for an ordinary method and for the open definition itself.
    pub generic_args: Vec<TypeId>,

    /// Declared parameter names, in signature order.
    ///
    /// From the `Param` table, which is separate from the signature: a
    /// signature carries types, and only this carries what the author called
    /// them. Empty for a method loaded from a source that has no `Param` rows
    /// — a native binding, or an assembly compiled without them — and
    /// reflection says `argN` there rather than inventing one.
    pub param_names: Vec<String>,
}

impl MethodInfo {
    pub fn is_static(&self) -> bool {
        self.flags & rustclr_metadata::method_attributes::STATIC != 0
    }
    pub fn is_virtual(&self) -> bool {
        self.flags & rustclr_metadata::method_attributes::VIRTUAL != 0
    }
    pub fn is_abstract(&self) -> bool {
        self.flags & rustclr_metadata::method_attributes::ABSTRACT != 0
    }
    /// Number of incoming argument slots, including `this`.
    pub fn arg_count(&self) -> usize {
        self.signature.arg_count()
    }
    /// True when the method returns nothing.
    ///
    /// The return type must be unwrapped first: a C# `init` accessor is
    /// `void modreq(IsExternalInit)`, and comparing the modified type directly
    /// reports it as value-returning — which makes `ret` pop from an empty
    /// stack.
    pub fn returns_void(&self) -> bool {
        *self.signature.return_type.unwrap_modifiers() == TypeSig::Void
    }
}

/// State of a type's static constructor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CctorState {
    NotRun,
    Running,
    Done,
    /// The class initialiser threw; every later access rethrows.
    Failed,
}

/// A loaded type.
#[derive(Debug, Clone)]
pub struct RuntimeType {
    pub id: TypeId,
    pub name: String,
    pub namespace: String,
    pub assembly: AssemblyId,
    pub token: Token,
    pub kind: TypeKind,
    pub base: Option<TypeId>,
    pub interfaces: Vec<TypeId>,
    /// Instance fields in slot order, including inherited ones.
    pub instance_fields: Vec<FieldId>,
    pub static_fields: Vec<FieldId>,
    pub methods: Vec<MethodId>,
    /// Properties declared on this type, in metadata order.
    pub properties: Vec<PropertyId>,
    /// Virtual dispatch table; index is the slot, value is the implementation.
    pub vtable: Vec<MethodId>,
    /// For arrays and pointers.
    pub element_type: Option<TypeId>,
    /// For enums.
    pub underlying: Option<Primitive>,
    /// For primitives.
    pub primitive: Option<Primitive>,
    pub is_abstract: bool,
    pub is_sealed: bool,
    /// Whether another assembly could derive from this type.
    ///
    /// What makes class-hierarchy analysis over the loaded types a *proof*
    /// rather than a guess: a type nothing outside this assembly can see is a
    /// type nothing outside can extend, so the overrides already loaded are
    /// all the overrides there will ever be — even though `Assembly.Load` can
    /// still bring in more types later. Defaults to `true`, so a type built
    /// any other way is assumed open and no analysis trusts it.
    pub externally_extensible: bool,
    /// Generic parameter count of the open definition.
    pub generic_param_count: u32,
    /// Type arguments, when this is a constructed generic type.
    pub generic_args: Vec<TypeId>,
    /// Open definition this was constructed from.
    pub generic_definition: Option<TypeId>,
    pub cctor: Option<MethodId>,
    pub cctor_state: CctorState,
    /// Declared size for explicit-layout / interop types.
    pub explicit_size: Option<u32>,
    pub packing_size: Option<u16>,
}

impl RuntimeType {
    pub fn full_name(&self) -> String {
        if self.namespace.is_empty() {
            self.name.clone()
        } else {
            format!("{}.{}", self.namespace, self.name)
        }
    }

    pub fn is_value_type(&self) -> bool {
        self.kind.is_value_like()
    }

    pub fn is_array(&self) -> bool {
        matches!(self.kind, TypeKind::SzArray | TypeKind::Array)
    }
}

/// Arena of every loaded type, method and field.
#[derive(Debug, Default)]
#[derive(Clone)]
pub struct TypeRegistry {
    types: Vec<RuntimeType>,
    methods: Vec<MethodInfo>,
    fields: Vec<FieldInfo>,
    properties: Vec<PropertyInfo>,
    /// `full_name` to id, for the current load set.
    by_name: HashMap<String, TypeId>,
    /// `(assembly, metadata token)` to id, the authoritative key.
    by_token: HashMap<(AssemblyId, u32), TypeId>,
    method_by_token: HashMap<(AssemblyId, u32), MethodId>,
    field_by_token: HashMap<(AssemblyId, u32), FieldId>,
    /// Constructed array types, keyed by element.
    sz_arrays: HashMap<TypeId, TypeId>,
}

impl TypeRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn type_count(&self) -> usize {
        self.types.len()
    }
    pub fn method_count(&self) -> usize {
        self.methods.len()
    }
    pub fn field_count(&self) -> usize {
        self.fields.len()
    }
    pub fn property_count(&self) -> usize {
        self.properties.len()
    }

    /// Interns a property and links it to its declaring type.
    pub fn add_property(
        &mut self,
        name: String,
        declaring_type: TypeId,
        getter: Option<MethodId>,
        setter: Option<MethodId>,
    ) -> PropertyId {
        let id = PropertyId(self.properties.len() as u32);
        self.properties.push(PropertyInfo { id, name, declaring_type, getter, setter });
        self.types[declaring_type.index()].properties.push(id);
        id
    }

    pub fn property(&self, id: PropertyId) -> &PropertyInfo {
        &self.properties[id.index()]
    }

    /// Properties on a type and everything it inherits from.
    ///
    /// Walks the base chain because `GetProperties()` reports inherited members,
    /// and a derived type's own list holds only what it declares. A name
    /// declared twice reports the most-derived one, which is what hiding means.
    pub fn properties_of(&self, ty: TypeId) -> Vec<PropertyId> {
        let mut found: Vec<PropertyId> = Vec::new();
        let mut seen: Vec<&str> = Vec::new();
        let mut current = Some(ty);
        while let Some(id) = current {
            let t = &self.types[id.index()];
            for &property in &t.properties {
                let name = self.properties[property.index()].name.as_str();
                if !seen.contains(&name) {
                    seen.push(name);
                    found.push(property);
                }
            }
            current = t.base;
        }
        found
    }

    /// A property by name, searching the base chain.
    pub fn find_property(&self, ty: TypeId, name: &str) -> Option<PropertyId> {
        self.properties_of(ty)
            .into_iter()
            .find(|p| self.properties[p.index()].name == name)
    }

    pub fn add_type(&mut self, mut ty: RuntimeType) -> TypeId {
        let id = TypeId(self.types.len() as u32);
        ty.id = id;
        let full = ty.full_name();
        let key = (ty.assembly, ty.token.raw());
        self.types.push(ty);
        self.by_name.entry(full).or_insert(id);
        self.by_token.insert(key, id);
        id
    }

    /// Interns a type that is a *closed construction* of a generic definition.
    ///
    /// Unlike [`Self::add_type`] this registers no name and no token. Both
    /// would be wrong: `Box<int>` and `Box<string>` share the name `Box`1`,
    /// which is what `.Name` reports on .NET, and they carry the definition's
    /// metadata token — so indexing either by name or by token would make a
    /// lookup answer arbitrarily, and inserting by token would displace the
    /// definition itself.
    ///
    /// A construction is reached through the `TypeSpec` that names it, which is
    /// the only place that knows which arguments were meant.
    pub fn add_constructed_type(&mut self, mut ty: RuntimeType) -> TypeId {
        let id = TypeId(self.types.len() as u32);
        ty.id = id;
        self.types.push(ty);
        id
    }

    pub fn add_method(&mut self, mut m: MethodInfo) -> MethodId {
        let id = MethodId(self.methods.len() as u32);
        m.id = id;
        let key = (m.assembly, m.token.raw());
        self.methods.push(m);
        self.method_by_token.insert(key, id);
        id
    }

    pub fn add_field(&mut self, mut f: FieldInfo, assembly: AssemblyId) -> FieldId {
        let id = FieldId(self.fields.len() as u32);
        f.id = id;
        let key = (assembly, f.token.raw());
        self.fields.push(f);
        self.field_by_token.insert(key, id);
        id
    }

    /// Interns a static field belonging to a closed construction.
    ///
    /// No token index, for the same reason [`Self::add_constructed_type`]
    /// registers none: the clone carries the definition's token, and indexing
    /// it would displace the definition's own field.
    pub fn add_constructed_field(&mut self, mut f: FieldInfo) -> FieldId {
        let id = FieldId(self.fields.len() as u32);
        f.id = id;
        self.fields.push(f);
        id
    }

    #[inline]
    pub fn ty(&self, id: TypeId) -> &RuntimeType {
        &self.types[id.index()]
    }

    #[inline]
    pub fn ty_mut(&mut self, id: TypeId) -> &mut RuntimeType {
        &mut self.types[id.index()]
    }

    #[inline]
    pub fn method(&self, id: MethodId) -> &MethodInfo {
        &self.methods[id.index()]
    }

    #[inline]
    pub fn method_mut(&mut self, id: MethodId) -> &mut MethodInfo {
        &mut self.methods[id.index()]
    }

    #[inline]
    pub fn field(&self, id: FieldId) -> &FieldInfo {
        &self.fields[id.index()]
    }

    #[inline]
    pub fn field_mut(&mut self, id: FieldId) -> &mut FieldInfo {
        &mut self.fields[id.index()]
    }

    pub fn find_type_by_name(&self, full_name: &str) -> Option<TypeId> {
        self.by_name.get(full_name).copied()
    }

    pub fn find_type_by_token(&self, assembly: AssemblyId, token: Token) -> Option<TypeId> {
        self.by_token.get(&(assembly, token.raw())).copied()
    }

    pub fn find_method_by_token(&self, assembly: AssemblyId, token: Token) -> Option<MethodId> {
        self.method_by_token.get(&(assembly, token.raw())).copied()
    }

    pub fn find_field_by_token(&self, assembly: AssemblyId, token: Token) -> Option<FieldId> {
        self.field_by_token.get(&(assembly, token.raw())).copied()
    }

    pub fn register_sz_array(&mut self, element: TypeId, array: TypeId) {
        self.sz_arrays.insert(element, array);
    }

    pub fn find_sz_array(&self, element: TypeId) -> Option<TypeId> {
        self.sz_arrays.get(&element).copied()
    }

    pub fn iter_types(&self) -> impl Iterator<Item = &RuntimeType> {
        self.types.iter()
    }

    pub fn iter_methods(&self) -> impl Iterator<Item = &MethodInfo> {
        self.methods.iter()
    }

    /// True when `derived` is `base` or inherits from it, or implements it.
    pub fn is_assignable_to(&self, derived: TypeId, base: TypeId) -> bool {
        if derived == base {
            return true;
        }
        let mut current = Some(derived);
        while let Some(id) = current {
            let ty = self.ty(id);
            if id == base || ty.interfaces.contains(&base) {
                return true;
            }
            // Interfaces of interfaces.
            if ty.interfaces.iter().any(|i| self.is_assignable_to(*i, base)) {
                return true;
            }
            current = ty.base;
        }
        // A program's own `class Props : Dictionary<string, object>` inherits
        // what its native base implements.
        self.base_chain(derived).any(|t| self.builtin_implements(t, base))
    }

    /// The interfaces arrays, strings and the natively implemented collections
    /// satisfy without declaring them.
    ///
    /// `List<T>`, `Dictionary<K,V>` and the rest carry no metadata here, so no
    /// interface list was ever read for them — and without this `list is
    /// IEnumerable`, `(IDictionary)dict` or a serializer's `case IEnumerable`
    /// all failed on the very objects they exist for. Type arguments are
    /// checked when both sides carry them; a natively allocated array whose
    /// element type was erased to `object` is accepted, since its elements
    /// are whatever the program stored.
    fn builtin_implements(&self, actual: TypeId, target: TypeId) -> bool {
        let valid = |id: TypeId| id.is_valid() && id.index() < self.types.len();
        if !valid(actual) || !valid(target) {
            return false;
        }
        let t = self.ty(target);
        if t.kind != TypeKind::Interface {
            return false;
        }
        let tdef = t.generic_definition.filter(|d| valid(*d)).map(|d| self.ty(d)).unwrap_or(t);
        let (ns, iface) = (tdef.namespace.as_str(), tdef.name.as_str());
        let generic = match ns {
            "System.Collections.Generic" => true,
            "System.Collections" => false,
            _ => return false,
        };
        let a = self.ty(actual);
        let adef = a.generic_definition.filter(|d| valid(*d)).map(|d| self.ty(d)).unwrap_or(a);

        const SEQUENCE: &[&str] = &["IEnumerable`1", "IReadOnlyCollection`1"];
        const LIST: &[&str] =
            &["IEnumerable`1", "IReadOnlyCollection`1", "ICollection`1", "IList`1", "IReadOnlyList`1"];
        const UNTYPED: &[&str] = &["IEnumerable", "ICollection", "IList"];
        let (typed, untyped): (&[&str], &[&str]) = match a.kind {
            TypeKind::SzArray | TypeKind::Array => (LIST, UNTYPED),
            // An array whose `T[]` type was never registered is typed as
            // `System.Array`; its elements are checked by nothing, so neither
            // is its type argument.
            _ if a.namespace == "System" && a.name == "Array" => (LIST, UNTYPED),
            TypeKind::String => (&["IEnumerable`1"], &["IEnumerable"]),
            _ if a.namespace == "System.Text.RegularExpressions" && matches!(a.name.as_str(), "MatchCollection" | "GroupCollection") => (LIST, UNTYPED),
            TypeKind::Interface => match adef.name.as_str() {
                "IList`1" | "ICollection`1" => (&["IEnumerable`1", "ICollection`1"], &["IEnumerable"]),
                "IReadOnlyList`1" | "IReadOnlyCollection`1" => (SEQUENCE, &["IEnumerable"]),
                "IDictionary`2" | "IReadOnlyDictionary`2" => (&["IEnumerable`1"], &["IEnumerable"]),
                "ISet`1" => (&["IEnumerable`1", "ICollection`1"], &["IEnumerable"]),
                "IEnumerable`1" => (&[], &["IEnumerable"]),
                "IList" | "IDictionary" => (&[], &["IEnumerable", "ICollection"]),
                "ICollection" => (&[], &["IEnumerable"]),
                _ => return false,
            },
            _ if adef.namespace == "System.Collections.Generic" => match adef.name.as_str() {
                "List`1" => (LIST, UNTYPED),
                "HashSet`1" => (&["IEnumerable`1", "IReadOnlyCollection`1", "ICollection`1", "ISet`1"], &["IEnumerable"]),
                "Queue`1" | "Stack`1" => (SEQUENCE, &["IEnumerable", "ICollection"]),
                "Dictionary`2" => (
                    &["IEnumerable`1", "IReadOnlyCollection`1", "ICollection`1", "IDictionary`2", "IReadOnlyDictionary`2"],
                    &["IEnumerable", "ICollection", "IDictionary"],
                ),
                "Dictionary`2+KeyCollection" | "Dictionary`2+ValueCollection" => {
                    (&["IEnumerable`1", "IReadOnlyCollection`1", "ICollection`1"], &["IEnumerable", "ICollection"])
                }
                n if n.ends_with("+Enumerator") => (&[], &["IEnumerator"]),
                _ => return false,
            },
            _ => return false,
        };
        if !(if generic { typed } else { untyped }).contains(&iface) {
            return false;
        }
        if !generic || t.generic_args.is_empty() {
            return true;
        }
        // The element type, where the receiver records one.
        let element = match a.kind {
            TypeKind::SzArray | TypeKind::Array => a.element_type,
            TypeKind::String => self.find_type_by_name("System.Char"),
            _ if adef.name == "Dictionary`2" => {
                // IDictionary<K,V> and IReadOnlyDictionary<K,V> name both
                // arguments; the sequence interfaces name KeyValuePair<K,V>.
                if iface.contains("Dictionary") && a.generic_args.len() == 2 && t.generic_args.len() == 2 {
                    return a.generic_args == t.generic_args;
                }
                return true;
            }
            _ => a.generic_args.first().copied(),
        };
        let (Some(element), Some(&wanted)) = (element, t.generic_args.first()) else {
            return true;
        };
        if element == wanted || !valid(element) || !valid(wanted) {
            return true;
        }
        let e = self.ty(element);
        if e.namespace == "System" && e.name == "Object" {
            return true;
        }
        // Covariance: a string[] is an IEnumerable<object>, an int[] is not.
        let by_reference = |ty: &RuntimeType| !matches!(ty.kind, TypeKind::ValueType | TypeKind::Enum | TypeKind::Primitive);
        by_reference(e) && by_reference(self.ty(wanted)) && matches!(iface, "IEnumerable`1" | "IReadOnlyCollection`1" | "IReadOnlyList`1") && self.is_assignable_to(element, wanted)
    }

    /// Walks the inheritance chain from `id` up to `System.Object`.
    ///
    /// An invalid id yields nothing rather than panicking: it means a handle
    /// went stale, and the caller can report that far more usefully than an
    /// index-out-of-bounds abort can.
    pub fn base_chain(&self, id: TypeId) -> impl Iterator<Item = TypeId> + '_ {
        let mut current = (id.is_valid() && id.index() < self.types.len()).then_some(id);
        core::iter::from_fn(move || {
            let out = current?;
            current = self.ty(out).base;
            Some(out)
        })
    }

    /// Finds an instance field slot by name, searching base types.
    pub fn find_instance_field(&self, ty: TypeId, name: &str) -> Option<FieldId> {
        for id in self.base_chain(ty) {
            for f in &self.ty(id).instance_fields {
                if self.field(*f).name == name {
                    return Some(*f);
                }
            }
        }
        None
    }

    /// Resolves a virtual call: finds the most-derived override of `slot`.
    pub fn resolve_virtual(&self, object_type: TypeId, slot: u32) -> Option<MethodId> {
        self.ty(object_type).vtable.get(slot as usize).copied()
    }
}
