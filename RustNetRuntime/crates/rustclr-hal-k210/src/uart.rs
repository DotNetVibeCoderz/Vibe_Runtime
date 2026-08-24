//! The K210's two kinds of serial port.
//!
//! **UARTHS** is a SiFive-style high-speed UART sitting on the TileLink bus,
//! clocked directly from the core. It is what the mask ROM's ISP talks over, so
//! it is also where a Maix board's USB serial bridge lands — pads IO4/IO5 by
//! convention — and therefore where RNDP goes. It is 8-bit, no-parity only, has
//! an 8-entry FIFO in each direction, and its baud divisor is the core clock
//! over the requested rate.
//!
//! **UART1..UART3** are conventional DesignWare 16550s on APB0, with 16-entry
//! FIFOs, real parity and a fractional divisor. They need their clocks ungated
//! and their pads muxed; UARTHS needs neither.
//!
//! Every wait on a status flag here is bounded. An unbounded one costs a board
//! that will not boot rather than a call that fails, and it presents as dead
//! hardware — a lesson the STM32 port paid for on its SPI bus.

use rustclr_hal::uart::{Parity, Uart, UartConfig};
use rustclr_hal::{HalError, HalResult};

use crate::{fpioa, reg, sysctl};

/// Spins to allow a status flag before giving up. Generous next to the ~87 µs a
/// byte takes at 115200, tiny next to a hang.
const SPIN_LIMIT: u32 = 4_000_000;

// ---------------------------------------------------------------------------
// UARTHS
// ---------------------------------------------------------------------------

const UARTHS_BASE: usize = 0x3800_0000;
const HS_TXDATA: usize = UARTHS_BASE + 0x00;
const HS_RXDATA: usize = UARTHS_BASE + 0x04;
const HS_TXCTRL: usize = UARTHS_BASE + 0x08;
const HS_RXCTRL: usize = UARTHS_BASE + 0x0C;
const HS_IE: usize = UARTHS_BASE + 0x10;
const HS_IP: usize = UARTHS_BASE + 0x14;
const HS_DIV: usize = UARTHS_BASE + 0x18;

/// `txdata` bit 31 reads high when the transmit FIFO has no room.
const HS_TX_FULL: u32 = 1 << 31;
/// `rxdata` bit 31 reads high when there was nothing to take — and the read
/// still consumed the slot, which is why [`Uarths::bytes_available`] goes
/// through `ip` instead.
const HS_RX_EMPTY: u32 = 1 << 31;
const HS_TXEN: u32 = 1 << 0;
const HS_NSTOP: u32 = 1 << 1;
const HS_RXEN: u32 = 1 << 0;
/// `ie`/`ip` bit 0: transmit watermark. With the watermark at 0 it means the
/// transmit FIFO has drained.
pub const HS_IP_TXWM: u32 = 1 << 0;
/// `ie`/`ip` bit 1: receive watermark. With the watermark at 0 it means at
/// least one byte is waiting.
pub const HS_IP_RXWM: u32 = 1 << 1;

/// PLIC source number for UARTHS.
pub const IRQ_UARTHS: u32 = 33;

/// PLIC sources for the three 16550 ports. Kendryte numbers them from 11.
pub const IRQ_UART1: u32 = 11;

/// The high-speed UART. Reads and writes go straight at the registers, so a
/// panic handler can borrow this without owning the board.
pub struct Uarths {
    cpu_hz: u32,
    /// Pads the console is muxed to. `None` leaves whatever the ROM set up,
    /// which is already IO4/IO5 on every Maix board — muxing again is
    /// harmless, but not muxing at all is one less thing to get wrong.
    pins: Option<(u8, u8)>,
}

impl Uarths {
    pub const fn new(cpu_hz: u32, pins: Option<(u8, u8)>) -> Self {
        Self { cpu_hz, pins }
    }

    pub fn set_cpu_hz(&mut self, cpu_hz: u32) {
        self.cpu_hz = cpu_hz;
    }

    /// `baud = cpu_hz / (div + 1)`, so `div = cpu_hz / baud - 1`.
    pub fn divisor(cpu_hz: u32, baud: u32) -> u32 {
        (cpu_hz / baud.max(1)).saturating_sub(1)
    }

    /// Enable or disable the receive-watermark interrupt.
    ///
    /// Associated rather than a method, like [`Uarths::take_byte`] below: the
    /// firmware turns this on from its interrupt setup, which has no `Board` to
    /// borrow, and the trap handler it arms cannot borrow one either.
    pub fn set_rx_interrupt(on: bool) {
        reg::write(HS_IE, if on { HS_IP_RXWM } else { 0 });
    }

