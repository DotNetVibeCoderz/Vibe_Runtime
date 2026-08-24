//! Simulators for the v0.3 HAL surface: CAN, 1-Wire, RTC, watchdog,
//! external memory, network interfaces and precise signal control.

use rustclr_hal::can::{CanBus, CanConfig, CanFrame};
use rustclr_hal::extmem::{ExtMemKind, ExtMemory};
use rustclr_hal::netif::{NetIfConfig, NetIfKind, NetIfStatus, NetInterface};
use rustclr_hal::onewire::{crc8, OneWireBus};
use rustclr_hal::rtc::{DateTime, Rtc};
use rustclr_hal::watchdog::Watchdog;
use rustclr_hal::{signal::SignalControl, HalError, HalResult};
use std::collections::VecDeque;
use std::time::Instant;

// ---------------------------------------------------------------- CAN --

pub struct SimCan {
    pub config: CanConfig,
    /// Everything transmitted, for test/panel inspection.
    pub tx_log: Vec<CanFrame>,
    pub rx: VecDeque<CanFrame>,
    filter: Option<(u32, u32)>,
}

impl SimCan {
    pub fn new() -> Self {
        SimCan { config: CanConfig::default(), tx_log: Vec::new(), rx: VecDeque::new(), filter: None }
    }

    fn accepts(&self, id: u32) -> bool {
        match self.filter {
            Some((fid, mask)) => (id & mask) == (fid & mask),
            None => true,
        }
    }

    /// Simulate a frame arriving from the bus.
    pub fn inject(&mut self, frame: CanFrame) {
        if self.accepts(frame.id) {
            self.rx.push_back(frame);
        }
    }
}

impl CanBus for SimCan {
    fn configure(&mut self, config: CanConfig) -> HalResult<()> {
        self.config = config;
        Ok(())
    }
    fn transmit(&mut self, frame: &CanFrame) -> HalResult<()> {
        if frame.data.len() > 8 {
            return Err(HalError::InvalidArgument("CAN data > 8 bytes"));
        }
        self.tx_log.push(frame.clone());
        if self.config.loopback && self.accepts(frame.id) {
            self.rx.push_back(frame.clone());
        }
        Ok(())
    }
    fn receive(&mut self) -> HalResult<Option<CanFrame>> {
        Ok(self.rx.pop_front())
    }
    fn rx_pending(&self) -> usize {
        self.rx.len()
    }
    fn set_filter(&mut self, id: u32, mask: u32) -> HalResult<()> {
        self.filter = if mask == 0 { None } else { Some((id, mask)) };
        Ok(())
    }
}

// ------------------------------------------------------------- 1-Wire --

/// A slave attached to the simulated bus. `command` receives every byte
/// written after this device was selected; `read_byte` produces response
/// bytes.
pub trait OneWireDevice: Send {
    fn rom(&self) -> u64;
    fn command(&mut self, byte: u8);
    fn read_byte(&mut self) -> u8;
}

/// DS18B20 temperature sensor good enough for driver development: responds
/// to CONVERT T (0x44) and READ SCRATCHPAD (0xBE) with a CRC8-valid
/// scratchpad.
pub struct SimDs18b20 {
    rom: u64,
    pub centi_c: i32,
    reading: VecDeque<u8>,
}

impl SimDs18b20 {
    pub fn new(rom: u64, centi_c: i32) -> Self {
        SimDs18b20 { rom, centi_c, reading: VecDeque::new() }
    }
}

impl OneWireDevice for SimDs18b20 {
    fn rom(&self) -> u64 {
        self.rom
    }
    fn command(&mut self, byte: u8) {
        match byte {
            0x44 => {} // convert: instantaneous in the sim
            0xBE => {
                // raw = sixteenths of a degree
                let raw = (self.centi_c * 16 / 100) as i16;
                let mut sp = vec![
                    (raw & 0xFF) as u8,
                    ((raw >> 8) & 0xFF) as u8,
                    0x4B, 0x46, 0x7F, 0xFF, 0x0C, 0x10,
                ];
                sp.push(crc8(&sp));
                self.reading = sp.into();
            }
            _ => {}
        }
    }
    fn read_byte(&mut self) -> u8 {
        self.reading.pop_front().unwrap_or(0xFF)
    }
}

