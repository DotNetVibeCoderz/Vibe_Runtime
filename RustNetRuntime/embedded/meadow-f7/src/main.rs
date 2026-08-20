//! RustCLR on a Wilderness Labs **Meadow F7 Micro v1.0** — STM32F777,
//! Cortex-M7 at 216 MHz.
//!
//! The third architecture. The same two crates already run on an ESP32-WROOM-32
//! (Xtensa LX6) and an ESP32-C3 (RISC-V); this adds Arm, and prints the same
//! report on all three.
//!
//! On the chip it does two things:
//!
//! 1. **Parses a real assembly.** `HelloWorld.dll`, built by Roslyn on a
//!    desktop, is embedded in flash and read by `rustclr-metadata` — PE header,
//!    CLI header, metadata tables, string heap. The same reader the desktop
//!    runtime uses, unchanged.
//!
//! 2. **Runs the collector.** `rustclr-gc` allocates into a heap with a hard
//!    slot ceiling, builds a reference cycle, drops the root, and collects.
//!
//! It does **not** execute IL: `rustclr-core` still needs `std`. That gap is
//! stated in `docs/limitations.md` rather than glossed over here.
//!
//! ## What flashing this replaces
//!
//! Whatever is in the part's internal flash, because DFU into internal flash is
//! the only way in without a probe. It is reversible — see this crate's README.
//!
//! ## The clock, and why the crystal is not a guess
//!
//! USB needs exactly 48 MHz, which needs the crystal frequency, which is not
//! published for this board. The RustNet Meadow F7 port established it by
//! sweeping candidates and letting a host adjudicate: **25 MHz**, an Abracon
//! ABM12W-25 at X401 on PH0/PH1. That answer is used directly here rather than
//! rediscovered — a board that has to search for its own crystal on every boot
//! is a board that boots slowly for no reason.
//!
//! 25 MHz does not divide to the 2 MHz the PLL prefers, so it takes a 1 MHz
//! input and twice the multiplier to reach the same 432 MHz VCO: `/M=25`,
//! `xN=432`, `/P=2` for a 216 MHz core and `/Q=9` for exactly 48 MHz.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;
use core::any::Any;
use core::fmt::Write as _;

use cortex_m_rt::entry;
use embedded_alloc::LlffHeap;

use rustclr_gc::{GcObject, Handle, Heap, RootSet, Tracer};
use rustclr_metadata::{Image, TableId};

mod usb;

/// The assembly to read, compiled by Roslyn for `net10.0` and linked into
/// flash. 4,608 bytes — small enough that the whole image sits in the binary.
static HELLO_WORLD: &[u8] = include_bytes!("HelloWorld.dll");

/// The board, for the banner.
const BOARD: &str = "Meadow F7 Micro v1.0 (STM32F777, Cortex-M7)";

/// How much of the part's 384 KB of DMA-reachable RAM the allocator gets.
const HEAP_BYTES: usize = 64 * 1024;

/// Slots the managed heap may use. A *ceiling*, not a hint: allocation past it
/// fails rather than growing into memory budgeted for something else.
const MANAGED_SLOTS: usize = 128;

#[global_allocator]
static ALLOCATOR: LlffHeap = LlffHeap::empty();

static mut HEAP_MEMORY: [u8; HEAP_BYTES] = [0; HEAP_BYTES];

// ---------------------------------------------------------------------------
// Registers
// ---------------------------------------------------------------------------

const RCC_BASE: usize = 0x4002_3800;
const RCC_CR: usize = RCC_BASE + 0x00;
const RCC_PLLCFGR: usize = RCC_BASE + 0x04;
const RCC_CFGR: usize = RCC_BASE + 0x08;
const RCC_AHB1ENR: usize = RCC_BASE + 0x30;
const RCC_APB1ENR: usize = RCC_BASE + 0x40;

const PWR_BASE: usize = 0x4000_7000;
const PWR_CR1: usize = PWR_BASE + 0x00;
const PWR_CSR1: usize = PWR_BASE + 0x04;

const FLASH_ACR: usize = 0x4002_3C00;
const GPIOH_BASE: usize = 0x4002_1C00;

/// The crystal on PH0/PH1: X401, an Abracon ABM12W-25. Recorded here because
/// it is the fact the whole clock tree hangs on, even though the divisors
/// below are what the registers actually take.
#[allow(dead_code)]
const HSE_HZ: u32 = 25_000_000;
const SYSCLK_HZ: u32 = 216_000_000;
/// `/M`, `xN`: 25 MHz to a 1 MHz PLL input, then to a 432 MHz VCO.
const PLL_M: u32 = 25;
const PLL_N: u32 = 432;
/// The VCO is always 432 MHz, and 432/9 is exactly 48.
const PLL_Q: u32 = 9;

