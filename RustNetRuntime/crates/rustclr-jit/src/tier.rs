//! Tiering: interpret first, compile what turns out to be hot.
//!
//! Compiling costs time and memory, and most methods in a program run once.
//! So nothing is compiled up front. Each call to a compilable method increments
//! a counter, and the [`JitTier::threshold`]th call triggers compilation; from
//! then on the method runs as machine code.
//!
//! A method the backend declines is recorded once and never reconsidered, so
//! the cost of declining is a hash lookup rather than a repeated analysis.

use crate::x64::{NativeMethod, X64Backend};
use crate::Compiler;
use rustclr_core::{Loader, MethodId, NativeTier, Value};
use rustclr_metadata::TypeSig;
use std::collections::{HashMap, HashSet};

/// The interpret-then-compile policy.
pub struct JitTier {
    backend: X64Backend,
    /// Calls before a method is compiled.
    pub threshold: u32,
    counts: HashMap<MethodId, u32>,
    compiled: HashMap<MethodId, NativeMethod>,
    /// Methods the backend cannot take, so they are not analysed again.
    declined: HashSet<MethodId>,
}

impl Default for JitTier {
    fn default() -> Self {
        Self::new()
    }
}

impl JitTier {
    pub fn new() -> Self {
        Self::with_threshold(32)
    }

    /// A threshold of zero compiles on the first call, which is what the
    /// tests and `rustnet jit` want.
    pub fn with_threshold(threshold: u32) -> Self {
        Self {
            backend: X64Backend::new(),
            threshold,
            counts: HashMap::new(),
            compiled: HashMap::new(),
            declined: HashSet::new(),
        }
    }

    pub fn compiled_count(&self) -> usize {
        self.compiled.len()
    }

    pub fn declined_count(&self) -> usize {
        self.declined.len()
    }

    /// Compiles `method` now, whatever its call count.
    ///
    /// Returns `false` when the backend declines it — which is not an error.
    pub fn compile_now(&mut self, loader: &Loader, method: MethodId) -> bool {
        if self.compiled.contains_key(&method) {
            return true;
        }
        if self.declined.contains(&method) {
            return false;
        }
        if !self.backend.can_compile(&loader.registry, method) {
            self.declined.insert(method);
            return false;
        }
        match self.backend.compile_native(loader, method) {
            Ok(native) => {
                self.compiled.insert(method, native);
                true
            }
            Err(_) => {
                // A method that passes `can_compile` but fails to emit is a
                // backend gap, not a program error: record it and interpret.
                self.declined.insert(method);
                false
            }
        }
    }
}

impl NativeTier for JitTier {
    fn name(&self) -> &'static str {
        "x86-64 baseline (tiered)"
    }

    fn try_execute(
        &mut self,
        loader: &Loader,
        method: MethodId,
        args: &[Value],
    ) -> Option<Option<Value>> {
        if self.declined.contains(&method) {
            return None;
        }

        if !self.compiled.contains_key(&method) {
            let count = self.counts.entry(method).or_insert(0);
            *count += 1;
            if *count < self.threshold.max(1) {
                return None;
            }
            if !self.compile_now(loader, method) {
                return None;
            }
        }

        let native = self.compiled.get(&method)?;
        let mut marshalled = Vec::with_capacity(native.arg_count.max(1));
        for i in 0..native.arg_count {
            // Anything that is not an integer means the caller and the compiled
            // signature disagree, which `can_compile` should have prevented.
            marshalled.push(as_i64(args.get(i)?)?);
        }
        // The emitted code always reads `arg_count` slots; give it a slot even
        // when there are no arguments so the pointer is never dangling.
        if marshalled.is_empty() {
            marshalled.push(0);
        }

        // SAFETY: `marshalled` holds at least `arg_count` values, which is
        // exactly what the emitted prologue reads.
        let raw = unsafe { native.call(&marshalled) };

        if !native.returns_value {
            return Some(None);
        }
        let info = loader.registry.method(method);
        Some(Some(widen(&info.signature.return_type, raw)))
    }

    fn stats(&self) -> (usize, usize) {
        (self.backend.methods_compiled, self.backend.bytes_emitted)
    }
}

/// An evaluation-stack value as a machine word.
fn as_i64(v: &Value) -> Option<i64> {
    match v {
        Value::I32(n) => Some(*n as i64),
        Value::I64(n) | Value::NativeInt(n) => Some(*n),
        _ => None,
    }
}

/// Rebuilds the evaluation-stack value a return type calls for.
///
/// The stack type matters: an `int` result must come back as `Value::I32`, or
/// the very next `add` would promote to 64-bit and diverge from the
/// interpreter. Narrow types are truncated exactly as `conv` would.
fn widen(return_type: &TypeSig, raw: i64) -> Value {
    match return_type.unwrap_modifiers() {
        TypeSig::Boolean => Value::I32((raw != 0) as i32),
        TypeSig::I1 => Value::I32(raw as i8 as i32),
        TypeSig::U1 => Value::I32(raw as u8 as i32),
        TypeSig::I2 => Value::I32(raw as i16 as i32),
        TypeSig::U2 | TypeSig::Char => Value::I32(raw as u16 as i32),
        TypeSig::I4 => Value::I32(raw as i32),
        TypeSig::U4 => Value::I32(raw as u32 as i32),
        TypeSig::I8 | TypeSig::U8 => Value::I64(raw),
        _ => Value::I64(raw),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_narrow_return_type_comes_back_as_int32() {
        // A compiled body leaves 64 bits in rax; the stack type decides how
        // much of it is the answer.
        assert_eq!(widen(&TypeSig::I4, -1), Value::I32(-1));
        assert_eq!(widen(&TypeSig::I8, -1), Value::I64(-1));
        assert_eq!(widen(&TypeSig::Boolean, 5), Value::I32(1));
        assert_eq!(widen(&TypeSig::U1, 0x1FF), Value::I32(0xFF));
        assert_eq!(widen(&TypeSig::I2, 0xFFFF), Value::I32(-1));
    }

    #[test]
    fn only_integers_marshal_into_a_register() {
        assert_eq!(as_i64(&Value::I32(7)), Some(7));
        assert_eq!(as_i64(&Value::I64(7)), Some(7));
        assert_eq!(as_i64(&Value::Null), None, "a reference is not a machine word");
        assert_eq!(as_i64(&Value::F(1.0)), None);
    }

    #[test]
    fn the_default_threshold_leaves_cold_methods_interpreted() {
        let tier = JitTier::new();
        assert!(tier.threshold > 1, "compiling everything on first call defeats tiering");
        assert_eq!(tier.compiled_count(), 0);
    }
}
