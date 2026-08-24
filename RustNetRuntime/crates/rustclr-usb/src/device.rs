#[allow(unused_imports)]
use alloc::{boxed::Box, format, string::{String, ToString}, vec, vec::Vec};

use crate::descriptor::DeviceDescriptor;
use core::any::Any;
use alloc::collections::VecDeque;

/// A USB device-side class implementation (CDC, HID, MSC, vendor...).
pub trait UsbDeviceClass: Send {
    /// Bulk OUT data arriving from the host.
    fn on_bulk_out(&mut self, data: &[u8]);
    /// Next bulk IN payload for the host, if any.
    fn poll_bulk_in(&mut self) -> Option<Vec<u8>>;
    fn as_any(&mut self) -> &mut dyn Any;
}

/// Device-side stack: descriptor + active class.
pub struct UsbDevice {
    pub descriptor: DeviceDescriptor,
    pub class_impl: Box<dyn UsbDeviceClass>,
}

impl UsbDevice {
    pub fn new(descriptor: DeviceDescriptor, class_impl: Box<dyn UsbDeviceClass>) -> Self {
        Self { descriptor, class_impl }
    }

    pub fn class_mut<T: 'static>(&mut self) -> Option<&mut T> {
        self.class_impl.as_any().downcast_mut::<T>()
    }
}

// ---------------- CDC-ACM (virtual serial port) ----------------

#[derive(Default)]
pub struct CdcAcm {
    rx: VecDeque<u8>,
    tx: VecDeque<u8>,
}

impl CdcAcm {
    pub fn new() -> Self {
        Self::default()
    }

    /// Bytes the host has written to us (device application reads these).
    pub fn take_rx(&mut self) -> Vec<u8> {
        self.rx.drain(..).collect()
    }

    /// Queue bytes for the host to read.
    pub fn queue_tx(&mut self, data: &[u8]) {
        self.tx.extend(data);
    }
}

impl UsbDeviceClass for CdcAcm {
    fn on_bulk_out(&mut self, data: &[u8]) {
        self.rx.extend(data);
    }

    fn poll_bulk_in(&mut self) -> Option<Vec<u8>> {
        if self.tx.is_empty() {
            None
        } else {
            Some(self.tx.drain(..).collect())
        }
    }

    fn as_any(&mut self) -> &mut dyn Any {
        self
    }
}

// ---------------- HID keyboard ----------------

#[derive(Default)]
pub struct HidKeyboard {
    reports: Vec<[u8; 8]>,
}

impl HidKeyboard {
    pub fn new() -> Self {
        Self::default()
    }

    /// Type an ASCII character (press + release boot-protocol reports).
    pub fn press(&mut self, ch: char) {
        if let Some((modifier, usage)) = ascii_to_usage(ch) {
            let mut report = [0u8; 8];
            report[0] = modifier;
            report[2] = usage;
            self.reports.push(report);
            self.reports.push([0u8; 8]);
        }
    }

    pub fn take_reports(&mut self) -> Vec<[u8; 8]> {
        core::mem::take(&mut self.reports)
    }
}

fn ascii_to_usage(ch: char) -> Option<(u8, u8)> {
    match ch {
        'a'..='z' => Some((0, 0x04 + (ch as u8 - b'a'))),
        'A'..='Z' => Some((0x02, 0x04 + (ch.to_ascii_lowercase() as u8 - b'a'))),
        '1'..='9' => Some((0, 0x1E + (ch as u8 - b'1'))),
        '0' => Some((0, 0x27)),
        ' ' => Some((0, 0x2C)),
        '\n' => Some((0, 0x28)),
        _ => None,
    }
}

impl UsbDeviceClass for HidKeyboard {
    fn on_bulk_out(&mut self, _data: &[u8]) {}

    fn poll_bulk_in(&mut self) -> Option<Vec<u8>> {
        if self.reports.is_empty() {
            None
        } else {
            Some(self.reports.remove(0).to_vec())
        }
    }

    fn as_any(&mut self) -> &mut dyn Any {
        self
    }
}

// ---------------- Mass storage (block device) ----------------

pub struct MassStorage {
    blocks: Vec<u8>,
    block_size: usize,
    block_count: usize,
}

impl MassStorage {
    pub fn new(block_count: usize, block_size: usize) -> Self {
        Self { blocks: vec![0; block_count * block_size], block_size, block_count }
    }

    pub fn capacity_bytes(&self) -> usize {
        self.blocks.len()
    }

    pub fn read_blocks(&self, lba: usize, count: usize) -> Result<Vec<u8>, String> {
        if lba + count > self.block_count {
            return Err(format!("LBA {lba}+{count} out of range"));
        }
        let start = lba * self.block_size;
        Ok(self.blocks[start..start + count * self.block_size].to_vec())
    }

    pub fn write_blocks(&mut self, lba: usize, data: &[u8]) -> Result<(), String> {
        let count = data.len() / self.block_size;
        if lba + count > self.block_count {
            return Err(format!("LBA {lba}+{count} out of range"));
        }
        let start = lba * self.block_size;
        self.blocks[start..start + data.len()].copy_from_slice(data);
        Ok(())
    }
}

impl UsbDeviceClass for MassStorage {
    fn on_bulk_out(&mut self, _data: &[u8]) {
        // Full SCSI/BOT command handling lives in chip firmware; the core
        // exposes the block API above.
    }

    fn poll_bulk_in(&mut self) -> Option<Vec<u8>> {
        None
    }

    fn as_any(&mut self) -> &mut dyn Any {
        self
    }
}
