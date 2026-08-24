#[allow(unused_imports)]
use alloc::{boxed::Box, format, string::{String, ToString}, vec, vec::Vec};

use crate::descriptor::{DeviceDescriptor, UsbClass};
use crate::device::UsbDevice;
use crate::sim::SimBus;

/// Host-side view of an attached device after enumeration.
#[derive(Debug, Clone)]
pub struct AttachedDevice {
    pub class: UsbClass,
    pub vendor_id: u16,
    pub product_id: u16,
    pub product: String,
}

/// Plug-and-play host class driver: claims devices it understands.
pub trait UsbHostDriver: Send {
    fn accepts(&self, descriptor: &DeviceDescriptor) -> bool;
    fn name(&self) -> &str;
}

#[derive(Default)]
pub struct CdcHostDriver;
impl UsbHostDriver for CdcHostDriver {
    fn accepts(&self, d: &DeviceDescriptor) -> bool {
        d.class == UsbClass::Cdc
    }
    fn name(&self) -> &str {
        "cdc-serial"
    }
}

#[derive(Default)]
pub struct HidHostDriver;
impl UsbHostDriver for HidHostDriver {
    fn accepts(&self, d: &DeviceDescriptor) -> bool {
        d.class == UsbClass::Hid
    }
    fn name(&self) -> &str {
        "hid"
    }
}

#[derive(Default)]
pub struct MscHostDriver;
impl UsbHostDriver for MscHostDriver {
    fn accepts(&self, d: &DeviceDescriptor) -> bool {
        d.class == UsbClass::MassStorage
    }
    fn name(&self) -> &str {
        "mass-storage"
    }
}

/// Minimal USB host: enumerates one port and matches a class driver.
#[derive(Default)]
pub struct UsbHost {
    drivers: Vec<Box<dyn UsbHostDriver>>,
}

impl UsbHost {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_driver(&mut self, driver: Box<dyn UsbHostDriver>) {
        self.drivers.push(driver);
    }

    /// Read the device descriptor off the bus and match a driver.
    pub fn enumerate(&mut self, bus: &mut SimBus) -> Option<AttachedDevice> {
        let raw = bus.control_get_descriptor()?;
        let descriptor = DeviceDescriptor::parse(&raw)?;
        let _driver = self.drivers.iter().find(|d| d.accepts(&descriptor))?;
        Some(AttachedDevice {
            class: descriptor.class,
            vendor_id: descriptor.vendor_id,
            product_id: descriptor.product_id,
            product: descriptor.product.clone(),
        })
    }

    pub fn bulk_out(&mut self, _bus: &mut SimBus, device: &mut UsbDevice, data: &[u8]) {
        device.class_impl.on_bulk_out(data);
    }

    pub fn bulk_in(&mut self, _bus: &mut SimBus, device: &mut UsbDevice) -> Vec<u8> {
        device.class_impl.poll_bulk_in().unwrap_or_default()
    }
}
