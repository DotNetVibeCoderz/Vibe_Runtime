//! RustCLR on a Kendryte **K210** — dual RV64GC, Sipeed Maix Go.
//!
//! The fifth board and the second RISC-V one, but the first 64-bit RISC-V: the
//! ESP32-C3 is RV32IMC. The report it prints is byte-identical to the ones from
//! Xtensa, RV32, Cortex-M7 and Cortex-M0+.
//!
//! It reads a Roslyn-built assembly out of flash with `rustclr-metadata` and
//! exercises `rustclr-gc`; it does **not** execute IL, because `rustclr-core`
//! still needs `std`.
//!
//! ## No internal flash
//!
//! The K210 has none. Its mask ROM reads the image out of the board's SPI NOR
//! part, copies it to `0x80000000` and jumps there — so the layout is RAM-only:
//! text, rodata, data, bss, heap and stack all live in SRAM, with no
//! load-address/run-address split to arrange.
//!
//! ## Console
//!
//! UARTHS, the high-speed UART, on IO4/IO5 — where the Maix Go's on-board
//! STM32F103 bridges it to USB. Its baud divisor is taken straight from the
//! core clock, so the clock has to be *read* rather than assumed: a firmware
//! that guesses 26 MHz on a board the ROM left at 400 MHz produces a port that
//! opens and prints nothing but noise. `cpu_hz` below reads PLL0 and the clock
//! selector and works it out, and the banner prints what it found.

#![no_std]
#![no_main]

extern crate alloc;

use core::fmt::Write as _;

use embedded_alloc::LlffHeap;
use riscv_rt::entry;

/// The assembly to read, compiled by Roslyn for `net10.0`.
static HELLO_WORLD: &[u8] = include_bytes!(env!("RUSTCLR_APP_PATH"));

const BOARD: &str = "Kendryte K210 (RV64GC, Sipeed Maix Go)";

/// The console baud. The Maix Go's USB bridge runs at this by default.
const BAUD: u32 = 115_200;

/// How much of the K210's 6 MB of general-purpose SRAM the allocator gets.
///
/// A static array in `.bss` rather than the linker's `.heap` region, which
/// leaves `_heap_size` at zero and gives `.stack` everything between the end of
/// `.bss` and the top of SRAM.
// The K210 is the one board with room to spare — 6 MB of SRAM against a peak
// need of 261 KB — so this is generous rather than calculated.
const HEAP_BYTES: usize = 1024 * 1024;

#[global_allocator]
static ALLOCATOR: LlffHeap = LlffHeap::empty();

static mut HEAP_MEMORY: [u8; HEAP_BYTES] = [0; HEAP_BYTES];

// ---------------------------------------------------------------------------
// Registers
// ---------------------------------------------------------------------------

#[inline(always)]
fn rd(addr: usize) -> u32 {
    // SAFETY: fixed peripheral addresses from the K210 datasheet.
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}

#[inline(always)]
fn wr(addr: usize, value: u32) {
    // SAFETY: as above.
    unsafe { core::ptr::write_volatile(addr as *mut u32, value) }
}

fn field(addr: usize, shift: u32, width: u32) -> u32 {
    (rd(addr) >> shift) & ((1u32 << width) - 1)
}

const SYSCTL_BASE: usize = 0x5044_0000;
const PLL0: usize = SYSCTL_BASE + 0x08;
const CLK_SEL0: usize = SYSCTL_BASE + 0x20;

const PLL0_BYPASS: u32 = 1 << 23;
const PLL0_OUT_EN: u32 = 1 << 25;

/// The crystal every K210 board carries.
const IN0_HZ: u32 = 26_000_000;

/// PLL0's output frequency, as configured.
///
/// `in0 / (clkr + 1) * (clkf + 1) / (clkod + 1)`. Bypassed or powered down, the
/// PLL passes the crystal straight through. Done in 64 bits because
/// `26 MHz * 64` overflows a `u32` on the way to the division.
fn pll0_hz() -> u32 {
    let word = rd(PLL0);
    if word & PLL0_BYPASS != 0 || word & PLL0_OUT_EN == 0 {
        return IN0_HZ;
    }
    let clkr = field(PLL0, 0, 4) + 1;
    let clkf = field(PLL0, 4, 6) + 1;
    let clkod = field(PLL0, 10, 4) + 1;
    let numerator = IN0_HZ as u64 * clkf as u64;
    let denominator = clkr.max(1) as u64 * clkod.max(1) as u64;
    (numerator / denominator) as u32
}

/// The core (ACLK) frequency, read from the chip rather than assumed.
fn cpu_hz() -> u32 {
    if field(CLK_SEL0, 0, 1) == 0 {
        return IN0_HZ;
    }
    let divider = field(CLK_SEL0, 1, 2) * 2 + 2;
    pll0_hz() / divider
}

// ---------------------------------------------------------------------------
// UARTHS
// ---------------------------------------------------------------------------

const UARTHS_BASE: usize = 0x3800_0000;
const HS_TXDATA: usize = UARTHS_BASE + 0x00;
const HS_TXCTRL: usize = UARTHS_BASE + 0x08;
const HS_RXCTRL: usize = UARTHS_BASE + 0x0C;
const HS_IE: usize = UARTHS_BASE + 0x10;
const HS_DIV: usize = UARTHS_BASE + 0x18;

