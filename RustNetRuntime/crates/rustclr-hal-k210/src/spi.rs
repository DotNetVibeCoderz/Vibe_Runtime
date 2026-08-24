//! SPI — the K210's three DesignWare SSI masters.
//!
//! | HAL bus | Controller | What it is for on a Maix board |
//! |---|---|---|
//! | 0 | SPI0 | the LCD header |
//! | 1 | SPI1 | the microSD slot |
//! | — | SPI2 | slave-only silicon; not a master, so not exposed |
//! | — | SPI3 | the boot flash, owned by [`crate::flash`] and reached as `extmem(0)` |
//!
//! Two K210 quirks are worth knowing before reading the register writes.
//!
//! **The transfer-mode field moves.** `tmod` sits at bits 8..9 of `ctrlr0` on
//! SPI0/1/2 but at bits 10..11 on SPI3. Kendryte's own driver special-cases it,
//! and getting it wrong on SPI3 leaves the controller in receive mode while you
//! think you are transmitting — a bus that clocks and says nothing.
//!
//! **Chip select is a mask, and it gates the clock.** Writing `ser` is what
//! starts a transfer; clearing it ends one. So the sequence is always: disable,
//! configure, enable, preload the transmit FIFO, *then* assert `ser` — which
//! keeps the first bytes from trickling out with gaps between them.
//!
//! Every FIFO wait is bounded. The STM32 port learned this the expensive way:
//! an unbounded wait on a bus that never clocks hangs the firmware before its
//! service loop starts, and that reads as a board which will not enumerate.

use rustclr_hal::spi::{SpiBus, SpiMode};
use rustclr_hal::{HalError, HalResult};

use crate::{reg, sysctl};

const CTRLR0: usize = 0x00;
const CTRLR1: usize = 0x04;
const SSIENR: usize = 0x08;
const SER: usize = 0x10;
const BAUDR: usize = 0x14;
const TXFTLR: usize = 0x18;
const RXFTLR: usize = 0x1C;
const TXFLR: usize = 0x20;
const RXFLR: usize = 0x24;
const SR: usize = 0x28;
const IMR: usize = 0x2C;
const DMACR: usize = 0x4C;
const DR: usize = 0x60;
const SPI_CTRLR0: usize = 0xF4;
const ENDIAN: usize = 0x118;

const SR_BUSY: u32 = 1 << 0;
const SR_TFE: u32 = 1 << 2;

/// Both FIFOs are 32 entries deep.
pub const FIFO_DEPTH: u32 = 32;

/// Transfer modes, in `tmod` encoding.
const TMOD_TX_RX: u32 = 0;
const TMOD_TX: u32 = 1;
const TMOD_RX: u32 = 2;
/// Send a command, then receive — the mode a NOR flash read wants, because the
/// controller holds chip select across the turnaround.
const TMOD_EEPROM: u32 = 3;

/// Iterations to allow a FIFO to move before declaring the bus dead.
const SPIN_LIMIT: u32 = 8_000_000;

/// Most frames one receive-only or EEPROM-mode transfer can ask for.
///
/// The count goes in `ctrlr1`, which is 16 bits, and the controller generates
/// exactly `ctrlr1 + 1` frames. A caller wanting more has to split the transfer
/// — and for a NOR flash that means re-issuing the read command at an advanced
/// address, which only the flash driver knows how to do. Hence a rejection here
/// rather than a silent truncation.
pub const MAX_RECEIVE_FRAMES: usize = 65536;

/// How many of the data lines a transfer uses. The panel on a Maix board is
/// the only reason anything but [`FrameFormat::Standard`] exists here.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FrameFormat {
    Standard = 0,
    Dual = 1,
    Quad = 2,
    Octal = 3,
}