    /// Take one byte if there is one, without blocking.
    ///
    /// Free-standing because both the trap handler and the polled drain need
    /// it, and neither of them has the `&mut Board` that the trait method
    /// requires.
    #[inline]
    pub fn take_byte() -> Option<u8> {
        let word = reg::read(HS_RXDATA);
        if word & HS_RX_EMPTY != 0 {
            None
        } else {
            Some((word & 0xFF) as u8)
        }
    }

    /// Put one byte in the transmit FIFO, waiting for room. Used by the panic
    /// path, which has no board to borrow.
    pub fn put_byte(byte: u8) {
        for _ in 0..SPIN_LIMIT {
            if reg::read(HS_TXDATA) & HS_TX_FULL == 0 {
                reg::write(HS_TXDATA, byte as u32);
                return;
            }
            core::hint::spin_loop();
        }
    }
}

impl Uart for Uarths {
    fn configure(&mut self, config: UartConfig) -> HalResult<()> {
        if config.data_bits != 8 || config.parity != Parity::None {
            return Err(HalError::InvalidArgument(
                "UARTHS is 8 data bits, no parity — use UART1..3 for anything else",
            ));
        }
        if let Some((tx, rx)) = self.pins {
            fpioa::set_function(tx, fpioa::UARTHS_TX);
            fpioa::set_function(rx, fpioa::UARTHS_RX);
        }

        reg::write(HS_DIV, Self::divisor(self.cpu_hz, config.baud));
        // Watermarks left at 0 in both directions, which is what makes `ip`
        // mean "FIFO drained" and "a byte is waiting" rather than "some level
        // was crossed".
        let stop = if config.stop_bits >= 2 { HS_NSTOP } else { 0 };
        reg::write(HS_TXCTRL, HS_TXEN | stop);
        reg::write(HS_RXCTRL, HS_RXEN);
        reg::write(HS_IE, 0);
        Ok(())
    }

    fn write(&mut self, data: &[u8]) -> HalResult<usize> {
        for byte in data {
            let mut room = false;
            for _ in 0..SPIN_LIMIT {
                if reg::read(HS_TXDATA) & HS_TX_FULL == 0 {
                    room = true;
                    break;
                }
                core::hint::spin_loop();
            }
            if !room {
                return Err(HalError::Timeout);
            }
            reg::write(HS_TXDATA, *byte as u32);
        }
        Ok(data.len())
    }

    fn read(&mut self, buf: &mut [u8]) -> HalResult<usize> {
        let mut n = 0;
        while n < buf.len() {
            match Self::take_byte() {
                Some(byte) => {
                    buf[n] = byte;
                    n += 1;
                }
                None => break,
            }
        }
        Ok(n)
    }

    /// At least one byte, or none. UARTHS has no receive-level register and
    /// reading `rxdata` consumes the slot, so an exact count cannot be had
    /// without destroying it — the watermark bit is the honest answer.
    fn bytes_available(&mut self) -> HalResult<usize> {
        Ok(usize::from(reg::read(HS_IP) & HS_IP_RXWM != 0))
    }

