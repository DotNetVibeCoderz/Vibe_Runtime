//! I²C — the K210's three DesignWare masters.
//!
//! | HAL bus | Controller | What it reaches on a Maix Go |
//! |---|---|---|
//! | 0 | I2C0 | the header's `IIC_SCL`/`IIC_SDA` on IO30/IO31 |
//! | 1 | I2C1 | free |
//! | 2 | I2C2 | the camera's control lines on IO40/IO41 |
//!
//! **The camera is on bus 2**, at 100 kHz. Its control channel is SCCB, which
//! looks enough like I²C to be driven by an I²C master, and MaixPy drives it
//! with exactly this one — despite the chip also having a dedicated SCCB
//! master inside the DVP block, and despite the pads being labelled
//! `DVP_SDA`/`DVP_SCL`. [`crate::camera`] has the story.
//!
//! Two details of this controller are worth knowing before reading the code.
//!
//! **The target address is a register, not part of the transfer.** `tar` has to
//! be written before a transaction and the controller has to be *disabled* to
//! change it, so every call here re-arms the peripheral rather than streaming
//! into an already-running one.
//!
//! **The controller must not be disabled while the bus is still busy.** An
//! empty transmit FIFO means the last byte has been *popped*, not that it has
//! been clocked out, so disabling there cuts the transfer off mid-byte and can
//! leave a slave holding SDA low. A bus stuck that way answers *every* address
//! with an acknowledge, which reads as a board covered in devices rather than
//! as a driver bug — this port scanned one and got acks from 0x30 upwards.
//! `status.activity` going quiet is the real end of a transfer.
//!
//! **A NAK is not an error return, it is a status bit.** `tx_abrt_source` latches
//! why a transfer gave up, and reading it is the only way to tell "the device
//! is not there" from "the bus is idle because nothing was sent". Missing that
//! turns an absent device into a silent success.

use rustclr_hal::i2c::I2cBus;
use rustclr_hal::{HalError, HalResult};

use crate::{reg, sysctl};

const CON: usize = 0x00;
const TAR: usize = 0x04;
const DATA_CMD: usize = 0x10;
const SS_SCL_HCNT: usize = 0x14;
const SS_SCL_LCNT: usize = 0x18;
const INTR_MASK: usize = 0x30;
const CLR_INTR: usize = 0x40;
const ENABLE: usize = 0x6C;
const STATUS: usize = 0x70;
const RXFLR: usize = 0x78;
const TX_ABRT_SOURCE: usize = 0x80;

/// `con`: master mode, standard speed, restart enabled, slave off.
const CON_MASTER: u32 = 1 << 0;
const CON_SPEED_STANDARD: u32 = 1 << 1;
const CON_RESTART_EN: u32 = 1 << 5;
const CON_SLAVE_DISABLE: u32 = 1 << 6;

/// `data_cmd`: the byte, plus what to do with it.
const CMD_READ: u32 = 1 << 8;
const CMD_STOP: u32 = 1 << 9;

/// `status`: bus activity, transmit FIFO not full, transmit FIFO empty,
/// receive FIFO not empty.
const STATUS_ACTIVITY: u32 = 1 << 0;
const STATUS_TFNF: u32 = 1 << 1;
const STATUS_TFE: u32 = 1 << 2;
const STATUS_RFNE: u32 = 1 << 3;

/// Iterations to allow a FIFO to move before declaring the bus dead. Bounded
/// for the same reason every wait in this crate is: an unbounded spin on a bus
/// with nothing on it hangs the firmware before its service loop starts, and
/// that reads as a board which will not enumerate.
const SPIN_LIMIT: u32 = 2_000_000;

#[derive(Clone, Copy)]
pub struct I2cDef {
    pub base: usize,
    pub clock: sysctl::Peripheral,
}

pub const I2C0: I2cDef = I2cDef { base: 0x5028_0000, clock: sysctl::Peripheral::I2c0 };
pub const I2C1: I2cDef = I2cDef { base: 0x5029_0000, clock: sysctl::Peripheral::I2c1 };
pub const I2C2: I2cDef = I2cDef { base: 0x502A_0000, clock: sysctl::Peripheral::I2c2 };