#[inline(always)]
fn rd(addr: usize) -> u32 {
    // SAFETY: fixed peripheral addresses from the reference manual.
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}

#[inline(always)]
fn wr(addr: usize, value: u32) {
    // SAFETY: as above.
    unsafe { core::ptr::write_volatile(addr as *mut u32, value) }
}

/// Bounded wait, ~2 seconds with the core on HSI. Bounded on purpose: a hang
/// here looks exactly like a dead board.
fn wait_for(ready: impl Fn() -> bool) -> bool {
    for _ in 0..4_000_000u32 {
        if ready() {
            return true;
        }
    }
    false
}

/// Everything that must be true before the core may run at 216 MHz.
///
/// The order is the part's, not a preference: the voltage scale and over-drive
/// have to be raised *before* the core is clocked past 180 MHz, and the flash
/// wait states before the clock is switched — a core running faster than its
/// flash can answer does not fault, it fetches rubbish.
fn prepare_for_216mhz() {
    wr(RCC_CR, rd(RCC_CR) | (1 << 0)); // HSION
    while rd(RCC_CR) & (1 << 1) == 0 {} // HSIRDY

    wr(RCC_APB1ENR, rd(RCC_APB1ENR) | (1 << 28)); // PWREN
    wr(PWR_CR1, (rd(PWR_CR1) & !(0b11 << 14)) | (0b11 << 14)); // VOS = scale 1

    // Over-drive, which 216 MHz requires and 180 MHz does not.
    wr(PWR_CR1, rd(PWR_CR1) | (1 << 16)); // ODEN
    while rd(PWR_CSR1) & (1 << 16) == 0 {} // ODRDY
    wr(PWR_CR1, rd(PWR_CR1) | (1 << 17)); // ODSWEN
    while rd(PWR_CSR1) & (1 << 17) == 0 {} // ODSWRDY

    // 7 wait states with the ART accelerator and prefetch on: 216 MHz at 3.3 V.
    wr(FLASH_ACR, (1 << 9) | (1 << 8) | 7);

    // APB1 /4 = 54 MHz and APB2 /2 = 108 MHz — the family's bus ceilings.
    let cfgr = (rd(RCC_CFGR) & !0x0000_FCFF) | (0b101 << 10) | (0b100 << 13);
    wr(RCC_CFGR, cfgr);
}

/// Switch the core to the crystal at 216 MHz. Returns false if it will not.
fn use_hse() -> bool {
    // Hand PH0 and PH1 back to the oscillator before asking it to run. This
    // firmware is not reached from reset — the ROM bootloader has been using
    // the chip — and a pin left driven is a crystal that cannot swing.
    wr(RCC_AHB1ENR, rd(RCC_AHB1ENR) | (1 << 7)); // GPIOHEN
    wr(GPIOH_BASE, rd(GPIOH_BASE) | 0b11 | (0b11 << 2)); // PH0, PH1 analog
    wr(GPIOH_BASE + 0x0C, rd(GPIOH_BASE + 0x0C) & !(0b11 | (0b11 << 2))); // no pull

    // Back to HSI first: the PLL cannot be reconfigured while the core runs
    // from it, and the registers are simply ignored if it does.
    wr(RCC_CFGR, rd(RCC_CFGR) & !0b11); // SW = HSI
    while (rd(RCC_CFGR) >> 2) & 0b11 != 0 {}
    wr(RCC_CR, rd(RCC_CR) & !(1 << 24)); // PLLON = 0
    while rd(RCC_CR) & (1 << 25) != 0 {}

    // A crystal, not an external clock: HSEBYP off.
    wr(RCC_CR, rd(RCC_CR) & !(1 << 16)); // HSEON = 0
    wr(RCC_CR, rd(RCC_CR) & !(1 << 18)); // HSEBYP = 0
    wr(RCC_CR, rd(RCC_CR) | (1 << 16)); // HSEON
    if !wait_for(|| rd(RCC_CR) & (1 << 17) != 0) {
        return false;
    }

    // Bit 22 (PLLSRC) selects HSE over HSI.
    wr(RCC_PLLCFGR, PLL_M | (PLL_N << 6) | (1 << 22) | (PLL_Q << 24));
    wr(RCC_CR, rd(RCC_CR) | (1 << 24)); // PLLON
    if !wait_for(|| rd(RCC_CR) & (1 << 25) != 0) {
        return false;
    }

    wr(RCC_CFGR, (rd(RCC_CFGR) & !0b11) | 0b10); // SW = PLL
    wait_for(|| (rd(RCC_CFGR) >> 2) & 0b11 == 0b10)
}