    fn flush(&mut self) -> HalResult<()> {
        for _ in 0..SPIN_LIMIT {
            if reg::read(HS_IP) & HS_IP_TXWM != 0 {
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err(HalError::Timeout)
    }
}

// ---------------------------------------------------------------------------
// UART1..UART3 (DesignWare 16550)
// ---------------------------------------------------------------------------

const RBR_DLL_THR: usize = 0x00;
const DLH_IER: usize = 0x04;
const IER_DLH: usize = 0x04;
const FCR: usize = 0x08;
/// Enable the received-data-available interrupt.
const IER_ERBFI: u32 = 1 << 0;
const LCR: usize = 0x0C;
const LSR: usize = 0x14;
const RFL: usize = 0x84;
const DLF: usize = 0xC0;

const LSR_DR: u32 = 1 << 0;
const LSR_THRE: u32 = 1 << 5;
const LSR_TEMT: u32 = 1 << 6;
const LCR_DLAB: u32 = 1 << 7;

/// Everything that differs between the three 16550 ports.
///
/// `pins` starts as `None`, and that is not laziness. On an STM32 each
/// peripheral has a short fixed menu of pins and picking the common one is a
/// sensible default; on the K210 any peripheral can go to any of 48 pads, so
/// there is no such thing as UART2's pins. The board says, or the pads keep
/// whatever they were already doing — which beats a guess that quietly steals a
/// pad from something else. On a Maix Go, for instance, IO6/IO7/IO8 are the
/// on-board ESP8285's UART and enable line, so a "default" of IO8 for UART2
/// would hold the WiFi module in reset.
#[derive(Clone, Copy)]
pub struct UartDef {
    pub base: usize,
    pub port: u8,
    pub clock: sysctl::Peripheral,
    /// `(tx_pad, rx_pad)`, once a board has said.
    pub pins: Option<(u8, u8)>,
}

/// UART1..UART3, in HAL port order (`Board::uart(1..=3)`).
/// Take one received byte from a 16550 port, by base address.
///
/// A free function rather than a method because the interrupt handler has no
/// `K210Uart` to borrow — the driver lives inside the board, behind a `&mut`
/// that an ISR cannot take without a lock the rest of this port does not have.
/// Reading a hardware FIFO needs neither.
///
/// # Safety
/// `base` must be one of [`UARTS`]' base addresses.
pub fn take_byte_at(base: usize) -> Option<u8> {
    if reg::read(base + LSR) & LSR_DR == 0 {
        return None;
    }
    Some((reg::read(base + RBR_DLL_THR) & 0xFF) as u8)
}

/// Raise an interrupt when the receive FIFO has something in it.
///
/// Without this the port is polled, and polling a 16-byte FIFO at 115200 baud
/// means anything arriving during a slow frame is lost rather than delayed:
/// 16 bytes is 1.4 ms, and a camera capture plus a panel blit is a hundred
/// times that.
pub fn enable_rx_interrupt(base: usize) {
    // Receive-data-available only. The transmit-empty interrupt would fire
    // continuously on an idle port.
    reg::write(base + IER_DLH, IER_ERBFI);
}

pub const UARTS: [UartDef; 3] = [
    UartDef { base: 0x5021_0000, port: 1, clock: sysctl::Peripheral::Uart1, pins: None },
    UartDef { base: 0x5022_0000, port: 2, clock: sysctl::Peripheral::Uart2, pins: None },
    UartDef { base: 0x5023_0000, port: 3, clock: sysctl::Peripheral::Uart3, pins: None },
];

pub struct K210Uart {
    def: UartDef,
    apb0_hz: u32,
}

impl K210Uart {
    pub const fn new(def: UartDef, apb0_hz: u32) -> Self {
        Self { def, apb0_hz }
    }

    pub fn set_apb0_hz(&mut self, hz: u32) {
        self.apb0_hz = hz;
    }

    /// Route this port to a pair of pads. Takes effect on the next
    /// [`Uart::configure`].
    pub fn set_pins(&mut self, tx: u8, rx: u8) {
        self.def.pins = Some((tx, rx));
    }

    /// `(dlh, dll, dlf)` for `baud` off an APB0 of `apb0_hz`.
    ///
    /// The IP oversamples 16×, so the integer divisor is `clk / (16 * baud)` —
    /// but it also has a 4-bit *fractional* divisor. Computing `clk / baud`
    /// first and then splitting it gives both at once: bits 4 and up are the
    /// integer divisor, the low nibble is the fraction in sixteenths. That is
    /// how Kendryte derives it, and it matters: 201.5 MHz needs a divisor of
    /// 109.32 for 115200, and rounding that to 109 alone is 0.3% off before the
    /// fraction is even considered.
    pub fn divisors(apb0_hz: u32, baud: u32) -> (u8, u8, u8) {
        let divisor = apb0_hz / baud.max(1);
        let dlh = ((divisor >> 12) & 0xFF) as u8;
        let dll = ((divisor >> 4) & 0xFF) as u8;
        let dlf = (divisor & 0xF) as u8;
        (dlh, dll, dlf)
    }

    /// The line-control word for a frame format.
    pub fn line_control(config: &UartConfig) -> HalResult<u32> {
        if !(5..=8).contains(&config.data_bits) {
            return Err(HalError::InvalidArgument("data_bits must be 5..=8"));
        }
        let mut word = u32::from(config.data_bits - 5);
        if config.stop_bits >= 2 {
            word |= 1 << 2;
        }
        match config.parity {
            Parity::None => {}
            Parity::Odd => word |= 1 << 3,
            Parity::Even => word |= (1 << 3) | (1 << 4),
        }
        Ok(word)
    }

    #[inline]
    fn r(&self, offset: usize) -> u32 {
        reg::read(self.def.base + offset)
    }

    #[inline]
    fn w(&self, offset: usize, value: u32) {
        reg::write(self.def.base + offset, value);
    }
}

impl Uart for K210Uart {
    fn configure(&mut self, config: UartConfig) -> HalResult<()> {
        let lcr = Self::line_control(&config)?;

        sysctl::clock_enable(self.def.clock);
        sysctl::reset(self.def.clock);

        if let Some((tx, rx)) = self.def.pins {
            let (rx_function, tx_function) = fpioa::uart(self.def.port);
            fpioa::set_function(tx, tx_function);
            fpioa::set_function(rx, rx_function);
        }

        let (dlh, dll, dlf) = Self::divisors(self.apb0_hz, config.baud);
        // DLAB swaps the first two registers over to the divisor latches, so
        // the frame format has to be written *after* it is cleared again.
        self.w(LCR, LCR_DLAB);
        self.w(DLH_IER, dlh as u32);
        self.w(RBR_DLL_THR, dll as u32);
        self.w(DLF, dlf as u32);
        self.w(LCR, lcr);

        // No interrupts: this port is a peripheral for application code, and
        // the console that needs interrupt-driven receive is UARTHS.
        self.w(DLH_IER, 0);
        // FIFOs on, both flushed, receive trigger at one character so a single
        // byte is visible immediately.
        self.w(FCR, 0b111);
        Ok(())
    }

    fn write(&mut self, data: &[u8]) -> HalResult<usize> {
        for byte in data {
            let mut room = false;
            for _ in 0..SPIN_LIMIT {
                if self.r(LSR) & LSR_THRE != 0 {
                    room = true;
                    break;
                }
                core::hint::spin_loop();
            }
            if !room {
                return Err(HalError::Timeout);
            }
            self.w(RBR_DLL_THR, *byte as u32);
        }
        Ok(data.len())
    }

    fn read(&mut self, buf: &mut [u8]) -> HalResult<usize> {
        let mut n = 0;
        while n < buf.len() && self.r(LSR) & LSR_DR != 0 {
            buf[n] = (self.r(RBR_DLL_THR) & 0xFF) as u8;
            n += 1;
        }
        Ok(n)
    }

    fn bytes_available(&mut self) -> HalResult<usize> {
        Ok((self.r(RFL) & 0x1F) as usize)
    }

    fn flush(&mut self) -> HalResult<()> {
        for _ in 0..SPIN_LIMIT {
            if self.r(LSR) & LSR_TEMT != 0 {
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err(HalError::Timeout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// UARTHS divides the *core* clock, which is why its divisor changes if the
    /// PLL setup ever does.
    #[test]
    fn uarths_divisor_is_core_clock_over_baud_minus_one() {
        // 403 MHz / 115200 = 3498.26, floored and less one.
        assert_eq!(Uarths::divisor(403_000_000, 115_200), 3497);
        // Straight off the crystal, if the ROM never left the core on PLL0.
        assert_eq!(Uarths::divisor(26_000_000, 115_200), 224);
    }

    /// A nonsense baud must not divide by zero during bring-up.
    #[test]
    fn uarths_divisor_survives_a_zero_baud() {
        assert_eq!(Uarths::divisor(403_000_000, 0), 402_999_999);
    }

    /// The fraction is the point. 201.5 MHz needs 109.32 for 115200; the
    /// integer latches carry 109 and DLF carries 5/16 of the rest.
    #[test]
    fn sixteen_five_fifty_divisor_splits_into_integer_and_fraction() {
        let (dlh, dll, dlf) = K210Uart::divisors(201_500_000, 115_200);
        assert_eq!((dlh, dll, dlf), (0, 109, 5));

        // Nothing is lost in the split: the three latches put the raw
        // clk/baud divisor back together exactly.
        let recombined = (u32::from(dlh) << 12) | (u32::from(dll) << 4) | u32::from(dlf);
        assert_eq!(recombined, 201_500_000 / 115_200);
    }

    /// dlh only comes into play at low baud rates off a fast bus, where the
    /// divisor outgrows twelve bits.
    #[test]
    fn a_slow_rate_pushes_the_divisor_into_the_high_latch() {
        let (dlh, dll, dlf) = K210Uart::divisors(201_500_000, 9_600);
        let recombined = (u32::from(dlh) << 12) | (u32::from(dll) << 4) | u32::from(dlf);
        assert_eq!(recombined, 201_500_000 / 9_600);
        assert!(dlh > 0, "expected the high latch to be in use");
    }

    #[test]
    fn line_control_encodes_the_frame_format() {
        let eight_n_1 = UartConfig::default();
        assert_eq!(K210Uart::line_control(&eight_n_1).unwrap(), 0b011);

        let eight_e_1 = UartConfig { parity: Parity::Even, ..Default::default() };
        assert_eq!(K210Uart::line_control(&eight_e_1).unwrap(), 0b1_1011);

        let seven_o_2 =
            UartConfig { data_bits: 7, stop_bits: 2, parity: Parity::Odd, ..Default::default() };
        assert_eq!(K210Uart::line_control(&seven_o_2).unwrap(), 0b0_1110);

        let nonsense = UartConfig { data_bits: 9, ..Default::default() };
        assert!(K210Uart::line_control(&nonsense).is_err());
    }

    #[test]
    fn ports_are_listed_in_hal_order() {
        for (index, def) in UARTS.iter().enumerate() {
            assert_eq!(def.port as usize, index + 1);
        }
    }
}
