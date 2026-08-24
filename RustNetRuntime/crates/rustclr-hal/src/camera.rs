use crate::HalResult;
use alloc::vec::Vec;

/// Pixel format a camera delivers frames in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    /// 16-bit RGB565, little-endian, 2 bytes per pixel.
    Rgb565,
    /// 8-bit grayscale, 1 byte per pixel.
    Grayscale,
}

#[derive(Debug, Clone, Copy)]
pub struct CameraConfig {
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
}

impl Default for CameraConfig {
    fn default() -> Self {
        Self { width: 160, height: 120, format: PixelFormat::Rgb565 }
    }
}

/// An image sensor. Chip-gated: only boards with a camera interface (e.g.
/// ESP32 DVP/CSI) implement it; elsewhere `configure`/`capture` return
/// [`HalError::NotSupported`](crate::HalError::NotSupported).
pub trait Camera: Send {
    fn configure(&mut self, config: CameraConfig) -> HalResult<()>;
    /// Capture one frame as raw bytes in the configured format
    /// (RGB565 little-endian: `width * height * 2` bytes).
    fn capture(&mut self) -> HalResult<Vec<u8>>;
    fn config(&self) -> CameraConfig;
}
