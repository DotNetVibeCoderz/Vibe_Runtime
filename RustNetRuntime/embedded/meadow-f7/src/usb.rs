//! A CDC serial console over the board's own USB socket.
//!
//! The Meadow has no USB-serial chip: the STM32 *is* the USB device, so one
//! cable carries both DFU and the console. The OTG driver is taken directly
//! rather than through a vendor HAL, because everything else here is
//! register-level and pulling a fifteen-device PAC for one peripheral would be
//! the odd one out.
//!
//! The register addresses, the VBUS override and the pin configuration below
//! are taken from the RustNet Meadow F7 port, which worked them out against
//! this board's schematic. Two of them are the difference between a device a
//! host sees and a device a host can read from, so they are worth naming:
//!
//! * **PA11/PA12 must be at the highest slew rate.** Full-speed USB is
//!   12 Mbit/s and will not meet the eye diagram at the reset drive setting.
//! * **B-session valid must be forced.** VBUS reaches this MCU through a power
//!   switch and a FET, so a core waiting to see VBUS attaches — the pull-up is
//!   independent — and then answers nothing.

use synopsys_usb_otg::{UsbBus, UsbPeripheral};
use usb_device::class_prelude::UsbBusAllocator;
use usb_device::prelude::*;
use usbd_serial::SerialPort;

/// The community shared CDC identifier, not a registered one. Fine for a
/// development board; a product needs its own.
const VID: u16 = 0x16C0;
const PID: u16 = 0x27DD;

/// How many times to re-poll while waiting for the host to take a write. A
/// host that has not opened the port never will, and blocking there would take
/// the whole service loop with it.
const WRITE_ATTEMPTS: u32 = 64;

const RCC_BASE: usize = 0x4002_3800;
const RCC_AHB1ENR: usize = RCC_BASE + 0x30;
const RCC_AHB2ENR: usize = RCC_BASE + 0x34;
const RCC_AHB2RSTR: usize = RCC_BASE + 0x14;
/// OTG_FS occupies bit 7 in both the AHB2 enable and reset registers.
const AHB2_OTGFS: u32 = 1 << 7;

const GPIOA_BASE: usize = 0x4002_0000;
const GPIO_MODER: usize = 0x00;
const GPIO_OSPEEDR: usize = 0x08;
const GPIO_AFRH: usize = 0x24;

/// The OTG_FS core's register block (RM0410 §42).
const OTG_FS_BASE: *const () = 0x5000_0000 as *const ();
const OTG_GOTGCTL: usize = 0x5000_0000;
const OTG_GCCFG: usize = 0x5000_0038;

/// The full-speed core's own packet memory, in 32-bit words: 1.25 KB.
const FS_FIFO_WORDS: usize = 320;
/// Six bidirectional endpoints, which is what the F7's full-speed core has.
const FS_ENDPOINTS: usize = 6;

/// The STM32F7's full-speed OTG core.
pub struct OtgFs {
    pub ahb_hz: u32,
}

// SAFETY: `REGISTERS` is the address the reference manual gives for the OTG_FS
// core on this family, the FIFO and endpoint counts are that core's, and
// `enable` performs exactly the clock gating the driver requires.
unsafe impl UsbPeripheral for OtgFs {
    const REGISTERS: *const () = OTG_FS_BASE;
    const HIGH_SPEED: bool = false;
    const FIFO_DEPTH_WORDS: usize = FS_FIFO_WORDS;
    const ENDPOINT_COUNT: usize = FS_ENDPOINTS;

    /// Clock the OTG core **and reset it**.
    ///
    /// The reset matters: this firmware is reached from the ROM bootloader,
    /// which has itself just been running USB to accept the DFU download, and
    /// `dfu-util`'s `:leave` jumps to the application rather than putting the
    /// chip through a power-on reset. So the core arrives configured for
    /// somebody else's session. The driver's own soft reset clears its
    /// internal logic but not that; only the peripheral reset does.
    fn enable() {
        cortex_m::interrupt::free(|_| {
            // SAFETY: fixed peripheral addresses, and the critical section
            // makes each read-modify-write of a shared RCC register atomic.
            unsafe {
                let rstr = core::ptr::read_volatile(RCC_AHB2RSTR as *const u32);
                core::ptr::write_volatile(RCC_AHB2RSTR as *mut u32, rstr | AHB2_OTGFS);
                for _ in 0..64 {
                    core::hint::spin_loop();
                }
                core::ptr::write_volatile(RCC_AHB2RSTR as *mut u32, rstr & !AHB2_OTGFS);

                let ahb2 = core::ptr::read_volatile(RCC_AHB2ENR as *const u32);
                core::ptr::write_volatile(RCC_AHB2ENR as *mut u32, ahb2 | AHB2_OTGFS);
            }
        });
    }

    fn ahb_frequency_hz(&self) -> u32 {
        self.ahb_hz
    }
}

pub struct UsbConsole {
    device: UsbDevice<'static, UsbBus<OtgFs>>,
    serial: SerialPort<'static, UsbBus<OtgFs>>,
}

impl UsbConsole {
    /// Bring up PA11/PA12 and enumerate as a CDC serial device.
    ///
    /// `ahb_hz` is what the core is actually running at, not what it was asked
    /// for: the driver uses it to time the turnaround the host expects.
    pub fn new(ahb_hz: u32) -> Self {
        Self::setup_pins();

        let bus: &'static mut _ = cortex_m::singleton!(
            : UsbBusAllocator<UsbBus<OtgFs>> = UsbBus::new(OtgFs { ahb_hz }, unsafe {
                // SAFETY: handed to the allocator once and borrowed for the
                // program's life; nothing else refers to it.
                &mut *core::ptr::addr_of_mut!(EP_MEMORY)
            })
        )
        .expect("the USB bus is built once");

