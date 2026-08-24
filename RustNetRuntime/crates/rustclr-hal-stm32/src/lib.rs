//! STM32 (Cortex-M4F and Cortex-M7) board implementation of the RustNet HAL.
//!
//! # Two families, one crate
//!
//! The `stm32f7` feature retargets this crate from the F4 to the F7. That is
//! not a convenience: **GPIO, the DWT cycle counter and the RCC clock gates
//! are register-identical across the two families** — same base addresses,
//! same stride, same bit for each port — so a second crate would be a copy
//! that drifts. What genuinely differs is gated and named:
//!
//! - the F7 has GPIOA..K where the F401 skips F and G,
//! - its USART is the newer peripheral (ISR/RDR/TDR, not SR/DR), so `uart()`
//!   refuses rather than writing into reserved space,
//! - its flash controller differs, so internal-flash storage is not offered.
//!
//! The F7 target here is the **Meadow F7 Micro** (Wilderness Labs), which
//! reaches its host over USB CDC and so needs no USART for bring-up.
//!
//! Bring-up status: GPIO, USART and the delay source run directly on the
//! chip's registers (RM0368 for F401). Every other peripheral returns
//! `NotSupported` with a pointer to its integration point — the intended
//! fill-in is `stm32f4xx-hal`/the PAC for I2C/SPI/ADC/PWM/CAN and lwIP or
//! an ESP-AT companion for `netif`, keeping this crate's trait surface
//! unchanged.
//!
//! Verified on real silicon on two boards: a **Nucleo-F401RE**
//! (STM32F401RET6, 84 MHz, 512 KB flash, 96 KB RAM) over SWD, and a
//! **Netduino 3 WiFi** (STM32F427VIT6, 168 MHz from a 25 MHz HSE, 2 MB flash,
//! 256 KB RAM) over DFU. Same register map across the family, so only the
//! clock numbers and the extra UART7/UART8 ports differ between them.
//!
//! The crate is `no_std` and carries no dependencies beyond `rustclr-hal`,
//! so it stays in the host workspace build:
//!
//! ```text
//! cargo build -p rustclr-hal-stm32
//! cargo build -p rustclr-hal-stm32 --target thumbv7em-none-eabihf
//! ```
//!
//! # Pin numbering
//!
//! The `Board::gpio` index encodes port and pin as `port * 16 + index`,
//! with port 0 = GPIOA. So PA5 (the Nucleo's LD2) is `5`, PB0 is `16`,
//! PC13 (the user button) is `45`. STM32F4 parts have no GPIOF/GPIOG in
//! the F401 line, so ports 5 and 6 are rejected.

// `no_std` on the chip; the host test harness needs std to link.
#![cfg_attr(not(test), no_std)]

extern crate alloc;

use core::sync::atomic::{AtomicU32, Ordering};

use rustclr_hal::extmem::{ExtMemKind, ExtMemory};
use rustclr_hal::gpio::{Edge, GpioPin, Level, PinMode};
use rustclr_hal::power::{BatteryStatus, PowerManager, SleepMode, WakeReason, WakeSource};
use rustclr_hal::rtc::{DateTime, Rtc};
use rustclr_hal::spi::{SpiBus, SpiMode};
use rustclr_hal::uart::{Parity, Uart, UartConfig};
use rustclr_hal::watchdog::Watchdog;
use rustclr_hal::{delay::Delay, Board, HalError, HalResult};

// ---------------------------------------------------------------------------
// Register map (RM0368 §6 RCC, §8 GPIO, §19 USART; ARMv7-M DDI0403 for DWT)
// ---------------------------------------------------------------------------

const RCC_BASE: usize = 0x4002_3800;
const RCC_AHB1ENR: usize = RCC_BASE + 0x30;
const RCC_APB1ENR: usize = RCC_BASE + 0x40;
const RCC_APB2ENR: usize = RCC_BASE + 0x44;

const GPIO_BASE: usize = 0x4002_0000;
const GPIO_PORT_STRIDE: usize = 0x400;
const GPIO_MODER: usize = 0x00;
const GPIO_OTYPER: usize = 0x04;
const GPIO_OSPEEDR: usize = 0x08;
const GPIO_PUPDR: usize = 0x0C;
const GPIO_IDR: usize = 0x10;
const GPIO_ODR: usize = 0x14;
const GPIO_BSRR: usize = 0x18;
const GPIO_AFRL: usize = 0x20;

const USART_SR: usize = 0x00;
const USART_DR: usize = 0x04;
const USART_BRR: usize = 0x08;
const USART_CR1: usize = 0x0C;
const USART_CR2: usize = 0x10;

/// USART_SR bits.
const SR_ORE: u32 = 1 << 3;
const SR_RXNE: u32 = 1 << 5;
const SR_TC: u32 = 1 << 6;
const SR_TXE: u32 = 1 << 7;

/// USART_CR1 bits.
const CR1_RE: u32 = 1 << 2;
const CR1_TE: u32 = 1 << 3;
const CR1_PS: u32 = 1 << 9;
const CR1_PCE: u32 = 1 << 10;
const CR1_M: u32 = 1 << 12;
const CR1_UE: u32 = 1 << 13;

/// Cortex-M DWT cycle counter — the delay/monotonic-clock source.
const DEMCR: usize = 0xE000_EDFC;
const DEMCR_TRCENA: u32 = 1 << 24;
const DWT_CTRL: usize = 0xE000_1000;
const DWT_CTRL_CYCCNTENA: u32 = 1 << 0;
const DWT_CYCCNT: usize = 0xE000_1004;
/// The DWT's CoreSight lock. Cortex-M7 ships the trace unit locked; writing
/// the key below is what makes `DWT_CTRL` writable at all.
const DWT_LAR: usize = 0xE000_1FB0;
const CORESIGHT_UNLOCK: u32 = 0xC5AC_CE55;

/// Ports A..=E and H exist on F401; F and G do not. The F7 parts this crate
/// also serves carry A..=K, so the array is sized for the wider family and
/// `gpio_clock_bit` is what actually decides which ports a chip has.
#[cfg(not(feature = "stm32f7"))]
const PORT_COUNT: u32 = 8;
#[cfg(feature = "stm32f7")]
const PORT_COUNT: u32 = 11;
const PIN_COUNT: usize = (PORT_COUNT * 16) as usize;

// ---------------------------------------------------------------------------
// MMIO helpers
// ---------------------------------------------------------------------------
//
// Guarded by target arch so the crate stays inert — and therefore safe to
// build and construct — on the host, where these addresses mean nothing.
// On the chip they are the fixed peripheral addresses from RM0368.

