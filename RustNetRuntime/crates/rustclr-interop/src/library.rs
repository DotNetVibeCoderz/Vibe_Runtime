//! Dynamic library loading.
//!
//! Deliberately dependency-free: the platform loaders are declared directly so
//! the toolchain can be cross-compiled without pulling in a crate that assumes
//! a hosted OS.

use std::collections::HashMap;
use std::ffi::CString;
use std::fmt;

/// An opaque address in the loaded module.
pub type Symbol = *const core::ffi::c_void;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadError {
    LibraryNotFound(String),
    SymbolNotFound { library: String, symbol: String },
    InvalidName(String),
    Unsupported,
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LibraryNotFound(l) => write!(f, "unable to load native library '{l}'"),
            Self::SymbolNotFound { library, symbol } => {
                write!(f, "unable to find entry point '{symbol}' in '{library}'")
            }
            Self::InvalidName(n) => write!(f, "'{n}' is not a valid library or symbol name"),
            Self::Unsupported => write!(f, "native library loading is unavailable on this target"),
        }
    }
}

impl std::error::Error for LoadError {}

#[cfg(windows)]
mod platform {
    use super::{LoadError, Symbol};
    use std::ffi::CString;

    pub type Handle = *mut core::ffi::c_void;

    unsafe extern "system" {
        fn LoadLibraryA(name: *const i8) -> Handle;
        fn GetProcAddress(module: Handle, name: *const i8) -> Symbol;
        fn FreeLibrary(module: Handle) -> i32;
    }

    pub fn open(name: &str) -> Result<Handle, LoadError> {
        // Windows resolves a bare name against the standard search path, and
        // adds the `.dll` suffix itself when absent.
        let c = CString::new(name).map_err(|_| LoadError::InvalidName(name.into()))?;
        let handle = unsafe { LoadLibraryA(c.as_ptr()) };
        if handle.is_null() {
            Err(LoadError::LibraryNotFound(name.into()))
        } else {
            Ok(handle)
        }
    }

    pub fn symbol(module: Handle, name: &str) -> Option<Symbol> {
        let c = CString::new(name).ok()?;
        let s = unsafe { GetProcAddress(module, c.as_ptr()) };
        (!s.is_null()).then_some(s)
    }

    pub fn close(module: Handle) {
        unsafe {
            FreeLibrary(module);
        }
    }
}

#[cfg(unix)]
mod platform {
    use super::{LoadError, Symbol};
    use std::ffi::CString;

    pub type Handle = *mut core::ffi::c_void;

    const RTLD_NOW: i32 = 2;

    unsafe extern "C" {
        fn dlopen(name: *const i8, flags: i32) -> Handle;
        fn dlsym(handle: Handle, name: *const i8) -> Symbol;
        fn dlclose(handle: Handle) -> i32;
    }

    pub fn open(name: &str) -> Result<Handle, LoadError> {
        // Try the name as given, then the conventional `lib<name>.so` form,
        // because P/Invoke declarations are usually written for Windows.
        for candidate in [name.to_string(), format!("lib{name}.so"), format!("{name}.so")] {
            let Ok(c) = CString::new(candidate.as_str()) else { continue };
            let handle = unsafe { dlopen(c.as_ptr(), RTLD_NOW) };
            if !handle.is_null() {
                return Ok(handle);
            }
        }
        Err(LoadError::LibraryNotFound(name.into()))
    }

    pub fn symbol(module: Handle, name: &str) -> Option<Symbol> {
        let c = CString::new(name).ok()?;
        let s = unsafe { dlsym(module, c.as_ptr()) };
        (!s.is_null()).then_some(s)
    }

    pub fn close(module: Handle) {
        unsafe {
            dlclose(module);
        }
    }
}

#[cfg(not(any(windows, unix)))]
mod platform {
    use super::{LoadError, Symbol};
    pub type Handle = *mut core::ffi::c_void;

    pub fn open(_name: &str) -> Result<Handle, LoadError> {
        Err(LoadError::Unsupported)
    }
    pub fn symbol(_module: Handle, _name: &str) -> Option<Symbol> {
        None
    }
    pub fn close(_module: Handle) {}
}

/// A loaded native library.
pub struct NativeLibrary {
    name: String,
    handle: platform::Handle,
    symbols: HashMap<String, Symbol>,
}

// The handle is just an OS module address; it is valid on any thread.
unsafe impl Send for NativeLibrary {}
unsafe impl Sync for NativeLibrary {}

impl NativeLibrary {
    pub fn open(name: &str) -> Result<Self, LoadError> {
        let handle = platform::open(name)?;
        Ok(Self {
            name: name.to_string(),
            handle,
            symbols: HashMap::new(),
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Resolves and caches an exported symbol.
    pub fn symbol(&mut self, name: &str) -> Result<Symbol, LoadError> {
        if let Some(s) = self.symbols.get(name) {
            return Ok(*s);
        }
        // Windows exports `stdcall` functions with a `@n` suffix and both
        // ANSI/wide variants; try the decorations P/Invoke would.
        for candidate in [name.to_string(), format!("{name}A"), format!("{name}W")] {
            if let Some(s) = platform::symbol(self.handle, &candidate) {
                self.symbols.insert(name.to_string(), s);
                return Ok(s);
            }
        }
        Err(LoadError::SymbolNotFound {
            library: self.name.clone(),
            symbol: name.to_string(),
        })
    }

    pub fn cached_symbol_count(&self) -> usize {
        self.symbols.len()
    }
}

impl Drop for NativeLibrary {
    fn drop(&mut self) {
        platform::close(self.handle);
    }
}

impl fmt::Debug for NativeLibrary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NativeLibrary")
            .field("name", &self.name)
            .field("cached_symbols", &self.symbols.len())
            .finish()
    }
}

/// Ensures a C string round-trips without an interior NUL.
pub fn to_c_string(s: &str) -> Result<CString, LoadError> {
    CString::new(s).map_err(|_| LoadError::InvalidName(s.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_library_reports_an_error_rather_than_aborting() {
        let result = NativeLibrary::open("definitely-not-a-real-library-xyz");
        assert!(matches!(result, Err(LoadError::LibraryNotFound(_))));
    }

    #[test]
    fn interior_nul_is_rejected() {
        assert!(to_c_string("bad\0name").is_err());
    }

    #[cfg(windows)]
    #[test]
    fn the_platform_c_runtime_loads_and_exports_known_symbols() {
        let mut lib = NativeLibrary::open("kernel32.dll").expect("kernel32 is always present");
        assert!(lib.symbol("GetTickCount").is_ok());
        assert!(lib.symbol("NoSuchExport").is_err());
        assert_eq!(lib.cached_symbol_count(), 1);
    }
}
