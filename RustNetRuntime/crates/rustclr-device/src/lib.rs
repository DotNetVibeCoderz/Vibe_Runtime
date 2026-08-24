//! The RNDP device service — what a board answers when the host tools ask.
//!
//! [`rustclr_rndp`] is the envelope; this is the letter. One request frame in,
//! one reply frame out, with no I/O of its own: the caller reads bytes off
//! whatever it has (a UART on a board, a socket on a desktop) and writes the
//! reply back the same way.
//!
//! # Why the transport is somebody else's problem
//!
//! Two callers use this. A firmware polls a UART from inside the interpreter's
//! instruction loop and must not block; `rustnet serve` reads a TCP socket on
//! a thread and may block all it likes. Neither shape fits the other, and a
//! service that owned its transport would have to pick one — so it owns
//! neither, and both drive [`handle`] with a decoded frame.
//!
//! That also makes the whole protocol testable on a desktop with no board and
//! no socket, which is what the tests at the bottom of this file do.
//!
//! # What is stored, and where
//!
//! Applications and data files both live in [`rustclr_flashfs`], the
//! append-only NOR store, under two prefixes:
//!
//! ```text
//! /apps/<name>     a PE assembly, exactly as Roslyn emitted it
//! /data/<path>     whatever FLASH_DATA was given
//! /sys/autostart   the app to run at power-up, if any
//! ```
//!
//! An assembly is stored **as the SDK built it**. The reference project had a
//! `DLL -> RNX` repackaging step; RustCLR reads PE/ECMA-335 directly, so what
//! is flashed is byte-for-byte what was compiled, and `FLASH_APP` is a file
//! write rather than a conversion.
//!
//! # What this deliberately does not answer
//!
//! OTA, the debugger commands, and the config store return an error naming
//! themselves. They need a second flash slot, a running-program hook and a
//! settings format respectively, and none of those exist yet. An unimplemented
//! command that answered `OK` would make the Workbench show a feature that is
//! not there, which is worse than a button that reports why it did nothing.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

mod json;
use json::escape;

use rustclr_hal::extmem::ExtMemory;
use rustclr_rndp::*;

/// Where an application's bytes live.
pub const APP_PREFIX: &str = "/apps";
/// Where `FLASH_DATA` puts a file.
pub const DATA_PREFIX: &str = "/data";
/// The file naming the app to run at power-up.
pub const AUTOSTART_PATH: &str = "/sys/autostart";

/// Longest application name accepted.
///
/// `FLASH_APP` encodes the name length in a single byte, so 255 is the
/// protocol's own ceiling; this is well below it because the name becomes a
/// path in a flash store whose records carry their names inline.
pub const MAX_APP_NAME: usize = 64;

/// What a device says about itself when asked.
///
/// A struct rather than a string, because the Workbench parses this as JSON
/// with specific field names — and if each board rendered its own, one of them
/// would eventually spell `uptime_ms` differently and its readout would go
/// quietly blank. [`DeviceInfo::to_json`] is the only place the field names
/// appear.
#[derive(Debug, Clone, Default)]
pub struct DeviceInfo {
    /// The silicon: `esp32`, `k210`, `host`.
    pub chip: String,
    /// The product: `M5Stack Tough`, `Sipeed Maix Go`, `virtual device`.
    pub board: String,
    /// The runtime's version.
    pub version: String,
    /// Milliseconds since the device started.
    pub uptime_ms: u64,
    /// Bytes of heap in use, or zero where the allocator cannot say.
    pub heap_used: u64,
    /// How many applications are stored.
    pub apps: usize,
    /// Whether a network interface is associated.
    pub wifi: bool,
    /// The application that is running, or was last started.
    pub active_app: String,
    /// Whether [`Self::active_app`] is running now.
    pub running: bool,
    /// The application set to run at power-up, if any.
    pub autostart: String,
}

