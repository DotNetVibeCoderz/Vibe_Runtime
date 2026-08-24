use rustclr_hal::adc::AdcChannel;
use rustclr_hal::delay::Delay;
use rustclr_hal::i2s::{I2sBus, I2sConfig};
use rustclr_hal::power::{BatteryStatus, PowerManager, SleepMode};
use rustclr_hal::pwm::PwmChannel;
use rustclr_hal::HalResult;
use std::collections::VecDeque;
use std::time::Instant;

pub struct SimPwm {
    pub hz: u32,
    pub duty: u16,
    pub enabled: bool,
}

impl PwmChannel for SimPwm {
    fn set_frequency(&mut self, hz: u32) -> HalResult<()> {
        self.hz = hz;
        Ok(())
    }
    fn set_duty(&mut self, duty: u16) -> HalResult<()> {
        self.duty = duty.min(10_000);
        Ok(())
    }
    fn enable(&mut self) -> HalResult<()> {
        self.enabled = true;
        Ok(())
    }
    fn disable(&mut self) -> HalResult<()> {
        self.enabled = false;
        Ok(())
    }
}

pub struct SimAdc {
    pub raw: u16,
}

impl AdcChannel for SimAdc {
    fn read_raw(&mut self) -> HalResult<u16> {
        Ok(self.raw)
    }
    fn resolution_bits(&self) -> u8 {
        12
    }
}

pub struct SimI2s {
    pub config: I2sConfig,
    pub written: VecDeque<i16>,
    pub to_read: VecDeque<i16>,
}

impl I2sBus for SimI2s {
    fn configure(&mut self, config: I2sConfig) -> HalResult<()> {
        self.config = config;
        Ok(())
    }
    fn write(&mut self, samples: &[i16]) -> HalResult<usize> {
        self.written.extend(samples);
        Ok(samples.len())
    }
    fn read(&mut self, samples: &mut [i16]) -> HalResult<usize> {
        let n = samples.len().min(self.to_read.len());
        for slot in samples.iter_mut().take(n) {
            *slot = self.to_read.pop_front().unwrap();
        }
        Ok(n)
    }
}

pub struct SimPower {
    pub last_sleep: Option<(SleepMode, Option<u64>)>,
    pub battery_mv: u32,
    pub wake_sources: Vec<rustclr_hal::power::WakeSource>,
    pub wake_reason: rustclr_hal::power::WakeReason,
}

impl PowerManager for SimPower {
    fn sleep(&mut self, mode: SleepMode, duration_ms: Option<u64>) -> HalResult<()> {
        self.last_sleep = Some((mode, duration_ms));
        Ok(())
    }
    fn battery(&mut self) -> HalResult<BatteryStatus> {
        Ok(BatteryStatus {
            millivolts: self.battery_mv,
            percent: Some(((self.battery_mv.saturating_sub(3300)) * 100 / 900).min(100) as u8),
            charging: false,
        })
    }
    fn cpu_frequency_hz(&self) -> u32 {
        240_000_000
    }
    fn set_cpu_frequency_hz(&mut self, _hz: u32) -> HalResult<()> {
        Ok(())
    }
    fn reset(&mut self) -> ! {
        panic!("simulated reset");
    }
    fn shutdown(&mut self) -> ! {
        panic!("simulated shutdown");
    }
    fn arm_wake(&mut self, source: rustclr_hal::power::WakeSource) -> HalResult<()> {
        self.wake_sources.push(source);
        Ok(())
    }
    fn clear_wake_sources(&mut self) {
        self.wake_sources.clear();
    }
    fn wake_reason(&self) -> rustclr_hal::power::WakeReason {
        self.wake_reason
    }
}

pub struct SimDelay {
    pub epoch: Instant,
}

impl Delay for SimDelay {
    fn delay_us(&mut self, us: u64) {
        std::thread::sleep(std::time::Duration::from_micros(us));
    }
    fn now_us(&self) -> u64 {
        self.epoch.elapsed().as_micros() as u64
    }
}
