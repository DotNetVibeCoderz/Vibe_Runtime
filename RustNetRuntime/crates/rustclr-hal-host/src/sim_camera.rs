//! Simulated image sensor: delivers a deterministic SMPTE-style colour-bar
//! test frame so camera capture can be exercised (and pixel-verified)
//! without real optics.

use rustclr_hal::camera::{Camera, CameraConfig, PixelFormat};
use rustclr_hal::{HalError, HalResult};

/// Eight vertical colour bars, left → right (RGB565).
const BARS: [u16; 8] = [
    0xFFFF, // white
    0xFFE0, // yellow
    0x07FF, // cyan
    0x07E0, // green
    0xF81F, // magenta
    0xF800, // red
    0x001F, // blue
    0x0000, // black
];

pub struct SimCamera {
    config: CameraConfig,
}

impl SimCamera {
    pub fn new() -> Self {
        Self { config: CameraConfig::default() }
    }
}

impl Default for SimCamera {
    fn default() -> Self {
        Self::new()
    }
}

impl Camera for SimCamera {
    fn configure(&mut self, config: CameraConfig) -> HalResult<()> {
        if config.width == 0 || config.height == 0 {
            return Err(HalError::InvalidArgument("camera dimensions must be non-zero"));
        }
        self.config = config;
        Ok(())
    }

    fn capture(&mut self) -> HalResult<Vec<u8>> {
        let (w, h) = (self.config.width, self.config.height);
        match self.config.format {
            PixelFormat::Rgb565 => {
                let mut out = Vec::with_capacity((w * h * 2) as usize);
                for _y in 0..h {
                    for x in 0..w {
                        let bar = (x * 8 / w).min(7) as usize;
                        out.extend_from_slice(&BARS[bar].to_le_bytes());
                    }
                }
                Ok(out)
            }
            PixelFormat::Grayscale => {
                // Luma of each bar (601 weights), one byte per pixel.
                let mut out = Vec::with_capacity((w * h) as usize);
                for _y in 0..h {
                    for x in 0..w {
                        let bar = (x * 8 / w).min(7) as usize;
                        let c = BARS[bar];
                        let r = (((c >> 11) & 0x1F) as u32) << 3;
                        let g = (((c >> 5) & 0x3F) as u32) << 2;
                        let b = ((c & 0x1F) as u32) << 3;
                        out.push(((r * 299 + g * 587 + b * 114) / 1000) as u8);
                    }
                }
                Ok(out)
            }
        }
    }

    fn config(&self) -> CameraConfig {
        self.config
    }
}
