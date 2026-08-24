#[allow(unused_imports)]
use alloc::{boxed::Box, format, string::{String, ToString}, vec, vec::Vec};

use crate::device::UsbDevice;

/// In-memory "cable" connecting a device stack to the host stack.
#[derive(Default)]
pub struct SimBus {
    descriptor: Option<Vec<u8>>,
}

impl SimBus {
    pub fn new() -> Self {
        Self::default()
    }

    /// Plug a device in: its descriptor becomes visible to the host.
    pub fn attach(&mut self, device: &mut UsbDevice) {
        self.descriptor = Some(device.descriptor.serialize());
    }

    pub fn detach(&mut self) {
        self.descriptor = None;
    }

    pub(crate) fn control_get_descriptor(&mut self) -> Option<Vec<u8>> {
        self.descriptor.clone()
    }
}
