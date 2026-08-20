//! Assembly loading: metadata to runtime types.
//!
//! Each assembly is decoded exactly once. Every `TypeDef`, `MethodDef` and
//! `Field` becomes a registry entry up front, and IL bodies are copied out of
//! the image, so nothing after load needs to re-parse metadata. Token
//! resolution afterwards is a hash lookup.
//!
//! Types that RustBCL implements natively (`System.Object`, `System.String`,
//! `System.Console`, …) are pre-registered in a synthetic assembly, and
//! `TypeRef`s into the framework bind there. That is how a C# program keeps
//! working without CoreLib: the *contract* is unchanged, the implementation is
//! Rust.

use crate::error::{ExecResult, ExecutionError};
use crate::naming::{native_key, native_key_typed};
use crate::types::*;
use crate::value::Value;
use rustclr_metadata::{
    method_attributes, method_impl_attributes, Image, MethodSig, SignatureParser, TableId, Token,
    TypeSig,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The static slot holding the cached `Default` instance of a comparer.
///
/// A record's compiler-generated `Equals` calls `EqualityComparer<T>.Default`
/// once per field, so the instance is allocated once and kept here rather than
/// reallocated on every comparison.
pub const DEFAULT_COMPARER_FIELD: &str = "s_default";

/// The framework types the runtime itself needs to reference.
#[derive(Debug, Clone, Copy)]
pub struct CoreTypes {
    pub object: TypeId,
    pub value_type: TypeId,
    pub enum_type: TypeId,
    pub string: TypeId,
    pub array: TypeId,
    pub void: TypeId,
    pub delegate: TypeId,
    pub multicast_delegate: TypeId,
    pub exception: TypeId,
    pub type_handle: TypeId,
}

/// One loaded assembly.
pub struct LoadedAssembly {
    pub id: AssemblyId,
    pub name: String,
    pub version: String,
    pub path: Option<PathBuf>,
    /// Copy of the `#US` heap, so `ldstr` needs no live metadata borrow.
    pub user_strings: Vec<u8>,
    /// `TypeDef` row to registry id.
    pub type_by_row: HashMap<u32, TypeId>,
    /// `MethodDef` row to registry id.
    pub method_by_row: HashMap<u32, MethodId>,
    /// `Field` row to registry id.
    pub field_by_row: HashMap<u32, FieldId>,
    /// Resolved `TypeRef` rows.
    pub type_ref_by_row: HashMap<u32, TypeId>,
    /// Resolved `MemberRef` rows.
    pub member_ref_by_row: HashMap<u32, MethodId>,
    /// `MemberRef` rows that name a field rather than a method.
    pub field_ref_by_row: HashMap<u32, FieldId>,
    /// Resolved `MethodSpec` rows — a generic instantiation mapped to the open
    /// definition, because this build erases generics.
    pub method_spec_by_row: HashMap<u32, MethodId>,
    /// `MemberRef` rows that could not be resolved, mapped to `Type::Member`.
    ///
    /// These never become methods, so nothing downstream can see them — which
    /// is exactly why they are recorded here. Keyed by row so a caller can ask
    /// whether any IL actually reaches one: an attribute constructor is
    /// referenced but never executed, and reporting it would be noise.
    pub unresolved_members: HashMap<u32, String>,
    /// Explicit interface implementations: `(implementing type, interface
    /// method)` to the method that implements it.
    ///
    /// A C# explicit implementation is emitted under a mangled name —
    /// `System.Collections.Generic.IEnumerable<int>.GetEnumerator` — so
    /// dispatch cannot find it by matching name and shape. The `MethodImpl`
    /// table is the only thing that records the link.
    pub method_impls: HashMap<(TypeId, MethodId), MethodId>,
    /// Signature blobs of `TypeSpec` rows, kept for generic instantiation.
    pub type_specs: HashMap<u32, TypeSig>,
    /// Initial data for fields with an RVA (array initialisers).
    pub field_data: HashMap<u32, Vec<u8>>,
    /// Entry point token, if this assembly is executable.
    pub entry_point: Option<Token>,
    /// Names of the assemblies this one references.
    pub references: Vec<String>,
}

/// Resolves assemblies and owns the type registry.
pub struct Loader {
    pub registry: TypeRegistry,
    assemblies: Vec<LoadedAssembly>,
    by_name: HashMap<String, AssemblyId>,
    search_paths: Vec<PathBuf>,
    primitives: HashMap<Primitive, TypeId>,
    core: CoreTypes,
    /// Synthetic assembly holding the natively implemented framework types.
    bcl: AssemblyId,
    /// Static-field storage, indexed by `FieldId`.
    statics: Vec<Value>,
    /// Internal-call stubs synthesised for interface dispatch onto native
    /// types, keyed by the receiver type and the native binding name.
    interned_calls: HashMap<(TypeId, String), MethodId>,
}

impl Loader {
    /// Creates a loader with the framework contract pre-registered.
    pub fn new() -> Self {
        let mut loader = Loader {
            registry: TypeRegistry::new(),
            assemblies: Vec::new(),
            by_name: HashMap::new(),
            search_paths: Vec::new(),
            primitives: HashMap::new(),
            // Placeholder; replaced by `install_core_types` below.
            core: CoreTypes {
                object: TypeId::INVALID,
                value_type: TypeId::INVALID,
                enum_type: TypeId::INVALID,
                string: TypeId::INVALID,
                array: TypeId::INVALID,
                void: TypeId::INVALID,
                delegate: TypeId::INVALID,
                multicast_delegate: TypeId::INVALID,
                exception: TypeId::INVALID,
                type_handle: TypeId::INVALID,
            },
            bcl: AssemblyId(0),
            statics: Vec::new(),
            interned_calls: HashMap::new(),
        };
        loader.install_core_types();
        loader
    }

    pub fn core(&self) -> CoreTypes {
        self.core
    }

    pub fn bcl_assembly(&self) -> AssemblyId {
        self.bcl
    }

    pub fn add_search_path(&mut self, path: impl Into<PathBuf>) {
        self.search_paths.push(path.into());
    }

    pub fn assemblies(&self) -> &[LoadedAssembly] {
        &self.assemblies
    }

    pub fn assembly(&self, id: AssemblyId) -> &LoadedAssembly {
        &self.assemblies[id.index()]
    }

    pub fn find_assembly(&self, name: &str) -> Option<AssemblyId> {
        self.by_name.get(name).copied()
    }

    pub fn primitive_type(&self, p: Primitive) -> TypeId {
        self.primitives[&p]
    }

    // -- static field storage ------------------------------------------------

    pub fn static_value(&self, field: FieldId) -> &Value {
        &self.statics[field.index()]
    }

    pub fn static_value_mut(&mut self, field: FieldId) -> &mut Value {
        &mut self.statics[field.index()]
    }

    fn ensure_static_slot(&mut self, field: FieldId) {
        if self.statics.len() <= field.index() {
            self.statics.resize(field.index() + 1, Value::Null);
        }
    }

    // -- core type installation ---------------------------------------------

    fn install_core_types(&mut self) {
        let bcl = AssemblyId(0);
        self.bcl = bcl;
        self.assemblies.push(LoadedAssembly {
            id: bcl,
            name: "RustBCL".into(),
            version: "0.1.0.0".into(),
            path: None,
            user_strings: Vec::new(),
            type_by_row: HashMap::new(),
            method_by_row: HashMap::new(),
            field_by_row: HashMap::new(),
            type_ref_by_row: HashMap::new(),
            member_ref_by_row: HashMap::new(),
            field_ref_by_row: HashMap::new(),
            method_spec_by_row: HashMap::new(),
            unresolved_members: HashMap::new(),
            method_impls: HashMap::new(),
            type_specs: HashMap::new(),
            field_data: HashMap::new(),
            entry_point: None,
            references: Vec::new(),
        });
        self.by_name.insert("RustBCL".into(), bcl);

        // Synthetic tokens keep the (assembly, token) key unique.
        let mut next_token = 1u32;
        let mut synth = move || {
            let t = Token::new(TableId::TypeDef, next_token);
            next_token += 1;
            t
        };

        let object = self.add_native_type("System", "Object", TypeKind::Class, None, synth());
        self.core.object = object;

        let value_type =
            self.add_native_type("System", "ValueType", TypeKind::Class, Some(object), synth());
        let enum_type =
            self.add_native_type("System", "Enum", TypeKind::Class, Some(value_type), synth());
        let string =
            self.add_native_type("System", "String", TypeKind::String, Some(object), synth());
        let array =
            self.add_native_type("System", "Array", TypeKind::Class, Some(object), synth());
        let void =
            self.add_native_type("System", "Void", TypeKind::Primitive, Some(value_type), synth());
        let delegate =
            self.add_native_type("System", "Delegate", TypeKind::Class, Some(object), synth());
        let multicast = self.add_native_type(
            "System",
            "MulticastDelegate",
            TypeKind::Delegate,
            Some(delegate),
            synth(),
        );
        let exception =
            self.add_native_type("System", "Exception", TypeKind::Class, Some(object), synth());
        let type_handle = self.add_native_type(
            "System",
            "RuntimeTypeHandle",
            TypeKind::ValueType,
            Some(value_type),
            synth(),
        );

        self.core = CoreTypes {
            object,
            value_type,
            enum_type,
            string,
            array,
            void,
            delegate,
            multicast_delegate: multicast,
            exception,
            type_handle,
        };

        // Primitives, each a value type deriving from System.ValueType.
        for p in [
            Primitive::Boolean,
            Primitive::Char,
            Primitive::SByte,
            Primitive::Byte,
            Primitive::Int16,
            Primitive::UInt16,
            Primitive::Int32,
            Primitive::UInt32,
            Primitive::Int64,
            Primitive::UInt64,
            Primitive::Single,
            Primitive::Double,
            Primitive::IntPtr,
            Primitive::UIntPtr,
        ] {
            let full = p.full_name();
            let (ns, name) = full.rsplit_once('.').unwrap_or(("", full));
            let id = self.add_native_type(ns, name, TypeKind::Primitive, Some(value_type), synth());
            self.registry.ty_mut(id).primitive = Some(p);
            self.primitives.insert(p, id);
        }
        self.primitives.insert(Primitive::Void, void);
        self.registry.ty_mut(void).primitive = Some(Primitive::Void);

        // Exception types the runtime raises itself.
        for name in [
            "NullReferenceException",
            "IndexOutOfRangeException",
            "InvalidCastException",
            "DivideByZeroException",
            "OverflowException",
            "OutOfMemoryException",
            "StackOverflowException",
            "ArgumentException",
            "ArgumentNullException",
            "ArgumentOutOfRangeException",
            "InvalidOperationException",
            "NotSupportedException",
            "NotImplementedException",
            "TypeLoadException",
            "MissingMethodException",
            "MissingFieldException",
            "EntryPointNotFoundException",
            "DllNotFoundException",
            "ArithmeticException",
            "FormatException",
            "SystemException",
            "ApplicationException",
        ] {
            self.add_native_type("System", name, TypeKind::Class, Some(exception), synth());
        }
        self.add_native_type("System.IO", "IOException", TypeKind::Class, Some(exception), synth());

        // Non-generic framework delegates. `new Thread(() => …)` allocates a
        // `ThreadStart` first, so the delegate type has to resolve before the
        // thread does.
        for (namespace_name, name) in [
            ("System.Threading", "ThreadStart"),
            ("System.Threading", "ParameterizedThreadStart"),
            ("System.Threading", "WaitCallback"),
            ("System", "Action"),
            ("System", "EventHandler"),
        ] {
            self.add_native_type(
                namespace_name,
                name,
                TypeKind::Delegate,
                Some(multicast),
                synth(),
            );
        }

        // Framework interfaces. `using` compiles to a `callvirt` on
        // `IDisposable::Dispose`, so the interface must resolve even though the
        // implementation always lives on the user's type.
        for name in [
            "IDisposable",
            "IAsyncDisposable",
            "IFormattable",
            "ICloneable",
            "IComparable",
        ] {
            self.add_native_type("System", name, TypeKind::Interface, None, synth());
        }

        // Framework types RustBCL implements natively. They carry no fields
        // here; their behaviour lives in the native method table.
        for (ns, name) in [
            ("System", "Console"),
            ("System", "Math"),
            ("System", "Convert"),
            ("System", "Environment"),
            ("System", "DateTime"),
            ("System", "TimeSpan"),
            ("System", "Guid"),
            ("System", "Random"),
            ("System", "Type"),
            ("System", "Activator"),
            ("System", "GC"),
            ("System", "Buffer"),
            ("System.Text", "StringBuilder"),
            ("System.Text", "Encoding"),
            ("System.Diagnostics", "Stopwatch"),
            ("System.Diagnostics", "Debug"),
            ("System.Threading", "Thread"),
            ("System.Threading", "Monitor"),
            ("System.Threading", "Interlocked"),
            ("System.IO", "File"),
            ("System.IO", "Directory"),
            ("System.IO", "Path"),
            ("System.Runtime.CompilerServices", "RuntimeHelpers"),
            ("System", "OperatingSystem"),
            ("System.Threading", "ThreadPool"),
            // The type C# 10+ compiles every interpolated string through.
            ("System.Runtime.CompilerServices", "DefaultInterpolatedStringHandler"),
            // Ranges and indices: `a[^1]` and `a[1..4]`.
            ("System", "Index"),
            ("System", "Range"),
        ] {
            let kind = if matches!(
                name,
                "DateTime"
                    | "TimeSpan"
                    | "Guid"
                    | "DefaultInterpolatedStringHandler"
                    | "Index"
                    | "Range"
            ) {
                TypeKind::ValueType
            } else {
                TypeKind::Class
            };
            let base = if kind == TypeKind::ValueType { value_type } else { object };
            self.add_native_type(ns, name, kind, Some(base), synth());
        }

        // Tuples. `(3, 9)` compiles to `ValueTuple`2`, whose elements are read
        // with `ldfld Item1` / `ldfld Item2` — so these need real field slots.
        for arity in 1..=8usize {
            let names: Vec<String> = (1..=arity).map(|i| format!("Item{i}")).collect();
            let borrowed: Vec<&str> = names.iter().map(String::as_str).collect();
            self.add_native_type_with_fields(
                "System",
                &format!("ValueTuple`{arity}"),
                Some(value_type),
                synth(),
                &borrowed,
                bcl,
            );
        }
        self.add_native_type("System", "ValueTuple", TypeKind::ValueType, Some(value_type), synth());

        // `int?` and friends. The layout mirrors the real one: a flag and a
        // payload, so `initobj` zeroes it to "no value" exactly as .NET does.
        self.add_native_type_with_fields(
            "System",
            "Nullable`1",
            Some(value_type),
            synth(),
            &["hasValue", "value"],
            bcl,
        );

        self.register_generic_collections(&mut synth, object, bcl);
    }

    /// Registers the generic collection surface.
    ///
    /// These are the types almost every real C# program reaches for, and none
    /// of them can be *managed* code here: their bodies live in CoreLib, which
    /// this runtime does not load. So each is a native type whose storage is an
    /// ordinary managed array — which means the collector traces the elements
    /// with no special case, and a `List<int>` costs one `Value` per element
    /// rather than a boxed object.
    ///
    /// Generic arity is recorded but not instantiated: `List<int>` and
    /// `List<string>` share one `RuntimeType`. That is sound *here* because the
    /// storage is `Value`, which already carries its own shape — an `I32` slot
    /// and an `Obj` slot are distinguishable without a type argument.
    fn register_generic_collections(
        &mut self,
        synth: &mut impl FnMut() -> Token,
        object: TypeId,
        bcl: AssemblyId,
    ) {
        const GENERIC: &str = "System.Collections.Generic";

        // Interfaces. A `foreach` over an `IEnumerable<T>`-typed variable, and
        // the `IEqualityComparer<T>` a generated record `Equals` reaches for,
        // both need the interface itself to resolve before dispatch can run.
        for (ns, name, arity) in [
            (GENERIC, "IEnumerable`1", 1),
            (GENERIC, "IEnumerator`1", 1),
            (GENERIC, "ICollection`1", 1),
            (GENERIC, "IList`1", 1),
            (GENERIC, "IReadOnlyCollection`1", 1),
            (GENERIC, "IReadOnlyList`1", 1),
            (GENERIC, "ISet`1", 1),
            (GENERIC, "IDictionary`2", 2),
            (GENERIC, "IReadOnlyDictionary`2", 2),
            (GENERIC, "IEqualityComparer`1", 1),
            (GENERIC, "IComparer`1", 1),
            ("System.Collections", "IEnumerable", 0),
            ("System.Collections", "IEnumerator", 0),
            ("System.Collections", "ICollection", 0),
            ("System.Collections", "IList", 0),
            ("System.Collections", "IStructuralEquatable", 0),
            ("System", "IEquatable`1", 1),
        ] {
            let id = self.add_native_type(ns, name, TypeKind::Interface, None, synth());
            self.registry.ty_mut(id).generic_param_count = arity;
        }

        // The collections themselves, each with the field slots its native
        // implementation stores through.
        //
        // `Dictionary` and `HashSet` carry a bucket table and a chain array
        // beside their entries so lookup is a hash probe, not a scan. Keeping
        // those as managed `int[]`s costs nothing and keeps every byte of a
        // collection's state on the traced heap.
        for (name, arity, fields) in [
            ("List`1", 1u32, &["_items"][..]),
            ("Queue`1", 1, &["_items"]),
            ("Stack`1", 1, &["_items"]),
            ("HashSet`1", 1, &["_buckets", "_next", "_items"]),
            ("Dictionary`2", 2, &["_buckets", "_next", "_keys", "_values"]),
        ] {
            let id = self.add_native_type_with_fields_of_kind(
                GENERIC,
                name,
                TypeKind::Class,
                Some(object),
                synth(),
                fields,
                bcl,
            );
            self.registry.ty_mut(id).generic_param_count = arity;
        }

        // `Dictionary<K,V>.Keys` and `.Values` are views, not copies: each
        // holds only its source, so a change to the dictionary is visible
        // through a view already taken — as it is on .NET.
        for name in ["Dictionary`2+KeyCollection", "Dictionary`2+ValueCollection"] {
            let id = self.add_native_type_with_fields_of_kind(
                GENERIC,
                name,
                TypeKind::Class,
                Some(object),
                synth(),
                &["_source"],
                bcl,
            );
            self.registry.ty_mut(id).generic_param_count = 2;
        }

        // Enumerators. On .NET each of these is a mutable struct, and `foreach`
        // holds it in a local it takes the address of. Here they are classes:
        // the local then holds a reference, `ldloca` produces a managed pointer
        // to that local, and the native `MoveNext` writes its position to the
        // heap object rather than back through the pointer. The observable
        // behaviour is the same for `foreach`, which is the only way C# lets
        // you obtain one.
        for owner in [
            "List`1",
            "Queue`1",
            "Stack`1",
            "HashSet`1",
            "Dictionary`2",
            "Dictionary`2+KeyCollection",
            "Dictionary`2+ValueCollection",
        ] {
            self.add_native_type_with_fields_of_kind(
                GENERIC,
                &format!("{owner}+Enumerator"),
                TypeKind::Class,
                Some(object),
                synth(),
                &["_source", "_index", "_current"],
                bcl,
            );
        }

        // `KeyValuePair<K,V>` is likewise a class here. `foreach` over a
        // dictionary assigns it with `stloc` and reads it through `get_Key` and
        // `get_Value`, none of which can tell the difference.
        let kvp = self.add_native_type_with_fields_of_kind(
            GENERIC,
            "KeyValuePair`2",
            TypeKind::Class,
            Some(object),
            synth(),
            &["key", "value"],
            bcl,
        );
        self.registry.ty_mut(kvp).generic_param_count = 2;

        // The default comparers. A record's compiler-generated `Equals` calls
        // `EqualityComparer<T>.Default` once per field, so the instance is
        // cached in a static slot rather than reallocated on every comparison.
        for (name, arity) in [("EqualityComparer`1", 1u32), ("Comparer`1", 1)] {
            let id = self.add_native_type(GENERIC, name, TypeKind::Class, Some(object), synth());
            self.registry.ty_mut(id).generic_param_count = arity;
            self.add_native_static_field(id, DEFAULT_COMPARER_FIELD, synth());
        }

        self.register_linq(synth, object);
        let value_type = self.core.value_type;
        self.register_tasks(synth, object, value_type);
    }

    /// Registers the `Task` and `async`/`await` surface.
    ///
    /// An `async` method is not special to the runtime: Roslyn lowers it to an
    /// ordinary state-machine type plus calls into a *builder*. Supporting
    /// `await` therefore means implementing that builder, the awaiter it hands
    /// out, and `Task` itself — after which the IL runs like any other.
    fn register_tasks(
        &mut self,
        synth: &mut impl FnMut() -> Token,
        object: TypeId,
        value_type: TypeId,
    ) {
        const TASKS: &str = "System.Threading.Tasks";
        const CS: &str = "System.Runtime.CompilerServices";

        // The interfaces the lowering references. A state machine implements
        // `IAsyncStateMachine` explicitly, which is why `MethodImpl` has to be
        // read for `MoveNext` to be reachable.
        for (ns, name) in [
            (CS, "IAsyncStateMachine"),
            (CS, "INotifyCompletion"),
            (CS, "ICriticalNotifyCompletion"),
            ("System.Threading.Tasks.Sources", "IValueTaskSource"),
        ] {
            self.add_native_type(ns, name, TypeKind::Interface, None, synth());
        }

        // `Task` carries its own completion state. Continuations are held in a
        // managed array so a suspended state machine stays reachable to the
        // collector for exactly as long as the task that will resume it.
        let task_fields = &["_status", "_result", "_exception", "_continuations"];
        let task = self.add_native_type_with_fields_of_kind(
            TASKS,
            "Task",
            TypeKind::Class,
            Some(object),
            synth(),
            task_fields,
            self.bcl,
        );
        // `Task<T>` derives from `Task`, so a `Task<int>` assigned to a `Task`
        // variable passes the cast checks the compiler emits.
        let generic_task = self.add_native_type_with_fields_of_kind(
            TASKS,
            "Task`1",
            TypeKind::Class,
            Some(task),
            synth(),
            &[],
            self.bcl,
        );
        self.registry.ty_mut(generic_task).generic_param_count = 1;

        for (name, arity) in [("TaskCompletionSource", 0u32), ("TaskCompletionSource`1", 1)] {
            let id = self.add_native_type_with_fields_of_kind(
                TASKS,
                name,
                TypeKind::Class,
                Some(object),
                synth(),
                &["_task"],
                self.bcl,
            );
            self.registry.ty_mut(id).generic_param_count = arity;
        }

        for (name, arity) in [("ValueTask", 0u32), ("ValueTask`1", 1)] {
            let id = self.add_native_type(TASKS, name, TypeKind::ValueType, Some(value_type), synth());
            self.registry.ty_mut(id).generic_param_count = arity;
        }
        self.add_native_type(TASKS, "TaskFactory", TypeKind::Class, Some(object), synth());
        self.add_native_type(TASKS, "Parallel", TypeKind::Class, Some(object), synth());
        self.add_native_type(
            TASKS,
            "TaskCanceledException",
            TypeKind::Class,
            Some(self.core.exception),
            synth(),
        );
        self.add_native_type(
            "System",
            "AggregateException",
            TypeKind::Class,
            Some(self.core.exception),
            synth(),
        );
        self.add_native_type(
            "System.Threading",
            "CancellationToken",
            TypeKind::ValueType,
            Some(value_type),
            synth(),
        );
        self.add_native_type(
            "System.Threading",
            "CancellationTokenSource",
            TypeKind::Class,
            Some(object),
            synth(),
        );

        // Builders and awaiters. Each is a value type holding a single slot,
        // which the native implementations write the task handle into — the
        // same shape `DefaultInterpolatedStringHandler` uses.
        for (name, arity) in [
            ("AsyncTaskMethodBuilder", 0u32),
            ("AsyncTaskMethodBuilder`1", 1),
            ("AsyncVoidMethodBuilder", 0),
            ("AsyncValueTaskMethodBuilder", 0),
            ("AsyncValueTaskMethodBuilder`1", 1),
            ("TaskAwaiter", 0),
            ("TaskAwaiter`1", 1),
            ("ConfiguredTaskAwaitable", 0),
            ("ConfiguredTaskAwaitable`1", 1),
            ("ConfiguredTaskAwaitable+ConfiguredTaskAwaiter", 0),
            ("ConfiguredTaskAwaitable`1+ConfiguredTaskAwaiter", 1),
            ("ConfiguredValueTaskAwaitable", 0),
            ("ConfiguredValueTaskAwaitable`1", 1),
            ("YieldAwaitable", 0),
            ("YieldAwaitable+YieldAwaiter", 0),
        ] {
            let id =
                self.add_native_type(CS, name, TypeKind::ValueType, Some(value_type), synth());
            self.registry.ty_mut(id).generic_param_count = arity;
        }
    }

    /// Registers the LINQ surface.
    ///
    /// `Enumerable` holds the operators; `Func` and `Action` are the delegate
    /// shapes lambdas compile to; `Grouping` and `OrderedEnumerable` are the
    /// concrete results this runtime returns from `GroupBy` and `OrderBy`,
    /// standing in for the compiler-invisible types .NET returns.
    fn register_linq(&mut self, synth: &mut impl FnMut() -> Token, object: TypeId) {
        let multicast = self.core.multicast_delegate;

        self.add_native_type("System.Linq", "Enumerable", TypeKind::Class, Some(object), synth());
        self.add_native_type("System.Linq", "Queryable", TypeKind::Class, Some(object), synth());

        // A lambda compiles to `newobj Func`n::.ctor(object, native int)`, so
        // the delegate type has to resolve before the lambda can be built.
        for arity in 1..=9usize {
            let id = self.add_native_type(
                "System",
                &format!("Func`{arity}"),
                TypeKind::Delegate,
                Some(multicast),
                synth(),
            );
            self.registry.ty_mut(id).generic_param_count = arity as u32;
        }
        for arity in 1..=8usize {
            let id = self.add_native_type(
                "System",
                &format!("Action`{arity}"),
                TypeKind::Delegate,
                Some(multicast),
                synth(),
            );
            self.registry.ty_mut(id).generic_param_count = arity as u32;
        }
        for arity in 1..=3usize {
            let id = self.add_native_type(
                "System",
                &format!("Predicate`{arity}"),
                TypeKind::Delegate,
                Some(multicast),
                synth(),
            );
            self.registry.ty_mut(id).generic_param_count = arity as u32;
        }
        for (name, arity) in [("Comparison`1", 1u32), ("Converter`2", 2)] {
            let id =
                self.add_native_type("System", name, TypeKind::Delegate, Some(multicast), synth());
            self.registry.ty_mut(id).generic_param_count = arity;
        }

        for (name, arity) in [("IGrouping`2", 2u32), ("IOrderedEnumerable`1", 1), ("ILookup`2", 2)] {
            let id = self.add_native_type("System.Linq", name, TypeKind::Interface, None, synth());
            self.registry.ty_mut(id).generic_param_count = arity;
        }

        let grouping = self.add_native_type_with_fields_of_kind(
            "System.Linq",
            "Grouping`2",
            TypeKind::Class,
            Some(object),
            synth(),
            &["_key", "_items"],
            self.bcl,
        );
        self.registry.ty_mut(grouping).generic_param_count = 2;

        // Ordering keys are kept per level rather than folded into one sort, so
        // `ThenBy` refines the previous ordering instead of replacing it.
        let ordered = self.add_native_type_with_fields_of_kind(
            "System.Linq",
            "OrderedEnumerable`1",
            TypeKind::Class,
            Some(object),
            synth(),
            &["_items", "_levels", "_descending"],
            self.bcl,
        );
        self.registry.ty_mut(ordered).generic_param_count = 1;
    }

    /// Adds a static field to a natively implemented type, with storage.
    fn add_native_static_field(&mut self, type_id: TypeId, name: &str, token: Token) {
        let field_id = self.registry.add_field(
            FieldInfo {
                id: FieldId::INVALID,
                name: name.to_string(),
                declaring_type: type_id,
                token,
                signature: TypeSig::Object,
                field_type: TypeId::INVALID,
                is_static: true,
                is_literal: false,
                slot: 0,
                offset: None,
                constant: None,
            },
            self.bcl,
        );
        self.ensure_static_slot(field_id);
        self.registry.ty_mut(type_id).static_fields.push(field_id);
    }

    /// Registers a native value type that carries instance fields.
    ///
    /// Most framework types this runtime implements are behaviour-only, but a
    /// few — tuples above all — are read with `ldfld`, which needs real field
    /// slots to resolve against.
    fn add_native_type_with_fields(
        &mut self,
        namespace: &str,
        name: &str,
        base: Option<TypeId>,
        token: Token,
        field_names: &[&str],
        assembly: AssemblyId,
    ) -> TypeId {
        self.add_native_type_with_fields_of_kind(
            namespace,
            name,
            TypeKind::ValueType,
            base,
            token,
            field_names,
            assembly,
        )
    }

    /// As [`Self::add_native_type_with_fields`], for a type that is not a
    /// value type — the generic collections are classes with backing storage.
    #[allow(clippy::too_many_arguments)]
    fn add_native_type_with_fields_of_kind(
        &mut self,
        namespace: &str,
        name: &str,
        kind: TypeKind,
        base: Option<TypeId>,
        token: Token,
        field_names: &[&str],
        assembly: AssemblyId,
    ) -> TypeId {
        let type_id = self.add_native_type(namespace, name, kind, base, token);

        for (slot, field_name) in field_names.iter().enumerate() {
            let field_token = Token::new(TableId::Field, token.row() * 16 + slot as u32 + 1);
            let field_id = self.registry.add_field(
                FieldInfo {
                    id: FieldId::INVALID,
                    name: (*field_name).to_string(),
                    declaring_type: type_id,
                    token: field_token,
                    // Erased: a tuple element can hold anything.
                    signature: TypeSig::Object,
                    field_type: TypeId::INVALID,
                    is_static: false,
                    is_literal: false,
                    slot: slot as u32,
                    offset: None,
                    constant: None,
                },
                assembly,
            );
            self.registry.ty_mut(type_id).instance_fields.push(field_id);
        }
        type_id
    }

    fn add_native_type(
        &mut self,
        namespace: &str,
        name: &str,
        kind: TypeKind,
        base: Option<TypeId>,
        token: Token,
    ) -> TypeId {
        self.registry.add_type(RuntimeType {
            id: TypeId::INVALID,
            name: name.into(),
            namespace: namespace.into(),
            assembly: self.bcl,
            token,
            kind,
            base,
            interfaces: Vec::new(),
            instance_fields: Vec::new(),
            static_fields: Vec::new(),
            methods: Vec::new(),
            vtable: Vec::new(),
            element_type: None,
            underlying: None,
            primitive: None,
            is_abstract: false,
            is_sealed: kind.is_value_like() || kind == TypeKind::String,
            generic_param_count: 0,
            generic_args: Vec::new(),
            generic_definition: None,
            cctor: None,
            cctor_state: CctorState::Done,
            explicit_size: None,
            packing_size: None,
        })
    }

    // -- assembly loading ----------------------------------------------------

    /// Loads an assembly from disk, or returns the id if already loaded.
    pub fn load_from_file(&mut self, path: impl AsRef<Path>) -> ExecResult<AssemblyId> {
        let path = path.as_ref();
        let image = Image::from_file(path).map_err(ExecutionError::Metadata)?;
        self.load_image(image)
    }

    /// Loads an already-parsed image.
    pub fn load_image(&mut self, image: Image) -> ExecResult<AssemblyId> {
        let name = image.assembly_name();
        if let Some(existing) = self.by_name.get(&name) {
            return Ok(*existing);
        }

        let id = AssemblyId(self.assemblies.len() as u32);
        let md = image.metadata();
        let pe = image.pe();
        let bytes = image.bytes();

        let version = if md.row_count(TableId::Assembly) > 0 {
            md.assembly(1).map(|a| a.version_string()).unwrap_or_default()
        } else {
            String::new()
        };

        let references: Vec<String> = (1..=md.row_count(TableId::AssemblyRef))
            .filter_map(|r| md.assembly_ref(r).ok().map(|a| a.name.to_string()))
            .collect();

        let mut assembly = LoadedAssembly {
            id,
            name: name.clone(),
            version,
            path: image.path().map(|p| p.to_path_buf()),
            user_strings: md.user_strings.0.to_vec(),
            type_by_row: HashMap::new(),
            method_by_row: HashMap::new(),
            field_by_row: HashMap::new(),
            type_ref_by_row: HashMap::new(),
            member_ref_by_row: HashMap::new(),
            field_ref_by_row: HashMap::new(),
            method_spec_by_row: HashMap::new(),
            unresolved_members: HashMap::new(),
            method_impls: HashMap::new(),
            type_specs: HashMap::new(),
            field_data: HashMap::new(),
            entry_point: image.entry_point(),
            references,
        };

        // --- pass 1: create a shell for every TypeDef ------------------------
        // `<Module>` (row 1) is a container for global functions, not a type.
        let type_count = md.row_count(TableId::TypeDef);
        for row in 1..=type_count {
            let td = md.type_def(row).map_err(ExecutionError::Metadata)?;
            let kind = if td.is_interface() { TypeKind::Interface } else { TypeKind::Class };
            let type_id = self.registry.add_type(RuntimeType {
                id: TypeId::INVALID,
                name: td.name.to_string(),
                namespace: td.namespace.to_string(),
                assembly: id,
                token: Token::new(TableId::TypeDef, row),
                kind,
                base: None,
                interfaces: Vec::new(),
                instance_fields: Vec::new(),
                static_fields: Vec::new(),
                methods: Vec::new(),
                vtable: Vec::new(),
                element_type: None,
                underlying: None,
                primitive: None,
                is_abstract: td.is_abstract(),
                is_sealed: td.is_sealed(),
                generic_param_count: 0,
                generic_args: Vec::new(),
                generic_definition: None,
                cctor: None,
                cctor_state: CctorState::NotRun,
                explicit_size: None,
                packing_size: None,
            });
            assembly.type_by_row.insert(row, type_id);
        }

        // --- pass 2: resolve TypeRefs ----------------------------------------
        for row in 1..=md.row_count(TableId::TypeRef) {
            let full = type_ref_full_name(&md, row).map_err(ExecutionError::Metadata)?;
            // Framework types bind to RustBCL; anything else must already be
            // loaded, or is recorded as unresolved and reported on first use.
            if let Some(target) = self.registry.find_type_by_name(&full) {
                assembly.type_ref_by_row.insert(row, target);
            }
        }

        // --- pass 3: generic parameter counts --------------------------------
        let mut generic_counts: HashMap<u32, u32> = HashMap::new();
        for row in 1..=md.row_count(TableId::GenericParam) {
            let gp = md.generic_param(row).map_err(ExecutionError::Metadata)?;
            if gp.owner.table() == Some(TableId::TypeDef) {
                let e = generic_counts.entry(gp.owner.row()).or_insert(0);
                *e = (*e).max(gp.number as u32 + 1);
            }
        }

        // --- pass 4: fields --------------------------------------------------
        let mut field_rvas: HashMap<u32, u32> = HashMap::new();
        for row in 1..=md.row_count(TableId::FieldRva) {
            let fr = md.field_rva(row).map_err(ExecutionError::Metadata)?;
            field_rvas.insert(fr.field.row(), fr.rva);
        }

        let mut constants: HashMap<u32, (u8, Vec<u8>)> = HashMap::new();
        for row in 1..=md.row_count(TableId::Constant) {
            let c = md.constant(row).map_err(ExecutionError::Metadata)?;
            if c.parent.table() == Some(TableId::Field) {
                constants.insert(c.parent.row(), (c.element_type, c.value.to_vec()));
            }
        }

        for type_row in 1..=type_count {
            let type_id = assembly.type_by_row[&type_row];
            let range = md.fields_of(type_row).map_err(ExecutionError::Metadata)?;
            let mut instance_slot = 0u32;
            let mut static_slot = 0u32;

            for field_row in range {
                if field_row > md.row_count(TableId::Field) {
                    break;
                }
                let f = md.field(field_row).map_err(ExecutionError::Metadata)?;
                let sig = SignatureParser::new(f.signature)
                    .parse_field()
                    .map_err(ExecutionError::Metadata)?;

                let is_static = f.is_static();
                let slot = if is_static {
                    let s = static_slot;
                    static_slot += 1;
                    s
                } else {
                    let s = instance_slot;
                    instance_slot += 1;
                    s
                };

                let constant = constants
                    .get(&field_row)
                    .and_then(|(et, bytes)| decode_constant(*et, bytes));

                let field_id = self.registry.add_field(
                    FieldInfo {
                        id: FieldId::INVALID,
                        name: f.name.to_string(),
                        declaring_type: type_id,
                        token: Token::new(TableId::Field, field_row),
                        signature: sig,
                        field_type: TypeId::INVALID,
                        is_static,
                        is_literal: f.is_literal(),
                        slot,
                        offset: None,
                        constant,
                    },
                    id,
                );
                assembly.field_by_row.insert(field_row, field_id);

                if let Some(rva) = field_rvas.get(&field_row) {
                    // Array-initialiser bytes live in the image, not the heap.
                    // Metadata does not record their length, so keep the rest
                    // of the containing section and let the consumer take the
                    // prefix it needs. Fields with an RVA are rare — one per
                    // array literal — so this costs little.
                    if let Ok(slice) = pe.slice_from_rva(*rva) {
                        assembly.field_data.insert(field_row, slice.to_vec());
                    }
                }

                if is_static {
                    self.ensure_static_slot(field_id);
                    if let Some(v) = self.registry.field(field_id).constant.clone() {
                        *self.static_value_mut(field_id) = v;
                    }
                }

                let ty = self.registry.ty_mut(type_id);
                if is_static {
                    ty.static_fields.push(field_id);
                } else {
                    ty.instance_fields.push(field_id);
                }
            }
        }

        // --- pass 5: methods --------------------------------------------------
        let mut pinvokes: HashMap<u32, (String, String, u16)> = HashMap::new();
        for row in 1..=md.row_count(TableId::ImplMap) {
            let im = md.impl_map(row).map_err(ExecutionError::Metadata)?;
            if im.member_forwarded.table() == Some(TableId::MethodDef) {
                let library = md
                    .module_ref(im.import_scope.row())
                    .map(|m| m.name.to_string())
                    .unwrap_or_default();
                pinvokes.insert(
                    im.member_forwarded.row(),
                    (library, im.import_name.to_string(), im.mapping_flags),
                );
            }
        }

        for type_row in 1..=type_count {
            let type_id = assembly.type_by_row[&type_row];
            let declaring_name = self.registry.ty(type_id).full_name();
            let range = md.methods_of(type_row).map_err(ExecutionError::Metadata)?;

            for method_row in range {
                if method_row > md.row_count(TableId::MethodDef) {
                    break;
                }
                let m = md.method_def(method_row).map_err(ExecutionError::Metadata)?;
                let sig = SignatureParser::new(m.signature)
                    .parse_method()
                    .map_err(ExecutionError::Metadata)?;

                let kind = if m.is_pinvoke() {
                    match pinvokes.get(&method_row) {
                        Some((lib, entry, flags)) => MethodKind::PInvoke {
                            library: lib.clone(),
                            entry_point: if entry.is_empty() { m.name.to_string() } else { entry.clone() },
                            flags: *flags,
                        },
                        None => MethodKind::PInvoke {
                            library: String::new(),
                            entry_point: m.name.to_string(),
                            flags: 0,
                        },
                    }
                } else if m.is_internal_call() {
                    MethodKind::InternalCall
                } else if m.is_runtime_provided() {
                    MethodKind::RuntimeProvided
                } else if m.is_abstract() || m.rva == 0 {
                    MethodKind::Abstract
                } else {
                    let offset = pe.rva_to_offset(m.rva).map_err(ExecutionError::Metadata)?;
                    let body = rustclr_metadata::MethodBody::parse(&bytes[offset..])
                        .map_err(ExecutionError::Metadata)?;

                    let locals = if body.local_var_sig_token.is_null() {
                        Vec::new()
                    } else {
                        let blob = md
                            .stand_alone_sig(body.local_var_sig_token.row())
                            .map_err(ExecutionError::Metadata)?;
                        SignatureParser::new(blob.signature)
                            .parse_locals()
                            .map_err(ExecutionError::Metadata)?
                            .locals
                    };

                    MethodKind::Il(Box::new(IlBody {
                        il: body.il.to_vec(),
                        max_stack: body.max_stack,
                        locals,
                        init_locals: body.init_locals,
                        exception_clauses: body.exception_clauses.clone(),
                    }))
                };

                let method_id = self.registry.add_method(MethodInfo {
                    id: MethodId::INVALID,
                    name: m.name.to_string(),
                    declaring_type: type_id,
                    token: Token::new(TableId::MethodDef, method_row),
                    assembly: id,
                    qualified_name: native_key_typed(&declaring_name, m.name, &sig),
                    signature: sig,
                    flags: m.flags,
                    impl_flags: m.impl_flags,
                    kind,
                    vtable_slot: None,
                });
                assembly.method_by_row.insert(method_row, method_id);
                self.registry.ty_mut(type_id).methods.push(method_id);

                if m.name == ".cctor" {
                    self.registry.ty_mut(type_id).cctor = Some(method_id);
                }
            }

            if let Some(count) = generic_counts.get(&type_row) {
                self.registry.ty_mut(type_id).generic_param_count = *count;
            }
        }

        // --- pass 6: base types, interfaces, layout ---------------------------
        for type_row in 1..=type_count {
            let type_id = assembly.type_by_row[&type_row];
            let td = md.type_def(type_row).map_err(ExecutionError::Metadata)?;

            let base = if td.extends.is_null() {
                None
            } else {
                self.resolve_type_token(&assembly, td.extends)
            };

            // A type extending System.ValueType is a struct; extending
            // System.Enum makes it an enum.
            let kind = match base {
                Some(b) if b == self.core.enum_type => TypeKind::Enum,
                Some(b) if b == self.core.value_type => TypeKind::ValueType,
                Some(b) if b == self.core.multicast_delegate || b == self.core.delegate => {
                    TypeKind::Delegate
                }
                _ => self.registry.ty(type_id).kind,
            };

            let ty = self.registry.ty_mut(type_id);
            ty.base = base.or(if type_row == 1 { None } else { Some(self.core.object) });
            ty.kind = kind;
        }

        for row in 1..=md.row_count(TableId::InterfaceImpl) {
            let ii = md.interface_impl(row).map_err(ExecutionError::Metadata)?;
            let Some(class) = assembly.type_by_row.get(&ii.class.row()).copied() else { continue };
            if let Some(iface) = self.resolve_type_token(&assembly, ii.interface) {
                self.registry.ty_mut(class).interfaces.push(iface);
            }
        }

        for row in 1..=md.row_count(TableId::ClassLayout) {
            let cl = md.class_layout(row).map_err(ExecutionError::Metadata)?;
            if let Some(t) = assembly.type_by_row.get(&cl.parent.row()).copied() {
                let ty = self.registry.ty_mut(t);
                ty.explicit_size = Some(cl.class_size);
                ty.packing_size = Some(cl.packing_size);
            }
        }

        // --- pass 7: TypeSpec signatures --------------------------------------
        for row in 1..=md.row_count(TableId::TypeSpec) {
            let ts = md.type_spec(row).map_err(ExecutionError::Metadata)?;
            if let Ok(sig) = SignatureParser::new(ts.signature).parse_type_spec() {
                assembly.type_specs.insert(row, sig);
            }
        }

        // --- pass 8: MemberRefs -----------------------------------------------
        for row in 1..=md.row_count(TableId::MemberRef) {
            let mr = md.member_ref(row).map_err(ExecutionError::Metadata)?;
            let Some(parent) = self.resolve_type_token(&assembly, mr.class) else {
                // The declaring type is not loaded, so this reference cannot be
                // bound. Record it: a caller reaching it at run time gets
                // "could not resolve token", and `verify` should have said so
                // first.
                let scope = describe_member_scope(&md, mr.class);
                assembly
                    .unresolved_members
                    .insert(row, format!("{scope}::{}", mr.name));
                continue;
            };

            // A member ref signature starting with FIELD names a field.
            if mr.signature.first().copied() == Some(0x06) {
                if let Some(f) = self.registry.find_instance_field(parent, mr.name) {
                    assembly.field_ref_by_row.insert(row, f);
                } else if let Some(f) = self
                    .registry
                    .ty(parent)
                    .static_fields
                    .iter()
                    .find(|f| self.registry.field(**f).name == mr.name)
                    .copied()
                {
                    assembly.field_ref_by_row.insert(row, f);
                }
                continue;
            }

            let Ok(sig) = SignatureParser::new(mr.signature).parse_method() else { continue };
            let resolved = self.find_method_on_type(parent, mr.name, &sig);
            match resolved {
                Some(m) => {
                    assembly.member_ref_by_row.insert(row, m);
                }
                None => {
                    // Framework member with no managed body: synthesise an
                    // internal-call stub that RustBCL will service by name.
                    let declaring_name = self.registry.ty(parent).full_name();
                    let is_static_guess = !sig.has_this;
                    let flags = if is_static_guess { method_attributes::STATIC } else { 0 };
                    let method_id = self.registry.add_method(MethodInfo {
                        id: MethodId::INVALID,
                        name: mr.name.to_string(),
                        declaring_type: parent,
                        token: Token::new(TableId::MemberRef, row),
                        assembly: id,
                        qualified_name: native_key_typed(&declaring_name, mr.name, &sig),
                        signature: sig,
                        flags,
                        impl_flags: method_impl_attributes::INTERNAL_CALL,
                        kind: MethodKind::InternalCall,
                        vtable_slot: None,
                    });
                    assembly.member_ref_by_row.insert(row, method_id);
                }
            }
        }

        // --- pass 9: MethodSpecs ----------------------------------------------
        // A `MethodSpec` names a generic method plus its type arguments. With
        // generics erased there is one implementation, so each spec resolves to
        // the open definition. Without this, every call to a generic method —
        // including the `AppendFormatted<T>` behind string interpolation — fails
        // to resolve.
        for row in 1..=md.row_count(TableId::MethodSpec) {
            let spec = md.method_spec(row).map_err(ExecutionError::Metadata)?;
            let target = match spec.method.table() {
                Some(TableId::MethodDef) => assembly.method_by_row.get(&spec.method.row()).copied(),
                Some(TableId::MemberRef) => {
                    assembly.member_ref_by_row.get(&spec.method.row()).copied()
                }
                _ => None,
            };
            let Some(method) = target else { continue };

            // Give the instantiation its own entry, named after the concrete
            // type arguments. Generics are still erased for execution — the
            // body is shared — but the *name* now distinguishes
            // `AppendFormatted<bool>` from `AppendFormatted<int>`, which is
            // what lets a native implementation render `True` rather than `1`.
            let specialised = match SignatureParser::new(spec.instantiation).parse_method_spec() {
                Ok(arguments) if !arguments.is_empty() => {
                    let base = self.registry.method(method);
                    let declaring = self.registry.ty(base.declaring_type).full_name();
                    let concrete = MethodSig {
                        params: base
                            .signature
                            .params
                            .iter()
                            .map(|p| substitute_method_generics(p, &arguments))
                            .collect(),
                        ..base.signature.clone()
                    };
                    let mut clone = base.clone();
                    clone.qualified_name = native_key_typed(&declaring, &base.name, &concrete);
                    Some(self.registry.add_method(clone))
                }
                _ => None,
            };

            assembly.method_spec_by_row.insert(row, specialised.unwrap_or(method));
        }

        // --- pass 10: explicit interface implementations ----------------------
        // Without this, a type that implements an interface explicitly — every
        // `yield return` state machine, among others — appears to implement
        // nothing, because the body's name is mangled.
        for row in 1..=md.row_count(TableId::MethodImpl) {
            let mi = md.method_impl(row).map_err(ExecutionError::Metadata)?;
            let Some(class) = assembly.type_by_row.get(&mi.class.row()).copied() else { continue };
            let body = match mi.body.table() {
                Some(TableId::MethodDef) => assembly.method_by_row.get(&mi.body.row()).copied(),
                Some(TableId::MemberRef) => {
                    assembly.member_ref_by_row.get(&mi.body.row()).copied()
                }
                _ => None,
            };
            let declaration = match mi.declaration.table() {
                Some(TableId::MethodDef) => {
                    assembly.method_by_row.get(&mi.declaration.row()).copied()
                }
                Some(TableId::MemberRef) => {
                    assembly.member_ref_by_row.get(&mi.declaration.row()).copied()
                }
                _ => None,
            };
            if let (Some(body), Some(declaration)) = (body, declaration) {
                assembly.method_impls.insert((class, declaration), body);
            }
        }

        self.by_name.insert(name, id);
        self.assemblies.push(assembly);

        // Vtables need every type in the assembly present, so build them last.
        self.build_vtables(id);
        Ok(id)
    }

    /// Lays out virtual dispatch tables for every type in an assembly.
    ///
    /// A derived type inherits its base's slots, then either overrides an
    /// inherited slot (matching name and signature, without `newslot`) or
    /// appends a new one.
    fn build_vtables(&mut self, assembly: AssemblyId) {
        let type_ids: Vec<TypeId> = self
            .assemblies[assembly.index()]
            .type_by_row
            .values()
            .copied()
            .collect();

        // Base types must be laid out first.
        let mut ordered = type_ids.clone();
        ordered.sort_by_key(|id| self.registry.base_chain(*id).count());

        for type_id in ordered {
            let base_vtable = match self.registry.ty(type_id).base {
                Some(b) => self.registry.ty(b).vtable.clone(),
                None => Vec::new(),
            };
            let mut vtable = base_vtable;
            let methods = self.registry.ty(type_id).methods.clone();

            for method_id in methods {
                let m = self.registry.method(method_id);
                if !m.is_virtual() {
                    continue;
                }
                let is_newslot = m.flags & method_attributes::NEW_SLOT != 0;
                let name = m.name.clone();
                let sig = m.signature.clone();

                let existing = if is_newslot {
                    None
                } else {
                    vtable.iter().position(|slot| {
                        let s = self.registry.method(*slot);
                        s.name == name && signatures_match(&s.signature, &sig)
                    })
                };

                let slot = match existing {
                    Some(index) => {
                        vtable[index] = method_id;
                        index
                    }
                    None => {
                        vtable.push(method_id);
                        vtable.len() - 1
                    }
                };
                self.registry.method_mut(method_id).vtable_slot = Some(slot as u32);
            }

            self.registry.ty_mut(type_id).vtable = vtable;
        }
    }

    /// Finds a method declared on `ty` or inherited, matching name and shape.
    pub fn find_method_on_type(
        &self,
        ty: TypeId,
        name: &str,
        sig: &MethodSig,
    ) -> Option<MethodId> {
        for id in self.registry.base_chain(ty) {
            for m in &self.registry.ty(id).methods {
                let info = self.registry.method(*m);
                if info.name == name && signatures_match(&info.signature, sig) {
                    return Some(*m);
                }
            }
        }
        None
    }

    /// Resolves a `TypeDefOrRef`-shaped token within an assembly.
    pub fn resolve_type_token(&self, assembly: &LoadedAssembly, token: Token) -> Option<TypeId> {
        match token.table() {
            Some(TableId::TypeDef) => assembly.type_by_row.get(&token.row()).copied(),
            Some(TableId::TypeRef) => assembly.type_ref_by_row.get(&token.row()).copied(),
            Some(TableId::TypeSpec) => {
                let sig = assembly.type_specs.get(&token.row())?;
                self.resolve_type_sig(assembly, sig)
            }
            _ => None,
        }
    }

    /// Maps a signature type onto a loaded runtime type.
    pub fn resolve_type_sig(
        &self,
        assembly: &LoadedAssembly,
        sig: &TypeSig,
    ) -> Option<TypeId> {
        Some(match sig.unwrap_modifiers() {
            TypeSig::Void => self.core.void,
            TypeSig::Boolean => self.primitives[&Primitive::Boolean],
            TypeSig::Char => self.primitives[&Primitive::Char],
            TypeSig::I1 => self.primitives[&Primitive::SByte],
            TypeSig::U1 => self.primitives[&Primitive::Byte],
            TypeSig::I2 => self.primitives[&Primitive::Int16],
            TypeSig::U2 => self.primitives[&Primitive::UInt16],
            TypeSig::I4 => self.primitives[&Primitive::Int32],
            TypeSig::U4 => self.primitives[&Primitive::UInt32],
            TypeSig::I8 => self.primitives[&Primitive::Int64],
            TypeSig::U8 => self.primitives[&Primitive::UInt64],
            TypeSig::R4 => self.primitives[&Primitive::Single],
            TypeSig::R8 => self.primitives[&Primitive::Double],
            TypeSig::IntPtr => self.primitives[&Primitive::IntPtr],
            TypeSig::UIntPtr => self.primitives[&Primitive::UIntPtr],
            TypeSig::String => self.core.string,
            TypeSig::Object => self.core.object,
            TypeSig::TypedByRef => self.core.object,
            TypeSig::ValueType(t) | TypeSig::Class(t) => self.resolve_type_token(assembly, *t)?,
            TypeSig::SzArray(_) | TypeSig::Array { .. } => self.core.array,
            TypeSig::ByRef(inner) | TypeSig::Ptr(inner) => {
                self.resolve_type_sig(assembly, inner)?
            }
            TypeSig::GenericInst { definition, .. } => {
                self.resolve_type_token(assembly, *definition)?
            }
            // Generic parameters are erased to object in this build.
            TypeSig::Var(_) | TypeSig::MVar(_) => self.core.object,
            TypeSig::FnPtr(_) => self.primitives[&Primitive::IntPtr],
            TypeSig::Modified { .. } | TypeSig::Pinned(_) => return None,
        })
    }

    /// The explicit implementation of `declared` on `type_id`, if there is one.
    ///
    /// Searched across every loaded assembly because the implementing type and
    /// the interface need not come from the same one.
    pub fn explicit_implementation(
        &self,
        type_id: TypeId,
        declared: MethodId,
    ) -> Option<MethodId> {
        for base in self.registry.base_chain(type_id) {
            for assembly in &self.assemblies {
                if let Some(m) = assembly.method_impls.get(&(base, declared)) {
                    return Some(*m);
                }
            }
        }
        None
    }

    /// Binds `name` on `type_id` to a native implementation, on demand.
    ///
    /// Interface dispatch onto a natively implemented type has no managed
    /// method to find: `List<T>` carries no `MethodDef` for
    /// `IEnumerable<T>::GetEnumerator`, because it carries no managed body at
    /// all. Rather than let the call fall back to the interface's own stub —
    /// which would run the wrong implementation, or none — this synthesises the
    /// missing link on first use and caches it, so the second call is a lookup.
    pub fn intern_internal_call(
        &mut self,
        type_id: TypeId,
        name: &str,
        signature: MethodSig,
        qualified_name: String,
    ) -> MethodId {
        let cache_key = (type_id, qualified_name.clone());
        if let Some(id) = self.interned_calls.get(&cache_key) {
            return *id;
        }
        let flags = if signature.has_this { 0 } else { method_attributes::STATIC };
        let token = Token::new(TableId::MemberRef, self.registry.method_count() as u32 + 1);
        let id = self.registry.add_method(MethodInfo {
            id: MethodId::INVALID,
            name: name.to_string(),
            declaring_type: type_id,
            token,
            assembly: self.bcl,
            qualified_name,
            signature,
            flags,
            impl_flags: method_impl_attributes::INTERNAL_CALL,
            kind: MethodKind::InternalCall,
            vtable_slot: None,
        });
        self.interned_calls.insert(cache_key, id);
        id
    }

    /// Resolves a method token (`MethodDef`, `MemberRef` or `MethodSpec`).
    pub fn resolve_method_token(
        &self,
        assembly: &LoadedAssembly,
        token: Token,
    ) -> Option<MethodId> {
        match token.table() {
            Some(TableId::MethodDef) => assembly.method_by_row.get(&token.row()).copied(),
            Some(TableId::MemberRef) => assembly.member_ref_by_row.get(&token.row()).copied(),
            // Generic instantiation is erased: dispatch to the open definition.
            Some(TableId::MethodSpec) => assembly.method_spec_by_row.get(&token.row()).copied(),
            _ => None,
        }
    }

    /// Resolves a field token (`Field` or `MemberRef`).
    pub fn resolve_field_token(&self, assembly: &LoadedAssembly, token: Token) -> Option<FieldId> {
        match token.table() {
            Some(TableId::Field) => assembly.field_by_row.get(&token.row()).copied(),
            Some(TableId::MemberRef) => assembly.field_ref_by_row.get(&token.row()).copied(),
            _ => None,
        }
    }

    /// The native binding keys to try for a method, most specific first.
    ///
    /// The first key is the method's recorded qualified name, which for a
    /// generic instantiation names the concrete type arguments — so
    /// `AppendFormatted<bool>` can bind to a different implementation than
    /// `AppendFormatted<int>` even though generics are otherwise erased.
    pub fn native_keys(&self, method: MethodId) -> [String; 2] {
        let m = self.registry.method(method);
        let declaring = self.registry.ty(m.declaring_type).full_name();
        [
            m.qualified_name.clone(),
            native_key(&declaring, &m.name, &m.signature),
        ]
    }

    /// Total static-field slots allocated.
    pub fn static_slot_count(&self) -> usize {
        self.statics.len()
    }

    /// Every static field value, for GC root scanning.
    pub fn static_values(&self) -> &[Value] {
        &self.statics
    }

    /// Initial bytes of a field that carries an RVA.
    ///
    /// Roslyn compiles `new int[] { 1, 2, 3 }` into a blob in the image plus a
    /// call to `RuntimeHelpers.InitializeArray`; this is where that blob lives.
    pub fn field_initial_data(&self, assembly: AssemblyId, token: Token) -> Option<&[u8]> {
        self.assemblies
            .get(assembly.index())?
            .field_data
            .get(&token.row())
            .map(|v| v.as_slice())
    }
}

impl Default for Loader {
    fn default() -> Self {
        Self::new()
    }
}

/// Structural signature comparison for override and overload matching.
///
/// Parameter *names* and custom modifiers are irrelevant; arity, `this`-ness
/// and the shape of each parameter are what identify a method.
fn signatures_match(a: &MethodSig, b: &MethodSig) -> bool {
    if a.params.len() != b.params.len() || a.has_this != b.has_this {
        return false;
    }
    a.params
        .iter()
        .zip(&b.params)
        .all(|(x, y)| type_sigs_match(x.unwrap_modifiers(), y.unwrap_modifiers()))
}

/// Compares two signature types, treating generic parameters as wildcards
/// because this build erases generics.
fn type_sigs_match(a: &TypeSig, b: &TypeSig) -> bool {
    use TypeSig::*;
    match (a, b) {
        (Var(_) | MVar(_), _) | (_, Var(_) | MVar(_)) => true,
        (SzArray(x), SzArray(y)) => type_sigs_match(x.unwrap_modifiers(), y.unwrap_modifiers()),
        (ByRef(x), ByRef(y)) => type_sigs_match(x.unwrap_modifiers(), y.unwrap_modifiers()),
        (Ptr(x), Ptr(y)) => type_sigs_match(x.unwrap_modifiers(), y.unwrap_modifiers()),
        (GenericInst { definition: d1, .. }, GenericInst { definition: d2, .. }) => d1 == d2,
        _ => a == b,
    }
}

/// The name a `TypeRef` row binds under, nesting included.
///
/// A nested type carries an empty namespace and names its enclosing type
/// through `ResolutionScope`, so `List`1/Enumerator` arrives as a bare
/// `Enumerator` — which would collide with every other `Enumerator` in every
/// other collection. Walking the scope produces `Outer+Nested`, matching the
/// name the runtime registers native nested types under.
fn type_ref_full_name(
    md: &rustclr_metadata::Metadata<'_>,
    row: u32,
) -> rustclr_metadata::Result<String> {
    let tr = md.type_ref(row)?;
    if tr.resolution_scope.table() == Some(TableId::TypeRef) {
        // Bounded by the row count: a scope chain cannot be longer than the
        // table, and a cyclic one would otherwise spin here forever.
        let mut names = vec![tr.name.to_string()];
        let mut scope = tr.resolution_scope;
        for _ in 0..md.row_count(TableId::TypeRef) {
            let Some(TableId::TypeRef) = scope.table() else { break };
            let outer = md.type_ref(scope.row())?;
            names.push(outer.full_name());
            if outer.resolution_scope.table() != Some(TableId::TypeRef) {
                break;
            }
            scope = outer.resolution_scope;
        }
        names.reverse();
        return Ok(names.join("+"));
    }
    Ok(tr.full_name())
}

/// Names the declaring scope of an unresolvable member reference.
///
/// The point is a message a person can act on, so a `TypeRef` is rendered by
/// name rather than as a raw token, and a generic instantiation by the name of
/// its open definition — `List`1` says what is missing, `<generic
/// instantiation>` does not.
fn describe_member_scope(md: &rustclr_metadata::Metadata<'_>, class: Token) -> String {
    match class.table() {
        Some(TableId::TypeRef) => {
            type_ref_full_name(md, class.row()).unwrap_or_else(|_| class.to_string())
        }
        Some(TableId::TypeDef) => md
            .type_def(class.row())
            .map(|t| t.full_name())
            .unwrap_or_else(|_| class.to_string()),
        Some(TableId::TypeSpec) => md
            .type_spec(class.row())
            .ok()
            .and_then(|ts| SignatureParser::new(ts.signature).parse_type_spec().ok())
            .map(|sig| match sig {
                TypeSig::GenericInst { definition, .. } => describe_member_scope(md, definition),
                other => format!("{other:?}"),
            })
            .unwrap_or_else(|| "<generic instantiation>".to_string()),
        _ => class.to_string(),
    }
}

/// Replaces `!!n` method type parameters with the instantiation's arguments.
///
/// Only method generics are substituted; `!n` type parameters belong to the
/// declaring type and are left alone.
fn substitute_method_generics(sig: &TypeSig, arguments: &[TypeSig]) -> TypeSig {
    match sig {
        TypeSig::MVar(index) => arguments
            .get(*index as usize)
            .cloned()
            .unwrap_or_else(|| sig.clone()),
        TypeSig::SzArray(inner) => {
            TypeSig::SzArray(Box::new(substitute_method_generics(inner, arguments)))
        }
        TypeSig::ByRef(inner) => {
            TypeSig::ByRef(Box::new(substitute_method_generics(inner, arguments)))
        }
        TypeSig::Ptr(inner) => {
            TypeSig::Ptr(Box::new(substitute_method_generics(inner, arguments)))
        }
        TypeSig::Modified { required, modifier, inner } => TypeSig::Modified {
            required: *required,
            modifier: *modifier,
            inner: Box::new(substitute_method_generics(inner, arguments)),
        },
        TypeSig::GenericInst { definition, is_value_type, args } => TypeSig::GenericInst {
            definition: *definition,
            is_value_type: *is_value_type,
            args: args
                .iter()
                .map(|a| substitute_method_generics(a, arguments))
                .collect(),
        },
        other => other.clone(),
    }
}

/// Decodes a `Constant` table blob into a runtime value.
fn decode_constant(element_type: u8, bytes: &[u8]) -> Option<Value> {
    use rustclr_metadata::signature::element_type as et;
    Some(match element_type {
        et::BOOLEAN => Value::I32(*bytes.first()? as i32),
        et::CHAR | et::U2 => Value::I32(u16::from_le_bytes([*bytes.first()?, *bytes.get(1)?]) as i32),
        et::I1 => Value::I32(*bytes.first()? as i8 as i32),
        et::U1 => Value::I32(*bytes.first()? as i32),
        et::I2 => Value::I32(i16::from_le_bytes([*bytes.first()?, *bytes.get(1)?]) as i32),
        et::I4 | et::U4 => {
            let mut b = [0u8; 4];
            b.copy_from_slice(bytes.get(..4)?);
            Value::I32(i32::from_le_bytes(b))
        }
        et::I8 | et::U8 => {
            let mut b = [0u8; 8];
            b.copy_from_slice(bytes.get(..8)?);
            Value::I64(i64::from_le_bytes(b))
        }
        et::R4 => {
            let mut b = [0u8; 4];
            b.copy_from_slice(bytes.get(..4)?);
            Value::F(f32::from_le_bytes(b) as f64)
        }
        et::R8 => {
            let mut b = [0u8; 8];
            b.copy_from_slice(bytes.get(..8)?);
            Value::F(f64::from_le_bytes(b))
        }
        // String constants are materialised on the heap at first use.
        _ => return None,
    })
}
