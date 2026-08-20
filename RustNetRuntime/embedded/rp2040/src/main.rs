//! RustCLR on a Raspberry Pi **RP2040** — dual Cortex-M0+ at 125 MHz.
//!
//! The fourth architecture, and the smallest core of the four: an ARMv6-M with
//! no divide instruction, no CAS, and 264 KB of SRAM for everything. The report
//! it prints is byte-identical to the ones from Xtensa, RISC-V and Cortex-M7 —
//! which is the point of putting it here.
//!
//! It reads a Roslyn-built assembly out of flash with `rustclr-metadata` and
//! exercises `rustclr-gc`; it does **not** execute IL, because `rustclr-core`
//! still needs `std`.
//!
//! ## Boot
//!
//! The RP2040 has no internal flash. Its ROM reads 256 bytes from offset 0 of
//! the QSPI part, checks their CRC and runs them; that stage sets up
//! execute-in-place and the image proper starts after it. `rp2040-boot2`
//! supplies those bytes prebuilt — a wrong CRC is a board that does nothing at
//! all, which is not a thing to hand-assemble.
//!
//! ## Console
//!
//! The board is its own USB device, so one cable carries both the UF2 drop and
//! the console. `rp2040-hal` is used for the clock tree and the USB bus: the
//! crystal-to-exactly-48 MHz path USB needs is precisely the kind of thing
//! worth taking from a maintained implementation rather than rediscovering.

#![no_std]
#![no_main]

extern crate alloc;

use core::fmt::Write as _;

use cortex_m_rt::entry;
use embedded_alloc::LlffHeap;
use hal::pac;
use hal::Clock as _;
use rp2040_hal as hal;
use usb_device::class_prelude::UsbBusAllocator;
use usb_device::prelude::*;
use usbd_serial::SerialPort;

/// The second-stage bootloader, at flash offset 0 where the ROM looks for it.
#[link_section = ".boot2"]
#[used]
pub static BOOT2: [u8; 256] = rp2040_boot2::BOOT_LOADER_W25Q080;

/// The assembly to read, compiled by Roslyn for `net10.0` and linked into
/// flash.
static HELLO_WORLD: &[u8] = include_bytes!("HelloWorld.dll");

const BOARD: &str = "Raspberry Pi RP2040 (Cortex-M0+)";

/// The crystal every RP2040 board carries; the ROM and USB both depend on it.
const XTAL_HZ: u32 = 12_000_000;

/// How much of the chip's 264 KB of SRAM the allocator gets.
// 192 KB of the RP2040's 256 KB. Code and read-only data are executed from
// flash, so RAM carries only `.data`, `.bss` and the stack — but this chip is
// still the tightest of the five: 192 KB clears the reduced binding set
// (192,045 bytes) by under 5 KB and does not come close to the full one.
// `Tier::for_budget` makes that call from the number rather than from a guess.
const HEAP_BYTES: usize = 192 * 1024;

#[global_allocator]
static ALLOCATOR: LlffHeap = LlffHeap::empty();

static mut HEAP_MEMORY: [u8; HEAP_BYTES] = [0; HEAP_BYTES];

/// A CDC serial console, wrapped so the shared demo can write to it.
struct Console<'a> {
    device: UsbDevice<'a, hal::usb::UsbBus>,
    serial: SerialPort<'a, hal::usb::UsbBus>,
}

impl Console<'_> {
    fn service(&mut self) {
        self.device.poll(&mut [&mut self.serial]);
    }

    fn is_open(&self) -> bool {
        self.device.state() == UsbDeviceState::Configured && self.serial.dtr()
    }

    /// Write, giving up rather than blocking if the host is not draining.
    fn put(&mut self, bytes: &[u8]) {
        let mut sent = 0;
        let mut attempts = 0;
        while sent < bytes.len() && attempts < 64 {
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

impl core::fmt::Write for Console<'_> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        // CDC is a byte pipe with no line discipline, so a bare newline stays
        // bare and most terminals show a staircase. Expand it here.
        for line in s.split_inclusive('\n') {
            match line.strip_suffix('\n') {
                Some(body) => {
                    self.put(body.as_bytes());
                    self.put(b"\r\n");
                }
                None => self.put(line.as_bytes()),
            }
        }
        Ok(())
    }
}

