//! RNDP on a board: the service, polled from the interpreter's own loop.
//!
//! [`rustclr_device`] answers the frames and [`rustclr_rndp`] carries them;
//! this is the part that is specific to running on a microcontroller, and it
//! is shared by all five firmwares rather than written once per chip.
//!
//! # One UART, two users
//!
//! A board has a console, and the program running on it is already writing to
//! that console. RNDP shares the same wire: replies are framed and carry a
//! checksum, so the host can pick them out of the program's output, and
//! console text that happens to contain the magic bytes fails the checksum
//! rather than becoming a command. See [`rustclr_rndp::Decoder`].
//!
//! Nothing is written to the UART except in reply to a frame, so a board that
//! is never asked anything behaves exactly as it did before.
//!
//! # Why `START_APP` cannot start the application
//!
//! This is polled from inside the interpreter that is *running* a program. It
//! cannot load a second assembly into that interpreter and jump to it — the
//! frames of the current program are still on the stack, and its statics and
//! heap belong to it.
//!
//! So `START_APP` records the request and raises the cancel flag. The running
//! program unwinds at its next safe point, [`crate::run_stored_app`] returns
//! to the firmware's loop, and *that* builds a fresh interpreter for the new
//! application. The protocol's answer is "accepted", which is honest: the
//! device has agreed to run it, and will, once the thing it is doing stops.
//!
//! # What a board can offer depends on the board
//!
//! `LIST_APPS` and everything else that touches storage needs
//! [`Board::extmem`]; answering at all needs [`Board::uart`]. Several chip
//! HALs here provide neither yet. Such a board reports what it cannot do at
//! boot and refuses those commands by name, rather than appearing to accept
//! them.

use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use core::sync::atomic::{AtomicBool, Ordering};

use rustclr_core::{DevicePoll, Interpreter};
use rustclr_device::{DeviceInfo, DeviceOps};
use rustclr_hal::extmem::ExtMemory;
use rustclr_hal::Board;
use rustclr_rndp::Decoder;

/// The console UART, which is also where RNDP is spoken.
const CONSOLE_PORT: u8 = 0;

/// The flash window applications are kept in.
///
/// Window 1, not 0: on the K210 window 0 is the firmware's own record store
/// and window 1 is the filesystem's, and they are separate precisely so a
/// filesystem that fills up and compacts cannot erase the records. A board
/// with only one window will answer `NotSupported` here, which is the right
/// answer rather than a shared region.
pub(crate) const STORAGE_WINDOW: u8 = 1;

/// Bytes read from the UART per poll.
///
/// Small on purpose. This runs on the same thread as the program, between two
/// of its instructions, so a large read is a long pause in whatever the board
/// is doing. A frame spread over several polls is reassembled by the decoder.
const READ_CHUNK: usize = 64;

/// How many instructions pass between two polls.
///
/// # Why this is small, and why it is still not enough on its own
///
/// The obvious reasoning — "the UART has a FIFO, so a late poll loses nothing"
/// — is wrong here in two ways.
///
/// The K210's UARTHS FIFO is **eight bytes**, and the smallest frame is nine.
/// So a request that is not drained promptly is not merely delayed, it is
/// *truncated*, and the host sees silence rather than an error.
///
/// And an instruction count is a poor proxy for time, because instructions
/// here differ by four orders of magnitude: `add` is nanoseconds and
/// `Display.Present()` is a single IL `call` that spends tens of milliseconds
/// inside a native binding. A graphics program can therefore go seconds
/// between two IL instructions while looking perfectly busy.
///
/// 256 keeps the cost negligible — one decrement per instruction, a peripheral
/// read every few hundred — and makes a poll frequent enough for any program
/// whose time goes on interpreted code. It does **not** fix the long-native-call
/// case; only interrupt-driven receive does, and that is noted in
/// `docs/limitations.md` rather than pretended away.
const POLL_INTERVAL: u32 = 256;

/// Log lines kept for `GET_LOGS`.
///
/// A ring, not a transcript: a device program loops forever, and a log that
/// only grows is a board that eventually stops for want of memory.
const LOG_LINES: usize = 64;

/// Set when a host asks the running program to stop.
///
/// A `static` because [`Interpreter::cancel`] holds a `&'static` — two of the
/// bare-metal targets here have no compare-and-swap, so this cannot be an
/// `Arc`. One board, one running program, one flag.
pub static CANCEL: AtomicBool = AtomicBool::new(false);

/// What the service wants the firmware to do once the current program stops.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pending {
    /// Nothing; carry on.
    None,
    /// Start this application after the current one unwinds.
    Start(String),
    /// Stop the current application and run nothing.
    Stop,
    /// Restart the board.
    Reboot,
}

