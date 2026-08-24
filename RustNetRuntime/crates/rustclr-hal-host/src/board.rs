use crate::sim_bus::{I2cDevice, SimI2cBus, SimSpiBus, SpiDevice};
use crate::sim_ext::{
    OneWireDevice, SimCan, SimExtMem, SimNetIf, SimOneWire, SimRtc, SimSignal, SimWatchdog,
};
use crate::sim_gpio::SimGpioPin;
use crate::sim_misc::{SimAdc, SimDelay, SimI2s, SimPower, SimPwm};
use crate::sim_uart::SimUart;
use rustclr_hal::gpio::Level;
use rustclr_hal::i2s::I2sConfig;
use rustclr_hal::netif::NetIfKind;
use rustclr_hal::power::SleepMode;
use rustclr_hal::{Board, HalError, HalResult};
use std::collections::VecDeque;
use std::time::Instant;

const PIN_COUNT: u32 = 48;
const BUS_COUNT: u8 = 2;
const UART_COUNT: u8 = 3;
const CHANNEL_COUNT: u8 = 8;

/// Fully in-memory board used by tests, the host firmware ("virtual MCU")
/// and driver development.
pub struct HostBoard {
    pins: Vec<SimGpioPin>,
    i2c: Vec<SimI2cBus>,
    spi: Vec<SimSpiBus>,
    uarts: Vec<SimUart>,
    i2s: Vec<SimI2s>,
    pwm: Vec<SimPwm>,
    adc: Vec<SimAdc>,
    power: SimPower,
    delay: SimDelay,
    can: Vec<SimCan>,
    onewire: Vec<SimOneWire>,
    rtc: SimRtc,
    watchdog: SimWatchdog,
    extmem: Vec<SimExtMem>,
    netifs: Vec<SimNetIf>,
    signals: Vec<SimSignal>,
    camera: crate::SimCamera,
}

impl Default for HostBoard {
    fn default() -> Self {
        Self::new()
    }
}

impl HostBoard {
    pub fn new() -> Self {
        Self {
            pins: (0..PIN_COUNT).map(|_| SimGpioPin::new()).collect(),
            i2c: (0..BUS_COUNT).map(|_| SimI2cBus::new()).collect(),
            spi: (0..BUS_COUNT).map(|_| SimSpiBus::new()).collect(),
            uarts: (0..UART_COUNT).map(|i| SimUart::new(i == 0)).collect(),
            i2s: (0..BUS_COUNT)
                .map(|_| SimI2s { config: I2sConfig::default(), written: VecDeque::new(), to_read: VecDeque::new() })
                .collect(),
            pwm: (0..CHANNEL_COUNT).map(|_| SimPwm { hz: 0, duty: 0, enabled: false }).collect(),
            adc: (0..CHANNEL_COUNT).map(|_| SimAdc { raw: 0 }).collect(),
            power: SimPower {
                last_sleep: None,
                battery_mv: 4100,
                wake_sources: Vec::new(),
                wake_reason: rustclr_hal::power::WakeReason::PowerOn,
            },
            delay: SimDelay { epoch: Instant::now() },
            can: (0..BUS_COUNT).map(|_| SimCan::new()).collect(),
            onewire: (0..BUS_COUNT).map(|_| SimOneWire::new()).collect(),
            rtc: SimRtc::new(),
            watchdog: SimWatchdog::new(),
            // Slot 0: 2 MiB QSPI flash; slot 1: 1 MiB SDRAM.
            extmem: vec![SimExtMem::qspi_flash(2 * 1024 * 1024), SimExtMem::sdram(1024 * 1024)],
            netifs: vec![
                SimNetIf::new(NetIfKind::Wifi),
                SimNetIf::new(NetIfKind::Ethernet),
                SimNetIf::new(NetIfKind::Ppp),
                SimNetIf::new(NetIfKind::Cellular),
            ],
            signals: (0..PIN_COUNT).map(|_| SimSignal::new()).collect(),
            camera: crate::SimCamera::new(),
        }
    }

    // ---- simulator control surface (not part of the HAL) ----

    /// Drive an input pin as if external hardware changed its level.
    pub fn drive_pin(&mut self, pin: u32, level: Level) {
        if let Some(p) = self.pins.get(pin as usize) {
            p.drive(level);
        }
    }