/// The three status LEDs on PA0/PA1/PA2.
///
/// The LED is common-anode — its shared pin is at VCC and each colour returns
/// to the MCU through its own resistor — so a pin driven **low** lights it,
/// which is the opposite of the obvious.
mod led {
    use super::{rd, wr, RCC_AHB1ENR};

    const GPIOA: usize = 0x4002_0000;
    const BSRR: usize = GPIOA + 0x18;

    pub const RED: u32 = 0;
    pub const GREEN: u32 = 1;

    pub fn init() {
        wr(RCC_AHB1ENR, rd(RCC_AHB1ENR) | 1); // GPIOAEN
        let moder = rd(GPIOA);
        let cleared = moder & !(0b11 | (0b11 << 2) | (0b11 << 4));
        wr(GPIOA, cleared | 0b01 | (0b01 << 2) | (0b01 << 4));
        for pin in [0, 1, 2] {
            set(pin, false);
        }
    }

    pub fn set(pin: u32, lit: bool) {
        // Common anode: low lights it, so "on" is the reset half of BSRR.
        if lit {
            wr(BSRR, 1 << (pin + 16));
        } else {
            wr(BSRR, 1 << pin);
        }
    }
}

// ---------------------------------------------------------------------------
// The demonstration
// ---------------------------------------------------------------------------

/// A node in the demonstration object graph.
struct Node {
    /// Kept so the graph is more than pointers.
    #[allow(dead_code)]
    id: u32,
    next: Handle,
}

