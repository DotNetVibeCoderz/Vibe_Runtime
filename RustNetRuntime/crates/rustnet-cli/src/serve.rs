//! `rustnet serve` — the desktop virtual device.
//!
//! A real RNDP endpoint over TCP, so the Workbench, the Deploy tool and
//! anything else that speaks the protocol can be developed and demonstrated
//! against a machine rather than a board. It listens on `127.0.0.1:7878`,
//! which is the Workbench's own default spec, so pressing CONNECT with no
//! configuration reaches it.
//!
//! # It is a device, not a mock
//!
//! Everything here is the real thing wearing a different transport:
//! [`rustclr_device::handle`] answers the frames, [`rustclr_flashfs`] stores
//! the applications in a simulated NOR region, and `StartApp` runs the stored
//! assembly on an actual interpreter with the host board installed. What
//! differs from a firmware is the wire (a socket rather than a UART) and the
//! silicon underneath the HAL — not the protocol, the storage format, or the
//! runtime.
//!
//! That is the point: a bug in the service is found here, with a debugger
//! attached, instead of on a chip that has neither.
//!
//! # One connection at a time
//!
//! A device has one console and one flash. Serving two clients concurrently
//! would mean two of them flashing over each other, so a second connection
//! waits. This is not a server; it is a stand-in for a thing on the end of a
//! cable.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use rustclr_core::host::Host;
use rustclr_core::Interpreter;
use rustclr_device::DeviceOps;
use rustclr_hal::extmem::ExtMemory;
use rustclr_hal::{Board, HalResult};
use rustclr_hal_host::{HostBoard, SimExtMem};
use rustclr_rndp::{Decoder, Frame};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// How much simulated NOR the virtual device has.
///
/// 4 MB — the same order as the SPI part on a Maix Go, so an application that
/// fits here fits there. A number far larger would let something be developed
/// that could never be flashed.
const FLASH_BYTES: u32 = 4 * 1024 * 1024;

/// Log lines kept for `GET_LOGS`.
const LOG_LINES: usize = 200;

/// The flag the running application polls at its safe point.
///
/// A `static` rather than something allocated per run, because
/// [`Interpreter::cancel`] holds a `&'static` — see the note there on why it
/// cannot be an `Arc` on the targets this runtime also supports.
static CANCEL: AtomicBool = AtomicBool::new(false);

/// The screen the running application last presented.
type SharedScreen = Arc<Mutex<Option<(u32, u32, Vec<u16>)>>>;
/// Console output from the running application, newest last.
type SharedLog = Arc<Mutex<Vec<String>>>;

/// A board that keeps a copy of every frame the program presents.
///
/// Wrapping the simulator rather than replacing it: `Display.Present()` already
/// goes through `Board::present_frame`, which is exactly where a real panel
/// gets its pixels. Capturing there means `GET_DISPLAY` shows what the program
/// actually put on screen, through the same call a wired panel would.
struct CapturingBoard {
    inner: HostBoard,
    screen: SharedScreen,
}

