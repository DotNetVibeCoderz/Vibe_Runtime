//! Host simulator for the RustNet HAL.
//!
//! Implements every HAL trait against in-memory state so the runtime,
//! drivers and firmware services can be developed and tested on a desktop
//! without hardware. I2C/SPI devices can be attached to buses to emulate
//! sensors and displays; GPIO pins can be externally driven to test edge
//! interrupts.

mod board;
mod sim_gpio;
mod sim_bus;
mod sim_uart;
mod sim_misc;
mod sim_ext;
mod sim_camera;

pub use board::HostBoard;
pub use sim_bus::{I2cDevice, SpiDevice};
pub use sim_camera::SimCamera;
pub use sim_ext::{OneWireDevice, SimDs18b20, SimExtMem};
pub use sim_gpio::SimGpioPin;

#[cfg(test)]
mod tests {
    use super::*;
    use rustclr_hal::gpio::{Level, PinMode};
    use rustclr_hal::Board;

    #[test]
    fn gpio_write_read_roundtrip() {
        let mut board = HostBoard::new();
        let pin = board.gpio(2).unwrap();
        pin.set_mode(PinMode::Output).unwrap();
        pin.write(Level::High).unwrap();
        assert_eq!(pin.read().unwrap(), Level::High);
        pin.toggle().unwrap();
        assert_eq!(pin.read().unwrap(), Level::Low);
    }

