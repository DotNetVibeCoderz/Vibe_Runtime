
#[allow(unused_imports)]
use alloc::{boxed::Box, string::{String, ToString}, vec::Vec};
use crate::HalResult;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinMode {
    Input,
    InputPullUp,
    InputPullDown,
    Output,
    OutputOpenDrain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Low,
    High,
}

impl From<bool> for Level {
    fn from(v: bool) -> Self {
        if v { Level::High } else { Level::Low }
    }
}

impl From<Level> for bool {
    fn from(l: Level) -> bool {
        l == Level::High
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Rising,
    Falling,
    Both,
}

pub trait GpioPin: Send {
    fn set_mode(&mut self, mode: PinMode) -> HalResult<()>;
    fn write(&mut self, level: Level) -> HalResult<()>;
    fn read(&mut self) -> HalResult<Level>;
    fn toggle(&mut self) -> HalResult<()> {
        let level = self.read()?;
        self.write(match level {
            Level::High => Level::Low,
            Level::Low => Level::High,
        })
    }
    /// Register an edge interrupt. The callback runs in interrupt context on
    /// real chips and on the simulator thread on the host.
    fn on_edge(&mut self, edge: Edge, callback: Box<dyn FnMut(Level) + Send>) -> HalResult<()>;
    fn clear_interrupt(&mut self) -> HalResult<()>;
}