    pub fn attach_i2c(&mut self, bus: u8, addr: u8, device: Box<dyn I2cDevice>) {
        if let Some(b) = self.i2c.get_mut(bus as usize) {
            b.devices.insert(addr, device);
        }
    }

    pub fn attach_spi(&mut self, bus: u8, device: Box<dyn SpiDevice>) {
        if let Some(b) = self.spi.get_mut(bus as usize) {
            b.device = Some(device);
        }
    }

    pub fn set_adc_raw(&mut self, channel: u8, raw: u16) {
        if let Some(a) = self.adc.get_mut(channel as usize) {
            a.raw = raw;
        }
    }

    pub fn set_battery_millivolts(&mut self, mv: u32) {
        self.power.battery_mv = mv;
    }

    pub fn last_sleep(&self) -> Option<(SleepMode, Option<u64>)> {
        self.power.last_sleep
    }

    /// Bytes written by firmware to a non-loopback UART port.
    pub fn uart_take_tx(&mut self, port: u8) -> Vec<u8> {
        self.uarts.get_mut(port as usize).map(|u| u.tx.drain(..).collect()).unwrap_or_default()
    }

    /// Feed bytes into a UART's receive queue.
    pub fn uart_inject_rx(&mut self, port: u8, data: &[u8]) {
        if let Some(u) = self.uarts.get_mut(port as usize) {
            u.rx.extend(data);
        }
    }

    /// SPI bytes shifted out so far (for display driver tests).
    pub fn spi_tx_log(&self, bus: u8) -> &[u8] {
        self.spi.get(bus as usize).map(|b| b.tx_log.as_slice()).unwrap_or(&[])
    }

    /// Simulate a CAN frame arriving from the wire.
    pub fn can_inject(&mut self, bus: u8, frame: rustclr_hal::can::CanFrame) {
        if let Some(b) = self.can.get_mut(bus as usize) {
            b.inject(frame);
        }
    }

    /// Frames the app transmitted on a CAN bus.
    pub fn can_tx_log(&self, bus: u8) -> &[rustclr_hal::can::CanFrame] {
        self.can.get(bus as usize).map(|b| b.tx_log.as_slice()).unwrap_or(&[])
    }

    /// Attach a 1-Wire slave (e.g. `SimDs18b20`).
    pub fn attach_onewire(&mut self, bus: u8, device: Box<dyn OneWireDevice>) {
        if let Some(b) = self.onewire.get_mut(bus as usize) {
            b.devices.push(device);
        }
    }

    /// Queue edge widths for the next SignalCapture on a pin.
    pub fn signal_inject_capture(&mut self, pin: u32, widths: Vec<u32>) {
        if let Some(s) = self.signals.get_mut(pin as usize) {
            s.capture_queue.push_back(widths);
        }
    }

    /// Set the echo width returned by PulseFeedback on a pin.
    pub fn signal_set_echo(&mut self, pin: u32, us: u32) {
        if let Some(s) = self.signals.get_mut(pin as usize) {
            s.echo_us = us;
        }
    }

    /// Simulator-side view of the watchdog (expired, feeds).
    pub fn watchdog_state(&self) -> &SimWatchdog {
        &self.watchdog
    }
}

impl Board for HostBoard {
    fn name(&self) -> &str {
        "rustnet-host-sim"
    }

    fn gpio(&mut self, pin: u32) -> HalResult<&mut dyn rustclr_hal::gpio::GpioPin> {
        self.pins
            .get_mut(pin as usize)
            .map(|p| p as &mut dyn rustclr_hal::gpio::GpioPin)
            .ok_or(HalError::InvalidArgument("pin out of range"))
    }

    fn i2c(&mut self, bus: u8) -> HalResult<&mut dyn rustclr_hal::i2c::I2cBus> {
        self.i2c
            .get_mut(bus as usize)
            .map(|b| b as &mut dyn rustclr_hal::i2c::I2cBus)
            .ok_or(HalError::InvalidArgument("i2c bus out of range"))
    }

