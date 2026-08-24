use crate::HalResult;

/// Hardware watchdog: once started it must be fed within the timeout or
/// the chip resets. On most chips it cannot be stopped after starting;
/// `stop` returns `NotSupported` there.
pub trait Watchdog: Send {
    fn start(&mut self, timeout_ms: u32) -> HalResult<()>;
    fn feed(&mut self) -> HalResult<()>;
    fn stop(&mut self) -> HalResult<()>;
    fn is_running(&self) -> bool;
    fn timeout_ms(&self) -> u32;
}