impl Board for CapturingBoard {
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn present_frame(&mut self, rgb565: &[u16], width: u32, height: u32) -> HalResult<()> {
        if let Ok(mut screen) = self.screen.lock() {
            *screen = Some((width, height, rgb565.to_vec()));
        }
        self.inner.present_frame(rgb565, width, height)
    }
    fn gpio(&mut self, pin: u32) -> HalResult<&mut dyn rustclr_hal::gpio::GpioPin> {
        self.inner.gpio(pin)
    }
    fn i2c(&mut self, bus: u8) -> HalResult<&mut dyn rustclr_hal::i2c::I2cBus> {
        self.inner.i2c(bus)
    }
    fn spi(&mut self, bus: u8) -> HalResult<&mut dyn rustclr_hal::spi::SpiBus> {
        self.inner.spi(bus)
    }
    fn uart(&mut self, port: u8) -> HalResult<&mut dyn rustclr_hal::uart::Uart> {
        self.inner.uart(port)
    }
    fn i2s(&mut self, port: u8) -> HalResult<&mut dyn rustclr_hal::i2s::I2sBus> {
        self.inner.i2s(port)
    }
    fn pwm(&mut self, channel: u8) -> HalResult<&mut dyn rustclr_hal::pwm::PwmChannel> {
        self.inner.pwm(channel)
    }
    fn adc(&mut self, channel: u8) -> HalResult<&mut dyn rustclr_hal::adc::AdcChannel> {
        self.inner.adc(channel)
    }
    fn power(&mut self) -> &mut dyn rustclr_hal::power::PowerManager {
        self.inner.power()
    }
    fn delay(&mut self) -> &mut dyn rustclr_hal::delay::Delay {
        self.inner.delay()
    }
    fn can(&mut self, bus: u8) -> HalResult<&mut dyn rustclr_hal::can::CanBus> {
        self.inner.can(bus)
    }
    fn onewire(&mut self, bus: u8) -> HalResult<&mut dyn rustclr_hal::onewire::OneWireBus> {
        self.inner.onewire(bus)
    }
    fn rtc(&mut self) -> &mut dyn rustclr_hal::rtc::Rtc {
        self.inner.rtc()
    }
    fn watchdog(&mut self) -> &mut dyn rustclr_hal::watchdog::Watchdog {
        self.inner.watchdog()
    }
    fn extmem(&mut self, index: u8) -> HalResult<&mut dyn ExtMemory> {
        self.inner.extmem(index)
    }
    fn netif(
        &mut self,
        kind: rustclr_hal::netif::NetIfKind,
    ) -> HalResult<&mut dyn rustclr_hal::netif::NetInterface> {
        self.inner.netif(kind)
    }
    fn signal(&mut self, pin: u32) -> HalResult<&mut dyn rustclr_hal::signal::SignalControl> {
        self.inner.signal(pin)
    }
    fn camera(&mut self) -> HalResult<&mut dyn rustclr_hal::camera::Camera> {
        self.inner.camera()
    }
}

/// A console that keeps what the program printed, and echoes it.
///
/// Echoed as well as kept because somebody watching the terminal that started
/// `serve` should see the program run; kept because `GET_LOGS` is how the
/// Workbench shows the same thing to somebody who is not watching.
struct TeeHost {
    inner: rustclr_core::SystemHost,
    log: SharedLog,
}

impl TeeHost {
    fn record(&mut self, text: &str) {
        let Ok(mut log) = self.log.lock() else { return };
        for line in text.lines() {
            log.push(line.to_string());
        }
        // A device has a ring, not a transcript: a program that loops forever
        // would otherwise turn its log into the machine's memory problem.
        if log.len() > LOG_LINES {
            let excess = log.len() - LOG_LINES;
            log.drain(..excess);
        }
    }
}

impl Host for TeeHost {
    fn write_out(&mut self, text: &str) {
        self.record(text);
        self.inner.write_out(text);
    }
    fn write_err(&mut self, text: &str) {
        self.record(text);
        self.inner.write_err(text);
    }
    fn read_line(&mut self) -> Option<String> {
        self.inner.read_line()
    }
    fn monotonic_millis(&mut self) -> u64 {
        self.inner.monotonic_millis()
    }
    fn wall_clock_millis(&mut self) -> i64 {
        self.inner.wall_clock_millis()
    }
    fn monotonic_nanos(&mut self) -> u64 {
        self.inner.monotonic_nanos()
    }
}

/// The application currently running, and how to wait for it.
struct Running {
    name: String,
    handle: std::thread::JoinHandle<()>,
}

/// The desktop virtual device.
struct HostDevice {
    flash: SimExtMem,
    /// Where the flash region is kept between runs.
    image_path: PathBuf,
    running: Option<Running>,
    screen: SharedScreen,
    log: SharedLog,
    started: std::time::Instant,
}

impl HostDevice {
    fn new(image_path: PathBuf) -> Result<Self> {
        let mut flash = SimExtMem::qspi_flash(FLASH_BYTES);
        if image_path.exists() {
            // Restored rather than started empty, so an application flashed to
            // the virtual device is still there after a restart — which is
            // what makes it behave like a device rather than a cache.
            let saved = std::fs::read(&image_path)?;
            if saved.len() as u32 != FLASH_BYTES {
                return Err(format!(
                    "{} holds {} bytes and this build's flash is {FLASH_BYTES}. \
                     Delete it to start from an empty device.",
                    image_path.display(),
                    saved.len()
                )
                .into());
            }
            flash.write(0, &saved).map_err(|e| format!("{e:?}"))?;
        }
        Ok(Self {
            flash,
            image_path,
            running: None,
            screen: Arc::new(Mutex::new(None)),
            log: Arc::new(Mutex::new(Vec::new())),
            started: std::time::Instant::now(),
        })
    }

