//! # rustclr-bcl
//!
//! RustBCL: the base class library, implemented in Rust rather than managed
//! code.
//!
//! A C# program compiled against the real .NET reference assemblies emits
//! `MemberRef`s into `System.Runtime` — `System.Console::WriteLine(string)`,
//! `System.String::Concat(string, string)`, and so on. Those references carry
//! no implementation. RustCLR's loader turns each one into an internal-call
//! stub keyed by a canonical name, and this crate supplies the function behind
//! that key.
//!
//! The consequence is that the *contract* C# was compiled against is preserved
//! exactly, while the implementation underneath is Rust. That is what makes
//! "C# with a Rust runtime" work without recompiling user code.
//!
//! ```no_run
//! let mut interp = rustclr_core::Interpreter::new();
//! rustclr_bcl::install(&mut interp);
//! // `Console.WriteLine` now resolves.
//! ```

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

/// The owned types every module here needs, from whichever crate provides them.
pub(crate) mod prelude {
    #[cfg(not(feature = "std"))]
    #[allow(unused_imports)]
    pub(crate) use alloc::{
        borrow::ToOwned,
        boxed::Box,
        collections::BTreeMap as HashMap,
        format,
        string::{String, ToString},
        vec,
        vec::Vec,
    };
    #[cfg(feature = "std")]
    #[allow(unused_imports)]
    pub(crate) use std::{
        borrow::ToOwned,
        boxed::Box,
        collections::HashMap,
        format,
        string::{String, ToString},
        vec,
        vec::Vec,
    };
}

/// Float maths, from `std` or from `libm`.
///
/// `System.Math` is largely a libm, and `core` has none of it — no `sqrt`, no
/// `sin`, not even `abs` for `f64`. On a host these forward to the standard
/// library's intrinsics; without `std` they forward to `libm`, which computes
/// the same values in software. A board with no FPU pays for that in cycles,
/// not in accuracy.
#[allow(dead_code)]
pub(crate) mod fmath {
    macro_rules! unary {
        ($($name:ident),* $(,)?) => {$(
            #[cfg(feature = "std")]
            #[inline]
            pub fn $name(x: f64) -> f64 { f64::$name(x) }
        )*};
    }
    unary!(sqrt, sin, cos, tan, asin, acos, atan, exp, log10, log2, floor, ceil, trunc, cbrt, round);

    #[cfg(feature = "std")]
    #[inline]
    pub fn ln(x: f64) -> f64 { f64::ln(x) }
    #[cfg(feature = "std")]
    #[inline]
    pub fn abs(x: f64) -> f64 { f64::abs(x) }
    #[cfg(feature = "std")]
    #[inline]
    pub fn powf(x: f64, y: f64) -> f64 { f64::powf(x, y) }
    #[cfg(feature = "std")]
    #[inline]
    pub fn atan2(y: f64, x: f64) -> f64 { f64::atan2(y, x) }

    #[cfg(not(feature = "std"))]
    pub use libm::{
        acos, asin, atan, atan2, cbrt, ceil, cos, exp, floor, log10, log2, pow as powf, round, sin,
        sqrt, tan, trunc,
    };
    #[cfg(not(feature = "std"))]
    pub use libm::{fabs as abs, log as ln};

    /// The fractional part, which `libm` spells `modf` and returns as a pair.
    #[inline]
    pub fn fract(x: f64) -> f64 {
        x - trunc(x)
    }

    /// The two `f32` operations `format_single` needs.
    ///
    /// Only these two: widening to `f64` and back is exact for both, so there
    /// is no reason for a parallel `f32` shim beyond what is used.
    #[inline]
    pub fn trunc_f32(x: f32) -> f32 {
        trunc(x as f64) as f32
    }
    #[inline]
    pub fn abs_f32(x: f32) -> f32 {
        abs(x as f64) as f32
    }

    /// `x` raised to an integer power. Present because `f64::powi` has no
    /// `libm` equivalent — libm is a C library and C has no `powi`.
    #[inline]
    pub fn powi(x: f64, n: i32) -> f64 {
        powf(x, n as f64)
    }
}

pub mod collections;
pub mod console;
pub mod interpolation;
pub mod linq;
pub mod numerics;
pub mod ranges;
pub mod reflection;
pub mod runtime;
pub mod strings;
pub mod tasks;
pub mod threading;
pub mod support;

use rustclr_core::Interpreter;

#[allow(unused_imports)]
use crate::prelude::*;

/// Registers every native BCL implementation on an interpreter.
pub fn install(interp: &mut Interpreter) {
    runtime::register(interp);
    console::register(interp);
    interpolation::register(interp);
    ranges::register(interp);
    strings::register(interp);
    numerics::register(interp);
    threading::register(interp);
    collections::register(interp);
    linq::register(interp);
    tasks::register(interp);
    reflection::register(interp);
}

/// Registers the bindings a console program needs, and no more.
///
/// `Console`, `String`, `Math`, interpolated strings and the object/runtime
/// basics — 314 bindings against [`install`]'s 821. No generic collections, no
/// LINQ, no reflection, no tasks.
///
/// This exists for boards that cannot hold the whole binding table. Registering
/// all of RustBCL costs 260,702 bytes of peak allocation and this subset costs
/// 192,045, which on a part with 192 KB of RAM is the difference between
/// running a program and not running one.
///
/// It is also what `rustnet run --bcl minimal` installs, so a program can be
/// checked against a small board's limits on a desktop, before it is flashed.
pub fn install_minimal(interp: &mut Interpreter) {
    runtime::register(interp);
    console::register(interp);
    interpolation::register(interp);
    strings::register(interp);
    numerics::register(interp);
}

/// The number of native bindings [`install`] provides.
pub fn binding_count() -> usize {
    let mut probe = Interpreter::with_host(Box::new(rustclr_core::CaptureHost::new()));
    install(&mut probe);
    probe.native_count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_registers_the_methods_hello_world_needs() {
        let mut interp = Interpreter::with_host(Box::new(rustclr_core::CaptureHost::new()));
        install(&mut interp);

        for key in [
            "System.Console::WriteLine(string)",
            "System.Console::WriteLine(int)",
            "System.Object::.ctor()",
            "System.String::Concat(string,string)",
            "System.Math::Sqrt(double)",
            "System.Int32::Parse(string)",
        ] {
            assert!(interp.has_native(key), "missing native binding: {key}");
        }
    }

    #[test]
    fn installing_twice_is_idempotent() {
        let mut interp = Interpreter::with_host(Box::new(rustclr_core::CaptureHost::new()));
        install(&mut interp);
        let first = interp.native_count();
        install(&mut interp);
        assert_eq!(interp.native_count(), first);
    }
}
