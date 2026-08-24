
#[allow(unused_imports)]
use alloc::{boxed::Box, string::{String, ToString}, vec::Vec};
use crate::HalResult;

/// TinyCLR-style precise signal control on a GPIO pin: timed edge
/// generation (SignalGenerator), pulse-train capture (SignalCapture) and
/// echo measurement (PulseFeedback, e.g. HC-SR04 ultrasonic).
pub trait SignalControl: Send {
    /// Drive a timed edge train: the pin starts at `initial_high`, then
    /// toggles after each duration in `timings_us`. Blocking; total
    /// duration is the sum of the array.
    fn generate(&mut self, initial_high: bool, timings_us: &[u32]) -> HalResult<()>;

    /// Record up to `max_edges` pulse widths (µs between successive
    /// edges), waiting at most `timeout_us` for the first edge.
    fn capture(&mut self, max_edges: usize, timeout_us: u32) -> HalResult<Vec<u32>>;

    /// Emit a trigger pulse of `pulse_us` (high when `pulse_high`), then
    /// measure the duration of the echo pulse on the same pin in µs
    /// (0 = timeout). Matches TinyCLR's PulseFeedback DurationUntilEcho.
    fn pulse_feedback(&mut self, pulse_high: bool, pulse_us: u32, timeout_us: u32)
        -> HalResult<u32>;
}