const HS_TX_FULL: u32 = 1 << 31;
const HS_TXEN: u32 = 1 << 0;
const HS_RXEN: u32 = 1 << 0;

/// How long to wait for room in the 8-entry transmit FIFO before giving up.
/// Console output is worth losing; a wedged board is not.
const SPIN_LIMIT: u32 = 4_000_000;

/// The high-speed UART, as a `core::fmt::Write` sink.
///
/// The pads are left alone on purpose. The mask ROM already muxes IO4/IO5 to
/// UARTHS on every Maix board — it used them itself to report — so muxing again
/// is harmless and *not* muxing is one less thing to get wrong.
struct Uarths;

impl Uarths {
    fn init(cpu_hz: u32) -> Self {
        // `baud = cpu_hz / (div + 1)`, so `div = cpu_hz / baud - 1` — but only
        // when the clock reading is believable.
        //
        // Whatever loaded this firmware already had UARTHS working: the mask
        // ROM used it to greet, and `kflash` used it to download at up to
        // 1.5 Mbaud. Its divisor is therefore known-good. Overwriting it with
        // one derived from a misread clock replaces a working UART with a
        // silent one, and a silent board tells you nothing about why.
        //
        // So a divisor is only programmed when the clock reads plausibly.
        // Otherwise the loader's is kept, and the board talks at whatever baud
        // the loader used — which is the baud whoever just flashed it already
        // has a terminal open on.
        let plausible = (1_000_000..=1_000_000_000).contains(&cpu_hz);
        if plausible {
            wr(HS_DIV, (cpu_hz / BAUD.max(1)).saturating_sub(1));
        }
        wr(HS_TXCTRL, HS_TXEN);
        wr(HS_RXCTRL, HS_RXEN);
        wr(HS_IE, 0);
        Self
    }

    fn put(&mut self, byte: u8) {
        for _ in 0..SPIN_LIMIT {
            if rd(HS_TXDATA) & HS_TX_FULL == 0 {
                wr(HS_TXDATA, byte as u32);
                return;
            }
            core::hint::spin_loop();
        }
    }
}

impl core::fmt::Write for Uarths {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for byte in s.as_bytes() {
            // A terminal on the other end of a raw UART wants CRLF.
            if *byte == b'\n' {
                self.put(b'\r');
            }
            self.put(*byte);
        }
        Ok(())
    }
}

#[entry]
fn main() -> ! {
    // SAFETY: called once, before anything allocates. Only hart 0 gets here;
    // riscv-rt parks the second in `wfi`.
    unsafe {
        ALLOCATOR.init(core::ptr::addr_of_mut!(HEAP_MEMORY) as usize, HEAP_BYTES);
    }

    let clock = cpu_hz();
    let mut console = Uarths::init(clock);

    let mut detail = Detail::new();
    let _ = write!(
        detail,
        "clock            {} MHz core, read from PLL0; UARTHS at {}",
        clock / 1_000_000,
        if (1_000_000..=1_000_000_000).contains(&clock) {
            "115200 baud"
        } else {
            "the loader's baud - the clock read implausibly"
        }
    );

    // Repeated rather than printed once, and the reason is specific to this
    // board: the K210 has no internal flash, so the firmware is loaded into
    // SRAM over the same UART a host would watch. Opening that port asserts
    // DTR, which resets the chip — and a reset discards SRAM and boots
    // whatever is on the SPI flash instead. A report printed once is therefore
    // unobservable: by the time anything is listening, the board is no longer
    // running this code.
    //
    // Repeating it means a host that attaches at any point sees a whole
    // report. The boards with flash print once, because there the firmware is
    // still there after the reset.
    loop {
        rustclr_demo_common::run(&mut console, BOARD, detail.as_str(), HELLO_WORLD, HEAP_BYTES);
        // A fixed cycle count, not one derived from `clock`: if the clock
        // reading is wrong the delay would be wrong with it, and the report
        // would repeat either far too often or never again.
        delay_cycles(600_000_000);
    }
}

/// A busy-wait of approximately `cycles` core cycles.
///
/// `mcycle` rather than a timer: the timer needs a peripheral clock this
/// firmware does not otherwise configure, and the only thing waiting on this is
/// how often the report repeats.
fn delay_cycles(cycles: u64) {
    let start = riscv::register::mcycle::read64();
    while riscv::register::mcycle::read64().wrapping_sub(start) < cycles {
        core::hint::spin_loop();
    }
}

/// A tiny fixed string builder, so the detail line needs no allocation.
struct Detail {
    bytes: [u8; 96],
    len: usize,
}

impl Detail {
    fn new() -> Self {
        Self { bytes: [0; 96], len: 0 }
    }

    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }
}

impl core::fmt::Write for Detail {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for b in s.as_bytes() {
            if self.len < self.bytes.len() {
                self.bytes[self.len] = *b;
                self.len += 1;
            }
        }
        Ok(())
    }
}

/// A panic has nowhere to go on a microcontroller. Say so on the console the
/// board already has, then stop.
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    let mut console = Uarths;
    let _ = write!(console, "\r\npanic: {info}\r\n");
    loop {
        riscv::asm::wfi();
    }
}
