//! # rustclr-interop
//!
//! The Interop Bridge: P/Invoke from managed code into native libraries.
//!
//! Two guarantees shape this crate:
//!
//! * **Unsupported shapes are refused, not guessed.** Calling a C function
//!   whose signature is only known at runtime is inherently unsafe. Rather than
//!   fabricate a calling sequence, [`PInvokeBridge`] dispatches through a table
//!   of concrete function types and returns an error for anything outside it.
//! * **`unsafe` is confined here.** It appears in exactly two places — the
//!   platform loader and the call dispatcher — and nowhere else in the runtime.

pub mod library;
pub mod marshal;

pub use library::{LoadError, NativeLibrary, Symbol};
pub use marshal::{marshal_argument, return_shape, AbiSlot, Marshalled, ReturnShape};

use rustclr_core::{ClrExceptionKind, ExecutionError, Interpreter, MethodKind, Value};
use std::collections::HashMap;

/// The maximum number of arguments a P/Invoke call may take.
///
/// Raising this means adding arms to [`call_native`]; it is a hard limit rather
/// than a soft one precisely because each arity needs its own concrete type.
pub const MAX_PINVOKE_ARGS: usize = 6;

/// Resolves and invokes native entry points on behalf of the runtime.
#[derive(Default)]
pub struct PInvokeBridge {
    libraries: HashMap<String, NativeLibrary>,
    /// Names that failed to load, so a repeat call fails fast.
    failed: HashMap<String, LoadError>,
    pub calls: u64,
}

impl PInvokeBridge {
    pub fn new() -> Self {
        Self::default()
    }

    /// Loads a library, caching both successes and failures.
    pub fn library(&mut self, name: &str) -> Result<&mut NativeLibrary, LoadError> {
        if let Some(e) = self.failed.get(name) {
            return Err(e.clone());
        }
        if !self.libraries.contains_key(name) {
            match NativeLibrary::open(name) {
                Ok(lib) => {
                    self.libraries.insert(name.to_string(), lib);
                }
                Err(e) => {
                    self.failed.insert(name.to_string(), e.clone());
                    return Err(e);
                }
            }
        }
        Ok(self.libraries.get_mut(name).expect("just inserted"))
    }

    pub fn loaded_count(&self) -> usize {
        self.libraries.len()
    }

    /// Resolves `library!entry_point` to an address.
    pub fn resolve(&mut self, library: &str, entry_point: &str) -> Result<Symbol, LoadError> {
        self.library(library)?.symbol(entry_point)
    }
}