#[derive(Default)]
enum OwTarget {
    #[default]
    None,
    All,
    Rom(u64),
}

pub struct SimOneWire {
    pub devices: Vec<Box<dyn OneWireDevice>>,
    target: OwTarget,
    /// Bytes after reset: first byte selects the addressing command.
    awaiting_rom: Vec<u8>,
}

impl SimOneWire {
    pub fn new() -> Self {
        SimOneWire { devices: Vec::new(), target: OwTarget::None, awaiting_rom: Vec::new() }
    }

    fn route(&mut self, byte: u8) {
        match self.target {
            OwTarget::All => {
                for d in &mut self.devices {
                    d.command(byte);
                }
            }
            OwTarget::Rom(rom) => {
                if let Some(d) = self.devices.iter_mut().find(|d| d.rom() == rom) {
                    d.command(byte);
                }
            }
            OwTarget::None => {}
        }
    }
}

impl OneWireBus for SimOneWire {
    fn reset(&mut self) -> HalResult<bool> {
        self.target = OwTarget::None;
        self.awaiting_rom.clear();
        Ok(!self.devices.is_empty())
    }
    fn write_byte(&mut self, byte: u8) -> HalResult<()> {
        match self.target {
            OwTarget::None if self.awaiting_rom.is_empty() => match byte {
                0xCC => self.target = OwTarget::All, // SKIP ROM
                0x55 => self.awaiting_rom.push(byte), // MATCH ROM, 8 ROM bytes follow
                _ => {}
            },
            OwTarget::None => {
                self.awaiting_rom.push(byte);
                if self.awaiting_rom.len() == 9 {
                    let mut rom = 0u64;
                    for (i, b) in self.awaiting_rom[1..].iter().enumerate() {
                        rom |= (*b as u64) << (8 * i);
                    }
                    self.target = OwTarget::Rom(rom);
                    self.awaiting_rom.clear();
                }
            }
            _ => self.route(byte),
        }
        Ok(())
    }
    fn read_byte(&mut self) -> HalResult<u8> {
        Ok(match self.target {
            OwTarget::Rom(rom) => self
                .devices
                .iter_mut()
                .find(|d| d.rom() == rom)
                .map(|d| d.read_byte())
                .unwrap_or(0xFF),
            OwTarget::All => self.devices.first_mut().map(|d| d.read_byte()).unwrap_or(0xFF),
            OwTarget::None => 0xFF,
        })
    }
    fn search(&mut self) -> HalResult<Vec<u64>> {
        let mut roms: Vec<u64> = self.devices.iter().map(|d| d.rom()).collect();
        roms.sort_unstable();
        Ok(roms)
    }
    fn select(&mut self, rom: u64) -> HalResult<()> {
        self.target = OwTarget::Rom(rom);
        Ok(())
    }
    fn skip(&mut self) -> HalResult<()> {
        self.target = OwTarget::All;
        Ok(())
    }
}

// ---------------------------------------------------------------- RTC --

pub struct SimRtc {
    /// Simulated wall-clock: epoch seconds at `base` instant.
    pub epoch_at_base: u64,
    base: Instant,
    alarm: Option<u64>,
}

impl SimRtc {
    pub fn new() -> Self {
        // 2026-01-01T00:00:00Z default until the app sets it.
        SimRtc { epoch_at_base: 1_767_225_600, base: Instant::now(), alarm: None }
    }
}