#[entry]
fn main() -> ! {
    let mut pac = pac::Peripherals::take().expect("peripherals are taken once");
    let mut watchdog = hal::Watchdog::new(pac.WATCHDOG);

    // Crystal to 125 MHz core and exactly 48 MHz for USB. `init_clocks_and_plls`
    // is the reason this port takes a HAL at all.
    let clocks = hal::clocks::init_clocks_and_plls(
        XTAL_HZ,
        pac.XOSC,
        pac.CLOCKS,
        pac.PLL_SYS,
        pac.PLL_USB,
        &mut pac.RESETS,
        &mut watchdog,
    )
    .ok()
    .expect("the crystal is 12 MHz on every RP2040 board");

    // SAFETY: called once, before anything allocates.
    unsafe {
        ALLOCATOR.init(core::ptr::addr_of_mut!(HEAP_MEMORY) as usize, HEAP_BYTES);
    }

    let bus = UsbBusAllocator::new(hal::usb::UsbBus::new(
        pac.USBCTRL_REGS,
        pac.USBCTRL_DPRAM,
        clocks.usb_clock,
        true,
        &mut pac.RESETS,
    ));
    // `singleton!` hands out a `&'static mut` exactly once, which is what the
    // borrowed SerialPort and UsbDevice both need.
    let bus: &'static UsbBusAllocator<hal::usb::UsbBus> =
        cortex_m::singleton!(: UsbBusAllocator<hal::usb::UsbBus> = bus)
            .expect("the USB bus is built once");

    let serial = SerialPort::new(bus);
    let device = UsbDeviceBuilder::new(bus, UsbVidPid(0x16C0, 0x27DD))
        .device_class(usbd_serial::USB_CLASS_CDC)
        .strings(&[StringDescriptors::default()
            .manufacturer("Gravicode Studios")
            .product("RustCLR RP2040")
            .serial_number("rustclr-rp2040")])
        .expect("descriptor strings fit")
        .build();

    let mut console = Console { device, serial };

    // Report once per terminal session, not once per boot.
    //
    // Enumeration is not the same as somebody reading: a configured device
    // with no terminal attached accepts writes into the endpoint and drops
    // them when it fills. `DTR` is the host saying it has actually opened the
    // port — so the report waits for that, and runs again for the next one.
    let mut was_open = false;
    loop {
        console.service();
        let open = console.is_open();
        if open && !was_open {
            for _ in 0..1_000_000u32 {
                console.service();
            }
            let mut detail = heapless_detail();
            let _ = write!(
                detail,
                "clock            {} MHz core, 48 MHz USB, from a {} MHz crystal",
                clocks.system_clock.freq().to_MHz(),
                XTAL_HZ / 1_000_000
            );
            rustclr_demo_common::run(&mut console, BOARD, detail.as_str(), HELLO_WORLD, HEAP_BYTES);
        }
        was_open = open;
    }
}

/// A tiny fixed string builder, so the detail line needs no allocation before
/// the report itself does.
struct Detail {
    bytes: [u8; 96],
    len: usize,
}

impl Detail {
    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }
}

impl core::fmt::Write for Detail {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for b in s.as_bytes() {
            if self.len < self.bytes.len() {
                self.bytes[self.len] = *b;
                self.len += 1;
            }
        }
        Ok(())
    }
}

fn heapless_detail() -> Detail {
    Detail { bytes: [0; 96], len: 0 }
}

/// A panic has nowhere to go on a microcontroller.
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        cortex_m::asm::wfi();
    }
}