/// Masters exposed through `Board::i2c`, in HAL bus order.
pub const BUSES: [I2cDef; 3] = [I2C0, I2C1, I2C2];

pub struct K210I2c {
    def: I2cDef,
    /// Frequency of the clock feeding the controller — APB0 for these.
    source_hz: u32,
    /// Bus clock asked for. 100 kHz until a caller says otherwise, which is
    /// the rate every I²C device is required to tolerate.
    target_hz: u32,
}

impl K210I2c {
    pub const fn new(def: I2cDef, source_hz: u32) -> Self {
        Self { def, source_hz, target_hz: 100_000 }
    }

    pub fn set_source_hz(&mut self, hz: u32) {
        self.source_hz = hz;
    }

    /// Half-period in source clocks, which is what `ss_scl_hcnt`/`ss_scl_lcnt`
    /// each hold.
    ///
    /// Clamped at both ends. Zero would mean "no clock" rather than "as fast as
    /// possible", so a request faster than the source floors at 1; and the
    /// count fields are sixteen bits, so an absurdly slow request saturates
    /// rather than wrapping round to a *fast* bus, which is the failure that
    /// would actually damage something.
    pub fn half_period(source_hz: u32, target_hz: u32) -> u32 {
        (source_hz / target_hz.max(1) / 2).clamp(1, 0xFFFF)
    }

    #[inline]
    fn r(&self, offset: usize) -> u32 {
        reg::read(self.def.base + offset)
    }

    #[inline]
    fn w(&self, offset: usize, value: u32) {
        reg::write(self.def.base + offset, value);
    }

    /// Point the controller at `addr` and enable it. Disabled first because
    /// `tar` is latched, not sampled.
    fn arm(&self, addr: u8) {
        let count = Self::half_period(self.source_hz, self.target_hz);
        self.w(ENABLE, 0);
        self.w(CON, CON_MASTER | CON_SPEED_STANDARD | CON_RESTART_EN | CON_SLAVE_DISABLE);
        self.w(SS_SCL_HCNT, count);
        self.w(SS_SCL_LCNT, count);
        self.w(TAR, addr as u32 & 0x7F);
        self.w(INTR_MASK, 0);
        let _ = self.r(CLR_INTR);
        self.w(ENABLE, 1);
    }

    /// Let the transfer finish on the wire, then disable.
    ///
    /// Bounded like every other wait here, and an abort ends it early: a NAK
    /// stops the transfer, so waiting for a quiet bus after one would always
    /// run out the clock.
    fn finish(&self) -> HalResult<()> {
        for _ in 0..SPIN_LIMIT {
            if self.r(STATUS) & STATUS_ACTIVITY == 0 {
                let done = self.check_abort();
                self.disable();
                return done;
            }
            if self.r(TX_ABRT_SOURCE) != 0 {
                return self.check_abort();
            }
            core::hint::spin_loop();
        }
        self.disable();
        Err(HalError::Timeout)
    }

    fn disable(&self) {
        self.w(ENABLE, 0);
    }

    fn wait_for(&self, bit: u32) -> HalResult<()> {
        for _ in 0..SPIN_LIMIT {
            if self.r(STATUS) & bit != 0 {
                return Ok(());
            }
            self.check_abort()?;
            core::hint::spin_loop();
        }
        Err(HalError::Timeout)
    }

    /// Turn a latched abort into an error.
    ///
    /// The common one by far is `ABRT_7B_ADDR_NOACK` — nothing answered the
    /// address — and without this check a transfer to an empty bus completes
    /// happily and returns data that was never received.
    fn check_abort(&self) -> HalResult<()> {
        if self.r(TX_ABRT_SOURCE) != 0 {
            let _ = self.r(CLR_INTR);
            self.disable();
            return Err(HalError::Bus("I2C transfer aborted (no acknowledge?)"));
        }
        Ok(())
    }
}