impl Rtc for SimRtc {
    fn now(&mut self) -> HalResult<DateTime> {
        let epoch = self.epoch_at_base + self.base.elapsed().as_secs();
        Ok(DateTime::from_epoch(epoch))
    }
    fn set(&mut self, dt: DateTime) -> HalResult<()> {
        self.epoch_at_base = dt.to_epoch();
        self.base = Instant::now();
        Ok(())
    }
    fn set_alarm(&mut self, epoch: u64) -> HalResult<()> {
        self.alarm = Some(epoch);
        Ok(())
    }
    fn clear_alarm(&mut self) -> HalResult<()> {
        self.alarm = None;
        Ok(())
    }
    fn alarm(&self) -> Option<u64> {
        self.alarm
    }
}

// ----------------------------------------------------------- watchdog --

pub struct SimWatchdog {
    running: bool,
    timeout_ms: u32,
    pub last_feed: Option<Instant>,
    pub feed_count: u32,
}

impl SimWatchdog {
    pub fn new() -> Self {
        SimWatchdog { running: false, timeout_ms: 0, last_feed: None, feed_count: 0 }
    }

    /// Would the hardware watchdog have bitten by now?
    pub fn expired(&self) -> bool {
        self.running
            && self
                .last_feed
                .map(|t| t.elapsed().as_millis() as u32 > self.timeout_ms)
                .unwrap_or(false)
    }
}

impl Watchdog for SimWatchdog {
    fn start(&mut self, timeout_ms: u32) -> HalResult<()> {
        self.running = true;
        self.timeout_ms = timeout_ms;
        self.last_feed = Some(Instant::now());
        Ok(())
    }
    fn feed(&mut self) -> HalResult<()> {
        if !self.running {
            return Err(HalError::InvalidArgument("watchdog not started"));
        }
        self.last_feed = Some(Instant::now());
        self.feed_count += 1;
        Ok(())
    }
    fn stop(&mut self) -> HalResult<()> {
        self.running = false;
        Ok(())
    }
    fn is_running(&self) -> bool {
        self.running
    }
    fn timeout_ms(&self) -> u32 {
        self.timeout_ms
    }
}

// ----------------------------------------------------- external memory --

pub struct SimExtMem {
    kind: ExtMemKind,
    pub data: Vec<u8>,
    sector: u32,
}

impl SimExtMem {
    pub fn qspi_flash(size: u32) -> Self {
        SimExtMem { kind: ExtMemKind::QspiFlash, data: vec![0xFF; size as usize], sector: 4096 }
    }
    pub fn sdram(size: u32) -> Self {
        SimExtMem { kind: ExtMemKind::Sdram, data: vec![0; size as usize], sector: 1 }
    }

    fn check(&self, addr: u32, len: usize) -> HalResult<()> {
        if addr as usize + len > self.data.len() {
            return Err(HalError::InvalidArgument("address out of range"));
        }
        Ok(())
    }
}

impl ExtMemory for SimExtMem {
    fn kind(&self) -> ExtMemKind {
        self.kind
    }
    fn size(&self) -> u32 {
        self.data.len() as u32
    }
    fn read(&mut self, addr: u32, buf: &mut [u8]) -> HalResult<()> {
        self.check(addr, buf.len())?;
        buf.copy_from_slice(&self.data[addr as usize..addr as usize + buf.len()]);
        Ok(())
    }
    fn write(&mut self, addr: u32, data: &[u8]) -> HalResult<()> {
        self.check(addr, data.len())?;
        let base = addr as usize;
        match self.kind {
            ExtMemKind::Sdram => self.data[base..base + data.len()].copy_from_slice(data),
            ExtMemKind::QspiFlash => {
                // NOR semantics: bits only clear.
                for (i, b) in data.iter().enumerate() {
                    self.data[base + i] &= *b;
                }
            }
        }
        Ok(())
    }
    fn erase(&mut self, addr: u32, len: u32) -> HalResult<()> {
        if self.kind == ExtMemKind::Sdram {
            return Err(HalError::NotSupported);
        }
        let start = addr / self.sector * self.sector;
        let end = ((addr + len).div_ceil(self.sector) * self.sector).min(self.size());
        self.check(start, (end - start) as usize)?;
        self.data[start as usize..end as usize].fill(0xFF);
        Ok(())
    }
    fn sector_size(&self) -> u32 {
        self.sector
    }
}