impl DeviceInfo {
    /// The JSON the Workbench reads.
    ///
    /// Hand-written, like the debug adapter's: this workspace has no external
    /// dependencies, and the shape is fixed and small enough that a serialiser
    /// would be more code than the thing it serialises.
    pub fn to_json(&self) -> String {
        format!(
            concat!(
                r#"{{"chip":"{}","board":"{}","version":"{}","protocol":{},"#,
                r#""uptime_ms":{},"heap_used":{},"apps":{},"wifi":{},"#,
                r#""active_app":"{}","running":{},"autostart":"{}"}}"#
            ),
            escape(&self.chip),
            escape(&self.board),
            escape(&self.version),
            PROTOCOL_VERSION,
            self.uptime_ms,
            self.heap_used,
            self.apps,
            self.wifi,
            escape(&self.active_app),
            self.running,
            escape(&self.autostart),
        )
    }
}

/// The parts of a device that differ between a firmware and the simulator.
///
/// Everything expressible in terms of a flash region is implemented once in
/// [`handle`] instead of here — a trait method per command would mean every
/// board reimplementing `LIST_APPS`, and eventually two of them disagreeing
/// about what a list looks like.
pub trait DeviceOps {
    /// What this device is. Rendered to JSON by [`handle`], not here.
    fn info(&mut self) -> DeviceInfo;

    /// The flash region applications and data are kept in, if the board has
    /// one. `None` is a board with no storage at all, and the commands that
    /// need it then say so rather than pretending to have written.
    fn storage(&mut self) -> Option<&mut dyn ExtMemory>;

    /// Recent log lines, newest last.
    fn logs(&mut self) -> String {
        String::new()
    }

    /// Counters as `key=value` lines: allocations, collections, uptime.
    fn perf(&mut self) -> String {
        String::new()
    }

    /// The current screen as `(width, height, RGB565 pixels)`.
    ///
    /// `None` when nothing has opened a display — which is not an error, just
    /// a device with nothing to show.
    fn display(&mut self) -> Option<(u32, u32, Vec<u16>)> {
        None
    }

    /// Run `image` as the named application.
    ///
    /// The bytes are passed in rather than read from storage by the caller,
    /// because a firmware may want to run an app it has just been sent without
    /// a round trip through flash.
    fn start_app(&mut self, name: &str, image: Vec<u8>) -> Result<(), String>;

    /// Stop whatever is running. Stopping nothing is success, not an error:
    /// the caller asked for a state, and the device is in it.
    fn stop_app(&mut self) -> Result<(), String>;

    /// The name of the running application, if one is running.
    fn running_app(&mut self) -> Option<String>;

    /// Restart the device. Returns only if it could not.
    fn reboot(&mut self) -> Result<(), String> {
        Err("this device cannot restart itself".to_string())
    }
}

/// The storage-backed half of the service, so a caller can reuse it.
///
/// Public because a firmware's autostart path wants exactly this at boot —
/// read the autostart name, read that app's bytes — without going through a
/// protocol frame to ask itself a question.
pub mod store {
    use super::*;

    fn path_for(prefix: &str, name: &str) -> String {
        format!("{prefix}/{name}")
    }

    /// Reject a name that would escape its prefix or overrun the store.
    ///
    /// A name arrives from the wire, and it becomes a path. `../` is the
    /// obvious hazard; an embedded `/` is the quieter one, because it would
    /// let `FLASH_APP` write outside `/apps` and, in particular, over
    /// `/sys/autostart`.
    pub fn check_name(name: &str) -> Result<(), String> {
        if name.is_empty() {
            return Err("an application name cannot be empty".to_string());
        }
        if name.len() > MAX_APP_NAME {
            return Err(format!(
                "the name is {} bytes and the limit is {MAX_APP_NAME}",
                name.len()
            ));
        }
        if name.contains('/') || name.contains('\\') || name.contains("..") {
            return Err(format!(
                "`{name}` is not a plain name; an application name may not contain a path"
            ));
        }
        if !name.is_ascii() {
            // The store holds names as bytes and the host renders them as
            // UTF-8. Restricting to ASCII keeps those two views identical
            // rather than mostly-identical.
            return Err(format!("`{name}` is not ASCII"));
        }
        Ok(())
    }

