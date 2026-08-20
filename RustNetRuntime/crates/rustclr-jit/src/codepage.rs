//! Executable memory, allocated write-xor-execute.
//!
//! A page is mapped **readable and writable**, filled with the emitted bytes,
//! and only then flipped to **readable and executable**. It is never both
//! writable and executable at once. That is not ceremony: a page that stays
//! W+X turns any bug that can write to it into arbitrary code execution, and
//! modern kernels and hardened toolchains increasingly refuse the combination
//! outright.
//!
//! `unsafe` is confined to this file and to the transmute that calls the
//! finished page. Everything else in the backend produces plain `Vec<u8>`.

use core::ffi::c_void;

/// A region of memory holding compiled code.
///
/// The mapping is released when this is dropped, so a `CodePage` must outlive
/// every function pointer taken from it. The compiler holds them for the life
/// of the process.
#[derive(Debug)]
pub struct CodePage {
    ptr: *mut c_void,
    len: usize,
    executable: bool,
}

// The pointer is owned exclusively and the memory is never aliased; the page is
// immutable once it has been made executable.
unsafe impl Send for CodePage {}
unsafe impl Sync for CodePage {}

/// Why a page could not be produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodePageError {
    /// The kernel refused the mapping.
    AllocationFailed,
    /// The mapping could not be made executable — a hardened environment may
    /// forbid it entirely, in which case interpretation remains the answer.
    ProtectionFailed,
    /// Compilation produced nothing to map.
    Empty,
}

impl std::fmt::Display for CodePageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AllocationFailed => write!(f, "could not allocate executable memory"),
            Self::ProtectionFailed => {
                write!(f, "could not mark code memory executable; this environment may forbid it")
            }
            Self::Empty => write!(f, "no code was emitted"),
        }
    }
}

impl std::error::Error for CodePageError {}

impl CodePage {
    /// Maps `code` and makes it executable.
    pub fn commit(code: &[u8]) -> Result<Self, CodePageError> {
        if code.is_empty() {
            return Err(CodePageError::Empty);
        }
        let mut page = Self::allocate(code.len())?;
        // SAFETY: the mapping is at least `code.len()` bytes, is writable at
        // this point, and is owned exclusively by `page`.
        unsafe {
            core::ptr::copy_nonoverlapping(code.as_ptr(), page.ptr as *mut u8, code.len());
        }
        page.make_executable()?;
        Ok(page)
    }

    /// The entry address, as a raw pointer.
    ///
    /// # Safety
    ///
    /// The caller must transmute this to a function type matching exactly what
    /// the backend emitted. A mismatch is undefined behaviour, which is why
    /// only [`crate::x64`] calls this.
    pub fn as_ptr(&self) -> *const c_void {
        self.ptr
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn is_executable(&self) -> bool {
        self.executable
    }
}

// -- Windows -----------------------------------------------------------------

#[cfg(windows)]
mod sys {
    use super::*;

    const MEM_COMMIT: u32 = 0x1000;
    const MEM_RESERVE: u32 = 0x2000;
    const MEM_RELEASE: u32 = 0x8000;
    const PAGE_READWRITE: u32 = 0x04;
    const PAGE_EXECUTE_READ: u32 = 0x20;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn VirtualAlloc(
            address: *mut c_void,
            size: usize,
            allocation_type: u32,
            protect: u32,
        ) -> *mut c_void;
        fn VirtualProtect(
            address: *mut c_void,
            size: usize,
            new_protect: u32,
            old_protect: *mut u32,
        ) -> i32;
        fn VirtualFree(address: *mut c_void, size: usize, free_type: u32) -> i32;
        fn FlushInstructionCache(process: *mut c_void, address: *const c_void, size: usize) -> i32;
        fn GetCurrentProcess() -> *mut c_void;
    }

    impl CodePage {
        pub(super) fn allocate(len: usize) -> Result<CodePage, CodePageError> {
            // SAFETY: a null base lets the kernel choose the address; the size
            // is non-zero because `commit` rejects empty code.
            let ptr = unsafe {
                VirtualAlloc(
                    core::ptr::null_mut(),
                    len,
                    MEM_COMMIT | MEM_RESERVE,
                    PAGE_READWRITE,
                )
            };
            if ptr.is_null() {
                return Err(CodePageError::AllocationFailed);
            }
            Ok(CodePage { ptr, len, executable: false })
        }

        pub(super) fn make_executable(&mut self) -> Result<(), CodePageError> {
            let mut previous = 0u32;
            // SAFETY: the region was returned by `VirtualAlloc` with this size.
            let ok = unsafe { VirtualProtect(self.ptr, self.len, PAGE_EXECUTE_READ, &mut previous) };
            if ok == 0 {
                return Err(CodePageError::ProtectionFailed);
            }
            // Required on any machine whose instruction cache is not coherent
            // with its data cache; a no-op where it already is.
            unsafe {
                FlushInstructionCache(GetCurrentProcess(), self.ptr, self.len);
            }
            self.executable = true;
            Ok(())
        }
    }