/// Everything that differs between the controllers.
///
/// The three shift fields are not decoration. `ctrlr0`'s layout genuinely moves
/// between SPI0/1 and SPI3, and the port got away with a single layout only
/// because every field it was writing to the wrong place happened to be zero:
/// `SpiMode::Mode0` encodes as 0, so a work-mode written at bit 6 instead of 8
/// changed nothing, and the frame size written at bit 16 instead of 0 left
/// SPI3's `dfs` at its reset value, which is the 8 bits the flash wanted. That
/// is luck, not correctness, and it would have expired the first time anything
/// asked SPI3 for `Mode3`.
#[derive(Clone, Copy)]
pub struct SpiDef {
    pub base: usize,
    pub bus: u8,
    pub clock: sysctl::Peripheral,
    /// Bit position of `tmod` within `ctrlr0` — 8 for SPI0/1/2, 10 for SPI3.
    pub tmod_shift: u32,
    /// Bit position of the CPOL/CPHA pair — 6 for SPI0/1/2, 8 for SPI3.
    pub work_mode_shift: u32,
    /// Bit position of the frame-format field — 21 for SPI0/1/2, 22 for SPI3.
    pub frame_format_shift: u32,
    /// Bit position of the data-frame-size field. SPI0/1 carry the 32-bit-capable
    /// `dfs_32` at bit 16; SPI3 has the legacy 4-bit `dfs` at bit 0 and so cannot
    /// do frames wider than 16 bits at all.
    pub frame_bits_shift: u32,
    /// Widest frame this controller's size field can encode.
    pub max_frame_bits: u32,
}

pub const SPI0: SpiDef = SpiDef {
    base: 0x5200_0000,
    bus: 0,
    clock: sysctl::Peripheral::Spi0,
    tmod_shift: 8,
    work_mode_shift: 6,
    frame_format_shift: 21,
    frame_bits_shift: 16,
    max_frame_bits: 32,
};
pub const SPI1: SpiDef = SpiDef {
    base: 0x5300_0000,
    bus: 1,
    clock: sysctl::Peripheral::Spi1,
    tmod_shift: 8,
    work_mode_shift: 6,
    frame_format_shift: 21,
    frame_bits_shift: 16,
    max_frame_bits: 32,
};
pub const SPI3: SpiDef = SpiDef {
    base: 0x5400_0000,
    bus: 3,
    clock: sysctl::Peripheral::Spi3,
    tmod_shift: 10,
    work_mode_shift: 8,
    frame_format_shift: 22,
    frame_bits_shift: 0,
    max_frame_bits: 16,
};

/// Masters exposed through `Board::spi`, in HAL bus order.
pub const BUSES: [SpiDef; 2] = [SPI0, SPI1];

pub struct K210Spi {
    def: SpiDef,
    /// Frequency of the clock feeding the controller, for the baud divisor.
    source_hz: u32,
    /// Bus clock asked for by the last `configure`. A conservative 1 MHz until
    /// then, so a driver that transfers before configuring is slow rather than
    /// out of spec.
    target_hz: u32,
    mode: SpiMode,
    /// Which hardware chip-select line to assert. Boards that drive CS from a
    /// GPIO leave this at 0 and simply never wire `ss0`.
    cs: u8,
    /// How many data lines a transfer uses.
    format: FrameFormat,
    /// Bits per frame. Eight unless a driver widens it — a panel streams pixels
    /// in 32-bit frames purely to get four bytes per FIFO entry.
    frame_bits: u32,
    /// `spi_ctrlr0`, the enhanced-format control register.
    ///
    /// Zero is right for [`FrameFormat::Standard`] and **wrong** for everything
    /// else. In the multi-line formats this register's `trans_type` says
    /// whether the instruction and address phases go out in standard one-bit
    /// SPI or in the wide frame format; left at zero they go out one-bit, and
    /// a transfer that is nominally octal never reaches the other seven lines.
    ///
    /// The value was read out of a running MaixPy, which drives this board's
    /// panel, after it had done so. See [`K210Spi::set_non_standard`].
    non_standard: u32,
}

impl K210Spi {
    pub const fn new(def: SpiDef, source_hz: u32) -> Self {
        Self {
            def,
            source_hz,
            target_hz: 1_000_000,
            mode: SpiMode::Mode0,
            cs: 0,
            format: FrameFormat::Standard,
            frame_bits: 8,
            non_standard: 0,
        }
    }

    /// Set `spi_ctrlr0` for the enhanced frame formats.
    ///
    /// The panel wants `0x22`: `trans_type = 2` — instruction and address are
    /// sent in the same wide format as the data rather than in one-bit SPI —
    /// with a 32-bit address field and no instruction. That is what a live
    /// MaixPy has in this register while it is driving the screen, and leaving
    /// it at zero is what kept RustNet's octal writes from ever reaching the
    /// panel: every control line worked, every transfer completed, and the data
    /// went out one line wide to a panel listening on eight.
    pub fn set_non_standard(&mut self, value: u32) {
        self.non_standard = value;
    }

