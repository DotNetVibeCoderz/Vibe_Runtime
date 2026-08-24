#[allow(unused_imports)]
use alloc::{boxed::Box, format, string::{String, ToString}, vec, vec::Vec};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsbClass {
    Cdc,
    Hid,
    MassStorage,
    Vendor,
}

impl UsbClass {
    pub fn code(self) -> u8 {
        match self {
            UsbClass::Cdc => 0x02,
            UsbClass::Hid => 0x03,
            UsbClass::MassStorage => 0x08,
            UsbClass::Vendor => 0xFF,
        }
    }

    pub fn from_code(code: u8) -> UsbClass {
        match code {
            0x02 => UsbClass::Cdc,
            0x03 => UsbClass::Hid,
            0x08 => UsbClass::MassStorage,
            _ => UsbClass::Vendor,
        }
    }
}

/// Simplified USB device descriptor (strings inline for convenience;
/// on-the-wire layout keeps the standard 18-byte prefix).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceDescriptor {
    pub vendor_id: u16,
    pub product_id: u16,
    pub class: UsbClass,
    pub manufacturer: String,
    pub product: String,
    pub serial: String,
}

impl DeviceDescriptor {
    pub fn serialize(&self) -> Vec<u8> {
        let strings = format!("{}\0{}\0{}", self.manufacturer, self.product, self.serial);
        let total = 18 + strings.len();
        let mut out = Vec::with_capacity(total);
        out.push(total as u8);
        out.push(0x01); // DEVICE
        out.extend_from_slice(&0x0200u16.to_le_bytes()); // bcdUSB 2.0
        out.push(self.class.code());
        out.push(0); // subclass
        out.push(0); // protocol
        out.push(64); // max packet size ep0
        out.extend_from_slice(&self.vendor_id.to_le_bytes());
        out.extend_from_slice(&self.product_id.to_le_bytes());
        out.extend_from_slice(&0x0100u16.to_le_bytes()); // bcdDevice
        out.push(1); // iManufacturer
        out.push(2); // iProduct
        out.push(3); // iSerial
        out.push(1); // num configurations
        out.extend_from_slice(strings.as_bytes());
        out
    }

    pub fn parse(data: &[u8]) -> Option<DeviceDescriptor> {
        if data.len() < 18 || data[1] != 0x01 {
            return None;
        }
        let strings = String::from_utf8_lossy(&data[18..]);
        let mut parts = strings.split('\0');
        Some(DeviceDescriptor {
            class: UsbClass::from_code(data[4]),
            vendor_id: u16::from_le_bytes([data[8], data[9]]),
            product_id: u16::from_le_bytes([data[10], data[11]]),
            manufacturer: parts.next().unwrap_or("").to_string(),
            product: parts.next().unwrap_or("").to_string(),
            serial: parts.next().unwrap_or("").to_string(),
        })
    }
}