/// The board-side RNDP service.
pub struct DeviceService {
    decoder: Decoder,
    countdown: u32,
    /// What the firmware should do next. Read and cleared by the outer loop.
    pending: Pending,
    /// The application running now, if the firmware started one from storage.
    running: Option<String>,
    /// Console output, most recent last.
    log: Vec<String>,
    chip: &'static str,
    board_name: String,
    /// Milliseconds at which this board started, from the host clock.
    booted_at_ms: u64,
}

impl DeviceService {
    pub fn new(chip: &'static str, board_name: &str) -> Self {
        Self {
            decoder: Decoder::new(),
            countdown: POLL_INTERVAL,
            pending: Pending::None,
            running: None,
            log: Vec::new(),
            chip,
            board_name: board_name.to_string(),
            booted_at_ms: 0,
        }
    }

    /// Note which application the firmware has started.
    pub fn set_running(&mut self, name: Option<&str>) {
        self.running = name.map(|n| n.to_string());
    }

    /// Take whatever the host asked for, leaving nothing behind.
    pub fn take_pending(&mut self) -> Pending {
        core::mem::replace(&mut self.pending, Pending::None)
    }

    /// Record a line the program printed, for `GET_LOGS`.
    pub fn log_line(&mut self, line: &str) {
        self.log.push(line.to_string());
        if self.log.len() > LOG_LINES {
            let excess = self.log.len() - LOG_LINES;
            self.log.drain(..excess);
        }
    }

    /// Whether this board can answer RNDP at all, and what it cannot do.
    ///
    /// Reported at boot rather than discovered a command at a time: somebody
    /// pointing the Workbench at a board deserves to know from its own console
    /// that it has no storage, instead of from a failed FLASH_APP.
    pub fn capability(board: &mut dyn Board) -> Capability {
        Capability {
            console: board.uart(CONSOLE_PORT).is_ok(),
            storage: board.extmem(STORAGE_WINDOW).is_ok(),
        }
    }

    /// Read whatever has arrived, answer any complete frames.
    pub fn service_once(&mut self, interp: &mut Interpreter) {
        let mut buffer = [0u8; READ_CHUNK];
        let read = match interp.board.as_mut() {
            Some(board) => match board.uart(CONSOLE_PORT) {
                Ok(uart) => uart.read(&mut buffer).unwrap_or(0),
                Err(_) => return,
            },
            None => return,
        };
        if read > 0 {
            self.decoder.push(&buffer[..read]);
        }

        // Frames are drained in a loop: a host that pipelined two commands
        // into one write would otherwise have the second answered a whole
        // poll interval later.
        while let Some(request) = self.decoder.next_frame() {
            let reply = {
                let mut ops = BoardOps { service: self, interp };
                rustclr_device::handle(&mut ops, &request)
            };
            let bytes = reply.encode();
            if let Some(board) = interp.board.as_mut() {
                if let Ok(uart) = board.uart(CONSOLE_PORT) {
                    // Written in full, looping over partial writes. A UART
                    // takes what fits in its transmit FIFO and reports how
                    // much that was; the ESP32's is 128 bytes and an INFO
                    // reply is longer, so a single write sends a *truncated*
                    // frame. The host then fails its checksum, resynchronises
                    // and reports nothing at all -- which looks like a board
                    // that ignored the command rather than one that answered
                    // it badly. PING fit and worked; INFO did not and did not.
                    //
                    // `write` makes progress on any non-empty slice, so this
                    // terminates; a driver that returned zero anyway would
                    // spin, hence the explicit break rather than a bare loop.
                    let mut sent = 0;
                    while sent < bytes.len() {
                        match uart.write(&bytes[sent..]) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => sent += n,
                        }
                    }
                }
            }
        }
    }
}

/// What a board can do over RNDP.
#[derive(Debug, Clone, Copy)]
pub struct Capability {
    /// A readable console UART, without which nothing can be answered.
    pub console: bool,
    /// A flash window, without which applications cannot be stored.
    pub storage: bool,
}

impl Capability {
    /// A sentence for the boot banner, or `None` when everything works.
    pub fn shortfall(&self) -> Option<&'static str> {
        match (self.console, self.storage) {
            (true, true) => None,
            (true, false) => Some("no flash window: apps cannot be stored, only inspected"),
            (false, true) => Some("no readable console UART: nothing can be answered"),
            (false, false) => Some("neither a readable console UART nor a flash window"),
        }
    }
}