    fn spi(&mut self, bus: u8) -> HalResult<&mut dyn rustclr_hal::spi::SpiBus> {
        self.spi
            .get_mut(bus as usize)
            .map(|b| b as &mut dyn rustclr_hal::spi::SpiBus)
            .ok_or(HalError::InvalidArgument("spi bus out of range"))
    }

    fn uart(&mut self, port: u8) -> HalResult<&mut dyn rustclr_hal::uart::Uart> {
        self.uarts
            .get_mut(port as usize)
            .map(|u| u as &mut dyn rustclr_hal::uart::Uart)
            .ok_or(HalError::InvalidArgument("uart port out of range"))
    }

    fn i2s(&mut self, port: u8) -> HalResult<&mut dyn rustclr_hal::i2s::I2sBus> {
        self.i2s
            .get_mut(port as usize)
            .map(|b| b as &mut dyn rustclr_hal::i2s::I2sBus)
            .ok_or(HalError::InvalidArgument("i2s port out of range"))
    }

    fn pwm(&mut self, channel: u8) -> HalResult<&mut dyn rustclr_hal::pwm::PwmChannel> {
        self.pwm
            .get_mut(channel as usize)
            .map(|c| c as &mut dyn rustclr_hal::pwm::PwmChannel)
            .ok_or(HalError::InvalidArgument("pwm channel out of range"))
    }

    fn adc(&mut self, channel: u8) -> HalResult<&mut dyn rustclr_hal::adc::AdcChannel> {
        self.adc
            .get_mut(channel as usize)
            .map(|c| c as &mut dyn rustclr_hal::adc::AdcChannel)
            .ok_or(HalError::InvalidArgument("adc channel out of range"))
    }

    fn power(&mut self) -> &mut dyn rustclr_hal::power::PowerManager {
        &mut self.power
    }

    fn delay(&mut self) -> &mut dyn rustclr_hal::delay::Delay {
        &mut self.delay
    }

    fn can(&mut self, bus: u8) -> HalResult<&mut dyn rustclr_hal::can::CanBus> {
        self.can
            .get_mut(bus as usize)
            .map(|b| b as &mut dyn rustclr_hal::can::CanBus)
            .ok_or(HalError::InvalidArgument("can bus out of range"))
    }

    fn onewire(&mut self, bus: u8) -> HalResult<&mut dyn rustclr_hal::onewire::OneWireBus> {
        self.onewire
            .get_mut(bus as usize)
            .map(|b| b as &mut dyn rustclr_hal::onewire::OneWireBus)
            .ok_or(HalError::InvalidArgument("1-wire bus out of range"))
    }

    fn rtc(&mut self) -> &mut dyn rustclr_hal::rtc::Rtc {
        &mut self.rtc
    }

    fn watchdog(&mut self) -> &mut dyn rustclr_hal::watchdog::Watchdog {
        &mut self.watchdog
    }

    fn extmem(&mut self, index: u8) -> HalResult<&mut dyn rustclr_hal::extmem::ExtMemory> {
        self.extmem
            .get_mut(index as usize)
            .map(|m| m as &mut dyn rustclr_hal::extmem::ExtMemory)
            .ok_or(HalError::InvalidArgument("extmem index out of range"))
    }

    fn netif(&mut self, kind: NetIfKind) -> HalResult<&mut dyn rustclr_hal::netif::NetInterface> {
        self.netifs
            .iter_mut()
            .find(|n| n.kind_is(kind))
            .map(|n| n as &mut dyn rustclr_hal::netif::NetInterface)
            .ok_or(HalError::NotSupported)
    }

    fn signal(&mut self, pin: u32) -> HalResult<&mut dyn rustclr_hal::signal::SignalControl> {
        self.signals
            .get_mut(pin as usize)
            .map(|s| s as &mut dyn rustclr_hal::signal::SignalControl)
            .ok_or(HalError::InvalidArgument("pin out of range"))
    }

    /// The colour-bar test sensor.
    ///
    /// A synthetic frame rather than no camera at all, so a capture pipeline
    /// can be written and pixel-checked on a desktop. It is deliberately a
    /// pattern nobody would mistake for optics.
    fn camera(&mut self) -> HalResult<&mut dyn rustclr_hal::camera::Camera> {
        Ok(&mut self.camera)
    }
}