    /// Write the flash region back to disk.
    ///
    /// After every command that changed it, not on exit: `serve` is stopped
    /// with Ctrl-C, and a device whose contents survive only a graceful
    /// shutdown is not simulating flash.
    fn persist(&mut self) {
        let mut buffer = vec![0u8; FLASH_BYTES as usize];
        if self.flash.read(0, &mut buffer).is_err() {
            return;
        }
        if let Err(e) = std::fs::write(&self.image_path, &buffer) {
            eprintln!("[rustnet] could not save the device image: {e}");
        }
    }

    /// Reap the thread if the application has finished on its own.
    fn reap(&mut self) {
        if self.running.as_ref().is_some_and(|r| r.handle.is_finished()) {
            self.running = None;
        }
    }
}

impl DeviceOps for HostDevice {
    fn info(&mut self) -> rustclr_device::DeviceInfo {
        self.reap();
        let running = self.running.as_ref().map(|r| r.name.clone());
        rustclr_device::DeviceInfo {
            chip: "host".into(),
            board: "virtual device (rustnet serve)".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            uptime_ms: self.started.elapsed().as_millis() as u64,
            // The host allocator does not report its own usage, and a number
            // invented here would be shown as fact in the Workbench's readout.
            // Zero is the documented "cannot say".
            heap_used: 0,
            wifi: false,
            active_app: running.clone().unwrap_or_default(),
            running: running.is_some(),
            // `apps` and `autostart` are filled in by the service, which owns
            // the store; anything set here would be overwritten.
            ..Default::default()
        }
    }

    fn storage(&mut self) -> Option<&mut dyn ExtMemory> {
        Some(&mut self.flash)
    }

    fn logs(&mut self) -> String {
        self.log.lock().map(|l| l.join("\n")).unwrap_or_default()
    }

    fn perf(&mut self) -> String {
        self.reap();
        // `key=value` lines, which is what the Workbench's PERF pane parses.
        let mut out = String::new();
        out.push_str(&format!("uptime_s={}\n", self.started.elapsed().as_secs()));
        out.push_str(&format!("flash_used={}\n", rustclr_flashfs::used(&mut self.flash)));
        out.push_str(&format!("flash_total={FLASH_BYTES}\n"));
        out.push_str(&format!("app_running={}\n", u8::from(self.running.is_some())));
        out
    }

    fn display(&mut self) -> Option<(u32, u32, Vec<u16>)> {
        self.screen.lock().ok()?.clone()
    }

    fn start_app(&mut self, name: &str, image: Vec<u8>) -> std::result::Result<(), String> {
        self.reap();
        if let Some(running) = self.running.as_ref() {
            return Err(format!("`{}` is already running; stop it first", running.name));
        }

        // Cleared before the thread starts, not after it ends: a previous
        // application that was stopped left this set, and the new one would
        // exit at its first instruction.
        CANCEL.store(false, Ordering::Relaxed);
        if let Ok(mut log) = self.log.lock() {
            log.clear();
        }
        if let Ok(mut screen) = self.screen.lock() {
            *screen = None;
        }

        let screen = Arc::clone(&self.screen);
        let log = Arc::clone(&self.log);
        let label = name.to_string();
        let handle = std::thread::Builder::new()
            .name(format!("app:{name}"))
            .spawn(move || run_app(&label, image, screen, log))
            .map_err(|e| format!("could not start a thread for `{name}`: {e}"))?;

        self.running = Some(Running { name: name.to_string(), handle });
        Ok(())
    }

    fn stop_app(&mut self) -> std::result::Result<(), String> {
        self.reap();
        let Some(running) = self.running.take() else {
            return Ok(());
        };
        CANCEL.store(true, Ordering::Relaxed);
        // Joined rather than detached: the caller is entitled to assume that
        // when STOP_APP returns, the application is not still drawing.
        if running.handle.join().is_err() {
            return Err(format!("`{}` panicked while stopping", running.name));
        }
        Ok(())
    }

    fn running_app(&mut self) -> Option<String> {
        self.reap();
        self.running.as_ref().map(|r| r.name.clone())
    }