    /// Names of the applications on the device.
    pub fn list_apps(flash: &mut dyn ExtMemory) -> Result<Vec<String>, String> {
        rustclr_flashfs::list(flash, APP_PREFIX)
    }

    pub fn read_app(flash: &mut dyn ExtMemory, name: &str) -> Result<Vec<u8>, String> {
        check_name(name)?;
        rustclr_flashfs::read(flash, &path_for(APP_PREFIX, name))
    }

    pub fn write_app(flash: &mut dyn ExtMemory, name: &str, image: &[u8]) -> Result<(), String> {
        check_name(name)?;
        if image.is_empty() {
            return Err("refusing to store an empty application".to_string());
        }
        // A PE file starts `MZ`. Checked here rather than at load time so a
        // mistyped upload fails while the host is still listening, instead of
        // at the next power-up with nobody attached.
        if image.len() < 2 || &image[..2] != b"MZ" {
            return Err(format!(
                "`{name}` does not start with `MZ`, so it is not a PE assembly. \
                 RustCLR runs what Roslyn emits; there is no repackaging step."
            ));
        }
        rustclr_flashfs::write(flash, &path_for(APP_PREFIX, name), image)
    }

    pub fn erase_app(flash: &mut dyn ExtMemory, name: &str) -> Result<(), String> {
        check_name(name)?;
        rustclr_flashfs::delete(flash, &path_for(APP_PREFIX, name))
    }

    /// The application to run at power-up, if one is set.
    pub fn autostart(flash: &mut dyn ExtMemory) -> Option<String> {
        let bytes = rustclr_flashfs::read(flash, AUTOSTART_PATH).ok()?;
        let name = String::from_utf8(bytes).ok()?;
        let name = name.trim().to_string();
        if name.is_empty() {
            None
        } else {
            Some(name)
        }
    }

    /// Set, or with `None` clear, the power-up application.
    pub fn set_autostart(flash: &mut dyn ExtMemory, name: Option<&str>) -> Result<(), String> {
        match name {
            Some(name) => {
                check_name(name)?;
                // Checked against what is actually stored: an autostart
                // pointing at nothing is a board that boots to an error every
                // time, discoverable only by watching its console.
                if !rustclr_flashfs::exists(flash, &path_for(APP_PREFIX, name))? {
                    return Err(format!("there is no application called `{name}` to autostart"));
                }
                rustclr_flashfs::write(flash, AUTOSTART_PATH, name.as_bytes())
            }
            // An empty record rather than a delete, so "cleared" and "never
            // set" read the same on the next boot.
            None => rustclr_flashfs::write(flash, AUTOSTART_PATH, b""),
        }
    }

    /// Normalise a `FLASH_DATA` / `READ_DATA` path into the data prefix.
    pub fn data_path(path: &str) -> Result<String, String> {
        let trimmed = path.trim_start_matches('/');
        if trimmed.is_empty() {
            return Err("a data path cannot be empty".to_string());
        }
        if trimmed.contains("..") {
            return Err(format!("`{path}` escapes {DATA_PREFIX}"));
        }
        Ok(format!("{DATA_PREFIX}/{trimmed}"))
    }
}

/// Answer one request.
///
/// Always returns a frame. A command that fails answers [`ST_ERR`] with a
/// message meant to be read by a person, because that message is what the
/// Workbench puts in its status bar.
pub fn handle(ops: &mut dyn DeviceOps, request: &Frame) -> Frame {
    match dispatch(ops, request) {
        Ok(reply) => reply,
        Err(message) => Frame::err(message),
    }
}

/// The storage a command needs, or the reason there is none.
fn need_storage(ops: &mut dyn DeviceOps) -> Result<&mut dyn ExtMemory, String> {
    ops.storage()
        .ok_or_else(|| "this board has no storage, so it cannot keep applications".to_string())
}