    /// Widen the bus. In octal mode one 8-bit frame is a single clock across
    /// eight lines, so this is an eightfold throughput change, not a subtlety.
    /// Clock polarity and phase, for a driver that owns its controller rather
    /// than reaching it through [`SpiBus::configure`].
    pub fn set_mode(&mut self, mode: SpiMode) {
        self.mode = mode;
    }

    pub fn set_frame_format(&mut self, format: FrameFormat) {
        self.format = format;
    }

    /// Bits per frame, clamped to what this controller's size field can hold.
    ///
    /// The multi-line formats need whole beats: a frame has to divide evenly
    /// into the number of data lines, or the controller shifts a partial beat
    /// and the bytes arrive rearranged.
    pub fn set_frame_bits(&mut self, bits: u32) -> HalResult<()> {
        let lanes = match self.format {
            FrameFormat::Standard => 1,
            FrameFormat::Dual => 2,
            FrameFormat::Quad => 4,
            FrameFormat::Octal => 8,
        };
        if !(4..=self.def.max_frame_bits).contains(&bits) || bits % lanes != 0 {
            return Err(HalError::InvalidArgument("unsupported SPI frame width"));
        }
        self.frame_bits = bits;
        Ok(())
    }

    pub fn frame_bits(&self) -> u32 {
        self.frame_bits
    }

    pub fn set_source_hz(&mut self, hz: u32) {
        self.source_hz = hz;
    }

    /// Choose which of the four `ss` lines a transfer asserts.
    pub fn set_chip_select(&mut self, cs: u8) {
        self.cs = cs & 0x3;
    }

    /// Bus clock to aim for, for drivers that own a controller outright rather
    /// than reaching it through [`SpiBus::configure`].
    pub fn set_target_hz(&mut self, hz: u32) {
        self.target_hz = hz.max(1);
    }

    /// `baudr` is a straight divide of the source clock and must be even; the
    /// controller treats 0 as "no clock", so 2 is the floor.
    pub fn baud_divisor(source_hz: u32, target_hz: u32) -> u32 {
        let divisor = source_hz / target_hz.max(1);
        divisor.clamp(2, 65534) & !1
    }

    /// `ctrlr0` for the current mode, frame format and frame width.
    pub fn control_word(&self, tmod: u32) -> u32 {
        let work_mode = match self.mode {
            SpiMode::Mode0 => 0,
            SpiMode::Mode1 => 1,
            SpiMode::Mode2 => 2,
            SpiMode::Mode3 => 3,
        };
        // Frame size is encoded as bits-1.
        (work_mode << self.def.work_mode_shift)
            | (tmod << self.def.tmod_shift)
            | ((self.format as u32) << self.def.frame_format_shift)
            | ((self.frame_bits - 1) << self.def.frame_bits_shift)
    }

    #[inline]
    fn r(&self, offset: usize) -> u32 {
        reg::read(self.def.base + offset)
    }

    #[inline]
    fn w(&self, offset: usize, value: u32) {
        reg::write(self.def.base + offset, value);
    }

    /// Put the controller in a known state for one transfer, still disabled.
    ///
    /// `ctrlr1` is cleared here and not only by the reads that set it. A
    /// receive leaves its frame count behind, and a later transmit on the same
    /// controller inherits it — measurably: a single four-byte panel read at
    /// boot doubled the cost of every frame the demo drew afterwards, 64 ms to
    /// 126, and put it back the moment the read was removed. Anything a
    /// transfer depends on belongs in the setup for *that* transfer.
    fn begin(&self, tmod: u32) {
        self.w(SSIENR, 0);
        self.w(CTRLR0, self.control_word(tmod));
        self.w(CTRLR1, 0);
        self.w(SPI_CTRLR0, self.non_standard);
        self.w(ENDIAN, 0);
        self.w(DMACR, 0);
        self.w(IMR, 0);
        self.w(TXFTLR, 0);
        self.w(RXFTLR, 0);
        self.w(BAUDR, Self::baud_divisor(self.source_hz, self.target_hz));
    }

    /// Release chip select and shut the controller down again. Leaving it
    /// enabled between transfers lets a stray clock edge out and, worse, leaves
    /// a partly-consumed FIFO to surface as the next command's response.
    fn end(&self) {
        self.w(SER, 0);
        self.w(SSIENR, 0);
    }