#[inline(always)]
fn reg_write(addr: usize, value: u32) {
    #[cfg(target_arch = "arm")]
    // SAFETY: fixed peripheral address; only reachable when executing on the chip.
    unsafe {
        core::ptr::write_volatile(addr as *mut u32, value)
    }
    #[cfg(not(target_arch = "arm"))]
    {
        let _ = (addr, value);
    }
}

#[inline(always)]
fn reg_read(addr: usize) -> u32 {
    #[cfg(target_arch = "arm")]
    // SAFETY: see `reg_write`.
    unsafe {
        core::ptr::read_volatile(addr as *const u32)
    }
    #[cfg(not(target_arch = "arm"))]
    {
        let _ = addr;
        0
    }
}

#[inline(always)]
fn reg_modify(addr: usize, clear: u32, set: u32) {
    reg_write(addr, (reg_read(addr) & !clear) | set);
}

#[inline(always)]
fn cyccnt() -> u32 {
    reg_read(DWT_CYCCNT)
}

/// AHB1ENR bit for a GPIO port. GPIOA..GPIOE are bits 0..4, GPIOH is bit 7 —
/// F401 has no GPIOF/GPIOG, so ports 5 and 6 have no clock gate.
/// Which AHB1ENR bit clocks a port, or `None` if the chip has no such port.
///
/// The bit *is* the port index on both families — GPIOA is bit 0, GPIOB bit 1
/// — so this only decides which ports exist. The F401 skips F and G; the F7
/// parts have the lot.
#[cfg(not(feature = "stm32f7"))]
fn gpio_clock_bit(port: u32) -> Option<u32> {
    match port {
        0..=4 => Some(port),
        7 => Some(7),
        _ => None,
    }
}

#[cfg(feature = "stm32f7")]
fn gpio_clock_bit(port: u32) -> Option<u32> {
    match port {
        0..=10 => Some(port),
        _ => None,
    }
}

fn port_base(port: u32) -> usize {
    GPIO_BASE + (port as usize) * GPIO_PORT_STRIDE
}

// ---------------------------------------------------------------------------
// GPIO
// ---------------------------------------------------------------------------

pub struct Stm32Pin {
    /// Encoded `port * 16 + index`.
    id: u32,
}

impl Stm32Pin {
    #[inline]
    fn port(&self) -> u32 {
        self.id / 16
    }

    #[inline]
    fn index(&self) -> u32 {
        self.id % 16
    }

    /// Ungate the port clock. Idempotent, and required before any of the
    /// port's registers respond to writes.
    fn enable_clock(&self) {
        if let Some(bit) = gpio_clock_bit(self.port()) {
            reg_modify(RCC_AHB1ENR, 0, 1 << bit);
        }
    }

    fn is_output(&self) -> bool {
        let moder = reg_read(port_base(self.port()) + GPIO_MODER);
        (moder >> (self.index() * 2)) & 0b11 == 0b01
    }
}

impl GpioPin for Stm32Pin {
    fn set_mode(&mut self, mode: PinMode) -> HalResult<()> {
        self.enable_clock();
        let base = port_base(self.port());
        let i = self.index();
        let pair = 0b11 << (i * 2);

        let (moder, pupdr, open_drain) = match mode {
            PinMode::Input => (0b00, 0b00, false),
            PinMode::InputPullUp => (0b00, 0b01, false),
            PinMode::InputPullDown => (0b00, 0b10, false),
            PinMode::Output => (0b01, 0b00, false),
            PinMode::OutputOpenDrain => (0b01, 0b00, true),
        };

        reg_modify(base + GPIO_MODER, pair, moder << (i * 2));
        reg_modify(base + GPIO_PUPDR, pair, pupdr << (i * 2));
        reg_modify(base + GPIO_OTYPER, 1 << i, u32::from(open_drain) << i);
        Ok(())
    }

    fn write(&mut self, level: Level) -> HalResult<()> {
        // BSRR is write-only and atomic: low half sets, high half resets —
        // no read-modify-write, so it cannot race an interrupt handler.
        let bit = match level {
            Level::High => 1 << self.index(),
            Level::Low => 1 << (self.index() + 16),
        };
        reg_write(port_base(self.port()) + GPIO_BSRR, bit);
        Ok(())
    }

    fn read(&mut self) -> HalResult<Level> {
        // Outputs read back the output latch, inputs the pad.
        let base = port_base(self.port());
        let reg = if self.is_output() { GPIO_ODR } else { GPIO_IDR };
        let bits = reg_read(base + reg);
        Ok(Level::from(bits & (1 << self.index()) != 0))
    }

    fn on_edge(
        &mut self,
        _edge: Edge,
        _callback: alloc::boxed::Box<dyn FnMut(Level) + Send>,
    ) -> HalResult<()> {
        // Integration point: SYSCFG_EXTICRn pin mux + EXTI IMR/RTSR/FTSR and
        // an NVIC handler that dispatches to the stored callback.
        Err(HalError::NotSupported)
    }

    fn clear_interrupt(&mut self) -> HalResult<()> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Delay / monotonic clock
// ---------------------------------------------------------------------------

/// Cycle-counted busy-wait backed by the Cortex-M DWT counter.
///
/// `CYCCNT` is 32-bit and wraps every `2^32 / cpu_hz` seconds — about 51 s at
/// 84 MHz. `now_us` extends it to 64-bit by counting wraps as it observes
/// them, so it must be polled at least once per wrap period to stay correct.
/// `delay_us` is immune to this: it compares with `wrapping_sub` in chunks
/// below 2^31 cycles.
pub struct DwtDelay {
    cpu_hz: u32,
    wraps: AtomicU32,
    last: AtomicU32,
}

impl DwtDelay {
    pub fn new(cpu_hz: u32) -> Self {
        Self { cpu_hz, wraps: AtomicU32::new(0), last: AtomicU32::new(0) }
    }