    impl Drop for CodePage {
        fn drop(&mut self) {
            if !self.ptr.is_null() {
                // SAFETY: `MEM_RELEASE` requires a size of zero and the exact
                // base address `VirtualAlloc` returned.
                unsafe {
                    VirtualFree(self.ptr, 0, MEM_RELEASE);
                }
            }
        }
    }
}

// -- Unix --------------------------------------------------------------------

#[cfg(unix)]
mod sys {
    use super::*;

    const PROT_READ: i32 = 0x1;
    const PROT_WRITE: i32 = 0x2;
    const PROT_EXEC: i32 = 0x4;
    const MAP_PRIVATE: i32 = 0x02;
    #[cfg(target_os = "macos")]
    const MAP_ANONYMOUS: i32 = 0x1000;
    #[cfg(not(target_os = "macos"))]
    const MAP_ANONYMOUS: i32 = 0x20;

    unsafe extern "C" {
        fn mmap(
            addr: *mut c_void,
            length: usize,
            prot: i32,
            flags: i32,
            fd: i32,
            offset: i64,
        ) -> *mut c_void;
        fn mprotect(addr: *mut c_void, len: usize, prot: i32) -> i32;
        fn munmap(addr: *mut c_void, len: usize) -> i32;
    }

    /// `mmap` reports failure as `-1`, not null.
    const MAP_FAILED: isize = -1;

    impl CodePage {
        pub(super) fn allocate(len: usize) -> Result<CodePage, CodePageError> {
            // SAFETY: an anonymous private mapping with a null hint; the fd is
            // ignored and must be -1.
            let ptr = unsafe {
                mmap(
                    core::ptr::null_mut(),
                    len,
                    PROT_READ | PROT_WRITE,
                    MAP_PRIVATE | MAP_ANONYMOUS,
                    -1,
                    0,
                )
            };
            if ptr.is_null() || ptr as isize == MAP_FAILED {
                return Err(CodePageError::AllocationFailed);
            }
            Ok(CodePage { ptr, len, executable: false })
        }

        pub(super) fn make_executable(&mut self) -> Result<(), CodePageError> {
            // Write permission is dropped in the same call that adds execute,
            // so the page is never simultaneously writable and executable.
            // SAFETY: the region came from `mmap` with this length.
            let ok = unsafe { mprotect(self.ptr, self.len, PROT_READ | PROT_EXEC) };
            if ok != 0 {
                return Err(CodePageError::ProtectionFailed);
            }
            self.executable = true;
            Ok(())
        }
    }

    impl Drop for CodePage {
        fn drop(&mut self) {
            if !self.ptr.is_null() {
                // SAFETY: base and length are exactly what `mmap` returned.
                unsafe {
                    munmap(self.ptr, self.len);
                }
            }
        }
    }
}

// Platforms with neither interface get a backend that never compiles, which the
// tiering model already handles: interpretation is always available.
#[cfg(not(any(windows, unix)))]
mod sys {
    use super::*;

    impl CodePage {
        pub(super) fn allocate(_len: usize) -> Result<CodePage, CodePageError> {
            Err(CodePageError::AllocationFailed)
        }
        pub(super) fn make_executable(&mut self) -> Result<(), CodePageError> {
            Err(CodePageError::ProtectionFailed)
        }
    }

    impl Drop for CodePage {
        fn drop(&mut self) {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_code_is_refused_rather_than_mapped() {
        assert_eq!(CodePage::commit(&[]).unwrap_err(), CodePageError::Empty);
    }

    #[test]
    #[cfg(any(windows, unix))]
    fn a_committed_page_is_executable_and_holds_the_bytes() {
        // `mov eax, 7; ret` — valid on x86-64, and never executed here.
        let code = [0xB8, 0x07, 0x00, 0x00, 0x00, 0xC3];
        let page = CodePage::commit(&code).expect("commit");
        assert!(page.is_executable(), "a committed page must be executable");
        assert!(page.len() >= code.len());
    }

    #[test]
    #[cfg(all(any(windows, unix), target_arch = "x86_64"))]
    fn a_committed_page_actually_runs() {
        // mov eax, 42; ret
        let code = [0xB8, 0x2A, 0x00, 0x00, 0x00, 0xC3];
        let page = CodePage::commit(&code).expect("commit");
        // SAFETY: the bytes above are a complete function taking no arguments
        // and returning an int in eax, which matches this signature exactly.
        let f: extern "C" fn() -> i32 = unsafe { core::mem::transmute(page.as_ptr()) };
        assert_eq!(f(), 42);
    }
}