    #[test]
    fn gpio_edge_interrupt_fires() {
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::Arc;
        let mut board = HostBoard::new();
        let hits = Arc::new(AtomicU32::new(0));
        let hits2 = hits.clone();
        let pin = board.gpio(4).unwrap();
        pin.set_mode(PinMode::Input).unwrap();
        pin.on_edge(rustclr_hal::gpio::Edge::Rising, Box::new(move |_| {
            hits2.fetch_add(1, Ordering::SeqCst);
        }))
        .unwrap();
        board.drive_pin(4, Level::High);
        board.drive_pin(4, Level::Low);
        board.drive_pin(4, Level::High);
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn camera_colour_bars() {
        use rustclr_hal::camera::{Camera, CameraConfig, PixelFormat};
        let mut cam = SimCamera::new();
        cam.configure(CameraConfig { width: 16, height: 4, format: PixelFormat::Rgb565 })
            .unwrap();
        let f = cam.capture().unwrap();
        assert_eq!(f.len(), 16 * 4 * 2);
        // First bar is white, last bar is black.
        assert_eq!(u16::from_le_bytes([f[0], f[1]]), 0xFFFF);
        let last = (16 * 4 - 1) * 2;
        assert_eq!(u16::from_le_bytes([f[last], f[last + 1]]), 0x0000);
        assert_eq!(cam.config().width, 16);
        // Grayscale frame is one byte per pixel; white bar -> ~0xFF.
        cam.configure(CameraConfig { width: 8, height: 1, format: PixelFormat::Grayscale })
            .unwrap();
        let g = cam.capture().unwrap();
        assert_eq!(g.len(), 8);
        assert!(g[0] >= 248, "white luma should be near max, got {}", g[0]);
        assert_eq!(g[7], 0, "black luma should be 0");
        // Zero dimensions are rejected.
        assert!(cam
            .configure(CameraConfig { width: 0, height: 4, format: PixelFormat::Rgb565 })
            .is_err());
    }

    #[test]
    fn i2s_audio_write_accumulates() {
        use rustclr_hal::i2s::{I2sConfig, I2sFormat};
        let mut board = HostBoard::new();
        let dev = board.i2s(0).unwrap();
        dev.configure(I2sConfig {
            sample_rate: 44_100,
            bits_per_sample: 16,
            channels: 1,
            format: I2sFormat::Standard,
        })
        .unwrap();
        assert_eq!(dev.write(&[1, 2, 3, 4]).unwrap(), 4);
        assert_eq!(dev.write(&[5, 6]).unwrap(), 2);
    }

    #[test]
    fn i2c_device_emulation() {
        struct Thermometer;
        impl I2cDevice for Thermometer {
            fn write(&mut self, _data: &[u8]) {}
            fn read(&mut self, buf: &mut [u8]) {
                // 25.5 C encoded as centi-degrees big-endian
                let v = 2550u16.to_be_bytes();
                let n = 2.min(buf.len());
                buf[..n].copy_from_slice(&v[..n]);
            }
        }
        let mut board = HostBoard::new();
        board.attach_i2c(0, 0x48, Box::new(Thermometer));
        let bus = board.i2c(0).unwrap();
        assert!(bus.probe(0x48));
        assert!(!bus.probe(0x49));
        let mut buf = [0u8; 2];
        bus.read(0x48, &mut buf).unwrap();
        assert_eq!(u16::from_be_bytes(buf), 2550);
    }

    #[test]
    fn uart_loopback() {
        let mut board = HostBoard::new();
        let uart = board.uart(0).unwrap();
        uart.write(b"hello").unwrap();
        // Port 0 is wired in loopback on the simulator.
        let mut buf = [0u8; 5];
        let n = uart.read(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"hello");
    }

    #[test]
    fn can_loopback_and_filter() {
        use rustclr_hal::can::{CanConfig, CanFrame};
        let mut board = HostBoard::new();
        let can = board.can(0).unwrap();
        can.configure(CanConfig { loopback: true, ..Default::default() }).unwrap();
        can.set_filter(0x100, 0x700).unwrap();
        can.transmit(&CanFrame::new(0x123, &[1, 2, 3])).unwrap(); // matches 0x1xx
        can.transmit(&CanFrame::new(0x400, &[9])).unwrap(); // filtered out
        assert_eq!(can.rx_pending(), 1);
        let rx = can.receive().unwrap().unwrap();
        assert_eq!(rx.id, 0x123);
        assert_eq!(rx.data, vec![1, 2, 3]);
        assert!(can.receive().unwrap().is_none());
    }

    #[test]
    fn onewire_ds18b20_scratchpad() {
        use rustclr_hal::onewire::crc8;
        let mut board = HostBoard::new();
        board.attach_onewire(0, Box::new(SimDs18b20::new(0x28_0000_0000_0001, 2550)));
        let bus = board.onewire(0).unwrap();
        assert!(bus.reset().unwrap());
        let roms = bus.search().unwrap();
        assert_eq!(roms, vec![0x28_0000_0000_0001]);
        bus.select(roms[0]).unwrap();
        bus.write_byte(0x44).unwrap(); // convert T
        bus.select(roms[0]).unwrap();
        bus.write_byte(0xBE).unwrap(); // read scratchpad
        let mut sp = [0u8; 9];
        bus.read(&mut sp).unwrap();
        assert_eq!(crc8(&sp[..8]), sp[8]);
        let raw = i16::from_le_bytes([sp[0], sp[1]]);
        assert_eq!(raw as i32 * 100 / 16, 2550); // 25.5 C survives the 1/16 grid
    }

    #[test]
    fn rtc_set_read_and_alarm() {
        use rustclr_hal::rtc::DateTime;
        let mut board = HostBoard::new();
        let rtc = board.rtc();
        let dt = DateTime { year: 2026, month: 7, day: 19, hour: 12, minute: 30, second: 0 };
        rtc.set(dt).unwrap();
        let now = rtc.now().unwrap();
        assert_eq!((now.year, now.month, now.day), (2026, 7, 19));
        let epoch = dt.to_epoch();
        assert_eq!(DateTime::from_epoch(epoch), dt);
        rtc.set_alarm(epoch + 3600).unwrap();
        assert_eq!(rtc.alarm(), Some(epoch + 3600));
    }

    #[test]
    fn watchdog_lifecycle() {
        let mut board = HostBoard::new();
        let wd = board.watchdog();
        assert!(wd.feed().is_err()); // not started
        wd.start(5000).unwrap();
        assert!(wd.is_running());
        wd.feed().unwrap();
        assert!(!board.watchdog_state().expired());
    }

    #[test]
    fn extmem_qspi_nor_semantics() {
        use rustclr_hal::extmem::ExtMemKind;
        let mut board = HostBoard::new();
        let mem = board.extmem(0).unwrap();
        assert_eq!(mem.kind(), ExtMemKind::QspiFlash);
        mem.write(100, &[0x12, 0x34]).unwrap();
        let mut buf = [0u8; 2];
        mem.read(100, &mut buf).unwrap();
        assert_eq!(buf, [0x12, 0x34]);
        // NOR: rewriting without erase can only clear bits.
        mem.write(100, &[0xFF, 0x00]).unwrap();
        mem.read(100, &mut buf).unwrap();
        assert_eq!(buf, [0x12, 0x00]);
        mem.erase(100, 1).unwrap();
        mem.read(100, &mut buf).unwrap();
        assert_eq!(buf, [0xFF, 0xFF]);
        // SDRAM writes freely and refuses erase.
        let ram = board.extmem(1).unwrap();
        assert_eq!(ram.kind(), ExtMemKind::Sdram);
        ram.write(0, &[1]).unwrap();
        ram.write(0, &[2]).unwrap();
        let mut b = [0u8; 1];
        ram.read(0, &mut b).unwrap();
        assert_eq!(b, [2]);
        assert!(ram.erase(0, 1).is_err());
    }

    #[test]
    fn netif_ethernet_and_cellular() {
        use rustclr_hal::netif::{NetIfConfig, NetIfKind};
        let mut board = HostBoard::new();
        let eth = board.netif(NetIfKind::Ethernet).unwrap();
        eth.bring_up(&NetIfConfig::default()).unwrap();
        let st = eth.status().unwrap();
        assert!(st.up);
        assert_eq!(st.ip, "192.168.1.50");
        let cell = board.netif(NetIfKind::Cellular).unwrap();
        assert!(cell.bring_up(&NetIfConfig::default()).is_err()); // APN required
        cell.bring_up(&NetIfConfig { apn: "internet".into(), ..Default::default() }).unwrap();
        let st = cell.status().unwrap();
        assert_eq!(st.operator_name, "RustNet-Cell");
        assert!(st.rssi_dbm < 0);
    }

    #[test]
    fn netif_wifi_reports_ssid_and_ip() {
        use rustclr_hal::netif::{NetIfConfig, NetIfKind};
        let mut board = HostBoard::new();
        let wifi = board.netif(NetIfKind::Wifi).unwrap();
        // Nothing joined yet: no SSID, no address.
        let st = wifi.status().unwrap();
        assert!(!st.up);
        assert_eq!(st.ssid, "");
        assert_eq!(st.ip, "");

        wifi.bring_up(&NetIfConfig {
            ssid: "RustNet-Test-AP".into(),
            password: "hunter2".into(),
            ..Default::default()
        })
        .unwrap();
        let st = wifi.status().unwrap();
        assert!(st.up);
        assert_eq!(st.ssid, "RustNet-Test-AP");
        assert_eq!(st.ip, "192.168.1.40");
        assert!(st.rssi_dbm < 0);

        // Dropping the link clears both, so a stale SSID can't be reported.
        wifi.bring_down().unwrap();
        let st = wifi.status().unwrap();
        assert!(!st.up);
        assert_eq!(st.ssid, "");
        assert_eq!(st.ip, "");
    }

    #[test]
    fn signal_generate_capture_echo() {
        let mut board = HostBoard::new();
        board.signal_inject_capture(5, vec![500, 1500, 500]);
        board.signal_set_echo(7, 5800);
        let sig = board.signal(5).unwrap();
        sig.generate(true, &[10, 20, 10]).unwrap();
        let widths = sig.capture(8, 100_000).unwrap();
        assert_eq!(widths, vec![500, 1500, 500]);
        let echo = board.signal(7).unwrap().pulse_feedback(true, 10, 100_000).unwrap();
        assert_eq!(echo, 5800);
    }

    #[test]
    fn adc_and_pwm() {
        let mut board = HostBoard::new();
        board.set_adc_raw(0, 2048);
        let adc = board.adc(0).unwrap();
        assert_eq!(adc.read_raw().unwrap(), 2048);
        let mv = adc.read_millivolts().unwrap();
        assert!((1640..=1660).contains(&mv), "mv={mv}");
        let pwm = board.pwm(0).unwrap();
        pwm.set_frequency(1000).unwrap();
        pwm.set_duty(5000).unwrap();
        pwm.enable().unwrap();
    }
}
