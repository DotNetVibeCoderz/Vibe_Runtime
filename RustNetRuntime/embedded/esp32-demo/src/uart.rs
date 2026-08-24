//! UART0 as a [`rustclr_hal::uart::Uart`], so an ESP32 board can speak RNDP.
//!
//! # Why UART0 and not one of the spare ones
//!
//! On an M5Stack the USB-serial bridge is wired to UART0's default pins,
//! GPIO1 and GPIO3. That is the cable somebody already has plugged in and the
//! console they are already watching, so it is the one a host tool can reach
//! without asking them to solder anything.
//!
//! # Sharing the port with the console
//!
//! `esp-println` writes the program's console output straight to UART0's
//! transmit FIFO, and this driver writes RNDP replies to the same FIFO. That
//! is deliberate and safe here, for a reason worth stating rather than
//! assuming:
//!
//! * the firmware is **single-threaded**, and a reply is written from inside
//!   the interpreter's poll — a point at which the program is not running, so
//!   it cannot interleave bytes into the middle of a frame;
//! * *between* frames the console writes freely, and the host's decoder is
//!   built to skip whatever is not a frame.
//!
//! What would break this is a second thread, or writing a reply from an
//! interrupt. Neither exists.
//!
//! # The receive FIFO is what makes this work at all
//!
//! The service is polled from the interpreter's instruction loop, and a single
//! IL instruction can be a `Display.Present()` costing tens of milliseconds.
//! Whatever arrives in the meantime has to survive in hardware until the next
//! poll. The ESP32's receive FIFO is **128 bytes**, which holds fourteen of
//! the smallest frame; the K210's is eight, which cannot hold even one, and
//! that is exactly why a graphics program on a Maix Go drops requests and one
//! on an M5Stack does not.

use esp_hal::uart::{Config, RxError, Uart as EspUart};

use rustclr_hal::uart::{Parity, Uart, UartConfig};
use rustclr_hal::{HalError, HalResult};

/// UART0, owned by the board and shared with the console.
pub struct Esp32Uart {
    inner: EspUart<'static, esp_hal::Blocking>,
}

impl Esp32Uart {
    /// Take UART0 on its default pins.
    ///
    /// `None` when the peripheral will not configure — a baud the divider
    /// cannot express, say. Refused rather than approximated, because a UART
    /// running at a baud nobody asked for is a console that prints rubbish.
    pub fn new(
        uart0: esp_hal::peripherals::UART0<'static>,
        tx: esp_hal::peripherals::GPIO1<'static>,
        rx: esp_hal::peripherals::GPIO3<'static>,
    ) -> Option<Self> {
        // 115200 8N1: what the bridge, `esp-println` and every terminal
        // already assume. Changing it here would silently break the console
        // this shares a wire with.
        let config = Config::default().with_baudrate(115_200);
        let inner = EspUart::new(uart0, config).ok()?.with_tx(tx).with_rx(rx);
        Some(Self { inner })
    }
}

impl Uart for Esp32Uart {
    fn configure(&mut self, config: UartConfig) -> HalResult<()> {
        // Deliberately narrow. This port is shared with the console and with
        // whatever terminal is watching it, so a program that reconfigured it
        // would cut off the very output somebody is reading to find out what
        // the program did. Anything that matches what is already set succeeds;
        // anything else is refused with the reason.
        if config.baud != 115_200 || config.data_bits != 8 || config.stop_bits != 1 {
            return Err(HalError::InvalidArgument(
                "UART0 is the shared console at 115200 8N1 and cannot be reconfigured",
            ));
        }
        if config.parity != Parity::None {
            return Err(HalError::InvalidArgument("UART0 is the shared console: no parity"));
        }
        Ok(())
    }

    fn write(&mut self, data: &[u8]) -> HalResult<usize> {
        self.inner.write(data).map_err(|_| HalError::Bus("UART0 transmit failed"))
    }

    fn read(&mut self, buf: &mut [u8]) -> HalResult<usize> {
        // `read_buffered` takes what the FIFO already holds and returns
        // immediately. `read` would block until the buffer filled, which on a
        // poll that runs between two instructions of a running program would
        // stop the board dead waiting for bytes nobody is sending.
        match self.inner.read_buffered(buf) {
            Ok(n) => Ok(n),
            // A framing or parity error means the *line* glitched, not that
            // the port is broken: the bytes in hand are rubbish, and the next
            // poll should try again. Reported as nothing read, which is what
            // the caller does with it anyway, rather than as a bus failure
            // that would take the service down for the rest of the run.
            Err(RxError::FifoOverflowed) | Err(RxError::GlitchOccurred) => Ok(0),
            Err(RxError::FrameFormatViolated) | Err(RxError::ParityMismatch) => Ok(0),
            Err(_) => Ok(0),
        }
    }

    fn bytes_available(&mut self) -> HalResult<usize> {
        // The driver answers "is there anything" rather than "how much", and
        // inventing a count would be worse than the one bit that is true.
        Ok(usize::from(self.inner.read_ready()))
    }

    fn flush(&mut self) -> HalResult<()> {
        self.inner.flush().map_err(|_| HalError::Bus("UART0 flush failed"))
    }
}
