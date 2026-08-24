use crate::HalResult;

pub trait PwmChannel: Send {
    fn set_frequency(&mut self, hz: u32) -> HalResult<()>;
    /// Duty cycle in the range 0..=10000 (hundredths of a percent).
    fn set_duty(&mut self, duty_permyriad: u16) -> HalResult<()>;
    fn enable(&mut self) -> HalResult<()>;
    fn disable(&mut self) -> HalResult<()>;
}
