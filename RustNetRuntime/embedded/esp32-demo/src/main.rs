//! RustCLR on ESP32 hardware.
//!
//! Builds for two chips from one source file: the ESP32-WROOM-32 (Xtensa LX6)
//! and the ESP32-C3 (RISC-V). Nothing below is core-specific — which is the
//! point, since the runtime is meant to be portable across exactly this kind of
//! gap.
//!
//! This is the firmware that turns "the crates compile for a microcontroller"
//! into "the code runs on one". It does three things on real silicon:
//!
//! 1. **Parses a real assembly.** `HelloWorld.dll`, built by Roslyn on a
//!    desktop, is embedded in flash and read by `rustclr-metadata` — PE header,
//!    CLI header, metadata tables, string heap, the entry point's name. Nothing
//!    about that code path is embedded-specific; it is the same reader the
//!    desktop runtime uses.
//!
//! 2. **Executes it.** The loader builds a type registry, RustBCL supplies
//!    `System.Console` and the rest as native functions, and the interpreter
//!    runs `Main`. The C# prints from the chip, and prints the same bytes it
//!    prints on a desktop.
//!
//! 3. **Runs the collector.** `rustclr-gc` allocates into a heap with a hard
//!    slot ceiling, builds a reference cycle, drops the root, and collects —
//!    proving both that the mark-sweep handles cycles and that a fixed heap
//!    refuses to grow past its budget rather than exhausting the chip's RAM.

#![no_std]
#![no_main]

extern crate alloc;


use esp_backtrace as _;
use esp_hal::main;

#[cfg(feature = "board-m5")]
mod m5;
mod flash;
mod uart;

/// Which M5 board this image is built for.
///
/// A build-time choice because it decides one PMIC register, and getting it
/// wrong on a Core2 turns its vibration motor on and leaves it on.
#[cfg(feature = "board-m5tough")]
const VARIANT: m5::Variant = m5::Variant::Tough;
#[cfg(all(feature = "board-m5core2", not(feature = "board-m5tough")))]
const VARIANT: m5::Variant = m5::Variant::Core2;


/// The assembly to read, compiled by Roslyn for `net10.0` and linked into
/// flash. `HelloWorld.dll` by default — 4,608 bytes, small enough that the
/// whole image sits in the binary — or whatever `RUSTCLR_APP` named at
/// build time. See `build.rs`.
static HELLO_WORLD: &[u8] = include_bytes!(env!("RUSTCLR_APP_PATH"));

/// Assemblies the embedded program references, in dependency order.
///
/// A board has no filesystem, so what a desktop finds beside the program has
/// to be compiled in. `RUSTCLR_LIB1_PATH`..`RUSTCLR_LIB8_PATH` name them, and
/// the build script turns each one that is set into a `cfg`. A program that
/// references nothing — `HelloWorld` — sets none and gets an empty list, which
/// costs nothing.
static APP_LIBRARIES: &[&[u8]] = &[
    #[cfg(rustclr_lib1)]
    include_bytes!(env!("RUSTCLR_LIB1_PATH")),
    #[cfg(rustclr_lib2)]
    include_bytes!(env!("RUSTCLR_LIB2_PATH")),
    #[cfg(rustclr_lib3)]
    include_bytes!(env!("RUSTCLR_LIB3_PATH")),
    #[cfg(rustclr_lib4)]
    include_bytes!(env!("RUSTCLR_LIB4_PATH")),
    #[cfg(rustclr_lib5)]
    include_bytes!(env!("RUSTCLR_LIB5_PATH")),
    #[cfg(rustclr_lib6)]
    include_bytes!(env!("RUSTCLR_LIB6_PATH")),
    #[cfg(rustclr_lib7)]
    include_bytes!(env!("RUSTCLR_LIB7_PATH")),
    #[cfg(rustclr_lib8)]
    include_bytes!(env!("RUSTCLR_LIB8_PATH")),
];


// The ESP-IDF second-stage bootloader reads a descriptor out of the image
// header — version, project name, build time — and refuses an image without
// one. This macro emits it.
esp_bootloader_esp_idf::esp_app_desc!();

/// The chip this firmware was built for, for the banner.
///
/// The *chip*, not the board. A firmware knows which part it was compiled for
/// and has no way to tell which board that part is soldered to — this said
/// "ESP32-WROOM-32" until it was run on an M5Stack Tough, which is a different
/// board carrying the same ESP32. Naming what is actually known keeps the
/// report from asserting something it cannot check; which board a capture came
/// from belongs in the log's header, where a person writes it.
#[cfg(all(feature = "esp32", not(feature = "board-m5")))]
const BOARD: &str = "ESP32 (Xtensa LX6)";
#[cfg(feature = "board-m5tough")]
const BOARD: &str = "M5Stack Tough (ESP32, Xtensa LX6)";
#[cfg(all(feature = "board-m5core2", not(feature = "board-m5tough")))]
const BOARD: &str = "M5Stack Core2 (ESP32, Xtensa LX6)";
#[cfg(feature = "esp32c3")]
const BOARD: &str = "ESP32-C3 (RISC-V 32)";

