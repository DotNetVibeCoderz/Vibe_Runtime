//! RustCLR on STM32F4 — a Nucleo-F401RE and a Netduino 3 WiFi.
//!
//! Two boards from one source file, as with the ESP32 demo. They are the same
//! Cortex-M4F core at different sizes, and the difference in size is the whole
//! point of having both:
//!
//! | | Nucleo-F401RE | Netduino 3 WiFi |
//! | --- | --- | --- |
//! | Part | STM32F401RET6 | STM32F427VIT6 |
//! | Clock | 84 MHz from the HSI | 168 MHz from a 25 MHz HSE |
//! | RAM | 96 KB | 192 KB + 64 KB CCM |
//! | Console | USART2, PA2 — the ST-Link's virtual COM port | UART7, PE8 |
//! | LED | LD2 on PA5 | USR_LED on PA10 |
//! | **Runs C#** | **no** | **yes, reduced bindings** |
//!
//! # The last row is the interesting one
//!
//! Loading the runtime costs 192,045 bytes with the smallest useful set of
//! RustBCL bindings — console, strings and maths. The F401RE has 96 KB of RAM
//! in total. No arrangement of that memory runs a C# program, so this firmware
//! does not pretend otherwise: it reads the assembly, exercises the collector,
//! and prints a line saying how much memory the interpreter wanted and how
//! much the board has. Discovering the same fact as an allocator panic would
//! tell the user less.
//!
//! The F427VI gets there, but only by putting its memories to unusual use —
//! see `memory-f427vi.x`. Its 192 KB of DMA-reachable SRAM becomes the heap in
//! one piece, and `.data`, `.bss` and the stack move into the 64 KB of CCM
//! that could not have held the heap anyway. That is the same shape of trick
//! as the ESP32-WROOM-32's second DRAM bank, for the same reason: the runtime
//! needs one large contiguous arena, and these parts do not offer one by
//! default.
//!
//! # Neither board has been flashed
//!
//! No F401RE or F427VI was connected when this was written. Both images build;
//! the run is the step outstanding, and the tables in `docs/limitations.md`
//! say so rather than implying hardware verification that did not happen.
//!
//! Flashing needs a probe — there is no bootloader involved:
//!
//! ```text
//! probe-rs run --chip STM32F401RETx target/thumbv7em-none-eabihf/release/rustclr-stm32f4
//! probe-rs run --chip STM32F427VITx target/thumbv7em-none-eabihf/release/rustclr-stm32f4
//! ```

#![no_std]
#![no_main]

extern crate alloc;

use core::fmt::Write as _;
use core::mem::MaybeUninit;
use core::ptr::addr_of_mut;

use alloc::string::String;
use cortex_m_rt::entry;
use embedded_alloc::LlffHeap;
use stm32f4xx_hal::{pac, prelude::*};

/// The assembly to read, compiled by Roslyn for `net10.0` and linked into
/// flash. `HelloWorld.dll` by default — 4,608 bytes, small enough that the
/// whole image sits in the binary — or whatever `RUSTCLR_APP` named at
/// build time. See `build.rs`.
static HELLO_WORLD: &[u8] = include_bytes!(env!("RUSTCLR_APP_PATH"));

#[cfg(feature = "nucleo-f401re")]
const BOARD: &str = "Nucleo-F401RE (STM32F401RE, Cortex-M4F)";
#[cfg(feature = "netduino3-f427vi")]
const BOARD: &str = "Netduino 3 WiFi (STM32F427VI, Cortex-M4F)";

/// How much of the part's RAM the allocator gets.
///
/// **F401RE: 64 KB of 96.** The remainder covers `.data`, `.bss` and the
/// stack. There is no figure here that would let the interpreter load, so this
/// is sized for the metadata reader and the collector and nothing more.
///
/// **F427VI: 192 KB**, which is all of the DMA-reachable SRAM, because
/// everything else was moved to CCM to free it. That clears the reduced
/// binding set by 4,563 bytes — a real margin, but a thin one, and worth
/// re-measuring if RustBCL grows.
#[cfg(feature = "nucleo-f401re")]
const HEAP_BYTES: usize = 64 * 1024;
#[cfg(feature = "netduino3-f427vi")]
const HEAP_BYTES: usize = 192 * 1024;