    fn reboot(&mut self) -> std::result::Result<(), String> {
        // A process cannot restart its own hardware, and pretending to would
        // leave the Workbench believing it had power-cycled something. What it
        // *can* do is honestly return to the state a reboot produces.
        self.stop_app()?;
        self.started = std::time::Instant::now();
        if let Ok(mut log) = self.log.lock() {
            log.clear();
        }
        if let Ok(mut screen) = self.screen.lock() {
            *screen = None;
        }
        Ok(())
    }
}

/// Run one application to completion on this thread.
fn run_app(name: &str, image: Vec<u8>, screen: SharedScreen, log: SharedLog) {
    let host = TeeHost { inner: rustclr_core::SystemHost::new(), log: Arc::clone(&log) };
    let mut interp = Interpreter::with_host(Box::new(host));
    rustclr_bcl::install(&mut interp);
    interp.board = Some(Box::new(CapturingBoard { inner: HostBoard::new(), screen }));
    interp.cancel = Some(&CANCEL);

    let outcome = rustclr_metadata::Image::from_bytes(image)
        .map_err(|e| format!("{e}"))
        .and_then(|image| interp.loader.load_image(image).map_err(|e| format!("{e}")))
        .and_then(|id| interp.run_entry_point(id).map_err(|e| format!("{e}")));

    let message = match outcome {
        Ok(code) => format!("[{name}] exited with {code}"),
        // A stop is not a failure, and reporting it as one would put a red
        // error in the Workbench every time somebody pressed the stop button.
        Err(e) if e.contains("stopped at an external request") => format!("[{name}] stopped"),
        Err(e) => format!("[{name}] failed: {e}"),
    };
    eprintln!("{message}");
    if let Ok(mut log) = log.lock() {
        log.push(message);
    }
}

/// Serve RNDP over TCP until interrupted.
pub fn serve(address: &str, image_path: Option<&str>) -> Result<i32> {
    let image_path = image_path
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new("rustnet-device.img").to_path_buf());
    let mut device = HostDevice::new(image_path.clone())?;

    let listener = TcpListener::bind(address)
        .map_err(|e| format!("could not listen on {address}: {e}"))?;
    println!("rustnet: virtual device on {address}");
    println!("rustnet: flash image {}", image_path.display());
    println!("rustnet: point the Workbench at tcp:{address}, or press CONNECT with its default");

    for stream in listener.incoming() {
        let mut stream = match stream {
            Ok(stream) => stream,
            Err(e) => {
                eprintln!("[rustnet] accept failed: {e}");
                continue;
            }
        };
        let peer = stream.peer_addr().map(|a| a.to_string()).unwrap_or_default();
        println!("rustnet: {peer} connected");
        if let Err(e) = converse(&mut device, &mut stream) {
            eprintln!("[rustnet] {peer}: {e}");
        }
        println!("rustnet: {peer} disconnected");
    }
    Ok(0)
}

/// Answer frames on one connection until it closes.
fn converse(device: &mut HostDevice, stream: &mut TcpStream) -> Result<()> {
    let mut decoder = Decoder::new();
    let mut buffer = [0u8; 8192];
    loop {
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            return Ok(());
        }
        decoder.push(&buffer[..read]);

        while let Some(request) = decoder.next_frame() {
            let name = rustclr_rndp::command_name(request.code).unwrap_or("?");
            let reply = rustclr_device::handle(device, &request);
            if !reply.is_ok() {
                // Printed on the serving side too: the Workbench shows the
                // message in a status bar that is easy to miss, and a refusal
                // nobody saw looks like a tool that did nothing.
                eprintln!("[rustnet] {name}: {}", reply.text());
            }
            stream.write_all(&Frame::encode(&reply))?;
            stream.flush()?;

            // After the reply, so a client waiting on it is not held up by a
            // four-megabyte write to disk.
            if changes_storage(request.code) {
                device.persist();
            }
        }
    }
}

/// Whether a command can have changed what is in flash.
///
/// Listed explicitly rather than persisting after every command: `GET_DISPLAY`
/// on a running program arrives many times a second, and writing the image
/// each time would make the virtual device slower than the real one.
fn changes_storage(code: u8) -> bool {
    use rustclr_rndp::*;
    matches!(code, CMD_FLASH_APP | CMD_ERASE_APP | CMD_SET_AUTOSTART | CMD_FLASH_DATA)
}
