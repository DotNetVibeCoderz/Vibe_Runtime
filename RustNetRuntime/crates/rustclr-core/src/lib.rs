//! # rustclr-core
//!
//! The RustCLR execution engine: type system, assembly loader and IL
//! interpreter. This is the crate that replaces CoreCLR's `vm` directory.
//!
//! The pieces fit together like this:
//!
//! ```text
//!   .dll/.exe ──► rustclr-metadata ──► Loader ──► TypeRegistry
//!                                        │            │
//!                                        ▼            ▼
//!                                   Interpreter ── rustclr-gc (Heap)
//!                                        │
//!                                        ├─► native methods (rustclr-bcl)
//!                                        └─► P/Invoke      (rustclr-interop)
//! ```
//!
//! [`Loader`] decodes each assembly exactly once, materialising every type,
//! method and field into [`TypeRegistry`]. [`Interpreter`] then executes IL
//! against that registry, allocating on a [`rustclr_gc::Heap`].
//!
//! Framework types are *not* loaded from a managed CoreLib. They are
//! pre-registered as a contract, and their behaviour is supplied by native
//! functions registered with [`Interpreter::register_native`] — which is how a
//! C# program runs unchanged on a Rust runtime.

pub mod error;
pub mod host;
pub mod interp;
pub mod loader;
pub mod naming;
pub mod objects;
pub mod opcode;
pub mod types;
pub mod value;

pub use error::{ClrExceptionKind, ExecResult, ExecutionError};
pub use host::{CaptureHost, Host, SystemHost};
pub use interp::{CompiledMethod, ExecutionStats, Frame, Interpreter, Limits, NativeFn};
pub use loader::{CoreTypes, LoadedAssembly, Loader, DEFAULT_COMPARER_FIELD};
pub use objects::{
    ArrayStorage, ClrArray, ClrBox, ClrDelegate, ClrException, ClrObject, ClrString, DelegateTarget,
};
pub use opcode::{decode, decode_all, Instruction, Op, Operand};
pub use types::{
    AssemblyId, CctorState, FieldId, FieldInfo, IlBody, MethodId, MethodInfo, MethodKind, Primitive,
    RuntimeType, TypeId, TypeKind, TypeRegistry,
};
pub use value::{ByRef, StructValue, Value};

pub use rustclr_gc as gc;
pub use rustclr_metadata as metadata;