impl GcObject for Node {
    fn trace(&self, tracer: &mut Tracer) {
        tracer.edge(self.next);
    }
    fn size_hint(&self) -> usize {
        core::mem::size_of::<Node>()
    }
    fn type_name(&self) -> &str {
        "Node"
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// A root set backed by a fixed list of handles.
struct Roots(Vec<Handle>);

impl RootSet for Roots {
    fn collect_roots(&self, out: &mut Vec<Handle>) {
        out.extend_from_slice(&self.0);
    }
}

#[entry]
fn main() -> ! {
    let on_crystal = {
        prepare_for_216mhz();
        use_hse()
    };
    led::init();
    // Red until the console is up, so a board that never enumerates still says
    // something about how far it got.
    led::set(led::RED, true);

    // SAFETY: called once, before anything allocates.
    unsafe {
        ALLOCATOR.init(core::ptr::addr_of_mut!(HEAP_MEMORY) as usize, HEAP_BYTES);
    }

    let mut console = usb::UsbConsole::new(SYSCLK_HZ);
    console.force_session_valid();

    // Wait for a host to enumerate the device. Bounded: a board left on a
    // charger should carry on rather than spin here for ever.
    for _ in 0..40_000_000u32 {
        console.service();
        if console.is_configured() {
            break;
        }
    }
    led::set(led::RED, false);
    led::set(led::GREEN, true);

    // Then report once per terminal session, rather than once per boot.
    //
    // Enumeration is not the same as somebody reading: a configured device
    // with no terminal attached accepts writes into the endpoint and drops
    // them when it fills, which is how the first run of this firmware lost
    // most of its output. `DTR` is the host saying it has actually opened the
    // port — so the report waits for that, and runs again for the next one.
    let mut was_open = false;
    loop {
        console.service();
        let open = console.is_configured() && console.dtr();
        if open && !was_open {
            // A moment for the terminal to finish attaching before the first
            // byte goes out.
            for _ in 0..2_000_000u32 {
                console.service();
            }
            report(&mut console, on_crystal);
        }
        was_open = open;
    }
}

/// The whole report, printed once per terminal session.
fn report(console: &mut usb::UsbConsole, on_crystal: bool) {
    let _ = writeln!(console);
    let _ = writeln!(console, "========================================");
    let _ = writeln!(console, " RustCLR on {BOARD}");
    let _ = writeln!(console, " Built by Gravicode Studios, led by Kang Fadhil");
    let _ = writeln!(console, "========================================");
    let _ = writeln!(console);
    let _ = writeln!(
        console,
        "clock            {} MHz from {} ({})",
        SYSCLK_HZ / 1_000_000,
        if on_crystal { "HSE" } else { "HSI" },
        if on_crystal { "25 MHz crystal" } else { "no crystal — USB may be out of spec" }
    );
    let _ = writeln!(console);

    read_assembly(console);
    let _ = writeln!(console);
    exercise_collector(console);

    let _ = writeln!(console);
    let _ = writeln!(console, "done.");
}

/// Reads the embedded assembly with the same metadata reader the desktop
/// runtime uses.
fn read_assembly(console: &mut usb::UsbConsole) {
    let _ = writeln!(console, "-- metadata reader --");
    let _ = writeln!(console, "image bytes      {}", HELLO_WORLD.len());

    let image = match Image::from_bytes(HELLO_WORLD.to_vec()) {
        Ok(image) => image,
        Err(e) => {
            let _ = writeln!(console, "FAILED to parse: {e}");
            return;
        }
    };

    let pe = image.pe();
    let _ = writeln!(console, "machine          {}", pe.machine.name());
    let _ = writeln!(console, "PE32+            {}", pe.is_pe32_plus);
    let _ = writeln!(console, "IL only          {}", pe.is_il_only());
    let _ = writeln!(console, "assembly         {}", image.assembly_name());

    let md = image.metadata();
    let _ = writeln!(console, "metadata version {}", md.version);
    let _ = writeln!(console, "types            {}", md.row_count(TableId::TypeDef));
    let _ = writeln!(console, "methods          {}", md.row_count(TableId::MethodDef));
    let _ = writeln!(console, "member refs      {}", md.row_count(TableId::MemberRef));
    let _ = writeln!(console, "assembly refs    {}", md.row_count(TableId::AssemblyRef));

    match image.entry_point() {
        Some(token) => match md.method_def(token.row()) {
            Ok(method) => {
                let _ = writeln!(console, "entry point      {}", method.name);
            }
            Err(e) => {
                let _ = writeln!(console, "entry point      <unreadable: {e}>");
            }
        },
        None => {
            let _ = writeln!(console, "entry point      <none>");
        }
    }

    let _ = writeln!(console, "declared types:");
    for row in 1..=md.row_count(TableId::TypeDef) {
        if let Ok(t) = md.type_def(row) {
            let _ = writeln!(console, "  {}", t.full_name());
        }
    }
}

/// Allocates, builds a cycle, drops the root and collects.
fn exercise_collector(console: &mut usb::UsbConsole) {
    let _ = writeln!(console, "-- garbage collector --");

    let mut heap = Heap::embedded(MANAGED_SLOTS);
    let _ = writeln!(console, "collector        {}", heap.collector_name());
    let _ = writeln!(console, "slot ceiling     {:?}", heap.slot_limit());

    // A three-node ring. Reference counting would never reclaim this; a tracing
    // collector reclaims it the moment nothing outside points in.
    let a = heap.alloc(Node { id: 1, next: Handle::NULL });
    let b = heap.alloc(Node { id: 2, next: a });
    let c = heap.alloc(Node { id: 3, next: b });
    if let Some(node) = heap.get_as_mut::<Node>(a) {
        node.next = c;
    }
    let _ = writeln!(
        console,
        "after 3 allocs   live={} bytes={}",
        heap.live_count(),
        heap.live_bytes()
    );

    let rooted = Roots(alloc::vec![c]);
    heap.collect(&rooted);
    let _ = writeln!(console, "cycle rooted     live={}", heap.live_count());

    let empty = Roots(Vec::new());
    heap.collect(&empty);
    let _ = writeln!(console, "cycle unrooted   live={}", heap.live_count());

    // A stale handle is detected rather than dereferenced — the property that
    // motivated a handle table instead of raw pointers.
    let _ = writeln!(console, "stale handle     valid={}", heap.is_valid(a));

    let mut allocated = 0usize;
    let mut refused = false;
    for id in 0..(MANAGED_SLOTS as u32 + 8) {
        match heap.try_alloc(Node { id, next: Handle::NULL }) {
            Some(_) => allocated += 1,
            None => {
                refused = true;
                break;
            }
        }
    }
    let _ = writeln!(console, "filled to        {allocated} slots");
    let _ = writeln!(console, "refused past it  {refused}");

    let stats = heap.stats();
    let _ = writeln!(console, "collections      {}", stats.collections);
    let _ = writeln!(console, "total allocs     {}", stats.total_allocations);
    let _ = writeln!(console, "objects freed    {}", stats.total_objects_freed);
    let _ = writeln!(console, "peak live bytes  {}", stats.peak_live_bytes);
}

/// A panic has nowhere to go on a microcontroller. Light the red LED and stop,
/// so a failed run is visible rather than silent.
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    led::set(led::GREEN, false);
    led::set(led::RED, true);
    loop {
        core::hint::spin_loop();
    }
}