impl I2cBus for K210I2c {
    fn set_frequency(&mut self, hz: u32) -> HalResult<()> {
        if hz == 0 {
            return Err(HalError::InvalidArgument("I2C clock must be non-zero"));
        }
        sysctl::clock_enable(self.def.clock);
        self.target_hz = hz;
        Ok(())
    }

    fn write(&mut self, addr: u8, data: &[u8]) -> HalResult<()> {
        sysctl::clock_enable(self.def.clock);
        self.arm(addr);

        // A zero-length write still addresses the device, which is what makes
        // `probe` meaningful: the address phase either gets an acknowledge or
        // latches an abort.
        if data.is_empty() {
            self.w(DATA_CMD, CMD_STOP);
        } else {
            for (i, byte) in data.iter().enumerate() {
                self.wait_for(STATUS_TFNF)?;
                let last = i + 1 == data.len();
                self.w(DATA_CMD, *byte as u32 | if last { CMD_STOP } else { 0 });
            }
        }
        self.wait_for(STATUS_TFE)?;
        self.finish()
    }

    fn read(&mut self, addr: u8, buf: &mut [u8]) -> HalResult<()> {
        if buf.is_empty() {
            return Ok(());
        }
        sysctl::clock_enable(self.def.clock);
        self.arm(addr);

        // Each byte received has to be *asked for* by pushing a read command,
        // so the transmit and receive sides are driven together rather than one
        // after the other.
        let mut asked = 0usize;
        let mut got = 0usize;
        while got < buf.len() {
            if asked < buf.len() && self.r(STATUS) & STATUS_TFNF != 0 {
                let last = asked + 1 == buf.len();
                self.w(DATA_CMD, CMD_READ | if last { CMD_STOP } else { 0 });
                asked += 1;
            }
            if self.r(STATUS) & STATUS_RFNE != 0 {
                buf[got] = (self.r(DATA_CMD) & 0xFF) as u8;
                got += 1;
                continue;
            }
            self.check_abort()?;
            if self.r(RXFLR) == 0 && asked == buf.len() {
                // Nothing pending and nothing left to ask for: give the bus a
                // bounded moment rather than spinning forever.
                for _ in 0..SPIN_LIMIT {
                    if self.r(STATUS) & STATUS_RFNE != 0 {
                        break;
                    }
                    self.check_abort()?;
                    core::hint::spin_loop();
                }
                if self.r(STATUS) & STATUS_RFNE == 0 {
                    self.disable();
                    return Err(HalError::Timeout);
                }
            }
        }
        self.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_three_controllers_sit_where_the_memory_map_says() {
        assert_eq!(I2C0.base, 0x5028_0000);
        assert_eq!(I2C1.base, 0x5029_0000);
        assert_eq!(I2C2.base, 0x502A_0000);
    }

    /// Both count registers hold a *half* period, so the divide is by two —
    /// getting this wrong runs the bus at half or double the requested rate,
    /// which most devices tolerate and a few do not.
    #[test]
    fn the_scl_counts_are_half_periods() {
        assert_eq!(K210I2c::half_period(200_000_000, 100_000), 1000);
        assert_eq!(K210I2c::half_period(200_000_000, 400_000), 250);
    }

    /// Zero would stop the clock entirely, so a rate faster than the source
    /// floors rather than wrapping to "never".
    #[test]
    fn an_impossible_rate_still_clocks() {
        assert_eq!(K210I2c::half_period(100_000, 10_000_000), 1);
    }

    /// A nonsensical request errs *slow*. `set_frequency` rejects zero before
    /// it reaches here, but if it ever did, a crawling bus is harmless and a
    /// wrapped count would silently be a fast one.
    #[test]
    fn a_nonsense_rate_errs_slow_and_saturates() {
        assert_eq!(K210I2c::half_period(200_000_000, 1), 0xFFFF);
        assert_eq!(K210I2c::half_period(100_000, 0), 50_000);
    }
}