        let serial = SerialPort::new(bus);
        let device = UsbDeviceBuilder::new(bus, UsbVidPid(VID, PID))
            // Declared at device level so a host binds its CDC driver rather
            // than leaving the device unclaimed.
            .device_class(usbd_serial::USB_CLASS_CDC)
            .strings(&[StringDescriptors::default()
                .manufacturer("Gravicode Studios")
                .product("RustCLR Meadow F7")
                .serial_number("rustclr-meadow-f7")])
            .expect("descriptor strings fit")
            .build();

        Self { device, serial }
    }

    /// PA11 and PA12 to alternate function 10, at the highest slew rate.
    fn setup_pins() {
        // SAFETY: fixed peripheral addresses; the critical section makes each
        // read-modify-write atomic.
        cortex_m::interrupt::free(|_| unsafe {
            let ahb1 = core::ptr::read_volatile(RCC_AHB1ENR as *const u32);
            core::ptr::write_volatile(RCC_AHB1ENR as *mut u32, ahb1 | 1); // GPIOAEN

            let moder = (GPIOA_BASE + GPIO_MODER) as *mut u32;
            let v = core::ptr::read_volatile(moder);
            // Pins 11 and 12 to 0b10 (alternate function).
            let v = (v & !((0b11 << 22) | (0b11 << 24))) | (0b10 << 22) | (0b10 << 24);
            core::ptr::write_volatile(moder, v);

            let ospeedr = (GPIOA_BASE + GPIO_OSPEEDR) as *mut u32;
            let v = core::ptr::read_volatile(ospeedr);
            core::ptr::write_volatile(ospeedr, v | (0b11 << 22) | (0b11 << 24)); // very high

            // AFRH covers pins 8..15, four bits each: pin 11 at bits 12..15,
            // pin 12 at 16..19. AF10 is OTG_FS.
            let afrh = (GPIOA_BASE + GPIO_AFRH) as *mut u32;
            let v = core::ptr::read_volatile(afrh);
            let v = (v & !(0xF << 12) & !(0xF << 16)) | (10 << 12) | (10 << 16);
            core::ptr::write_volatile(afrh, v);
        });
    }

    /// Tell the core the USB session is valid, whatever it thinks of VBUS.
    ///
    /// On this board VBUS reaches the MCU through a power switch and a FET, so
    /// a core that waits to see VBUS attaches and then answers nothing — a
    /// device the host sees and cannot read a descriptor from.
    pub fn force_session_valid(&mut self) {
        // SAFETY: fixed peripheral addresses; the core is initialised by now.
        unsafe {
            let cfg = core::ptr::read_volatile(OTG_GCCFG as *const u32);
            // PWRDWN on (transceiver powered), VBDEN off (do not gate on VBUS).
            core::ptr::write_volatile(OTG_GCCFG as *mut u32, (cfg | (1 << 16)) & !(1 << 21));

            let otg = core::ptr::read_volatile(OTG_GOTGCTL as *const u32);
            // BVALOEN | BVALOVAL: override the B-session signal, and say valid.
            core::ptr::write_volatile(OTG_GOTGCTL as *mut u32, otg | (1 << 6) | (1 << 7));
        }
    }

    /// The host's `DTR` line: true once a terminal has actually *opened* the
    /// port, as opposed to the OS merely having enumerated the device.
    ///
    /// The difference matters. A configured device with nobody reading it
    /// still accepts writes into the endpoint, and then drops them when the
    /// buffer fills — which is exactly how the first run of this firmware lost
    /// most of its report.
    pub fn dtr(&self) -> bool {
        self.serial.dtr()
    }

    /// Answer whatever the host is asking. Cheap, and safe to call often.
    pub fn service(&mut self) {
        self.device.poll(&mut [&mut self.serial]);
    }

    /// Has a host configured this device? True once it is a usable port.
    pub fn is_configured(&self) -> bool {
        self.device.state() == UsbDeviceState::Configured
    }

    /// Write, giving up rather than blocking if the host is not draining.
    pub fn write(&mut self, bytes: &[u8]) {
        if !self.is_configured() {
            return;
        }
        let mut sent = 0;
        let mut attempts = 0;
        while sent < bytes.len() && attempts < WRITE_ATTEMPTS {
            match self.serial.write(&bytes[sent..]) {
                Ok(0) | Err(_) => {
                    attempts += 1;
                    self.service();
                }
                Ok(n) => {
                    sent += n;
                    attempts = 0;
                }
            }
        }
        let _ = self.serial.flush();
        self.service();
    }
}

impl core::fmt::Write for UsbConsole {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        // CDC is a byte pipe with no line discipline, so a bare newline stays
        // bare and most terminals show a staircase. Expand it here.
        for line in s.split_inclusive('\n') {
            match line.strip_suffix('\n') {
                Some(body) => {
                    self.write(body.as_bytes());
                    self.write(b"\r\n");
                }
                None => self.write(line.as_bytes()),
            }
        }
        Ok(())
    }
}

/// Endpoint buffers for the OTG core. Must outlive the bus, hence static.
static mut EP_MEMORY: [u32; FS_FIFO_WORDS] = [0; FS_FIFO_WORDS];
