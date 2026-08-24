//! RP2040 — a register-level HAL for RustNet, no PAC and no vendor SDK.
//!
//! Same shape as [`rustclr_hal_k210`] and `rustclr-hal-stm32`: a small crate
//! that talks to the chip through its memory map and implements the
//! [`rustclr_hal`] traits, so the interpreter and the firmware above it do not
//! know which silicon they are on.
//!
//! Three things about this part decide most of what follows.
//!
//! **Every peripheral comes out of reset held.** The `RESETS` block gates all
//! of them at power-up and a register write to a held peripheral is simply
//! lost — no fault, no hint. So [`resets::unreset`] comes before every driver
//! here, and it waits for the acknowledge rather than assuming it.
//!
//! **The chip boots at about 6 MHz on a ring oscillator**, not on its crystal.
//! Nothing in the datasheet's timing tables is true until the crystal and the
//! PLLs are running, so a UART configured before [`clocks::init`] is a UART at
//! whatever rate the ring oscillator drifted to that day.
//!
//! **Atomic register aliases replace read-modify-write.** Every peripheral
//! register is mirrored at `+0x1000` (XOR), `+0x2000` (set) and `+0x3000`
//! (clear). Using them is not an optimisation: a read-modify-write on a
//! register another core or an interrupt also touches loses whichever update
//! landed in between, and the RP2040 has two cores.
//!
//! Nothing here has run on hardware yet — see the port's README.

#![no_std]

extern crate alloc;

pub mod clocks;
pub mod gpio;
pub mod reg;
pub mod resets;
pub mod timer;
pub mod uart;

mod board;

pub use board::Rp2040Board;
pub use clocks::Clocks;
pub use uart::Rp2040Uart;

/// Peripheral base addresses, from the RP2040 datasheet's memory map (2.2).
pub mod base {
    pub const XOSC: usize = 0x4002_4000;
    pub const PLL_SYS: usize = 0x4002_8000;
    pub const PLL_USB: usize = 0x4002_C000;
    pub const RESETS: usize = 0x4000_C000;
    pub const CLOCKS: usize = 0x4000_8000;
    pub const IO_BANK0: usize = 0x4001_4000;
    pub const PADS_BANK0: usize = 0x4001_C000;
    pub const SIO: usize = 0xD000_0000;
    pub const TIMER: usize = 0x4005_4000;
    pub const UART0: usize = 0x4003_4000;
    pub const UART1: usize = 0x4003_8000;
    pub const WATCHDOG: usize = 0x4005_8000;
}