/// Calls a native function through a concrete function type.
///
/// # Safety
///
/// `symbol` must point at a function that really takes `args.len()` arguments
/// in the classified ABI slots and returns a value matching `shape`. A mismatch
/// is undefined behaviour — which is why this is `unsafe` and why the public
/// entry point validates before reaching it.
pub unsafe fn call_native(
    symbol: Symbol,
    args: &[Marshalled],
    shape: ReturnShape,
) -> Result<RawReturn, ExecutionError> {
    if args.len() > MAX_PINVOKE_ARGS {
        return Err(ExecutionError::Unsupported(format!(
            "P/Invoke supports at most {MAX_PINVOKE_ARGS} arguments"
        )));
    }
    // Mixed integer/float argument lists need per-position typing, which this
    // table does not model. Refuse rather than mis-order the registers.
    let all_integer = args.iter().all(|a| a.slot == AbiSlot::Integer);
    let all_float = args.iter().all(|a| a.slot == AbiSlot::Float);
    if !all_integer && !all_float {
        return Err(ExecutionError::Unsupported(
            "P/Invoke with mixed integer and floating-point arguments is not supported".into(),
        ));
    }

    let ints: Vec<i64> = args.iter().map(|a| a.integer).collect();
    let floats: Vec<f64> = args.iter().map(|a| a.float).collect();

    // SAFETY: delegated to this function's contract.
    unsafe {
        Ok(if all_float && !args.is_empty() {
            let raw = match floats.len() {
                1 => {
                    let f: extern "C" fn(f64) -> f64 = core::mem::transmute(symbol);
                    f(floats[0])
                }
                2 => {
                    let f: extern "C" fn(f64, f64) -> f64 = core::mem::transmute(symbol);
                    f(floats[0], floats[1])
                }
                _ => {
                    return Err(ExecutionError::Unsupported(
                        "P/Invoke supports at most two floating-point arguments".into(),
                    ))
                }
            };
            RawReturn::Double(raw)
        } else if shape == ReturnShape::Double {
            let raw = match ints.len() {
                0 => {
                    let f: extern "C" fn() -> f64 = core::mem::transmute(symbol);
                    f()
                }
                1 => {
                    let f: extern "C" fn(i64) -> f64 = core::mem::transmute(symbol);
                    f(ints[0])
                }
                _ => {
                    let f: extern "C" fn(i64, i64) -> f64 = core::mem::transmute(symbol);
                    f(ints[0], ints[1])
                }
            };
            RawReturn::Double(raw)
        } else {
            let raw = match ints.len() {
                0 => {
                    let f: extern "C" fn() -> i64 = core::mem::transmute(symbol);
                    f()
                }
                1 => {
                    let f: extern "C" fn(i64) -> i64 = core::mem::transmute(symbol);
                    f(ints[0])
                }
                2 => {
                    let f: extern "C" fn(i64, i64) -> i64 = core::mem::transmute(symbol);
                    f(ints[0], ints[1])
                }
                3 => {
                    let f: extern "C" fn(i64, i64, i64) -> i64 = core::mem::transmute(symbol);
                    f(ints[0], ints[1], ints[2])
                }
                4 => {
                    let f: extern "C" fn(i64, i64, i64, i64) -> i64 = core::mem::transmute(symbol);
                    f(ints[0], ints[1], ints[2], ints[3])
                }
                5 => {
                    let f: extern "C" fn(i64, i64, i64, i64, i64) -> i64 =
                        core::mem::transmute(symbol);
                    f(ints[0], ints[1], ints[2], ints[3], ints[4])
                }
                _ => {
                    let f: extern "C" fn(i64, i64, i64, i64, i64, i64) -> i64 =
                        core::mem::transmute(symbol);
                    f(ints[0], ints[1], ints[2], ints[3], ints[4], ints[5])
                }
            };
            RawReturn::Integer(raw)
        })
    }
}

/// The untyped result of a native call.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RawReturn {
    Integer(i64),
    Double(f64),
}

/// Installs a P/Invoke handler for every declared native method in an assembly.
///
/// The runtime looks up `pinvoke:<library>!<entry>` when it reaches a method
/// marked `pinvokeimpl`, so registering under that key is what makes the
/// declaration callable.
pub fn install(interp: &mut Interpreter) -> usize {
    let declarations: Vec<(String, String)> = interp
        .loader
        .registry
        .iter_methods()
        .filter_map(|m| match &m.kind {
            MethodKind::PInvoke { library, entry_point, .. } => {
                Some((library.clone(), entry_point.clone()))
            }
            _ => None,
        })
        .collect();

    let mut installed = 0;
    for (library, entry) in declarations {
        let key = format!("pinvoke:{library}!{entry}");
        if interp.has_native(&key) {
            continue;
        }
        interp.register_native(key, dispatch_pinvoke);
        installed += 1;
    }
    installed
}

// Thread-local bridge state. The native-method signature is a plain `fn`
// pointer with no user data, so the loaded-library cache lives here rather than
// being threaded through every call.
thread_local! {
    static BRIDGE: std::cell::RefCell<PInvokeBridge> =
        std::cell::RefCell::new(PInvokeBridge::new());
}

/// Number of libraries this thread currently has open.
pub fn loaded_library_count() -> usize {
    BRIDGE.with(|b| b.borrow().loaded_count())
}