    /// Wait for the transmit FIFO to drain and the shifter to stop.
    fn wait_idle(&self) -> HalResult<()> {
        for _ in 0..SPIN_LIMIT {
            let sr = self.r(SR);
            if sr & SR_TFE != 0 && sr & SR_BUSY == 0 {
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err(HalError::Timeout)
    }

    /// Push `bytes` into the transmit FIFO, waiting for room.
    fn push(&self, bytes: &[u8]) -> HalResult<()> {
        let mut sent = 0;
        let mut spins = 0u32;
        while sent < bytes.len() {
            let room = FIFO_DEPTH.saturating_sub(self.r(TXFLR));
            if room == 0 {
                spins += 1;
                if spins > SPIN_LIMIT {
                    return Err(HalError::Timeout);
                }
                core::hint::spin_loop();
                continue;
            }
            spins = 0;
            for _ in 0..room.min((bytes.len() - sent) as u32) {
                self.w(DR, bytes[sent] as u32);
                sent += 1;
            }
        }
        Ok(())
    }

    /// Send a command and read the response with chip select held across the
    /// turnaround. This is the primitive a NOR flash read is built from, and it
    /// is why the driver in [`crate::flash`] does not need to touch registers.
    pub fn read_after_command(&mut self, command: &[u8], rx: &mut [u8]) -> HalResult<()> {
        if rx.is_empty() {
            return Ok(());
        }
        if rx.len() > MAX_RECEIVE_FRAMES {
            return Err(HalError::InvalidArgument(
                "one transfer can receive at most 65536 frames (ctrlr1 is 16 bits)",
            ));
        }
        self.begin(if command.is_empty() { TMOD_RX } else { TMOD_EEPROM });
        // The controller generates exactly this many receive frames itself,
        // which is what makes the read happen without anything to transmit.
        self.w(CTRLR1, (rx.len() - 1) as u32);
        self.w(SSIENR, 1);
        if !command.is_empty() {
            self.push(command)?;
        }
        self.w(SER, 1 << self.cs);

        let mut received = 0;
        let mut spins = 0u32;
        while received < rx.len() {
            let ready = self.r(RXFLR);
            if ready == 0 {
                spins += 1;
                if spins > SPIN_LIMIT {
                    self.end();
                    return Err(HalError::Timeout);
                }
                core::hint::spin_loop();
                continue;
            }
            spins = 0;
            for _ in 0..ready.min((rx.len() - received) as u32) {
                rx[received] = (self.r(DR) & 0xFF) as u8;
                received += 1;
            }
        }
        self.end();
        Ok(())
    }

    /// Send a command followed by data, as one chip-select assertion.
    pub fn write_after_command(&mut self, command: &[u8], data: &[u8]) -> HalResult<()> {
        self.begin(TMOD_TX);
        self.w(SSIENR, 1);
        // Preload before asserting chip select so the frame goes out contiguous.
        if !command.is_empty() {
            self.push(command)?;
        }
        self.w(SER, 1 << self.cs);
        let result = self.push(data).and_then(|()| self.wait_idle());
        self.end();
        result
    }

    /// Stream RGB565 pixels as one chip-select assertion, two pixels per
    /// 32-bit frame.
    ///
    /// Packing pairs is not about the wire — in octal mode a byte costs one
    /// clock either way — it is about the FIFO. Thirty-two entries of 8-bit
    /// frames is 32 bytes of slack between refills; of 32-bit frames it is 128.
    /// At a panel's clock that is the difference between a refill loop that has
    /// to be prompt and one that cannot plausibly be late, and a transmit FIFO
    /// that runs dry drops chip select in the middle of the pixel run.
    ///
    /// The pair is packed big-endian within the word because the controller
    /// shifts a frame out most-significant byte first, and the panel wants each
    /// pixel's high byte first.
    ///
    /// Sending a byte per frame instead was tried on hardware, on the theory
    /// that the frame width was why the panel took commands and ignored pixels.
    /// It made no difference to the panel and cost 50% of the frame rate —
    /// 64 ms a frame became 95 — so the pairs stayed.
    pub fn write_rgb565(&mut self, pixels: &[u16]) -> HalResult<()> {
        if pixels.is_empty() {
            return Ok(());
        }
        if pixels.len() % 2 != 0 {
            return Err(HalError::InvalidArgument("pixel runs pack in pairs"));
        }
        if self.def.max_frame_bits < 32 {
            return Err(HalError::InvalidArgument("this controller cannot do 32-bit frames"));
        }
        let restore = self.frame_bits;
        self.frame_bits = 32;
        self.begin(TMOD_TX);
        self.w(SSIENR, 1);
        self.w(SER, 1 << self.cs);

        let mut result = Ok(());
        let mut sent = 0usize;
        let mut spins = 0u32;
        while sent < pixels.len() {
            let room = FIFO_DEPTH.saturating_sub(self.r(TXFLR));
            if room == 0 {
                spins += 1;
                if spins > SPIN_LIMIT {
                    result = Err(HalError::Timeout);
                    break;
                }
                core::hint::spin_loop();
                continue;
            }
            spins = 0;
            let pairs = room.min(((pixels.len() - sent) / 2) as u32);
            for _ in 0..pairs {
                let word = ((pixels[sent] as u32) << 16) | pixels[sent + 1] as u32;
                self.w(DR, word);
                sent += 2;
            }
        }
        if result.is_ok() {
            result = self.wait_idle();
        }
        self.end();
        self.frame_bits = restore;
        result
    }
}

impl SpiBus for K210Spi {
    fn configure(&mut self, hz: u32, mode: SpiMode) -> HalResult<()> {
        if hz == 0 {
            return Err(HalError::InvalidArgument("SPI clock must be non-zero"));
        }
        self.mode = mode;
        self.target_hz = hz;
        sysctl::clock_enable(self.def.clock);
        // SPI3 is the path the ROM just read this image over. Everything else
        // gets a reset; that one only gets configured.
        if self.def.bus != 3 {
            sysctl::reset(self.def.clock);
        }
        self.begin(TMOD_TX_RX);
        Ok(())
    }

    fn transfer(&mut self, tx: &[u8], rx: &mut [u8]) -> HalResult<()> {
        if tx.len() != rx.len() {
            return Err(HalError::InvalidArgument("full-duplex transfer needs equal lengths"));
        }
        if tx.is_empty() {
            return Ok(());
        }

        self.begin(TMOD_TX_RX);
        self.w(SSIENR, 1);

        // Preload, then assert chip select, then keep both FIFOs moving. The
        // in-flight count is capped at the FIFO depth on purpose: every byte
        // sent produces one to receive, so letting transmit run ahead would
        // overflow the receive FIFO and lose the answer rather than delay it.
        let mut sent = 0;
        let first = tx.len().min(FIFO_DEPTH as usize);
        self.push(&tx[..first])?;
        sent += first;
        self.w(SER, 1 << self.cs);

        let mut received = 0;
        let mut spins = 0u32;
        while received < rx.len() {
            let ready = self.r(RXFLR);
            if ready > 0 {
                spins = 0;
                for _ in 0..ready.min((rx.len() - received) as u32) {
                    rx[received] = (self.r(DR) & 0xFF) as u8;
                    received += 1;
                }
                let in_flight = sent - received;
                let room = (FIFO_DEPTH as usize).saturating_sub(in_flight);
                let next = room.min(tx.len() - sent);
                if next > 0 {
                    self.push(&tx[sent..sent + next])?;
                    sent += next;
                }
            } else {
                spins += 1;
                if spins > SPIN_LIMIT {
                    self.end();
                    return Err(HalError::Timeout);
                }
                core::hint::spin_loop();
            }
        }

        let result = self.wait_idle();
        self.end();
        result
    }

    fn write(&mut self, tx: &[u8]) -> HalResult<()> {
        // Overrides the trait default, which allocates a receive buffer the
        // same size as the data — wasteful for a display or a flash page, and
        // pointless when the controller can be told not to receive at all.
        self.write_after_command(&[], tx)
    }

    fn read(&mut self, rx: &mut [u8]) -> HalResult<()> {
        self.read_after_command(&[], rx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spi3_keeps_its_transfer_mode_field_two_bits_higher() {
        assert_eq!(SPI0.tmod_shift, 8);
        assert_eq!(SPI1.tmod_shift, 8);
        assert_eq!(SPI3.tmod_shift, 10);
    }

    /// The same requested mode has to land in a different place for SPI3, which
    /// is the whole reason `tmod_shift` exists.
    #[test]
    fn the_control_word_places_transfer_mode_per_controller() {
        let on0 = K210Spi::new(SPI0, 100_000_000);
        let on3 = K210Spi::new(SPI3, 100_000_000);

        assert_eq!(on0.control_word(TMOD_EEPROM) & (0b11 << 8), 0b11 << 8);
        assert_eq!(on3.control_word(TMOD_EEPROM) & (0b11 << 10), 0b11 << 10);
        // ...and does not leak into the other controller's field.
        assert_eq!(on3.control_word(TMOD_EEPROM) & (0b11 << 8), 0);
    }

    #[test]
    fn eight_bit_frames_and_the_standard_format() {
        let spi = K210Spi::new(SPI0, 100_000_000);
        let word = spi.control_word(TMOD_TX_RX);
        // data_bit_length - 1 at bits 16..20.
        assert_eq!((word >> 16) & 0x1F, 7);
        // frame_format 0 = standard, at bits 21..22.
        assert_eq!((word >> 21) & 0b11, 0);
    }

    #[test]
    fn spi_mode_lands_in_the_work_mode_field() {
        let mut spi = K210Spi::new(SPI0, 100_000_000);
        spi.mode = SpiMode::Mode3;
        assert_eq!((spi.control_word(TMOD_TX_RX) >> 6) & 0b11, 3);
    }

    /// SPI3's `ctrlr0` moves three fields, not just `tmod`. The port shipped
    /// with one layout for every controller and worked anyway, because Mode0
    /// and the flash's 8-bit frames both encode as values that are harmless in
    /// the wrong bits. This pins the layouts so that luck is not load-bearing.
    #[test]
    fn spi3_moves_work_mode_and_frame_fields_too() {
        let mut spi = K210Spi::new(SPI3, 100_000_000);
        spi.mode = SpiMode::Mode3;
        let word = spi.control_word(TMOD_TX);
        assert_eq!((word >> 8) & 0b11, 3, "work mode sits at bit 8 on SPI3");
        assert_eq!((word >> 10) & 0b11, TMOD_TX, "tmod at bit 10");
        assert_eq!(word & 0xF, 7, "the legacy 4-bit dfs carries 8-bit frames");
        assert_eq!((word >> 16) & 0x1F, 0, "and dfs_32 is not SPI3's field");
    }

    #[test]
    fn octal_format_and_wide_frames_encode_for_the_panel() {
        let mut spi = K210Spi::new(SPI0, 100_000_000);
        spi.set_frame_format(FrameFormat::Octal);
        spi.set_frame_bits(32).unwrap();
        let word = spi.control_word(TMOD_TX);
        assert_eq!((word >> 21) & 0b11, FrameFormat::Octal as u32);
        assert_eq!((word >> 16) & 0x1F, 31);
    }

    /// A frame has to divide into the lane count, and SPI3's 4-bit size field
    /// cannot reach 32 however it is asked.
    #[test]
    fn frame_widths_are_rejected_when_the_hardware_cannot_hold_them() {
        let mut octal = K210Spi::new(SPI0, 100_000_000);
        octal.set_frame_format(FrameFormat::Octal);
        assert!(octal.set_frame_bits(12).is_err(), "12 is not a whole number of octal beats");
        assert!(octal.set_frame_bits(16).is_ok());

        let mut flash = K210Spi::new(SPI3, 100_000_000);
        assert!(flash.set_frame_bits(32).is_err(), "SPI3 tops out at 16-bit frames");
        assert!(flash.set_frame_bits(8).is_ok());
    }

    /// The divisor must be even, at least 2, and never overflow the field —
    /// asking for a clock faster than the source is the interesting case,
    /// because 0 means "no clock at all" rather than "as fast as possible".
    #[test]
    fn baud_divisor_stays_even_and_within_range() {
        assert_eq!(K210Spi::baud_divisor(100_000_000, 10_000_000), 10);
        // 100 MHz / 30 MHz = 3, rounded down to the even 2.
        assert_eq!(K210Spi::baud_divisor(100_000_000, 30_000_000), 2);
        assert_eq!(K210Spi::baud_divisor(100_000_000, 400_000_000), 2);
        assert_eq!(K210Spi::baud_divisor(100_000_000, 1), 65534);
        assert_eq!(K210Spi::baud_divisor(100_000_000, 0), 65534);
    }
}
