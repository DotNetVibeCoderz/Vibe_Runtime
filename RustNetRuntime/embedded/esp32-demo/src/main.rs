//! RustCLR on ESP32 hardware.
//!
//! Builds for two chips from one source file: the ESP32-WROOM-32 (Xtensa LX6)
//! and the ESP32-C3 (RISC-V). Nothing below is core-specific — which is the
//! point, since the runtime is meant to be portable across exactly this kind of
//! gap.
//!
//! This is the firmware that turns "the crates compile for a microcontroller"
//! into "the code runs on one". It does two things on real silicon:
//!
//! 1. **Parses a real assembly.** `HelloWorld.dll`, built by Roslyn on a
//!    desktop, is embedded in flash and read by `rustclr-metadata` — PE header,
//!    CLI header, metadata tables, string heap, the entry point's name. Nothing
//!    about that code path is embedded-specific; it is the same reader the
//!    desktop runtime uses.
//!
//! 2. **Runs the collector.** `rustclr-gc` allocates into a heap with a hard
//!    slot ceiling, builds a reference cycle, drops the root, and collects —
//!    proving both that the mark-sweep handles cycles and that a fixed heap
//!    refuses to grow past its budget rather than exhausting the chip's RAM.
//!
//! What it does *not* do is execute IL: `rustclr-core` still needs `std`. That
//! gap is stated in `docs/limitations.md` rather than glossed over here.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;
use core::any::Any;

use esp_backtrace as _;
use esp_hal::main;
use esp_println::println;

use rustclr_gc::{GcObject, Handle, Heap, RootSet, Tracer};
use rustclr_metadata::{Image, TableId};

/// The assembly to read, compiled by Roslyn for `net10.0` and linked into
/// flash. 4,608 bytes — small enough that the whole image sits in the binary.
static HELLO_WORLD: &[u8] = include_bytes!("HelloWorld.dll");

// The ESP-IDF second-stage bootloader reads a descriptor out of the image
// header — version, project name, build time — and refuses an image without
// one. This macro emits it.
esp_bootloader_esp_idf::esp_app_desc!();

/// The board this firmware was built for, for the banner.
#[cfg(feature = "esp32")]
const BOARD: &str = "ESP32-WROOM-32 (Xtensa LX6)";
#[cfg(feature = "esp32c3")]
const BOARD: &str = "ESP32-C3 (RISC-V)";

/// How much RAM the allocator gets. The WROOM-32 has 520 KB of SRAM and the C3
/// has 400 KB, most of which the radio and the ROM stack want; 64 KB is a
/// comfortable slice for parsing a small assembly on either.
const HEAP_BYTES: usize = 64 * 1024;

/// Slots the managed heap may use. A *ceiling*, not a hint: allocation past it
/// fails rather than growing into memory that was budgeted for something else.
const MANAGED_SLOTS: usize = 128;

/// A node in the demonstration object graph.
struct Node {
    /// Kept so the graph is more than pointers; read back in the monitor log.
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

#[main]
fn main() -> ! {
    let _peripherals = esp_hal::init(esp_hal::Config::default());
    esp_alloc::heap_allocator!(size: HEAP_BYTES);

    println!();
    println!("========================================");
    println!(" RustCLR on {BOARD}");
    println!(" Built by Gravicode Studios, led by Kang Fadhil");
    println!("========================================");
    println!();

    read_assembly();
    println!();
    exercise_collector();

    println!();
    println!("done.");
    loop {
        // Nothing left to do; the demonstration is the output above.
        core::hint::spin_loop();
    }
}

/// Reads the embedded assembly with the same metadata reader the desktop
/// runtime uses.
fn read_assembly() {
    println!("-- metadata reader --");
    println!("image bytes      {}", HELLO_WORLD.len());

    let image = match Image::from_bytes(HELLO_WORLD.to_vec()) {
        Ok(image) => image,
        Err(e) => {
            println!("FAILED to parse: {e}");
            return;
        }
    };

    let pe = image.pe();
    println!("machine          {}", pe.machine.name());
    println!("PE32+            {}", pe.is_pe32_plus);
    println!("IL only          {}", pe.is_il_only());
    println!("assembly         {}", image.assembly_name());

    let md = image.metadata();
    println!("metadata version {}", md.version);
    println!("types            {}", md.row_count(TableId::TypeDef));
    println!("methods          {}", md.row_count(TableId::MethodDef));
    println!("member refs      {}", md.row_count(TableId::MemberRef));
    println!("assembly refs    {}", md.row_count(TableId::AssemblyRef));

    match image.entry_point() {
        Some(token) => match md.method_def(token.row()) {
            Ok(method) => println!("entry point      {}", method.name),
            Err(e) => println!("entry point      <unreadable: {e}>"),
        },
        None => println!("entry point      <none>"),
    }

    // Every type the assembly declares, straight out of the string heap.
    println!("declared types:");
    for row in 1..=md.row_count(TableId::TypeDef) {
        if let Ok(t) = md.type_def(row) {
            println!("  {}", t.full_name());
        }
    }
}

/// Allocates, builds a cycle, drops the root and collects.
fn exercise_collector() {
    println!("-- garbage collector --");

    let mut heap = Heap::embedded(MANAGED_SLOTS);
    println!("collector        {}", heap.collector_name());
    println!("slot ceiling     {:?}", heap.slot_limit());

    // A three-node ring. Reference counting would never reclaim this; a tracing
    // collector reclaims it the moment nothing outside points in.
    let a = heap.alloc(Node { id: 1, next: Handle::NULL });
    let b = heap.alloc(Node { id: 2, next: a });
    let c = heap.alloc(Node { id: 3, next: b });
    if let Some(node) = heap.get_as_mut::<Node>(a) {
        node.next = c;
    }
    println!("after 3 allocs   live={} bytes={}", heap.live_count(), heap.live_bytes());

    // Still rooted: the ring survives.
    let rooted = Roots(alloc::vec![c]);
    heap.collect(&rooted);
    println!("cycle rooted     live={}", heap.live_count());

    // Root dropped: the whole ring goes, cycle and all.
    let empty = Roots(Vec::new());
    heap.collect(&empty);
    println!("cycle unrooted   live={}", heap.live_count());

    // A stale handle is detected rather than dereferenced — the property that
    // motivated a handle table instead of raw pointers.
    println!("stale handle     valid={}", heap.is_valid(a));

    // Fill the heap to its ceiling and check that it refuses to exceed it.
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
    println!("filled to        {allocated} slots");
    println!("refused past it  {refused}");

    let stats = heap.stats();
    println!("collections      {}", stats.collections);
    println!("total allocs     {}", stats.total_allocations);
    println!("objects freed    {}", stats.total_objects_freed);
    println!("peak live bytes  {}", stats.peak_live_bytes);
}