    /// Ungate the trace unit and start the cycle counter. Must run on the
    /// chip before any delay call; `Stm32F4Board::init` does it for you.
    ///
    /// # The unlock is not optional on Cortex-M7
    ///
    /// A Cortex-M4's DWT has no lock, so enabling `CYCCNTENA` is enough and
    /// this worked on the F4 boards for a year. **The M7's DWT is CoreSight
    /// and ships locked**: without the key in `DWT_LAR` the write to
    /// `DWT_CTRL` is silently discarded and the counter never leaves zero.
    ///
    /// Nothing reports an error when that happens; the delay source simply
    /// says no time has passed. Every wait returns instantly, every timed
    /// blink is invisible, and any loop that waits for a deadline never
    /// reaches it — a failure that looks like a dozen unrelated faults and is
    /// one. On the Meadow F7 it cost several hours of blaming USB, a serial
    /// adapter and the wiring.
    ///
    /// Writing the key on an M4 is harmless: the address reads as zero and
    /// ignores writes there.
    pub fn start(&self) {
        reg_write(DWT_LAR, CORESIGHT_UNLOCK);
        reg_modify(DEMCR, 0, DEMCR_TRCENA);
        reg_write(DWT_CYCCNT, 0);
        reg_modify(DWT_CTRL, 0, DWT_CTRL_CYCCNTENA);
    }

    fn cycles(&self) -> u64 {
        let now = cyccnt();
        let last = self.last.swap(now, Ordering::Relaxed);
        let wraps = if now < last {
            self.wraps.fetch_add(1, Ordering::Relaxed).wrapping_add(1)
        } else {
            self.wraps.load(Ordering::Relaxed)
        };
        ((wraps as u64) << 32) | now as u64
    }

    #[inline]
    fn cycles_per_us(&self) -> u64 {
        (self.cpu_hz as u64 / 1_000_000).max(1)
    }
}

impl Delay for DwtDelay {
    fn delay_us(&mut self, us: u64) {
        let mut remaining = us.saturating_mul(self.cycles_per_us());
        while remaining > 0 {
            // Stay below 2^31 per chunk so `wrapping_sub` cannot be
            // misread across a counter wrap.
            let chunk = remaining.min(0x4000_0000) as u32;
            let start = cyccnt();
            while cyccnt().wrapping_sub(start) < chunk {
                core::hint::spin_loop();
            }
            remaining -= chunk as u64;
        }
    }

    fn now_us(&self) -> u64 {
        self.cycles() / self.cycles_per_us()
    }
}

// ---------------------------------------------------------------------------
// USART
// ---------------------------------------------------------------------------

/// Everything that differs between the F401's three USARTs.
#[derive(Clone, Copy)]
struct UsartDef {
    base: usize,
    /// Clock-enable register and bit.
    en_reg: usize,
    en_bit: u32,
    /// TX/RX pins as `port * 16 + index`, and their alternate function.
    tx: u32,
    rx: u32,
    af: u32,
    /// Which APB clock feeds it — needed for the baud divisor.
    on_apb2: bool,
}

/// Index selects the port: 0/1/2 are USART1/USART2/USART6, present across the
/// F4 line; 3/4 are UART7/UART8, which exist on F42x/F43x but **not** on F401.
/// Pin assignments are each part's common default — other packages and boards
/// can route the same peripheral elsewhere.
const USARTS: [UsartDef; 5] = [
    // USART1: PA9 TX, PA10 RX, AF7
    UsartDef { base: 0x4001_1000, en_reg: RCC_APB2ENR, en_bit: 4, tx: 9, rx: 10, af: 7, on_apb2: true },
    // USART2: PA2 TX, PA3 RX, AF7 — the Nucleo's ST-LINK VCP port
    UsartDef { base: 0x4000_4400, en_reg: RCC_APB1ENR, en_bit: 17, tx: 2, rx: 3, af: 7, on_apb2: false },
    // USART6: PC6 TX, PC7 RX, AF8
    UsartDef { base: 0x4001_1400, en_reg: RCC_APB2ENR, en_bit: 5, tx: 38, rx: 39, af: 8, on_apb2: true },
    // UART7: PE8 TX, PE7 RX, AF8 — the Netduino 3's goPort2 serial header
    UsartDef { base: 0x4000_7800, en_reg: RCC_APB1ENR, en_bit: 30, tx: 72, rx: 71, af: 8, on_apb2: false },
    // UART8: PE1 TX, PE0 RX, AF8 — the Netduino 3's goPort3 serial header
    UsartDef { base: 0x4000_7C00, en_reg: RCC_APB1ENR, en_bit: 31, tx: 65, rx: 64, af: 8, on_apb2: false },
];

/// Derived from the table so the board's array can never fall out of step
/// with it — a mismatch silently drops ports rather than failing to compile.
const USART_COUNT: usize = USARTS.len();

pub struct Stm32Uart {
    def: UsartDef,
    pclk_hz: u32,
}

impl Stm32Uart {
    fn mux_pin(&self, id: u32) {
        let port = id / 16;
        let i = id % 16;
        if let Some(bit) = gpio_clock_bit(port) {
            reg_modify(RCC_AHB1ENR, 0, 1 << bit);
        }
        let base = port_base(port);
        // MODER = 0b10 (alternate function)
        reg_modify(base + GPIO_MODER, 0b11 << (i * 2), 0b10 << (i * 2));
        // AFRL covers pins 0..7, AFRH pins 8..15 — 4 bits each.
        let (afr, shift) = if i < 8 { (GPIO_AFRL, i * 4) } else { (GPIO_AFRL + 4, (i - 8) * 4) };
        reg_modify(base + afr, 0b1111 << shift, self.def.af << shift);
    }
}

impl Uart for Stm32Uart {
    fn configure(&mut self, config: UartConfig) -> HalResult<()> {
        if config.baud == 0 {
            return Err(HalError::InvalidArgument("baud must be non-zero"));
        }
        if !matches!(config.data_bits, 8 | 9) {
            // The F401 USART frames 8 or 9 bits; 7-bit needs parity to
            // occupy the 8th, which `Parity` below already handles.
            return Err(HalError::InvalidArgument("data_bits must be 8 or 9"));
        }

        reg_modify(self.def.en_reg, 0, 1 << self.def.en_bit);
        self.mux_pin(self.def.tx);
        self.mux_pin(self.def.rx);

        let base = self.def.base;
        // Disable while reconfiguring; CR1 writes are ignored piecemeal otherwise.
        reg_modify(base + USART_CR1, CR1_UE, 0);

        // OVER8 = 0, so BRR holds USARTDIV*16 outright: fck/baud, rounded.
        let brr = (self.pclk_hz + config.baud / 2) / config.baud;
        if brr < 16 {
            return Err(HalError::InvalidArgument("baud too high for this PCLK"));
        }
        reg_write(base + USART_BRR, brr);

        let stop = match config.stop_bits {
            1 => 0b00,
            2 => 0b10,
            _ => return Err(HalError::InvalidArgument("stop_bits must be 1 or 2")),
        };
        reg_modify(base + USART_CR2, 0b11 << 12, stop << 12);

        // Parity steals the most significant frame bit, so 8 data bits plus
        // parity needs the 9-bit word length.
        let mut cr1 = CR1_UE | CR1_TE | CR1_RE;
        match config.parity {
            Parity::None => {
                if config.data_bits == 9 {
                    cr1 |= CR1_M;
                }
            }
            Parity::Even => {
                cr1 |= CR1_PCE;
                if config.data_bits == 8 {
                    cr1 |= CR1_M;
                }
            }
            Parity::Odd => {
                cr1 |= CR1_PCE | CR1_PS;
                if config.data_bits == 8 {
                    cr1 |= CR1_M;
                }
            }
        }
        reg_write(base + USART_CR1, cr1);
        Ok(())
    }

