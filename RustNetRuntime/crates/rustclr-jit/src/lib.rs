//! # rustclr-jit
//!
//! The compilation seam between IL and native code.
//!
//! RustCLR executes IL through an interpreter today. This crate defines the
//! interface a JIT or AOT backend plugs into, and implements the analysis every
//! backend needs first: [`verify`] computes basic blocks, per-instruction stack
//! depth and the properties that decide whether a method is compilable or
//! inlinable.
//!
//! The tiering model is deliberate. [`Tier::Interpreted`] is always available;
//! a backend advertises which methods it can take by answering
//! [`Compiler::can_compile`], and the runtime falls back for the rest. That
//! means a partial backend is useful immediately rather than all-or-nothing.
//!
//! # Status
//!
//! [`X64Backend`] emits real x86-64 machine code into write-xor-execute pages
//! for leaf methods doing integer arithmetic — no calls, no allocation, no
//! exception handling, no floating point. [`JitTier`] applies it after a method
//! has been called enough times to be worth compiling, and everything it
//! declines keeps running in the interpreter.
//!
//! AArch64 and RISC-V backends do not exist yet. `can_compile` reports what is
//! actually handled rather than implying more.

pub mod aarch64;
pub mod codepage;
pub mod inline;
pub mod riscv64;
pub mod tier;
pub mod translate;
pub mod verify;
pub mod x64;

pub use codepage::{CodePage, CodePageError};
pub use verify::{analyse, MethodAnalysis, VerifyError, UNRESOLVED_CALL};
pub use aarch64::Arm64Backend;
pub use inline::inline_calls;
pub use riscv64::RiscVBackend;
pub use tier::JitTier;
pub use x64::{NativeMethod, X64Backend};

use rustclr_core::{Loader, MethodId, MethodKind, TypeRegistry};

/// How a method is executed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// Executed instruction by instruction.
    Interpreted,
    /// Compiled on first call.
    Jit,
    /// Compiled ahead of time and loaded from the image.
    Aot,
}

/// The result of compiling one method.
#[derive(Debug, Clone)]
pub struct CompiledCode {
    pub method: MethodId,
    pub tier: Tier,
    /// Emitted machine code, empty for the interpreted tier.
    pub bytes: Vec<u8>,
    pub analysis: MethodAnalysis,
}

/// Why a method could not be compiled by a backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompileError {
    /// The method has no IL to compile.
    NoBody,
    /// The IL did not verify.
    Invalid(VerifyError),
    /// The backend does not implement something the method uses.
    Unsupported(String),
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoBody => write!(f, "method has no IL body"),
            Self::Invalid(e) => write!(f, "IL verification failed: {e}"),
            Self::Unsupported(what) => write!(f, "backend does not support {what}"),
        }
    }
}

impl std::error::Error for CompileError {}

/// A code generator for the runtime.
pub trait Compiler: Send {
    fn name(&self) -> &'static str;

    fn tier(&self) -> Tier;

    /// Whether this backend can take the method. Returning `false` is normal:
    /// the runtime falls back to the interpreter.
    fn can_compile(&self, registry: &TypeRegistry, method: MethodId) -> bool;

    /// Analyses or compiles the method.
    ///
    /// The loader is needed, not just the registry: computing the stack effect
    /// of a `call` means resolving its token, and tokens are only meaningful
    /// relative to the assembly that emitted them.
    fn compile(
        &mut self,
        loader: &Loader,
        method: MethodId,
    ) -> Result<CompiledCode, CompileError>;
}

/// The always-available tier: analyse the method, then let the interpreter run
/// it. Useful on its own as a verification pass.
#[derive(Debug, Default)]
pub struct InterpreterTier {
    pub methods_analysed: usize,
}

impl InterpreterTier {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Compiler for InterpreterTier {
    fn name(&self) -> &'static str {
        "interpreter"
    }

    fn tier(&self) -> Tier {
        Tier::Interpreted
    }

    fn can_compile(&self, registry: &TypeRegistry, method: MethodId) -> bool {
        matches!(registry.method(method).kind, MethodKind::Il(_))
    }

    fn compile(
        &mut self,
        loader: &Loader,
        method: MethodId,
    ) -> Result<CompiledCode, CompileError> {
        let registry = &loader.registry;
        let info = registry.method(method);
        let MethodKind::Il(body) = &info.kind else {
            return Err(CompileError::NoBody);
        };
        let returns_value = !info.returns_void();
        let assembly = loader.assembly(info.assembly);

        let instructions =
            rustclr_core::opcode::decode_all(&body.il).map_err(|e| {
                CompileError::Unsupported(format!("undecodable IL: {e}"))
            })?;

        let analysis = analyse(
            &instructions,
            body.max_stack,
            &body.exception_clauses,
            |ins| call_stack_delta(loader, assembly, returns_value, ins),
        )
        .map_err(CompileError::Invalid)?;

        self.methods_analysed += 1;
        Ok(CompiledCode {
            method,
            tier: Tier::Interpreted,
            bytes: Vec::new(),
            analysis,
        })
    }
}

/// Net stack effect of a signature-dependent instruction.
///
/// This has to be exact. An approximation makes the verifier report problems
/// that are not there — and a verifier that cries wolf is worse than none,
/// because people stop reading it.
fn call_stack_delta(
    loader: &Loader,
    assembly: &rustclr_core::LoadedAssembly,
    enclosing_returns_value: bool,
    ins: &rustclr_core::opcode::Instruction,
) -> i32 {
    use rustclr_core::opcode::Op;

    match ins.op {
        // `ret` pops the return value only when the *enclosing* method has one.
        Op::Ret => {
            if enclosing_returns_value {
                -1
            } else {
                0
            }
        }
        Op::Jmp => 0,
        Op::Call | Op::Callvirt | Op::Newobj | Op::Calli => {
            let Some(token) = ins.operand.as_token() else { return 0 };
            let Some(callee) = loader.resolve_method_token(assembly, token) else {
                // Reported separately as an unresolved member. Signal it so the
                // analysis stops trusting the running depth rather than
                // inventing a stack error on top of a resolution failure.
                return verify::UNRESOLVED_CALL;
            };
            let info = loader.registry.method(callee);

            if ins.op == Op::Newobj {
                // Pops the constructor arguments but not `this`; pushes the
                // new instance.
                return 1 - info.signature.params.len() as i32;
            }

            let popped = info.arg_count() as i32;
            let pushed = if info.returns_void() { 0 } else { 1 };
            pushed - popped
        }
        _ => 0,
    }
}

/// Verifies every IL method the loader knows about, returning the failures.
///
/// This is what `rustnet verify` runs.
pub fn verify_all(loader: &Loader) -> Vec<(MethodId, CompileError)> {
    let mut tier = InterpreterTier::new();
    let mut failures = Vec::new();
    let methods: Vec<MethodId> = loader
        .registry
        .iter_methods()
        .filter(|m| matches!(m.kind, MethodKind::Il(_)))
        .map(|m| m.id)
        .collect();

    for method in methods {
        if let Err(e) = tier.compile(loader, method) {
            failures.push((method, e));
        }
    }
    failures
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_interpreter_tier_reports_itself_honestly() {
        let tier = InterpreterTier::new();
        assert_eq!(tier.tier(), Tier::Interpreted);
        assert_eq!(tier.name(), "interpreter");
    }

    #[test]
    fn compile_errors_render_usefully() {
        let e = CompileError::Invalid(VerifyError::FallsOffTheEnd);
        assert!(e.to_string().contains("verification failed"));
    }
}
