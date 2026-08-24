//! USB stack abstraction.
//!
//! Device side: descriptor construction plus class implementations for
//! CDC-ACM (serial), HID (keyboard) and Mass Storage (block device over
//! bulk transport). Host side: enumeration over a [`UsbHostBus`] with
//! pluggable class drivers — the same plug-and-play driver model the C#
//! layer exposes. Chip crates supply the physical controller; the
//! [`sim::SimBus`] wires a device stack to the host stack in memory for
//! development and tests.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod descriptor;
pub mod device;
pub mod host;
pub mod sim;

#[cfg(test)]
mod tests {
    use crate::descriptor::*;
    use crate::device::*;
    use crate::host::*;
    use crate::sim::SimBus;

    #[test]
    fn device_descriptor_serializes() {
        let desc = DeviceDescriptor {
            vendor_id: 0x1209,
            product_id: 0x0010,
            class: UsbClass::Cdc,
            manufacturer: "RustNet".into(),
            product: "RustNet Device".into(),
            serial: "RN-0001".into(),
        };
        let bytes = desc.serialize();
        assert_eq!(bytes[0] as usize, bytes.len());
        assert_eq!(bytes[1], 0x01); // DEVICE descriptor type
        assert_eq!(u16::from_le_bytes([bytes[8], bytes[9]]), 0x1209);
        let parsed = DeviceDescriptor::parse(&bytes).unwrap();
        assert_eq!(parsed.vendor_id, 0x1209);
        assert_eq!(parsed.class, UsbClass::Cdc);
    }

    #[test]
    fn cdc_serial_roundtrip_through_sim_bus() {
        let mut bus = SimBus::new();
        let mut device = UsbDevice::new(
            DeviceDescriptor {
                vendor_id: 0x1209,
                product_id: 0x0010,
                class: UsbClass::Cdc,
                manufacturer: "RustNet".into(),
                product: "Serial".into(),
                serial: "1".into(),
            },
            Box::new(CdcAcm::new()),
        );
        let mut host = UsbHost::new();
        host.register_driver(Box::new(CdcHostDriver::default()));

        bus.attach(&mut device);
        let attached = host.enumerate(&mut bus).expect("enumeration failed");
        assert_eq!(attached.class, UsbClass::Cdc);
        assert_eq!(attached.product, "Serial");

        // Host -> device
        host.bulk_out(&mut bus, &mut device, b"AT+INFO\r\n");
        let received = device.class_mut::<CdcAcm>().unwrap().take_rx();
        assert_eq!(received, b"AT+INFO\r\n");

        // Device -> host
        device.class_mut::<CdcAcm>().unwrap().queue_tx(b"OK\r\n");
        let answer = host.bulk_in(&mut bus, &mut device);
        assert_eq!(answer, b"OK\r\n");
    }

    #[test]
    fn hid_keyboard_reports() {
        let mut kbd = HidKeyboard::new();
        kbd.press('a');
        kbd.press('B');
        let reports = kbd.take_reports();
        assert_eq!(reports.len(), 4); // press+release per key
        assert_eq!(reports[0][2], 0x04); // usage id for 'a'
        assert_eq!(reports[2][0], 0x02); // shift modifier for 'B'
        assert!(reports[1].iter().all(|&b| b == 0)); // release
    }

    #[test]
    fn mass_storage_block_io() {
        let mut msc = MassStorage::new(64, 512); // 32 KiB volume
        let block = vec![0xA5u8; 512];
        msc.write_blocks(3, &block).unwrap();
        let read = msc.read_blocks(3, 1).unwrap();
        assert_eq!(read, block);
        assert!(msc.write_blocks(64, &block).is_err(), "out of range must fail");
        assert_eq!(msc.capacity_bytes(), 64 * 512);
    }
}
