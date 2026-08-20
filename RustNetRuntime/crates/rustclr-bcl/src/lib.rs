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