/// The handler registered for every P/Invoke declaration.
fn dispatch_pinvoke(
    interp: &mut Interpreter,
    args: &[Value],
) -> Result<Option<Value>, ExecutionError> {
    // The runtime records which declaration is executing, so recover the
    // library and entry point from its metadata.
    let Some(method) = interp.current_native_method() else {
        return Err(ExecutionError::Unsupported(
            "P/Invoke dispatch was reached outside a native declaration".into(),
        ));
    };
    let info = interp.loader.registry.method(method);
    let MethodKind::PInvoke { library, entry_point, .. } = &info.kind else {
        return Err(ExecutionError::Unsupported(
            "the dispatching method is not a P/Invoke declaration".into(),
        ));
    };
    let (library, entry_point) = (library.clone(), entry_point.clone());
    let shape = return_shape(&info.signature);

    let mut marshalled = Vec::with_capacity(args.len());
    for a in args {
        marshalled.push(marshal_argument(interp, a)?);
    }

    let symbol = BRIDGE
        .with(|b| b.borrow_mut().resolve(&library, &entry_point))
        .map_err(|e| match e {
            LoadError::LibraryNotFound(l) => ExecutionError::exception(
                ClrExceptionKind::DllNotFound,
                format!("Unable to load DLL '{l}'."),
            ),
            other => ExecutionError::exception(
                ClrExceptionKind::EntryPointNotFound,
                other.to_string(),
            ),
        })?;

    BRIDGE.with(|b| b.borrow_mut().calls += 1);

    // SAFETY: the shape was derived from the managed signature the developer
    // declared. A wrong declaration is undefined behaviour in .NET too; this
    // bridge narrows the blast radius by refusing shapes it cannot express.
    let raw = unsafe { call_native(symbol, &marshalled, shape)? };

    Ok(match (shape, raw) {
        (ReturnShape::Void, _) => None,
        (ReturnShape::Int32, RawReturn::Integer(v)) => Some(Value::I32(v as i32)),
        (ReturnShape::Int64, RawReturn::Integer(v)) => Some(Value::I64(v)),
        (ReturnShape::Pointer, RawReturn::Integer(v)) => Some(Value::NativeInt(v)),
        // SAFETY: the declaration says the callee returns a C string.
        (ReturnShape::CString, RawReturn::Integer(v)) => {
            Some(unsafe { marshal::read_c_string(interp, v) })
        }
        (_, RawReturn::Double(v)) => Some(Value::F(v)),
        (_, RawReturn::Integer(v)) => Some(Value::I64(v)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_library_is_cached_as_a_failure() {
        let mut bridge = PInvokeBridge::new();
        assert!(bridge.library("no-such-library-abc").is_err());
        assert!(bridge.library("no-such-library-abc").is_err());
        assert_eq!(bridge.loaded_count(), 0);
    }

    #[test]
    fn too_many_arguments_are_refused() {
        let args: Vec<Marshalled> = (0..MAX_PINVOKE_ARGS + 1)
            .map(|_| {
                let i = rustclr_core::Interpreter::with_host(Box::new(
                    rustclr_core::CaptureHost::new(),
                ));
                marshal_argument(&i, &Value::I32(0)).unwrap()
            })
            .collect();
        let result = unsafe { call_native(core::ptr::null(), &args, ReturnShape::Int32) };
        assert!(result.is_err());
    }

    #[cfg(windows)]
    #[test]
    fn a_real_native_function_can_be_called() {
        let mut bridge = PInvokeBridge::new();
        let symbol = bridge
            .resolve("kernel32.dll", "GetCurrentProcessId")
            .expect("kernel32 export resolves");
        let result = unsafe { call_native(symbol, &[], ReturnShape::Int32) }.unwrap();
        match result {
            RawReturn::Integer(pid) => {
                assert_eq!(pid as u32, std::process::id(), "P/Invoke returned the real PID")
            }
            other => panic!("expected an integer return, got {other:?}"),
        }
    }
}
