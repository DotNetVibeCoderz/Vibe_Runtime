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

#[cfg(feature = "board-m5core2")]
mod m5core2;


/// The assembly to read, compiled by Roslyn for `net10.0` and linked into
/// flash. `HelloWorld.dll` by default — 4,608 bytes, small enough that the
/// whole image sits in the binary — or whatever `RUSTCLR_APP` named at
/// build time. See `build.rs`.
static HELLO_WORLD: &[u8] = include_bytes!(env!("RUSTCLR_APP_PATH"));

/// Assemblies the embedded program references, in dependency order.
///
/// A board has no filesystem, so what a desktop finds beside the program has
/// to be compiled in. `RUSTCLR_LIB1_PATH` and `RUSTCLR_LIB2_PATH` name them,
/// and the build script turns each one that is set into a `cfg`. A program
/// that references nothing — `HelloWorld` — sets neither and gets an empty
/// list, which costs nothing.
static APP_LIBRARIES: &[&[u8]] = &[
    #[cfg(rustclr_lib1)]
    include_bytes!(env!("RUSTCLR_LIB1_PATH")),
    #[cfg(rustclr_lib2)]
    include_bytes!(env!("RUSTCLR_LIB2_PATH")),
    #[cfg(rustclr_lib3)]
    include_bytes!(env!("RUSTCLR_LIB3_PATH")),
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
#[cfg(feature = "esp32")]
const BOARD: &str = "ESP32 (Xtensa LX6)";
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
#[cfg(feature = "esp32")]
const HEAP_BYTES: usize = 176 * 1024;

/// The ESP32's second DRAM bank. `dram2_seg` is 98,768 bytes; this is the
/// largest round number that fits.
#[cfg(feature = "esp32")]
const RECLAIMED_BYTES: usize = 96 * 1024;

/// What the allocator ends up with, which is what decides how much of RustBCL
/// will fit. See `rustclr_demo_common::Tier::for_budget`.
#[cfg(feature = "esp32c3")]
const TOTAL_HEAP: usize = HEAP_BYTES;
#[cfg(feature = "esp32")]
const TOTAL_HEAP: usize = HEAP_BYTES + RECLAIMED_BYTES;

#[main]
fn main() -> ! {
    let _peripherals = esp_hal::init(esp_hal::Config::default());
    esp_alloc::heap_allocator!(size: HEAP_BYTES);
    // The ESP32's second bank, which nothing else can use.
    #[cfg(feature = "esp32")]
    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: RECLAIMED_BYTES);

    #[cfg(feature = "esp32c3")]
    let board: Option<alloc::boxed::Box<dyn rustclr_hal::Board>> =
        Some(alloc::boxed::Box::new(rustclr_hal_esp32c3::Esp32C3Board::new()));

    // The Core2's panel. Nothing here works until the AXP192 has been told to
    // power the LCD rail, the backlight rail and the panel's reset — a board
    // whose SPI writes are perfect and whose PMIC was never configured keeps
    // whatever was in the panel's RAM from whatever ran before it.
    #[cfg(feature = "board-m5core2")]
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
                match m5core2::M5Core2Panel::new(i2c, spi, dc, cs) {
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
        Some(alloc::boxed::Box::new(m5core2::M5Core2Board::new(panel)))
    };

    // Every other Xtensa build: the ESP32's peripherals are a different set of
    // registers from the C3's and no crate drives them, so it reports no board
    // rather than one it cannot reach.
    #[cfg(not(any(feature = "esp32c3", feature = "board-m5core2")))]
    let board: Option<alloc::boxed::Box<dyn rustclr_hal::Board>> = None;

    rustclr_demo_common::run(&mut Console, BOARD, "", HELLO_WORLD, TOTAL_HEAP, board, None, APP_LIBRARIES);

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