impl DevicePoll for DeviceService {
    fn poll(&mut self, interp: &mut Interpreter) {
        // A decrement and a compare, the same shape the profiler's tick uses.
        self.countdown -= 1;
        if self.countdown != 0 {
            return;
        }
        self.countdown = POLL_INTERVAL;

        if self.booted_at_ms == 0 {
            self.booted_at_ms = interp.host.monotonic_millis();
        }
        self.service_once(interp);
    }

    fn into_any(self: Box<Self>) -> Box<dyn core::any::Any> {
        self
    }
}

/// The service and the interpreter, together, as a device.
///
/// Built for the duration of one command and dropped: it borrows the
/// interpreter, which the service cannot hold on to.
struct BoardOps<'a> {
    service: &'a mut DeviceService,
    interp: &'a mut Interpreter,
}

impl DeviceOps for BoardOps<'_> {
    fn info(&mut self) -> DeviceInfo {
        let now = self.interp.host.monotonic_millis();
        DeviceInfo {
            chip: self.service.chip.to_string(),
            board: self.service.board_name.clone(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            uptime_ms: now.saturating_sub(self.service.booted_at_ms),
            // What the managed heap holds. Not the whole allocator's usage --
            // the firmware's arena does not report itself -- so this is the
            // part the runtime can actually account for.
            heap_used: self.interp.heap.live_bytes() as u64,
            wifi: self
                .interp
                .board
                .as_mut()
                .and_then(|b| b.netif(rustclr_hal::netif::NetIfKind::Wifi).ok())
                .is_some_and(|n| n.is_up()),
            active_app: self.service.running.clone().unwrap_or_default(),
            running: self.service.running.is_some(),
            ..Default::default()
        }
    }

    fn storage(&mut self) -> Option<&mut dyn ExtMemory> {
        self.interp.board.as_mut()?.extmem(STORAGE_WINDOW).ok()
    }

    fn logs(&mut self) -> String {
        self.service.log.join("\n")
    }

    fn perf(&mut self) -> String {
        let stats = self.interp.heap.stats();
        let mut out = String::new();
        push_kv(&mut out, "instructions", self.interp.stats.instructions_executed);
        push_kv(&mut out, "calls", self.interp.stats.calls);
        push_kv(&mut out, "heap_live", self.interp.heap.live_bytes() as u64);
        push_kv(&mut out, "heap_peak", stats.peak_live_bytes as u64);
        push_kv(&mut out, "allocations", stats.total_allocations);
        push_kv(&mut out, "collections", stats.collections);
        out
    }

    fn display(&mut self) -> Option<(u32, u32, Vec<u16>)> {
        let canvas = self.interp.canvas.as_ref()?;
        Some((canvas.width, canvas.height, canvas.pixels.clone()))
    }

    fn start_app(&mut self, name: &str, _image: Vec<u8>) -> Result<(), String> {
        // Recorded, not done -- see the module note. The image is dropped
        // rather than kept: it came out of flash and the firmware will read it
        // again, and holding a second copy of an assembly on a board with a
        // few hundred kilobytes is exactly the wrong thing to do.
        self.service.pending = Pending::Start(name.to_string());
        CANCEL.store(true, Ordering::Relaxed);
        Ok(())
    }

    fn stop_app(&mut self) -> Result<(), String> {
        self.service.pending = Pending::Stop;
        CANCEL.store(true, Ordering::Relaxed);
        Ok(())
    }

    fn running_app(&mut self) -> Option<String> {
        self.service.running.clone()
    }

    fn reboot(&mut self) -> Result<(), String> {
        self.service.pending = Pending::Reboot;
        CANCEL.store(true, Ordering::Relaxed);
        Ok(())
    }
}

/// `key=value` on its own line, which is what the Workbench's perf pane reads.
fn push_kv(out: &mut String, key: &str, value: u64) {
    use core::fmt::Write as _;
    let _ = writeln!(out, "{key}={value}");
}

/// Install the service on an interpreter about to run a program.
pub fn attach(interp: &mut Interpreter, service: Box<DeviceService>) {
    CANCEL.store(false, Ordering::Relaxed);
    interp.cancel = Some(&CANCEL);
    interp.device = Some(service);
}

/// Take the service back off, so the next program gets the same one.
///
/// The service outlives the interpreter deliberately: the log, the stored
/// application list and the uptime all belong to the *board*, and starting a
/// new program must not look like a reboot to whoever is watching.
pub fn detach(interp: &mut Interpreter) -> Option<Box<DeviceService>> {
    interp.device.take()?.into_any().downcast::<DeviceService>().ok()
}
