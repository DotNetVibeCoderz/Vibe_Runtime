use crate::HalResult;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SleepMode {
    /// CPU halted, peripherals running; any interrupt wakes.
    Light,
    /// Most peripherals off; RTC/wake pins only.
    Deep,
    /// Everything off except wake sources; resume = reboot.
    Hibernate,
}

#[derive(Debug, Clone, Copy)]
pub struct BatteryStatus {
    pub millivolts: u32,
    /// 0-100, or None when no fuel gauge is present.
    pub percent: Option<u8>,
    pub charging: bool,
}

/// What may bring the chip out of sleep. Multiple sources can be armed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeSource {
    /// Edge on a wake-capable GPIO pin (true = rising).
    Gpio { pin: u32, rising: bool },
    /// RTC alarm after `seconds` from now.
    Rtc { seconds: u64 },
}

/// Why the chip is running right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeReason {
    PowerOn,
    ExternalReset,
    SoftwareReset,
    WatchdogReset,
    RtcAlarm,
    GpioPin(u32),
    Unknown,
}

impl WakeReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            WakeReason::PowerOn => "power-on",
            WakeReason::ExternalReset => "external-reset",
            WakeReason::SoftwareReset => "software-reset",
            WakeReason::WatchdogReset => "watchdog-reset",
            WakeReason::RtcAlarm => "rtc-alarm",
            WakeReason::GpioPin(_) => "gpio",
            WakeReason::Unknown => "unknown",
        }
    }
}

pub trait PowerManager: Send {
    fn sleep(&mut self, mode: SleepMode, duration_ms: Option<u64>) -> HalResult<()>;
    fn battery(&mut self) -> HalResult<BatteryStatus>;
    fn cpu_frequency_hz(&self) -> u32;
    fn set_cpu_frequency_hz(&mut self, hz: u32) -> HalResult<()>;
    fn reset(&mut self) -> !;
    /// Power the device off entirely (wake only via armed sources/reset pin).
    fn shutdown(&mut self) -> !;
    /// Arm a wake source for the next sleep/shutdown. Cumulative until
    /// `clear_wake_sources`.
    fn arm_wake(&mut self, source: WakeSource) -> HalResult<()>;
    fn clear_wake_sources(&mut self);
    fn wake_reason(&self) -> WakeReason;
}