fn dispatch(ops: &mut dyn DeviceOps, request: &Frame) -> Result<Frame, String> {
    match request.code {
        // The protocol version, not an empty payload: the C# client reads
        // `payload[0]`, so an empty reply is an index out of range on the host
        // rather than a clean answer. It is also the only chance either end
        // gets to notice it is talking to a different version.
        CMD_PING => Ok(Frame::ok(alloc::vec![PROTOCOL_VERSION])),

        CMD_INFO => {
            let mut info = ops.info();
            // Filled in here rather than by every device: these two come from
            // the store, which this module already owns.
            if let Some(flash) = ops.storage() {
                info.apps = store::list_apps(flash).map(|a| a.len()).unwrap_or(0);
                info.autostart = store::autostart(flash).unwrap_or_default();
            }
            Ok(Frame::ok(info.to_json().into_bytes()))
        }

        CMD_LIST_APPS => {
            let running = ops.running_app();
            let flash = need_storage(ops)?;
            let apps = store::list_apps(flash)?;
            let autostart = store::autostart(flash);
            // A JSON array, because that is what the Workbench parses. It was
            // a line per app until a screenshot of the real tool showed
            // `hello<TAB>autostart}` in the list: the client splits on `},`
            // and re-appends the brace, so a format invented at this end came
            // out mangled at that one.
            //
            // `name`, `size` and `active` are the reference implementation's
            // fields; `autostart` is added because this build has the concept
            // and the client shows each entry verbatim.
            let mut out = String::from("[");
            for (i, app) in apps.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                let size = store::read_app(flash, app).map(|b| b.len()).unwrap_or(0);
                let active = running.as_deref() == Some(app.as_str());
                let auto = autostart.as_deref() == Some(app.as_str());
                out.push_str(&format!(
                    r#"{{"name":"{}","size":{},"active":{},"autostart":{}}}"#,
                    crate::json::escape(app),
                    size,
                    active,
                    auto
                ));
            }
            out.push(']');
            Ok(Frame::ok(out.into_bytes()))
        }

        CMD_FLASH_APP => {
            // `[u8 name_len][name][image]`, matching `RndpClient.FlashApp`.
            let payload = &request.payload;
            if payload.is_empty() {
                return Err("FLASH_APP needs a name and an image".to_string());
            }
            let name_len = payload[0] as usize;
            if payload.len() < 1 + name_len {
                return Err(format!(
                    "FLASH_APP declared a {name_len}-byte name but the frame holds {} bytes",
                    payload.len() - 1
                ));
            }
            let name = core::str::from_utf8(&payload[1..1 + name_len])
                .map_err(|_| "the application name is not valid UTF-8".to_string())?
                .to_string();
            let image = &payload[1 + name_len..];
            let flash = need_storage(ops)?;
            store::write_app(flash, &name, image)?;
            Ok(Frame::ok(format!("{name}: {} bytes", image.len()).into_bytes()))
        }

        CMD_ERASE_APP => {
            let name = request.text();
            let running = ops.running_app();
            // Refused rather than done: erasing the bytes underneath a running
            // program leaves it running with nothing to restart from, and the
            // app list would show it gone while it is plainly still drawing.
            if running.as_deref() == Some(name.as_str()) {
                return Err(format!("`{name}` is running; stop it before erasing it"));
            }
            let flash = need_storage(ops)?;
            store::erase_app(flash, &name)?;
            Ok(Frame::ok(Vec::new()))
        }

        CMD_START_APP => {
            let name = request.text();
            let flash = need_storage(ops)?;
            let image = store::read_app(flash, &name)?;
            ops.start_app(&name, image)?;
            Ok(Frame::ok(Vec::new()))
        }

        CMD_STOP_APP => {
            ops.stop_app()?;
            Ok(Frame::ok(Vec::new()))
        }

        CMD_SET_AUTOSTART => {
            let name = request.text();
            let name = name.trim();
            let flash = need_storage(ops)?;
            store::set_autostart(flash, if name.is_empty() { None } else { Some(name) })?;
            Ok(Frame::ok(Vec::new()))
        }

        CMD_FLASH_DATA => {
            // `[u16 path_len][path][data]`, matching `RndpClient.FlashData`.
            let payload = &request.payload;
            if payload.len() < 2 {
                return Err("FLASH_DATA needs a path and some data".to_string());
            }
            let path_len = u16::from_le_bytes([payload[0], payload[1]]) as usize;
            if payload.len() < 2 + path_len {
                return Err(format!(
                    "FLASH_DATA declared a {path_len}-byte path but the frame holds {} bytes",
                    payload.len() - 2
                ));
            }
            let path = core::str::from_utf8(&payload[2..2 + path_len])
                .map_err(|_| "the path is not valid UTF-8".to_string())?;
            let path = store::data_path(path)?;
            let data = &payload[2 + path_len..];
            let flash = need_storage(ops)?;
            rustclr_flashfs::write(flash, &path, data)?;
            Ok(Frame::ok(format!("{path}: {} bytes", data.len()).into_bytes()))
        }

        CMD_READ_DATA => {
            let path = store::data_path(&request.text())?;
            let flash = need_storage(ops)?;
            Ok(Frame::ok(rustclr_flashfs::read(flash, &path)?))
        }

        CMD_GET_LOGS => Ok(Frame::ok(ops.logs().into_bytes())),

        CMD_GET_PERF => Ok(Frame::ok(ops.perf().into_bytes())),

        CMD_GET_DISPLAY => {
            let Some((width, height, pixels)) = ops.display() else {
                return Err("nothing has opened a display on this device".to_string());
            };
            // `u16 width | u16 height | RGB565 little-endian`, which is the
            // framebuffer's own layout with its dimensions in front.
            let mut out = Vec::with_capacity(4 + pixels.len() * 2);
            out.extend_from_slice(&(width as u16).to_le_bytes());
            out.extend_from_slice(&(height as u16).to_le_bytes());
            for pixel in &pixels {
                out.extend_from_slice(&pixel.to_le_bytes());
            }
            Ok(Frame::ok(out))
        }

        CMD_REBOOT => {
            ops.reboot()?;
            Ok(Frame::ok(Vec::new()))
        }

        // Named rather than lumped together, so the Workbench's status bar
        // says which button did nothing and why.
        other => Err(match command_name(other) {
            Some(name) => format!("{name} is not implemented on this device"),
            None => format!("unknown command {other:#04x}"),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustclr_hal_host::SimExtMem;

    /// A device with a flash region and nothing else, for testing the half of
    /// the service that is pure storage.
    struct TestDevice {
        flash: SimExtMem,
        running: Option<String>,
        started: Vec<(String, usize)>,
    }

    impl TestDevice {
        fn new() -> Self {
            // A megabyte of simulated NOR: enough for several assemblies, and
            // small enough that a runaway test fills it rather than the machine.
            Self { flash: SimExtMem::qspi_flash(1024 * 1024), running: None, started: Vec::new() }
        }
    }

    impl DeviceOps for TestDevice {
        fn info(&mut self) -> DeviceInfo {
            DeviceInfo { chip: "test".into(), ..Default::default() }
        }
        fn storage(&mut self) -> Option<&mut dyn ExtMemory> {
            Some(&mut self.flash)
        }
        fn start_app(&mut self, name: &str, image: Vec<u8>) -> Result<(), String> {
            self.started.push((name.to_string(), image.len()));
            self.running = Some(name.to_string());
            Ok(())
        }
        fn stop_app(&mut self) -> Result<(), String> {
            self.running = None;
            Ok(())
        }
        fn running_app(&mut self) -> Option<String> {
            self.running.clone()
        }
    }

    /// Anything starting `MZ` passes the shape check; the content is the
    /// loader's business, not the service's.
    fn fake_assembly(size: usize) -> Vec<u8> {
        let mut image = b"MZ".to_vec();
        image.resize(size, 0x42);
        image
    }

    fn flash_app(device: &mut TestDevice, name: &str, image: &[u8]) -> Frame {
        let mut payload = alloc::vec![name.len() as u8];
        payload.extend_from_slice(name.as_bytes());
        payload.extend_from_slice(image);
        handle(device, &Frame::new(CMD_FLASH_APP, payload))
    }

    #[test]
    fn a_ping_is_answered() {
        let mut device = TestDevice::new();
        assert!(handle(&mut device, &Frame::new(CMD_PING, Vec::new())).is_ok());
    }

    #[test]
    fn an_app_can_be_flashed_listed_started_and_erased() {
        let mut device = TestDevice::new();

        assert!(flash_app(&mut device, "demo", &fake_assembly(512)).is_ok());

        let listed = handle(&mut device, &Frame::new(CMD_LIST_APPS, Vec::new()));
        assert!(listed.is_ok());
        assert_eq!(
            listed.text(),
            r#"[{"name":"demo","size":512,"active":false,"autostart":false}]"#
        );

        assert!(handle(&mut device, &Frame::new(CMD_START_APP, b"demo".to_vec())).is_ok());
        assert_eq!(device.started, alloc::vec![("demo".to_string(), 512)]);

        // While it runs the listing says so, and erasing is refused.
        let listed = handle(&mut device, &Frame::new(CMD_LIST_APPS, Vec::new()));
        assert!(listed.text().contains(r#""active":true"#), "got {:?}", listed.text());
        let refused = handle(&mut device, &Frame::new(CMD_ERASE_APP, b"demo".to_vec()));
        assert!(!refused.is_ok(), "erasing a running app must be refused");

        assert!(handle(&mut device, &Frame::new(CMD_STOP_APP, Vec::new())).is_ok());
        assert!(handle(&mut device, &Frame::new(CMD_ERASE_APP, b"demo".to_vec())).is_ok());
        let listed = handle(&mut device, &Frame::new(CMD_LIST_APPS, Vec::new()));
        assert_eq!(listed.text(), "[]", "the app should be gone");
    }

    /// A name from the wire becomes a path, so it must not be able to leave
    /// `/apps` — least of all to land on `/sys/autostart`.
    #[test]
    fn an_app_name_cannot_escape_its_prefix() {
        let mut device = TestDevice::new();
        for hostile in ["../sys/autostart", "a/b", "..", "a\\b", ""] {
            let reply = flash_app(&mut device, hostile, &fake_assembly(16));
            assert!(!reply.is_ok(), "`{hostile}` should have been refused");
        }
        // And nothing was written under any name.
        let listed = handle(&mut device, &Frame::new(CMD_LIST_APPS, Vec::new()));
        assert_eq!(listed.text(), "[]");
    }

    #[test]
    fn something_that_is_not_a_pe_file_is_refused_at_upload() {
        let mut device = TestDevice::new();
        let reply = flash_app(&mut device, "notpe", b"<html>not an assembly</html>");
        assert!(!reply.is_ok());
        assert!(
            reply.text().contains("MZ"),
            "the message should say what was expected, got {:?}",
            reply.text()
        );
    }

    #[test]
    fn autostart_must_name_an_app_that_exists() {
        let mut device = TestDevice::new();
        let reply = handle(&mut device, &Frame::new(CMD_SET_AUTOSTART, b"ghost".to_vec()));
        assert!(!reply.is_ok(), "autostart pointing at nothing must be refused");

        flash_app(&mut device, "demo", &fake_assembly(64));
        assert!(handle(&mut device, &Frame::new(CMD_SET_AUTOSTART, b"demo".to_vec())).is_ok());
        assert_eq!(store::autostart(&mut device.flash).as_deref(), Some("demo"));

        let listed = handle(&mut device, &Frame::new(CMD_LIST_APPS, Vec::new()));
        assert!(listed.text().contains(r#""autostart":true"#), "got {:?}", listed.text());

        // An empty payload clears it.
        assert!(handle(&mut device, &Frame::new(CMD_SET_AUTOSTART, Vec::new())).is_ok());
        assert_eq!(store::autostart(&mut device.flash), None);
    }

    #[test]
    fn data_files_round_trip() {
        let mut device = TestDevice::new();
        let mut payload = (7u16).to_le_bytes().to_vec();
        payload.extend_from_slice(b"cfg.txt");
        payload.extend_from_slice(b"threshold=42");
        assert!(handle(&mut device, &Frame::new(CMD_FLASH_DATA, payload)).is_ok());

        let read = handle(&mut device, &Frame::new(CMD_READ_DATA, b"cfg.txt".to_vec()));
        assert!(read.is_ok());
        assert_eq!(read.text(), "threshold=42");
    }

    /// An unimplemented command must name itself rather than answering OK.
    #[test]
    fn an_unimplemented_command_says_which_one_it_was() {
        let mut device = TestDevice::new();
        let reply = handle(&mut device, &Frame::new(CMD_OTA_BEGIN, Vec::new()));
        assert!(!reply.is_ok());
        assert!(
            reply.text().contains("OTA_BEGIN"),
            "expected the command's name, got {:?}",
            reply.text()
        );

        let reply = handle(&mut device, &Frame::new(0xAB, Vec::new()));
        assert!(reply.text().contains("0xab"), "got {:?}", reply.text());
    }

    /// A board with no flash must say so, not fail obscurely.
    #[test]
    fn a_board_with_no_storage_says_so() {
        struct NoStorage;
        impl DeviceOps for NoStorage {
            fn info(&mut self) -> DeviceInfo {
                DeviceInfo { chip: "storageless".into(), ..Default::default() }
            }
            fn storage(&mut self) -> Option<&mut dyn ExtMemory> {
                None
            }
            fn start_app(&mut self, _n: &str, _i: Vec<u8>) -> Result<(), String> {
                Err("nothing to start".to_string())
            }
            fn stop_app(&mut self) -> Result<(), String> {
                Ok(())
            }
            fn running_app(&mut self) -> Option<String> {
                None
            }
        }

        let mut device = NoStorage;
        let reply = handle(&mut device, &Frame::new(CMD_LIST_APPS, Vec::new()));
        assert!(!reply.is_ok());
        assert!(reply.text().contains("no storage"), "got {:?}", reply.text());

        // PING and INFO still work: they need nothing from flash, and a board
        // that cannot be identified is much harder to diagnose.
        assert!(handle(&mut device, &Frame::new(CMD_PING, Vec::new())).is_ok());
        assert!(handle(&mut device, &Frame::new(CMD_INFO, Vec::new())).is_ok());
    }

    #[test]
    fn a_display_frame_carries_its_dimensions_first() {
        struct Screen;
        impl DeviceOps for Screen {
            fn info(&mut self) -> DeviceInfo {
                DeviceInfo { chip: "screen".into(), ..Default::default() }
            }
            fn storage(&mut self) -> Option<&mut dyn ExtMemory> {
                None
            }
            fn display(&mut self) -> Option<(u32, u32, Vec<u16>)> {
                Some((2, 1, alloc::vec![0xF800, 0x001F]))
            }
            fn start_app(&mut self, _n: &str, _i: Vec<u8>) -> Result<(), String> {
                Ok(())
            }
            fn stop_app(&mut self) -> Result<(), String> {
                Ok(())
            }
            fn running_app(&mut self) -> Option<String> {
                None
            }
        }

        let reply = handle(&mut Screen, &Frame::new(CMD_GET_DISPLAY, Vec::new()));
        assert!(reply.is_ok());
        assert_eq!(reply.payload, alloc::vec![2, 0, 1, 0, 0x00, 0xF8, 0x1F, 0x00]);
    }
}