/// How much RAM the allocator gets, and where it comes from.
///
/// Was 64 KB when this firmware only parsed metadata. Running the interpreter
/// needs far more, and the figure is measured rather than guessed — a first
/// attempt at 192 KB died inside `rustclr_bcl::install`, which is a clearer
/// way to learn this than reasoning about it:
///
/// | | bytes |
/// | --- | ---: |
/// | loader, 202 pre-registered framework types | 127 K |
/// | managed heap, 512 slots at 41 bytes | 21 K |
/// | RustBCL, 826 native bindings | 106 K |
/// | loading and running `HelloWorld` | 6 K |
/// | **peak** | **260,702** |
///
/// The two chips reach that total differently, and the difference is the
/// interesting part.
///
/// **The C3** has one contiguous DRAM segment and 288 KB fits in it. That is
/// close to the ceiling: at 320 KB the linker rejects the image outright —
/// `.bss will not fit in region DRAM, overflowed by 13844 bytes`, with the
/// stack being squeezed out. Being told at link time rather than discovering
/// it as a crash is the reason to size the heap statically.
///
/// **The WROOM-32** cannot do it with one segment at all. Its `dram_seg` tops
/// out at 176 KB here — measured by bisecting until the link succeeded — which
/// is below even the reduced binding set. What rescues it is a second bank:
/// the ESP32 has 98,768 bytes at `0x3ffe7e30`, past the ROM's data and stacks,
/// which esp-hal exposes as `#[ram(reclaimed)]` and which the linker will not
/// place normal statics in. `esp-alloc` takes regions rather than a single
/// arena, so both are added and the allocator treats them as one heap.
///
/// A single allocation still cannot span the two. The largest the runtime
/// makes is 67,584 bytes, and both regions clear that comfortably.
#[cfg(feature = "esp32c3")]
const HEAP_BYTES: usize = 288 * 1024;
#[cfg(all(feature = "esp32", not(feature = "psram")))]
const HEAP_BYTES: usize = 176 * 1024;

/// Internal DRAM for the allocator when PSRAM is carrying the bulk.
///
/// **The stack is what is left of `dram_seg` after `.bss`**, and esp-alloc's
/// arena is a static array in `.bss`. Asking for 176 KB of a ~200 KB segment
/// therefore leaves the stack a sliver, and the board dies with "write to the
/// stack guard value" — which is what it did, and which reads like a runaway
/// recursion rather than a linker arithmetic problem.
///
/// With PSRAM registered behind it, internal memory only has to cover what
/// must be internal, so most of the segment can stay stack.
#[cfg(all(feature = "esp32", feature = "psram"))]
const HEAP_BYTES: usize = 64 * 1024;

/// The ESP32's second DRAM bank. `dram2_seg` is 98,768 bytes; this is the
/// largest round number that fits.
///
/// # This bank is not entirely ours, and that is measured
///
/// It works for a small program — an M5Stack Tough ran `HelloWorld` out of it
/// — and it **corrupts under pressure**. Loading a second assembly after
/// RustBCL has taken its share produced, on a Tough, an `Image` whose bytes
/// were byte-perfect (`BSJB` at offset 620, exactly the host's) and whose
/// stored metadata range had been overwritten between one call and the next.
/// The same firmware on a Core2 died as `LoadProhibited`. Different symptoms,
/// one cause.
///
/// Building with `--features one-bank` leaves this alone, and the same board
/// then says `bcl tier none - SKIPPED` and keeps running — a clean refusal
/// instead of a corrupted heap. That is the diagnostic: if a crash goes away
/// with one bank, the second bank is the crash.
///
/// 96 KB is a guess inherited from the linker's `dram2_seg` size, and the
/// evidence says it is too large. What is actually free above the ROM's data
/// and stacks has not been measured, so this is a hazard that is documented
/// rather than fixed.
#[cfg(all(feature = "esp32", not(feature = "one-bank")))]
const RECLAIMED_BYTES: usize = 96 * 1024;