    fn write(&mut self, data: &[u8]) -> HalResult<usize> {
        let base = self.def.base;
        for byte in data {
            while reg_read(base + USART_SR) & SR_TXE == 0 {
                core::hint::spin_loop();
            }
            reg_write(base + USART_DR, *byte as u32);
        }
        Ok(data.len())
    }

    fn read(&mut self, buf: &mut [u8]) -> HalResult<usize> {
        let base = self.def.base;
        let mut n = 0;
        while n < buf.len() {
            let sr = reg_read(base + USART_SR);
            if sr & SR_RXNE == 0 {
                break;
            }
            // Reading DR after SR also clears a pending overrun.
            buf[n] = (reg_read(base + USART_DR) & 0xFF) as u8;
            n += 1;
        }
        Ok(n)
    }

    fn bytes_available(&mut self) -> HalResult<usize> {
        // The F401 USART has a one-byte receive register, no FIFO.
        let sr = reg_read(self.def.base + USART_SR);
        if sr & SR_ORE != 0 {
            // Clear the overrun so the port keeps receiving; the byte that
            // caused it is already lost.
            let _ = reg_read(self.def.base + USART_DR);
        }
        Ok(usize::from(sr & SR_RXNE != 0))
    }

    fn flush(&mut self) -> HalResult<()> {
        while reg_read(self.def.base + USART_SR) & SR_TC == 0 {
            core::hint::spin_loop();
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Power / RTC / watchdog stubs
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// SPI
// ---------------------------------------------------------------------------

const SPI_CR1: usize = 0x00;
const SPI_SR: usize = 0x08;
const SPI_DR: usize = 0x0C;

/// SPI_CR1 bits.
const SPI_CPHA: u32 = 1 << 0;
const SPI_CPOL: u32 = 1 << 1;
const SPI_MSTR: u32 = 1 << 2;
const SPI_SPE: u32 = 1 << 6;
/// Software slave management, with the internal select held high: without
/// both, a master with NSS floating drops into slave mode the moment the pin
/// reads low, and the transfer silently stops.
const SPI_SSI: u32 = 1 << 8;
const SPI_SSM: u32 = 1 << 9;

/// SPI_SR bits.
const SPI_RXNE: u32 = 1 << 0;
const SPI_TXE: u32 = 1 << 1;
const SPI_BSY: u32 = 1 << 7;

/// Spin until a status bit reaches `want`, or give up.
///
/// Bounded on purpose. An unbounded wait on a peripheral flag is a hang
/// waiting for a wiring mistake: if the clock never runs, `RXNE` never
/// arrives, and the firmware stops with no way to say why. The budget is
/// generous next to any real transfer — at the slowest prescaler a byte is
/// some thousands of cycles — so only a genuinely dead bus reaches it.
fn wait_for(status: usize, bit: u32, want: bool) -> HalResult<()> {
    for _ in 0..1_000_000 {
        if (reg_read(status) & bit != 0) == want {
            return Ok(());
        }
        core::hint::spin_loop();
    }
    Err(HalError::Timeout)
}

/// Everything that differs between the F4's SPI blocks.
#[derive(Clone, Copy)]
struct SpiDef {
    base: usize,
    en_reg: usize,
    en_bit: u32,
    /// SCK, MISO, MOSI as `port * 16 + index`, and their alternate function.
    sck: u32,
    miso: u32,
    mosi: u32,
    af: u32,
    on_apb2: bool,
}

/// Index 0/1/2 select SPI1/SPI2/SPI3. Pins are each block's common default;
/// SPI3's are the ones the Netduino wires its microSD slot to.
const SPIS: [SpiDef; 3] = [
    // SPI1: PA5 SCK, PA6 MISO, PA7 MOSI, AF5
    SpiDef { base: 0x4001_3000, en_reg: RCC_APB2ENR, en_bit: 12, sck: 5, miso: 6, mosi: 7, af: 5, on_apb2: true },
    // SPI2: PB13 SCK, PB14 MISO, PB15 MOSI, AF5
    SpiDef { base: 0x4000_3800, en_reg: RCC_APB1ENR, en_bit: 14, sck: 29, miso: 30, mosi: 31, af: 5, on_apb2: false },
    // SPI3: PC10 SCK, PC11 MISO, PC12 MOSI, AF6
    SpiDef { base: 0x4000_3C00, en_reg: RCC_APB1ENR, en_bit: 15, sck: 42, miso: 43, mosi: 44, af: 6, on_apb2: false },
];

pub struct Stm32Spi {
    def: SpiDef,
    pclk_hz: u32,
}

impl Stm32Spi {
    /// Route a pin to this block's alternate function.
    ///
    /// `pull_up` matters on exactly one pin and matters a great deal there.
    /// MISO is driven by the peripheral, not the MCU, so it floats whenever
    /// nothing is selected or a device is still thinking. Floating low reads
    /// as `0x00` — and for an SPI-mode SD card `0x00` is a *successful* R1, so
    /// every command appears to succeed and an entire initialisation sequence
    /// reports itself complete without a card having said anything. Idle high
    /// reads as `0xFF`, which is the "busy, ask again" the protocol expects.
    fn mux_pin(&self, id: u32, pull_up: bool) {
        let port = id / 16;
        let i = id % 16;
        if let Some(bit) = gpio_clock_bit(port) {
            reg_modify(RCC_AHB1ENR, 0, 1 << bit);
        }
        let base = port_base(port);
        reg_modify(base + GPIO_MODER, 0b11 << (i * 2), 0b10 << (i * 2));
        // SPI runs far faster than a GPIO's default slew rate is meant for.
        reg_modify(base + GPIO_OSPEEDR, 0b11 << (i * 2), 0b11 << (i * 2));
        reg_modify(
            base + GPIO_PUPDR,
            0b11 << (i * 2),
            if pull_up { 0b01 << (i * 2) } else { 0 },
        );
        let (afr, shift) = if i < 8 { (GPIO_AFRL, i * 4) } else { (GPIO_AFRL + 4, (i - 8) * 4) };
        reg_modify(base + afr, 0b1111 << shift, self.def.af << shift);
    }
}

impl SpiBus for Stm32Spi {
    fn configure(&mut self, hz: u32, mode: SpiMode) -> HalResult<()> {
        if hz == 0 {
            return Err(HalError::InvalidArgument("spi clock must be non-zero"));
        }
        reg_modify(self.def.en_reg, 0, 1 << self.def.en_bit);
        self.mux_pin(self.def.sck, false);
        self.mux_pin(self.def.miso, true);
        self.mux_pin(self.def.mosi, false);

        // The prescaler only goes in powers of two from /2 to /256, so round
        // *down* in frequency: a card rated for 400 kHz during init must not
        // be handed 800.
        let mut br = 0u32;
        while br < 7 && self.pclk_hz >> (br + 1) > hz {
            br += 1;
        }

        let (cpol, cpha) = match mode {
            SpiMode::Mode0 => (0, 0),
            SpiMode::Mode1 => (0, SPI_CPHA),
            SpiMode::Mode2 => (SPI_CPOL, 0),
            SpiMode::Mode3 => (SPI_CPOL, SPI_CPHA),
        };

        reg_write(self.def.base + SPI_CR1, 0);
        reg_write(
            self.def.base + SPI_CR1,
            SPI_MSTR | SPI_SSM | SPI_SSI | cpol | cpha | (br << 3) | SPI_SPE,
        );
        Ok(())
    }

    fn transfer(&mut self, tx: &[u8], rx: &mut [u8]) -> HalResult<()> {
        if tx.len() != rx.len() {
            return Err(HalError::InvalidArgument("spi transfer buffers must match"));
        }
        let base = self.def.base;
        for (out, input) in tx.iter().zip(rx.iter_mut()) {
            wait_for(base + SPI_SR, SPI_TXE, true)?;
            // The data register is byte-wide in 8-bit frame format, and a
            // 32-bit write would push two frames.
            // SAFETY: fixed peripheral address for this SPI block.
            unsafe { core::ptr::write_volatile((base + SPI_DR) as *mut u8, *out) };
            wait_for(base + SPI_SR, SPI_RXNE, true)?;
            // SAFETY: see above.
            *input = unsafe { core::ptr::read_volatile((base + SPI_DR) as *const u8) };
        }
        wait_for(base + SPI_SR, SPI_BSY, false)
    }
}

// ---------------------------------------------------------------------------
// Internal flash as an ExtMemory
// ---------------------------------------------------------------------------
//
// The `ExtMemory` trait already describes NOR flash exactly — erase to 0xFF,
// bits only go 1 -> 0, sector granularity — so the MCU's own flash fits it
// without any reshaping. This is what gives a bare-metal target somewhere to
// keep a provisioned key or an uploaded application across a reset.

const FLASH_R_BASE: usize = 0x4002_3C00;
const FLASH_KEYR: usize = FLASH_R_BASE + 0x04;
const FLASH_SR: usize = FLASH_R_BASE + 0x0C;
const FLASH_CR: usize = FLASH_R_BASE + 0x10;

const FLASH_KEY1: u32 = 0x4567_0123;
const FLASH_KEY2: u32 = 0xCDEF_89AB;

const SR_BSY: u32 = 1 << 16;
/// Any of PGSERR/PGPERR/PGAERR/WRPERR/OPERR.
const SR_ERRORS: u32 = 0b1111_0011 << 1 | 0;

const CR_PG: u32 = 1 << 0;
const CR_SER: u32 = 1 << 1;
const CR_STRT: u32 = 1 << 16;
const CR_LOCK: u32 = 1 << 31;
/// PSIZE = 2, i.e. 32-bit programming. Valid at 2.7-3.6 V, which is every
/// board this crate targets.
const CR_PSIZE_WORD: u32 = 0b10 << 8;

/// One erasable region of the MCU's own flash, handed to the runtime as an
/// external-memory device.
///
/// # Executing while erasing
///
/// A flash operation stalls every access to the flash, and on these parts the
/// code and the storage share one interface. So the core — and every
/// interrupt handler, since those live in flash too — is frozen for the
/// duration: roughly a second for a 128 KB sector erase. Anything that must
/// not miss an interrupt has to be quiet across the call.
pub struct InternalFlash {
    base: u32,
    len: u32,
    sector: u32,
    sector_size: u32,
}

impl InternalFlash {
    /// `sector` is the F4 sector number that `base` starts at; the caller is
    /// responsible for it lying outside the image, which `memory.x` enforces
    /// by not describing it as FLASH at all.
    pub const fn new(base: u32, len: u32, sector: u32, sector_size: u32) -> Self {
        Self { base, len, sector, sector_size }
    }

    fn wait_idle() -> HalResult<()> {
        while reg_read(FLASH_SR) & SR_BSY != 0 {
            core::hint::spin_loop();
        }
        let sr = reg_read(FLASH_SR);
        if sr & SR_ERRORS != 0 {
            // Write-1-to-clear, so the next operation starts from a clean slate.
            reg_write(FLASH_SR, sr & SR_ERRORS);
            return Err(HalError::Bus("flash operation failed"));
        }
        Ok(())
    }

    fn unlock() -> HalResult<()> {
        if reg_read(FLASH_CR) & CR_LOCK != 0 {
            reg_write(FLASH_KEYR, FLASH_KEY1);
            reg_write(FLASH_KEYR, FLASH_KEY2);
            if reg_read(FLASH_CR) & CR_LOCK != 0 {
                return Err(HalError::Bus("flash stayed locked"));
            }
        }
        Ok(())
    }

    fn lock() {
        reg_modify(FLASH_CR, 0, CR_LOCK);
    }

    fn in_range(&self, addr: u32, len: usize) -> HalResult<()> {
        let end = addr.checked_add(len as u32).ok_or(HalError::InvalidArgument("range overflows"))?;
        if end > self.len {
            return Err(HalError::InvalidArgument("outside the storage region"));
        }
        Ok(())
    }
}

impl ExtMemory for InternalFlash {
    fn kind(&self) -> ExtMemKind {
        ExtMemKind::QspiFlash
    }

    fn size(&self) -> u32 {
        self.len
    }

    fn read(&mut self, addr: u32, buf: &mut [u8]) -> HalResult<()> {
        self.in_range(addr, buf.len())?;
        for (i, byte) in buf.iter_mut().enumerate() {
            let at = (self.base + addr + i as u32) as usize;
            // SAFETY: inside the region checked above, and flash is readable
            // as ordinary memory.
            *byte = unsafe { core::ptr::read_volatile(at as *const u8) };
        }
        Ok(())
    }

    fn write(&mut self, addr: u32, data: &[u8]) -> HalResult<()> {
        self.in_range(addr, data.len())?;
        if addr % 4 != 0 || data.len() % 4 != 0 {
            // 32-bit programming: anything else would need PSIZE juggling and
            // read-modify-write, which NOR flash cannot do anyway.
            return Err(HalError::InvalidArgument("flash writes must be 4-byte aligned"));
        }

        Self::unlock()?;
        Self::wait_idle()?;
        reg_modify(FLASH_CR, 0, CR_PG | CR_PSIZE_WORD);

        let result = (|| {
            for (i, word) in data.chunks_exact(4).enumerate() {
                let value = u32::from_le_bytes([word[0], word[1], word[2], word[3]]);
                let at = (self.base + addr + (i * 4) as u32) as usize;
                // SAFETY: aligned, in range, and the flash controller is armed
                // for programming.
                unsafe { core::ptr::write_volatile(at as *mut u32, value) };
                Self::wait_idle()?;
            }
            Ok(())
        })();

        reg_modify(FLASH_CR, CR_PG, 0);
        Self::lock();
        result
    }

    fn erase(&mut self, addr: u32, len: u32) -> HalResult<()> {
        if addr != 0 || len > self.len {
            // One sector is the whole region here; partial erase has no
            // meaning at this granularity.
            return Err(HalError::InvalidArgument("only whole-region erase is supported"));
        }

        Self::unlock()?;
        Self::wait_idle()?;
        reg_modify(FLASH_CR, 0b1111 << 3, CR_SER | CR_PSIZE_WORD | (self.sector << 3));
        reg_modify(FLASH_CR, 0, CR_STRT);
        let result = Self::wait_idle();
        reg_modify(FLASH_CR, CR_SER, 0);
        Self::lock();
        result
    }

    fn sector_size(&self) -> u32 {
        self.sector_size
    }
}

/// Software RTC: seconds handed to `set()` plus cycle-counter drift.
pub struct SoftRtc {
    epoch_base: u64,
    alarm: Option<u64>,
}

impl Rtc for SoftRtc {
    fn now(&mut self) -> HalResult<DateTime> {
        Ok(DateTime::from_epoch(self.epoch_base))
    }
    fn set(&mut self, dt: DateTime) -> HalResult<()> {
        self.epoch_base = dt.to_epoch();
        Ok(())
    }
    fn set_alarm(&mut self, epoch: u64) -> HalResult<()> {
        self.alarm = Some(epoch);
        Ok(())
    }
    fn clear_alarm(&mut self) -> HalResult<()> {
        self.alarm = None;
        Ok(())
    }
    fn alarm(&self) -> Option<u64> {
        self.alarm
    }
}

pub struct Stm32Power {
    cpu_hz: u32,
}

impl PowerManager for Stm32Power {
    fn sleep(&mut self, _mode: SleepMode, _duration_ms: Option<u64>) -> HalResult<()> {
        // Integration point: SCB_SCR SLEEPDEEP + PWR_CR PDDS/LPDS, then WFI.
        Err(HalError::NotSupported)
    }
    fn battery(&mut self) -> HalResult<BatteryStatus> {
        Err(HalError::NotSupported)
    }
    fn cpu_frequency_hz(&self) -> u32 {
        self.cpu_hz
    }
    fn set_cpu_frequency_hz(&mut self, _hz: u32) -> HalResult<()> {
        // Integration point: RCC PLL reconfiguration + flash latency.
        Err(HalError::NotSupported)
    }
    fn reset(&mut self) -> ! {
        // AIRCR: VECTKEY 0x05FA plus SYSRESETREQ.
        reg_write(0xE000_ED0C, (0x05FA << 16) | (1 << 2));
        loop {
            core::hint::spin_loop();
        }
    }
    fn shutdown(&mut self) -> ! {
        loop {
            core::hint::spin_loop();
        }
    }
    fn arm_wake(&mut self, _source: WakeSource) -> HalResult<()> {
        Err(HalError::NotSupported)
    }
    fn clear_wake_sources(&mut self) {}
    fn wake_reason(&self) -> WakeReason {
        WakeReason::PowerOn
    }
}

pub struct Stm32Watchdog;

impl Watchdog for Stm32Watchdog {
    fn start(&mut self, _timeout_ms: u32) -> HalResult<()> {
        // Integration point: IWDG_KR/PR/RLR (LSI-clocked independent watchdog).
        Err(HalError::NotSupported)
    }
    fn feed(&mut self) -> HalResult<()> {
        Err(HalError::NotSupported)
    }
    fn stop(&mut self) -> HalResult<()> {
        Err(HalError::NotSupported)
    }
    fn is_running(&self) -> bool {
        false
    }
    fn timeout_ms(&self) -> u32 {
        0
    }
}

// ---------------------------------------------------------------------------
// Board
// ---------------------------------------------------------------------------

/// Clock frequencies the board was brought up at. The firmware configures the
/// PLL before constructing the board and reports the result here — this crate
/// does not own the clock tree, it only needs the numbers for baud rates and
/// delay scaling.
#[derive(Debug, Clone, Copy)]
pub struct Clocks {
    pub sysclk_hz: u32,
    pub pclk1_hz: u32,
    pub pclk2_hz: u32,
}

impl Clocks {
    /// Nucleo-F401RE running from HSI through the PLL: 84 MHz SYSCLK,
    /// APB1 divided by 2 (42 MHz max), APB2 undivided.
    pub const NUCLEO_F401RE: Clocks =
        Clocks { sysclk_hz: 84_000_000, pclk1_hz: 42_000_000, pclk2_hz: 84_000_000 };

    /// Netduino 3 WiFi (STM32F427VIT6) from its 25 MHz HSE crystal: 168 MHz
    /// SYSCLK, APB1 /4 and APB2 /2 — the F427's 42/84 MHz bus ceilings.
    pub const NETDUINO3_WIFI: Clocks =
        Clocks { sysclk_hz: 168_000_000, pclk1_hz: 42_000_000, pclk2_hz: 84_000_000 };

    /// Meadow F7 Micro (Wilderness Labs) at the F7's full 216 MHz, with APB1
    /// /4 and APB2 /2 — the 54/108 MHz ceilings of that family.
    ///
    /// The part is an **STM32F777**: 2 MB flash, 512 KB RAM, 216 MHz. The
    /// vendor page only says "STM32F7 ... up to 216 MHz", so this was
    /// confirmed by the board's owner rather than derived. `sysclk_hz` is the
    /// figure that matters, because the delay source counts CPU cycles — a
    /// wrong value shows up as every timing being out by a constant ratio.
    pub const MEADOW_F7: Clocks =
        Clocks { sysclk_hz: 216_000_000, pclk1_hz: 54_000_000, pclk2_hz: 108_000_000 };
}

/// The STM32F4 board. GPIO, USART and delay are live; the remaining
/// peripherals name their integration points and fail fast.
pub struct Stm32F4Board {
    pins: [Stm32Pin; PIN_COUNT],
    uarts: [Stm32Uart; USART_COUNT],
    spis: [Stm32Spi; SPIS.len()],
    delay: DwtDelay,
    rtc: SoftRtc,
    power: Stm32Power,
    watchdog: Stm32Watchdog,
    /// A region of the MCU's own flash, if the firmware set one aside.
    storage: Option<InternalFlash>,
}

impl Stm32F4Board {
    pub fn new(clocks: Clocks) -> Self {
        Stm32F4Board {
            pins: core::array::from_fn(|i| Stm32Pin { id: i as u32 }),
            uarts: core::array::from_fn(|i| {
                let def = USARTS[i];
                Stm32Uart {
                    def,
                    pclk_hz: if def.on_apb2 { clocks.pclk2_hz } else { clocks.pclk1_hz },
                }
            }),
            spis: core::array::from_fn(|i| {
                let def = SPIS[i];
                Stm32Spi {
                    def,
                    pclk_hz: if def.on_apb2 { clocks.pclk2_hz } else { clocks.pclk1_hz },
                }
            }),
            delay: DwtDelay::new(clocks.sysclk_hz),
            rtc: SoftRtc { epoch_base: 0, alarm: None },
            power: Stm32Power { cpu_hz: clocks.sysclk_hz },
            watchdog: Stm32Watchdog,
            storage: None,
        }
    }

    /// Hand the board a slice of internal flash to expose as `extmem(0)`.
    /// The firmware owns this decision because only its linker script knows
    /// which sectors the image does not occupy.
    pub fn attach_storage(&mut self, flash: InternalFlash) {
        self.storage = Some(flash);
    }

    /// Touch the registers that must be live before the board is usable.
    /// Split out of `new` so constructing a board is side-effect free —
    /// only this call assumes it is running on the chip.
    pub fn init(&mut self) {
        self.delay.start();
    }
}

impl Board for Stm32F4Board {
    fn name(&self) -> &str {
        #[cfg(feature = "stm32f7")]
        {
            "stm32f7xx"
        }
        #[cfg(not(feature = "stm32f7"))]
        {
            "stm32f4xx"
        }
    }

    fn gpio(&mut self, pin: u32) -> HalResult<&mut dyn GpioPin> {
        if gpio_clock_bit(pin / 16).is_none() {
            return Err(HalError::InvalidArgument(
                "STM32F401 has ports A-E and H (pin = port*16 + index)",
            ));
        }
        self.pins
            .get_mut(pin as usize)
            .map(|p| p as &mut dyn GpioPin)
            .ok_or(HalError::InvalidArgument("pin out of range"))
    }

    fn uart(&mut self, port: u8) -> HalResult<&mut dyn Uart> {
        // The F7's USART is the newer peripheral — status and data live in
        // ISR/RDR/TDR where the F4 has SR/DR, and the baud divisor is computed
        // differently. Driving it through the F4 offsets would write into
        // reserved space and read status bits that are not there, so it is
        // refused by name rather than silently wrong. The F7 firmware reaches
        // its host over USB CDC, which needs no USART at all.
        #[cfg(feature = "stm32f7")]
        {
            let _ = port;
            return Err(HalError::NotSupported); // integration point: USARTv2
        }
        #[cfg(not(feature = "stm32f7"))]
        self.uarts
            .get_mut(port as usize)
            .map(|u| u as &mut dyn Uart)
            .ok_or(HalError::InvalidArgument(
                "uart 0..4 = USART1/USART2/USART6/UART7/UART8 (7 and 8 are F42x+)",
            ))
    }

    fn i2c(&mut self, _bus: u8) -> HalResult<&mut dyn rustclr_hal::i2c::I2cBus> {
        Err(HalError::NotSupported) // integration point: I2C1..3
    }
    fn spi(&mut self, bus: u8) -> HalResult<&mut dyn SpiBus> {
        self.spis
            .get_mut(bus as usize)
            .map(|s| s as &mut dyn SpiBus)
            .ok_or(HalError::InvalidArgument("spi 0/1/2 = SPI1/SPI2/SPI3"))
    }
    fn i2s(&mut self, _port: u8) -> HalResult<&mut dyn rustclr_hal::i2s::I2sBus> {
        Err(HalError::NotSupported) // integration point: SPI2/SPI3 in I2S mode
    }
    fn pwm(&mut self, _channel: u8) -> HalResult<&mut dyn rustclr_hal::pwm::PwmChannel> {
        Err(HalError::NotSupported) // integration point: TIM1..5/9..11 capture-compare
    }
    fn adc(&mut self, _channel: u8) -> HalResult<&mut dyn rustclr_hal::adc::AdcChannel> {
        Err(HalError::NotSupported) // integration point: ADC1
    }
    fn power(&mut self) -> &mut dyn PowerManager {
        &mut self.power
    }
    fn delay(&mut self) -> &mut dyn Delay {
        &mut self.delay
    }
    fn can(&mut self, _bus: u8) -> HalResult<&mut dyn rustclr_hal::can::CanBus> {
        // F401 has no bxCAN; F405/F407 and up do.
        Err(HalError::NotSupported)
    }
    fn onewire(&mut self, _bus: u8) -> HalResult<&mut dyn rustclr_hal::onewire::OneWireBus> {
        Err(HalError::NotSupported) // integration point: timer-driven bit banging on a GPIO
    }
    fn rtc(&mut self) -> &mut dyn Rtc {
        &mut self.rtc
    }
    fn watchdog(&mut self) -> &mut dyn Watchdog {
        &mut self.watchdog
    }
    fn extmem(&mut self, index: u8) -> HalResult<&mut dyn ExtMemory> {
        // 0 is the internal-flash region; QUADSPI would be the next index on a
        // board that has one.
        if index != 0 {
            return Err(HalError::NotSupported);
        }
        self.storage
            .as_mut()
            .map(|f| f as &mut dyn ExtMemory)
            .ok_or(HalError::NotSupported)
    }
    fn netif(
        &mut self,
        _kind: rustclr_hal::netif::NetIfKind,
    ) -> HalResult<&mut dyn rustclr_hal::netif::NetInterface> {
        // F401 has no MAC; an ESP-AT companion over USART is the intended path.
        Err(HalError::NotSupported)
    }
    fn signal(&mut self, _pin: u32) -> HalResult<&mut dyn rustclr_hal::signal::SignalControl> {
        Err(HalError::NotSupported) // integration point: TIM input capture / output compare
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pin_encoding_matches_port_and_index() {
        let pa5 = Stm32Pin { id: 5 };
        assert_eq!((pa5.port(), pa5.index()), (0, 5));
        let pc13 = Stm32Pin { id: 45 };
        assert_eq!((pc13.port(), pc13.index()), (2, 13));
    }

    /// Ports the F401 does not have must be refused, not silently accepted:
    /// a pin index that lands on absent hardware writes into reserved address
    /// space and reads back nothing, which looks like a dead wire.
    #[cfg(not(feature = "stm32f7"))]
    #[test]
    fn missing_ports_are_rejected() {
        let mut board = Stm32F4Board::new(Clocks::NUCLEO_F401RE);
        assert!(board.gpio(5).is_ok()); // PA5
        assert!(board.gpio(45).is_ok()); // PC13
        assert!(board.gpio(16 * 5).is_err()); // GPIOF does not exist
        assert!(board.gpio(16 * 6).is_err()); // GPIOG does not exist
        assert!(board.gpio(16 * 7).is_ok()); // GPIOH does
    }

    /// The same contract on the F7, where the family genuinely has A..K and
    /// the boundary sits one port higher.
    #[cfg(feature = "stm32f7")]
    #[test]
    fn the_f7_accepts_every_port_it_has_and_no_more() {
        let mut board = Stm32F4Board::new(Clocks::MEADOW_F7);
        assert!(board.gpio(5).is_ok()); // PA5
        assert!(board.gpio(16 * 5).is_ok()); // GPIOF exists here
        assert!(board.gpio(16 * 10 + 15).is_ok()); // PK15, the last one
        assert!(board.gpio(16 * 11).is_err()); // there is no GPIOL
    }

    #[test]
    fn usarts_take_their_own_apb_clock() {
        let board = Stm32F4Board::new(Clocks::NUCLEO_F401RE);
        assert_eq!(board.uarts[0].pclk_hz, 84_000_000); // USART1 on APB2
        assert_eq!(board.uarts[1].pclk_hz, 42_000_000); // USART2 on APB1
        assert_eq!(board.uarts[2].pclk_hz, 84_000_000); // USART6 on APB2
        assert_eq!(board.uarts[3].pclk_hz, 42_000_000); // UART7 on APB1
        assert_eq!(board.uarts[4].pclk_hz, 42_000_000); // UART8 on APB1
    }

    #[test]
    fn netduino_runs_faster_but_on_the_same_bus_ceilings() {
        // The F427 doubles the core clock over the F401RE, but APB1/APB2 stay
        // at their 42/84 MHz maxima — so baud divisors are unchanged.
        let n3 = Clocks::NETDUINO3_WIFI;
        assert_eq!(n3.sysclk_hz, 168_000_000);
        assert_eq!((n3.pclk1_hz, n3.pclk2_hz), (42_000_000, 84_000_000));
        let f401 = Clocks::NUCLEO_F401RE;
        assert_eq!((n3.pclk1_hz, n3.pclk2_hz), (f401.pclk1_hz, f401.pclk2_hz));
    }

    #[test]
    fn baud_divisor_rounds_to_nearest() {
        // USART2 at 42 MHz, 115200 baud: 42e6/115200 = 364.58 -> 365
        assert_eq!((42_000_000 + 115_200 / 2) / 115_200, 365);
    }

    #[test]
    fn the_f7_clock_tree_stays_inside_its_bus_ceilings() {
        // 216 MHz is the F7's full speed, and its APB buses cap at a quarter
        // and a half of that. Getting these wrong does not fail to build — it
        // makes every baud rate and every timeout wrong by a fixed ratio.
        let m = Clocks::MEADOW_F7;
        assert_eq!(m.sysclk_hz, 216_000_000);
        assert_eq!(m.pclk1_hz, m.sysclk_hz / 4);
        assert_eq!(m.pclk2_hz, m.sysclk_hz / 2);
        // Comfortably faster than either F4 board, which is the point of it.
        assert!(m.sysclk_hz > Clocks::NETDUINO3_WIFI.sysclk_hz);
    }

    #[test]
    fn the_port_map_matches_the_family_being_built() {
        // The clock bit is the port index on both families; what differs is
        // which ports exist at all. GPIOA and GPIOE are on every part here.
        assert_eq!(gpio_clock_bit(0), Some(0));
        assert_eq!(gpio_clock_bit(4), Some(4));

        // F401 has no F or G, but does have H. The F7 parts have A..K.
        #[cfg(not(feature = "stm32f7"))]
        {
            assert_eq!(gpio_clock_bit(5), None);
            assert_eq!(gpio_clock_bit(6), None);
            assert_eq!(gpio_clock_bit(7), Some(7));
            assert_eq!(gpio_clock_bit(8), None);
        }
        #[cfg(feature = "stm32f7")]
        {
            assert_eq!(gpio_clock_bit(5), Some(5));
            assert_eq!(gpio_clock_bit(10), Some(10)); // GPIOK
            assert_eq!(gpio_clock_bit(11), None);
        }

        // Whatever the family, every port the chip claims must have pins
        // allocated for it — `PIN_COUNT` sizes the array `Board::gpio` indexes.
        let ports = (0..16).filter(|p| gpio_clock_bit(*p).is_some()).count();
        assert!(ports as u32 <= PORT_COUNT, "PIN_COUNT is too small for this family");
    }
}