// -------------------------------------------------- network interfaces --

pub struct SimNetIf {
    kind: NetIfKind,
    pub status: NetIfStatus,
}

impl SimNetIf {
    pub fn kind_is(&self, kind: NetIfKind) -> bool {
        self.kind == kind
    }

    pub fn new(kind: NetIfKind) -> Self {
        let mac = match kind {
            NetIfKind::Wifi => "02:AA:BB:00:00:01",
            NetIfKind::Ethernet => "02:AA:BB:00:00:02",
            _ => "",
        };
        SimNetIf {
            kind,
            status: NetIfStatus { mac: mac.into(), ..Default::default() },
        }
    }
}

impl NetInterface for SimNetIf {
    fn kind(&self) -> NetIfKind {
        self.kind
    }
    fn bring_up(&mut self, config: &NetIfConfig) -> HalResult<()> {
        if self.kind == NetIfKind::Cellular && config.apn.is_empty() {
            return Err(HalError::InvalidArgument("cellular needs an APN"));
        }
        self.status.up = true;
        self.status.ip = if !config.static_ip.is_empty() {
            config.static_ip.clone()
        } else {
            match self.kind {
                NetIfKind::Wifi => "192.168.1.40".into(),
                NetIfKind::Ethernet => "192.168.1.50".into(),
                NetIfKind::Ppp => "10.64.0.2".into(),
                NetIfKind::Cellular => "100.66.0.2".into(),
            }
        };
        self.status.gateway = if !config.gateway.is_empty() {
            config.gateway.clone()
        } else {
            "192.168.1.1".into()
        };
        if self.kind == NetIfKind::Cellular {
            self.status.rssi_dbm = -67;
            self.status.operator_name = "RustNet-Cell".into();
        }
        if self.kind == NetIfKind::Wifi {
            self.status.rssi_dbm = -55;
            // Echo back the network we were told to join, so the simulated
            // interface reports the same SSID a real one would.
            self.status.ssid = config.ssid.clone();
        }
        Ok(())
    }
    fn bring_down(&mut self) -> HalResult<()> {
        self.status.up = false;
        self.status.ip.clear();
        self.status.ssid.clear();
        Ok(())
    }
    fn status(&mut self) -> HalResult<NetIfStatus> {
        Ok(self.status.clone())
    }
}

// ------------------------------------------------------ signal control --

pub struct SimSignal {
    /// Timings generated by the app, for inspection.
    pub generated: Vec<(bool, Vec<u32>)>,
    /// Edge widths handed to the next `capture` call.
    pub capture_queue: VecDeque<Vec<u32>>,
    /// Echo width (µs) returned by the next `pulse_feedback` call.
    pub echo_us: u32,
}

impl SimSignal {
    pub fn new() -> Self {
        SimSignal { generated: Vec::new(), capture_queue: VecDeque::new(), echo_us: 580 }
    }
}

impl SignalControl for SimSignal {
    fn generate(&mut self, initial_high: bool, timings_us: &[u32]) -> HalResult<()> {
        self.generated.push((initial_high, timings_us.to_vec()));
        Ok(())
    }
    fn capture(&mut self, max_edges: usize, _timeout_us: u32) -> HalResult<Vec<u32>> {
        let mut widths = self.capture_queue.pop_front().unwrap_or_default();
        widths.truncate(max_edges);
        Ok(widths)
    }
    fn pulse_feedback(
        &mut self,
        _pulse_high: bool,
        _pulse_us: u32,
        _timeout_us: u32,
    ) -> HalResult<u32> {
        Ok(self.echo_us)
    }
}
