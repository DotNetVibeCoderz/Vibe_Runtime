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

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

/// The owned types every module here needs, from whichever crate provides them.
///
/// Same pattern as `rustclr-metadata`: a `use alloc::…` reaches only the file
/// it is written in, so every module imports this instead and the same source
/// compiles for a host and for a microcontroller.
///
/// Two of these are not simple re-exports, and the differences are real:
///
/// * `HashMap` is a `BTreeMap` without `std`. The runtime's maps are keyed by
///   small integer ids, tuples of them, or names — all `Ord` — so this costs
///   a different iteration order, which nothing depends on, and a
///   microcontroller has no business pulling in a hasher. Measured on the
///   host with every map swapped to a `BTreeMap`, a call-heavy workload ran
///   within noise of `std`'s `HashMap` once the per-call caches stopped
///   building `Vec`/`String` keys (and ~15% slower before) — so a hash table
///   was not worth a dependency. What did matter was allocation: see
///   `docs/limitations.md` on embedders with a slow allocator.
/// * `Arc` is an `Rc` without `std` -- unless the `threads` feature is on.
///   RISC-V `imc` has no atomics, so `Arc` does not exist there; the
///   interpreter is single-threaded on a chip, which is exactly when `Rc` is
///   the right type anyway. An embedder that supplies threads without `std`
///   (`rustclr-thread`'s hooks) needs the real `Arc`, since what it shares
///   crosses threads.
pub(crate) mod prelude {
    #[cfg(not(feature = "std"))]
    #[allow(unused_imports)]
    pub(crate) use alloc::{
        borrow::ToOwned,
        boxed::Box,
        collections::BTreeMap as HashMap,
        collections::VecDeque,
        format,
        string::{String, ToString},
        vec,
        vec::Vec,
    };
    #[cfg(all(not(feature = "std"), not(feature = "threads")))]
    #[allow(unused_imports)]
    pub(crate) use alloc::rc::Rc as Arc;
    #[cfg(all(not(feature = "std"), feature = "threads"))]
    #[allow(unused_imports)]
    pub(crate) use alloc::sync::Arc;
    #[cfg(feature = "std")]
    #[allow(unused_imports)]
    pub(crate) use std::{
        borrow::ToOwned,
        boxed::Box,
        collections::HashMap,
        collections::VecDeque,
        format,
        string::{String, ToString},
        sync::Arc,
        vec,
        vec::Vec,
    };
}

#[allow(unused_imports)]
use crate::prelude::*;

pub mod debug;
pub mod error;
pub mod host;
pub mod interp;
pub mod loader;
pub mod monitor;
#[cfg(feature = "threads")]
pub mod pool;
pub mod profile;
pub mod rowmap;
pub mod statics;
pub mod naming;
pub mod objects;
pub mod opcode;
pub mod types;
pub mod value;

pub use error::{ClrExceptionKind, ExecResult, ExecutionError};
pub use host::{CaptureHost, Host};
#[cfg(feature = "std")]
pub use host::SystemHost;
pub use interp::{
    CompiledMethod, DevicePoll, ExecutionStats, Frame, Interpreter, Limits, NativeFn, NativeTier,
};
pub use loader::{
    CoreTypes, CustomAttribute, LoadedAssembly, Loader, DEFAULT_COMPARER_FIELD,
};
pub use objects::{
    ArrayStorage, ClrArray, ClrBox, ClrDelegate, ClrException, ClrObject, ClrString, DelegateTarget,
};
pub use opcode::{decode, decode_all, Instruction, Op, Operand};
pub use types::{
    AssemblyId, CctorState, FieldId, FieldInfo, IlBody, MethodId, MethodInfo, MethodKind, Primitive,
    PropertyId, PropertyInfo, RuntimeType, TypeId, TypeKind, TypeRegistry,
};
pub use value::{ByRef, StructValue, Value, RawPtr};

pub use rustclr_gc as gc;
pub use rustclr_metadata as metadata;
