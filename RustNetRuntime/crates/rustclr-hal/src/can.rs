
#[allow(unused_imports)]
use alloc::{boxed::Box, string::{String, ToString}, vec::Vec};
use crate::HalResult;

/// A classic CAN 2.0 frame (11-bit or 29-bit identifier, 0..=8 data bytes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanFrame {
    pub id: u32,
    /// 29-bit extended identifier when true, 11-bit standard otherwise.
    pub extended: bool,
    /// Remote transmission request (no data).
    pub rtr: bool,
    pub data: Vec<u8>,
}

impl CanFrame {
    pub fn new(id: u32, data: &[u8]) -> Self {
        CanFrame { id, extended: id > 0x7FF, rtr: false, data: data.to_vec() }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CanConfig {
    pub bitrate: u32,
    /// Frames transmitted are also received locally (self-test mode).
    pub loopback: bool,
    /// Listen-only: no ACKs are generated on the wire.
    pub silent: bool,
}

impl Default for CanConfig {
    fn default() -> Self {
        CanConfig { bitrate: 500_000, loopback: false, silent: false }
    }
}

pub trait CanBus: Send {
    fn configure(&mut self, config: CanConfig) -> HalResult<()>;
    /// Queue a frame for transmission. Non-blocking; the controller FIFO
    /// drains in the background.
    fn transmit(&mut self, frame: &CanFrame) -> HalResult<()>;
    /// Pop the next received frame, if any (non-blocking).
    fn receive(&mut self) -> HalResult<Option<CanFrame>>;
    /// Number of frames waiting in the receive FIFO.
    fn rx_pending(&self) -> usize;
    /// Hardware acceptance filter: a frame is accepted when
    /// `(frame.id & mask) == (id & mask)`. Pass mask 0 to accept all.
    fn set_filter(&mut self, id: u32, mask: u32) -> HalResult<()>;
}
