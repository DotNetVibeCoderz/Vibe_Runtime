use crate::HalResult;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum I2sFormat {
    Standard,
    LeftJustified,
    RightJustified,
}

#[derive(Debug, Clone, Copy)]
pub struct I2sConfig {
    pub sample_rate: u32,
    pub bits_per_sample: u8,
    pub channels: u8,
    pub format: I2sFormat,
}

impl Default for I2sConfig {
    fn default() -> Self {
        Self { sample_rate: 44_100, bits_per_sample: 16, channels: 2, format: I2sFormat::Standard }
    }
}

pub trait I2sBus: Send {
    fn configure(&mut self, config: I2sConfig) -> HalResult<()>;
    /// Write PCM frames; blocks until the DMA buffer accepts all samples.
    fn write(&mut self, samples: &[i16]) -> HalResult<usize>;
    /// Read PCM frames (microphone input).
    fn read(&mut self, samples: &mut [i16]) -> HalResult<usize>;
}