#[main]
fn main() -> ! {
    let _peripherals = esp_hal::init(esp_hal::Config::default());
    esp_alloc::heap_allocator!(size: HEAP_BYTES);
    // The ESP32's second bank, which nothing else can use.
    #[cfg(all(feature = "esp32", not(feature = "one-bank")))]
    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: RECLAIMED_BYTES);

    // PSRAM last, so it is reached only when internal DRAM is gone. See the
    // `psram` feature in Cargo.toml for what that costs and why it is opt-in.
    #[cfg(all(feature = "esp32", feature = "psram"))]
    esp_alloc::psram_allocator!(_peripherals.PSRAM, esp_hal::psram);

    #[cfg(feature = "esp32c3")]
    let board: Option<alloc::boxed::Box<dyn rustclr_hal::Board>> =
        Some(alloc::boxed::Box::new(rustclr_hal_esp32c3::Esp32C3Board::new()));

    // The Core2's panel. Nothing here works until the AXP192 has been told to
    // power the LCD rail, the backlight rail and the panel's reset — a board
    // whose SPI writes are perfect and whose PMIC was never configured keeps
    // whatever was in the panel's RAM from whatever ran before it.
    #[cfg(feature = "board-m5")]
    let board: Option<alloc::boxed::Box<dyn rustclr_hal::Board>> = {
        use esp_hal::gpio::{Level, Output, OutputConfig};
        let i2c = esp_hal::i2c::master::I2c::new(
            _peripherals.I2C0,
            esp_hal::i2c::master::Config::default(),
        )
        .map(|bus| bus.with_sda(_peripherals.GPIO21).with_scl(_peripherals.GPIO22));
        let spi = esp_hal::spi::master::Spi::new(
            _peripherals.SPI2,
            esp_hal::spi::master::Config::default()
                .with_frequency(esp_hal::time::Rate::from_mhz(40))
                .with_mode(esp_hal::spi::Mode::_0),
        )
        .map(|bus| bus.with_sck(_peripherals.GPIO18).with_mosi(_peripherals.GPIO23));

        let panel = match (i2c, spi) {
            (Ok(i2c), Ok(spi)) => {
                let dc = Output::new(_peripherals.GPIO15, Level::High, OutputConfig::default());
                let cs = Output::new(_peripherals.GPIO5, Level::High, OutputConfig::default());
                match m5::M5Panel::new(i2c, spi, dc, cs, VARIANT) {
                    Ok(mut panel) => {
                        // Green, before any managed code runs. It separates
                        // "the bus is dead" from "the framebuffer never
                        // reached present", and those look identical on a
                        // screen showing a previous firmware's picture.
                        let _ = panel.fill(0x07E0);
                        Some(panel)
                    }
                    Err(_) => None,
                }
            }
            _ => None,
        };
        // UART0 on its default pins, which is the USB bridge and the console.
        // Taken here rather than inside the board because the peripherals are
        // owned by `main` and handed out once.
        let uart = uart::Esp32Uart::new(
            _peripherals.UART0,
            _peripherals.GPIO1,
            _peripherals.GPIO3,
        );
        // The `storage` partition, found in the table rather than assumed.
        let store = flash::EspFlash::open(_peripherals.FLASH);
        Some(alloc::boxed::Box::new(m5::M5Board::new(panel, VARIANT, uart, store)))
    };

    // Every other Xtensa build: the ESP32's peripherals are a different set of
    // registers from the C3's and no crate drives them, so it reports no board
    // rather than one it cannot reach.
    #[cfg(not(any(feature = "esp32c3", feature = "board-m5")))]
    let board: Option<alloc::boxed::Box<dyn rustclr_hal::Board>> = None;

    // What the allocator actually holds, asked at boot rather than summed at
    // compile time. PSRAM is auto-detected, so its size is not a constant —
    // and the figure that decides whether RustBCL fits should be the real one
    // in any case.
    let budget = esp_alloc::HEAP.free();
    rustclr_demo_common::run(
        &mut Console,
        BOARD,
        "",
        HELLO_WORLD,
        budget,
        board,
        Some(alloc::boxed::Box::new(Console)),
        APP_LIBRARIES,
    );

    loop {
        // Nothing left to do; the demonstration is the output above.
        core::hint::spin_loop();
    }
}

/// `esp-println` writes to the console directly, so this only has to forward.
struct Console;

impl core::fmt::Write for Console {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        esp_println::print!("{s}");
        Ok(())
    }
}

/// The program's console: straight out, no buffering.
///
/// A device program's output has to appear *as it is written*. Buffering it
/// until the entry point returns works for `HelloWorld` and hides everything a
/// program that loops forever ever says — which is most of them, and which
/// looks exactly like a board that failed to start.
impl rustclr_demo_common::Host for Console {
    fn write_out(&mut self, text: &str) {
        esp_println::print!("{text}");
    }

    fn write_err(&mut self, text: &str) {
        esp_println::print!("{text}");
    }

    fn read_line(&mut self) -> Option<alloc::string::String> {
        // Output only: the same UART carries the flasher, and reading it would
        // race whatever is watching.
        None
    }

    fn monotonic_millis(&mut self) -> u64 {
        esp_hal::time::Instant::now()
            .duration_since_epoch()
            .as_millis()
    }

    fn wall_clock_millis(&mut self) -> i64 {
        // No RTC battery is assumed, so there is no wall clock to report.
        0
    }
}