#[global_allocator]
static ALLOCATOR: LlffHeap = LlffHeap::empty();

/// The allocator's arena.
///
/// `MaybeUninit` rather than `[0; N]` because the allocator does not need it
/// zeroed, and on the F427VI it lives in a `(NOLOAD)` section that startup
/// never touches. On the F401RE it is an ordinary `.bss` static.
#[cfg(feature = "nucleo-f401re")]
static mut HEAP_MEMORY: MaybeUninit<[u8; HEAP_BYTES]> = MaybeUninit::uninit();

/// On the F427VI this must **not** land in `.bss`: `.bss` is in CCM, and the
/// heap is the reason all of SRAM was kept free. The section name matches the
/// one `memory-f427vi.x` places in `SRAM`.
#[cfg(feature = "netduino3-f427vi")]
#[link_section = ".sram_heap"]
static mut HEAP_MEMORY: MaybeUninit<[u8; HEAP_BYTES]> = MaybeUninit::uninit();

#[entry]
fn main() -> ! {
    let dp = pac::Peripherals::take().unwrap();

    // Before anything allocates. `run` below builds a `String`.
    unsafe {
        ALLOCATOR.init(addr_of_mut!(HEAP_MEMORY) as usize, HEAP_BYTES);
    }

    #[cfg(feature = "nucleo-f401re")]
    {
        // No crystal is fitted on a Nucleo by default — the ST-Link can feed
        // MCO, but the HSI is accurate enough for a console and needs no
        // assumption about what is on the board.
        let rcc = dp.RCC.constrain();
        let clocks = rcc.cfgr.sysclk(84.MHz()).freeze();

        let gpioa = dp.GPIOA.split();
        // PA2 is USART2_TX, which the on-board ST-Link presents to the host as
        // a virtual COM port. Nothing external needs wiring.
        let mut tx = dp.USART2.tx(gpioa.pa2, 115_200.bps(), &clocks).unwrap();
        let mut led = gpioa.pa5.into_push_pull_output();

        report(&mut tx, clocks.sysclk().raw(), "USART2 PA2, ST-Link VCP");
        led.set_high();
    }

    #[cfg(feature = "netduino3-f427vi")]
    {
        // 25 MHz crystal. `sysclk` asks the HAL for 168 MHz and it solves the
        // PLL dividers; the F427 tops out there without over-volting.
        let rcc = dp.RCC.constrain();
        let clocks = rcc.cfgr.use_hse(25.MHz()).sysclk(168.MHz()).freeze();

        let gpioa = dp.GPIOA.split();
        let gpioe = dp.GPIOE.split();
        // PE8 is UART7_TX on the goPort2 header. This board has no USB-serial
        // bridge to the target, so a host needs an adapter on that pin.
        let mut tx = dp.UART7.tx(gpioe.pe8, 115_200.bps(), &clocks).unwrap();
        let mut led = gpioa.pa10.into_push_pull_output();

        report(&mut tx, clocks.sysclk().raw(), "UART7 PE8, goPort2 header");
        led.set_high();
    }

    loop {
        // Nothing left to do; the demonstration is the output above. `wfi`
        // rather than a spin so the part idles instead of burning current.
        cortex_m::asm::wfi();
    }
}

/// Prints the shared report, with a line about how this board got here.
///
/// Generic over the writer because the two boards' `Tx` types differ — that is
/// the only reason this is a function rather than inline.
fn report<W: core::fmt::Write>(out: &mut W, sysclk_hz: u32, console: &str) {
    let mut detail = String::new();
    let _ = write!(
        detail,
        "sysclk           {} MHz\nconsole          {console}\nheap             {} KB",
        sysclk_hz / 1_000_000,
        HEAP_BYTES / 1024
    );
    rustclr_demo_common::run(out, BOARD, detail.as_str(), HELLO_WORLD, HEAP_BYTES);
}

/// A panic has nowhere to unwind to here.
///
/// Deliberately silent: the console is owned by `main` by the time anything can
/// panic, and reaching for it from here would need a static and a critical
/// section to hand out a second one. A halted board with its LED off is the
/// signal.
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        cortex_m::asm::wfi();
    }
}
