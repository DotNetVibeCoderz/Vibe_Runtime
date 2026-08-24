//! Display drivers on top of the RustNet HAL bus traits.

use alloc::vec::Vec;

use crate::display::{DisplayDriver, Rect};
use crate::fb::Framebuffer;
use rustclr_hal::i2c::I2cBus;
use rustclr_hal::spi::SpiBus;
use rustclr_hal::HalError;

/// SSD1306 128x64 monochrome OLED over I2C. RGB565 content is thresholded
/// by luma into on/off pixels.
pub struct Ssd1306<'b> {
    pub bus: &'b mut dyn I2cBus,
    pub addr: u8,
    pub width: u32,
    pub height: u32,
}

impl<'b> Ssd1306<'b> {
    pub fn new(bus: &'b mut dyn I2cBus, addr: u8) -> Result<Self, HalError> {
        let mut drv = Self { bus, addr, width: 128, height: 64 };
        drv.init()?;
        Ok(drv)
    }

    fn init(&mut self) -> Result<(), HalError> {
        // Standard SSD1306 init sequence (control byte 0x00 = command).
        for cmd in [
            0xAEu8, 0xD5, 0x80, 0xA8, 0x3F, 0xD3, 0x00, 0x40, 0x8D, 0x14, 0x20, 0x00, 0xA1,
            0xC8, 0xDA, 0x12, 0x81, 0xCF, 0xD9, 0xF1, 0xDB, 0x40, 0xA4, 0xA6, 0xAF,
        ] {
            self.bus.write(self.addr, &[0x00, cmd])?;
        }
        Ok(())
    }
}

impl DisplayDriver for Ssd1306<'_> {
    fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn flush(&mut self, fb: &Framebuffer, _region: Rect) -> Result<(), HalError> {
        // Page-addressed full refresh (8 rows per page).
        for page in 0..(self.height / 8) {
            self.bus.write(self.addr, &[0x00, 0xB0 | page as u8, 0x00, 0x10])?;
            let mut data = Vec::with_capacity(self.width as usize + 1);
            data.push(0x40); // control byte: data
            for x in 0..self.width {
                let mut byte = 0u8;
                for bit in 0..8 {
                    let y = page * 8 + bit;
                    if let Some(c) = fb.get_pixel(x as i32, y as i32) {
                        if c.luma() > 127 {
                            byte |= 1 << bit;
                        }
                    }
                }
                data.push(byte);
            }
            self.bus.write(self.addr, &data)?;
        }
        Ok(())
    }
}

/// ST7735 160x128 color TFT over SPI (window update per dirty region).
pub struct St7735<'b> {
    pub bus: &'b mut dyn SpiBus,
    pub width: u32,
    pub height: u32,
}

impl<'b> St7735<'b> {
    pub fn new(bus: &'b mut dyn SpiBus) -> Result<Self, HalError> {
        bus.configure(24_000_000, rustclr_hal::spi::SpiMode::Mode0)?;
        let mut drv = Self { bus, width: 160, height: 128 };
        drv.init()?;
        Ok(drv)
    }

    fn cmd(&mut self, c: u8, data: &[u8]) -> Result<(), HalError> {
        // Command marker 0x01 + opcode, then data marker 0x00 + payload.
        // (DC pin handling is folded into the stream for bus-agnostic tests;
        // chip firmware maps markers onto the DC GPIO.)
        self.bus.write(&[c])?;
        if !data.is_empty() {
            self.bus.write(data)?;
        }
        Ok(())
    }

    fn init(&mut self) -> Result<(), HalError> {
        self.cmd(0x01, &[])?; // SWRESET
        self.cmd(0x11, &[])?; // SLPOUT
        self.cmd(0x3A, &[0x05])?; // COLMOD: 16-bit
        self.cmd(0x29, &[])?; // DISPON
        Ok(())
    }
}

impl DisplayDriver for St7735<'_> {
    fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn flush(&mut self, fb: &Framebuffer, region: Rect) -> Result<(), HalError> {
        let x_end = (region.x + region.w - 1) as u16;
        let y_end = (region.y + region.h - 1) as u16;
        self.cmd(0x2A, &[(region.x >> 8) as u8, region.x as u8, (x_end >> 8) as u8, x_end as u8])?;
        self.cmd(0x2B, &[(region.y >> 8) as u8, region.y as u8, (y_end >> 8) as u8, y_end as u8])?;
        let mut data = Vec::with_capacity((region.w * region.h * 2) as usize);
        for y in region.y..region.y + region.h {
            for x in region.x..region.x + region.w {
                let c = fb.get_pixel(x as i32, y as i32).map(|c| c.0).unwrap_or(0);
                data.extend_from_slice(&c.to_be_bytes());
            }
        }
        self.cmd(0x2C, &data) // RAMWR
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fb::Color;
    use rustclr_hal::Board;
    use rustclr_hal_host::HostBoard;

    #[test]
    fn ssd1306_writes_pages_over_i2c() {
        struct Sink;
        impl rustclr_hal_host::I2cDevice for Sink {
            fn write(&mut self, _data: &[u8]) {}
            fn read(&mut self, _buf: &mut [u8]) {}
        }
        let mut board = HostBoard::new();
        board.attach_i2c(0, 0x3C, Box::new(Sink));
        let bus = board.i2c(0).unwrap();
        let mut fb = Framebuffer::new(128, 64);
        fb.clear(Color::BLACK);
        fb.fill_rect(0, 0, 8, 8, Color::WHITE);
        let mut drv = Ssd1306::new(bus, 0x3C).unwrap();
        drv.flush(&fb, Rect { x: 0, y: 0, w: 128, h: 64 }).unwrap();
    }

    #[test]
    fn st7735_streams_window_pixels_over_spi() {
        let mut board = HostBoard::new();
        {
            let bus = board.spi(0).unwrap();
            let mut fb = Framebuffer::new(160, 128);
            fb.clear(Color::RED);
            let mut drv = St7735::new(bus).unwrap();
            drv.flush(&fb, Rect { x: 0, y: 0, w: 4, h: 2 }).unwrap();
        }
        let log = board.spi_tx_log(0);
        // RAMWR (0x2C) followed by 4*2 RED pixels big-endian.
        let ramwr = log.iter().rposition(|&b| b == 0x2C).unwrap();
        let pixels = &log[ramwr + 1..];
        assert_eq!(pixels.len(), 4 * 2 * 2);
        assert_eq!(&pixels[..2], &Color::RED.0.to_be_bytes());
    }
}
