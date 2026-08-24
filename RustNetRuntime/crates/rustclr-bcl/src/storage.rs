//! `RustNet.IO.FileSystem` — files on a device.
//!
//! Backed by [`rustclr_flashfs`] over the board's external memory: named blobs
//! in an append-only log, newest wins. That is a deliberate choice of shape
//! rather than a smaller `System.IO`.
//!
//! # Why not `FileStream`
//!
//! NOR flash clears bits and cannot set them; the only way back is erasing a
//! whole sector. A `Stream` with `Seek` and partial writes over that would
//! mean read-erase-write of the surrounding sector on every `Write`, which is
//! slow and opens a power-loss window over data that was previously fine. So a
//! file here is read and written **whole**, and the API says so: `ReadAllBytes`,
//! `WriteAllBytes`, `AppendAllText`. A program that wants random access wants a
//! database, and `RustNet.Data` is where that goes.
//!
//! # Region 0
//!
//! Every call goes to external-memory region 0, the one a board designates for
//! application storage. A board with no external memory answers `NotSupported`
//! from the HAL and the call throws, rather than silently writing to RAM that
//! disappears at the next reset.

use crate::support::*;
use rustclr_core::{ExecResult, ExecutionError, Interpreter, Value};

#[allow(unused_imports)]
use crate::prelude::*;

/// The external-memory region application files live in.
const STORAGE_REGION: u8 = 0;

/// Turns a filesystem failure into the managed exception a program catches.
///
/// Everything the store can say is a string, and the string is the message:
/// `no such file`, `region full`, `path escapes the root`. Losing it in favour
/// of a generic `IOException` would take away the only thing that says which.
fn io_error(what: &str, message: String) -> ExecutionError {
    ExecutionError::exception(
        rustclr_core::ClrExceptionKind::Io,
        &format!("{what}: {message}"),
    )
}

fn no_storage(what: &str) -> ExecutionError {
    ExecutionError::exception(
        rustclr_core::ClrExceptionKind::Io,
        &format!(
            "{what} needs storage, and this board has none. \
             Run with `--board host` for the simulator, or use a board with external memory."
        ),
    )
}

/// Runs `f` against the board's storage region, or throws.
fn with_storage<T>(
    interp: &mut Interpreter,
    what: &str,
    f: impl FnOnce(&mut dyn rustclr_hal::extmem::ExtMemory) -> Result<T, String>,
) -> ExecResult<T> {
    let Some(board) = interp.board.as_mut() else {
        return Err(no_storage(what));
    };
    let region = board.extmem(STORAGE_REGION).map_err(|_| no_storage(what))?;
    f(region).map_err(|e| io_error(what, e))
}

pub fn register(interp: &mut Interpreter) {
    interp.register_native("RustNet.IO.FileSystem::Exists(string)", |i, a| {
        let path = arg_string(i, a, 0)?;
        let yes = with_storage(i, "FileSystem.Exists", |r| rustclr_flashfs::exists(r, &path))?;
        Ok(Some(Value::I32(i32::from(yes))))
    });
    interp.register_native("RustNet.IO.FileSystem::ReadAllBytes(string)", |i, a| {
        let path = arg_string(i, a, 0)?;
        let bytes = with_storage(i, "FileSystem.ReadAllBytes", |r| rustclr_flashfs::read(r, &path))?;
        let array = byte_array(i, &bytes);
        Ok(Some(Value::Obj(array)))
    });
    interp.register_native("RustNet.IO.FileSystem::WriteAllBytes(string,byte[])", |i, a| {
        let path = arg_string(i, a, 0)?;
        let data = arg_bytes(i, a, 1)?;
        with_storage(i, "FileSystem.WriteAllBytes", |r| {
            rustclr_flashfs::write(r, &path, &data)
        })?;
        Ok(None)
    });
    interp.register_native("RustNet.IO.FileSystem::ReadAllText(string)", |i, a| {
        let path = arg_string(i, a, 0)?;
        let bytes = with_storage(i, "FileSystem.ReadAllText", |r| rustclr_flashfs::read(r, &path))?;
        // Invalid UTF-8 is refused rather than lossily replaced: a file that
        // came back with U+FFFD where a byte used to be is a corrupted file
        // presented as a valid one.
        let text = String::from_utf8(bytes).map_err(|_| {
            io_error(
                "FileSystem.ReadAllText",
                "the file is not valid UTF-8; use ReadAllBytes".to_string(),
            )
        })?;
        Ok(Some(string_value(i, &text)))
    });
    interp.register_native("RustNet.IO.FileSystem::WriteAllText(string,string)", |i, a| {
        let path = arg_string(i, a, 0)?;
        let text = arg_string_or_empty(i, a, 1)?;
        with_storage(i, "FileSystem.WriteAllText", |r| {
            rustclr_flashfs::write(r, &path, text.as_bytes())
        })?;
        Ok(None)
    });
    interp.register_native("RustNet.IO.FileSystem::AppendText(string,string)", |i, a| {
        let path = arg_string(i, a, 0)?;
        let text = arg_string_or_empty(i, a, 1)?;
        with_storage(i, "FileSystem.AppendAllText", |r| {
            rustclr_flashfs::append_bytes(r, &path, text.as_bytes())
        })?;
        Ok(None)
    });
    interp.register_native("RustNet.IO.FileSystem::Delete(string)", |i, a| {
        let path = arg_string(i, a, 0)?;
        with_storage(i, "FileSystem.Delete", |r| rustclr_flashfs::delete(r, &path))?;
        Ok(None)
    });
    interp.register_native("RustNet.IO.FileSystem::CreateDirectory(string)", |i, a| {
        let path = arg_string(i, a, 0)?;
        with_storage(i, "FileSystem.CreateDirectory", |r| {
            rustclr_flashfs::create_directory(r, &path)
        })?;
        Ok(None)
    });
    interp.register_native("RustNet.IO.FileSystem::List(string)", |i, a| {
        let path = arg_string_or_empty(i, a, 0)?;
        let names = with_storage(i, "FileSystem.List", |r| rustclr_flashfs::list(r, &path))?;
        let array = string_array(i, &names);
        Ok(Some(Value::Obj(array)))
    });
    // How much of the region is spoken for, superseded records included. A
    // program watching this rise towards the region size is watching for the
    // compaction it will eventually have to do.
    interp.register_native("RustNet.IO.FileSystem::UsedBytes()", |i, _a| {
        let used = with_storage(i, "FileSystem.UsedBytes", |r| Ok(rustclr_flashfs::used(r)))?;
        Ok(Some(Value::I32(used as i32)))
    });
    interp.register_native("RustNet.IO.FileSystem::IsAvailable()", |i, _a| {
        let available = match i.board.as_mut() {
            Some(b) => b.extmem(STORAGE_REGION).is_ok(),
            None => false,
        };
        Ok(Some(Value::I32(i32::from(available))))
    });
}
