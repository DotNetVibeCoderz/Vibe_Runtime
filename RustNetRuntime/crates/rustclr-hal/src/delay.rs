pub trait Delay: Send {
    fn delay_us(&mut self, us: u64);
    fn delay_ms(&mut self, ms: u64) {
        self.delay_us(ms * 1000);
    }
    /// Monotonic microseconds since boot.
    fn now_us(&self) -> u64;
}
