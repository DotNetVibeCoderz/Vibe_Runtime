//! The on-chip demonstration, shared by every board.
//!
//! Four firmwares now run this — Xtensa, RISC-V 32, Arm Cortex-M7 and
//! Cortex-M0+ — and the whole point is that they print the *same* report. That
//! only stays true if there is one copy of it, so this crate holds the report
//! and each board supplies a `core::fmt::Write` to receive it.
//!
//! It does three things on the chip:
//!
//! 1. **Parses a real assembly.** A Roslyn-built `HelloWorld.dll`, supplied by
//!    the caller from flash, read by `rustclr-metadata` — PE header, CLI
//!    header, metadata tables, string heap. The same reader the desktop
//!    runtime uses, unchanged.
//!
//! 2. **Executes it.** `rustclr-core` loads the assembly into a type registry,
//!    `rustclr-bcl` supplies `System.Console` and the rest, and the interpreter
//!    runs the entry point. The C# prints from the chip. This is the whole
//!    project's claim in miniature: the same IL, the same runtime source, a
//!    different processor and no operating system underneath.
//!
//! 3. **Runs the collector.** `rustclr-gc` allocates into a heap with a hard
//!    slot ceiling, builds a reference cycle, drops the root, and collects.

#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::any::Any;
use core::fmt::Write;

use rustclr_core::{CaptureHost, Interpreter};
use rustclr_gc::{GcObject, Handle, Heap, RootSet, Tracer};
use rustclr_metadata::{Image, TableId};

/// Peak allocation needed to run a program with the whole of RustBCL.
///
/// Measured, not estimated — a counting allocator around
/// `Interpreter::with_host` → `rustclr_bcl::install` → `load_image` →
/// `run_entry_point`, with [`EXEC_SLOTS`] slots:
///
/// | | bytes |
/// | --- | ---: |
/// | loader, 202 pre-registered framework types | 127 K |
/// | managed heap slot table | 21 K |
/// | RustBCL, 826 native bindings | 106 K |
/// | loading and running `HelloWorld` | 6 K |
/// | **peak** | **260,702** |
pub const FULL_BCL_BYTES: usize = 260_702;

/// Peak allocation with only the bindings a console program needs.
///
/// `System.Object`, `Console`, `String`, `Math` and interpolated strings — 300
/// bindings instead of 826. No LINQ, no collections, no reflection, no tasks.
/// That is 68 KB less, which is the difference between running and not running
/// on a board with 256 KB of usable RAM.
pub const MINIMAL_BCL_BYTES: usize = 192_045;

/// What a board's memory budget allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// Every native binding RustBCL has.
    Full,
    /// Console, strings and maths only.
    Minimal,
    /// Not enough RAM to load the runtime at all.
    None,
}

impl Tier {
    /// The tier a heap of `heap_bytes` can support.
    ///
    /// A plain comparison against measured figures. It errs by refusing rather
    /// than by trying and panicking: an allocation failure on a chip is a hard
    /// fault with a backtrace, which tells the user far less than a line of
    /// text saying how much memory the runtime wanted.
    pub const fn for_budget(heap_bytes: usize) -> Self {
        if heap_bytes >= FULL_BCL_BYTES {
            Tier::Full
        } else if heap_bytes >= MINIMAL_BCL_BYTES {
            Tier::Minimal
        } else {
            Tier::None
        }
    }
}

/// Slots the *interpreter's* managed heap may use.
///
/// Larger than [`MANAGED_SLOTS`], and for a different job: that one demonstrates
/// a ceiling being hit, this one has to be big enough to actually run a program.
/// Still a hard limit — a runaway program on a chip must fail an allocation, not
/// exhaust the RAM the HAL is using.
///
/// **The ceiling is paid for up front.** `Heap::embedded` reserves the whole
/// slot table on construction, deliberately: a bounded heap that grows into
/// fragmented memory is not bounded. At 24 bytes a slot, 4,096 slots is a
/// single 96 KB allocation, which is what a first attempt at this asked the
/// ESP32-C3 for — and did not get.
///
/// Measured rather than guessed: a slot costs 41 bytes, so 512 slots is 21 KB.
/// The rest of the budget is the runtime itself — 127 KB for the loader's 202
/// pre-registered framework types, and 106 KB for RustBCL's 826 native
/// bindings. `HelloWorld` uses exactly one slot; the other 511 are headroom.
pub const EXEC_SLOTS: usize = 512;

/// Slots the managed heap may use. A *ceiling*, not a hint: allocation past it
/// fails rather than growing into memory budgeted for something else.
pub const MANAGED_SLOTS: usize = 128;

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

/// The banner, the metadata report, the program, and the collector report.
///
/// `board` names the hardware; `detail` is anything the board wants to record
/// about how it got there — a clock source, a console — or empty. `heap_bytes`
/// is the size of the board's allocator arena, which decides how much of the
/// runtime will fit: see [`Tier::for_budget`].
pub fn run<W: Write>(out: &mut W, board: &str, detail: &str, assembly: &[u8], heap_bytes: usize) {
    let _ = writeln!(out);
    let _ = writeln!(out, "========================================");
    let _ = writeln!(out, " RustCLR on {board}");
    let _ = writeln!(out, " Built by Gravicode Studios, led by Kang Fadhil");
    let _ = writeln!(out, "========================================");
    let _ = writeln!(out);
    if !detail.is_empty() {
        let _ = writeln!(out, "{detail}");
        let _ = writeln!(out);
    }

    read_assembly(out, assembly);
    let _ = writeln!(out);
    execute_assembly(out, assembly, heap_bytes);
    let _ = writeln!(out);
    exercise_collector(out);

    let _ = writeln!(out);
    let _ = writeln!(out, "done.");
}

/// Reads an assembly with the same metadata reader the desktop runtime uses.
fn read_assembly<W: Write>(out: &mut W, assembly: &[u8]) {
    let _ = writeln!(out, "-- metadata reader --");
    let _ = writeln!(out, "image bytes      {}", assembly.len());

    let image = match Image::from_bytes(assembly.to_vec()) {
        Ok(image) => image,
        Err(e) => {
            let _ = writeln!(out, "FAILED to parse: {e}");
            return;
        }
    };

    let pe = image.pe();
    let _ = writeln!(out, "machine          {}", pe.machine.name());
    let _ = writeln!(out, "PE32+            {}", pe.is_pe32_plus);
    let _ = writeln!(out, "IL only          {}", pe.is_il_only());
    let _ = writeln!(out, "assembly         {}", image.assembly_name());

    let md = image.metadata();
    let _ = writeln!(out, "metadata version {}", md.version);
    let _ = writeln!(out, "types            {}", md.row_count(TableId::TypeDef));
    let _ = writeln!(out, "methods          {}", md.row_count(TableId::MethodDef));
    let _ = writeln!(out, "member refs      {}", md.row_count(TableId::MemberRef));
    let _ = writeln!(out, "assembly refs    {}", md.row_count(TableId::AssemblyRef));

    match image.entry_point() {
        Some(token) => match md.method_def(token.row()) {
            Ok(method) => {
                let _ = writeln!(out, "entry point      {}", method.name);
            }
            Err(e) => {
                let _ = writeln!(out, "entry point      <unreadable: {e}>");
            }
        },
        None => {
            let _ = writeln!(out, "entry point      <none>");
        }
    }

    let _ = writeln!(out, "declared types:");
    for row in 1..=md.row_count(TableId::TypeDef) {
        if let Ok(t) = md.type_def(row) {
            let _ = writeln!(out, "  {}", t.full_name());
        }
    }
}

/// Loads the assembly into the runtime and runs its entry point.
///
/// The program's own output is buffered rather than streamed: [`CaptureHost`]
/// collects it and this replays it afterwards, which avoids handing the
/// interpreter a borrow of the board's console for the length of the run. The
/// bytes are the same either way.
fn execute_assembly<W: Write>(out: &mut W, assembly: &[u8], heap_bytes: usize) {
    let _ = writeln!(out, "-- il interpreter --");

    let tier = Tier::for_budget(heap_bytes);
    let _ = writeln!(out, "heap budget      {heap_bytes} bytes");
    match tier {
        Tier::Full => {
            let _ = writeln!(out, "bcl tier         full ({FULL_BCL_BYTES} bytes needed)");
        }
        Tier::Minimal => {
            let _ = writeln!(out, "bcl tier         minimal ({MINIMAL_BCL_BYTES} bytes needed)");
            let _ = writeln!(out, "                 console, strings and maths; no LINQ,");
            let _ = writeln!(out, "                 collections, reflection or tasks");
        }
        Tier::None => {
            // Said plainly rather than discovered as an allocator panic. This
            // board can read metadata and collect garbage; it cannot hold the
            // type registry and the binding table at the same time.
            let _ = writeln!(out, "bcl tier         none - SKIPPED");
            let _ = writeln!(out, "                 the runtime needs {MINIMAL_BCL_BYTES} bytes");
            let _ = writeln!(out, "                 to load at all, and this board has");
            let _ = writeln!(out, "                 {heap_bytes}. Metadata and the collector");
            let _ = writeln!(out, "                 still run; IL execution does not.");
            return;
        }
    }

    let image = match Image::from_bytes(assembly.to_vec()) {
        Ok(image) => image,
        Err(e) => {
            let _ = writeln!(out, "FAILED to parse: {e}");
            return;
        }
    };

    let mut interp = Interpreter::with_host(Box::new(CaptureHost::new()));
    // A hard ceiling here too: the interpreter allocates every string and boxed
    // value the program makes, and it must not be able to take the whole chip.
    interp.heap = Heap::embedded(EXEC_SLOTS).into();
    match tier {
        Tier::Full => rustclr_bcl::install(&mut interp),
        // Pay for what you use. `install_minimal` is the same subset
        // `rustnet run --bcl minimal` installs, so a program can be checked
        // against this board's limits on a desktop before it is flashed.
        Tier::Minimal => rustclr_bcl::install_minimal(&mut interp),
        Tier::None => unreachable!("returned above"),
    }
    let _ = writeln!(out, "native bindings  {}", interp.native_count());

    let id = match interp.loader.load_image(image) {
        Ok(id) => id,
        Err(e) => {
            let _ = writeln!(out, "FAILED to load: {e}");
            return;
        }
    };
    let _ = writeln!(out, "types registered {}", interp.loader.registry.type_count());
    let _ = writeln!(out, "methods          {}", interp.loader.registry.method_count());

    let outcome = interp.run_entry_point(id);

    let _ = writeln!(out);
    let _ = writeln!(out, "--- program output ---");
    if let Some(text) = interp.host.captured_output() {
        for line in text.split('\n') {
            // Trailing CR, because `Console.WriteLine` emits CRLF and the board
            // console adds its own newline.
            let _ = writeln!(out, "{}", line.trim_end_matches('\r'));
        }
    }
    let _ = writeln!(out, "--- end ---");

    match outcome {
        Ok(code) => {
            let _ = writeln!(out, "exit code        {code}");
        }
        Err(e) => {
            let _ = writeln!(out, "FAILED to run: {e}");
        }
    }

    let stats = interp.heap.stats();
    let _ = writeln!(out, "il executed      {}", interp.stats.instructions_executed);
    let _ = writeln!(out, "calls            {}", interp.stats.calls);
    let _ = writeln!(out, "managed allocs   {}", stats.total_allocations);
    let _ = writeln!(out, "peak live bytes  {}", stats.peak_live_bytes);
}

/// Allocates, builds a cycle, drops the root and collects.
fn exercise_collector<W: Write>(out: &mut W) {
    let _ = writeln!(out, "-- garbage collector --");

    let mut heap = Heap::embedded(MANAGED_SLOTS);
    let _ = writeln!(out, "collector        {}", heap.collector_name());
    let _ = writeln!(out, "slot ceiling     {:?}", heap.slot_limit());

    // A three-node ring. Reference counting would never reclaim this; a tracing
    // collector reclaims it the moment nothing outside points in.
    let a = heap.alloc(Node { id: 1, next: Handle::NULL });
    let b = heap.alloc(Node { id: 2, next: a });
    let c = heap.alloc(Node { id: 3, next: b });
    if let Some(node) = heap.get_as_mut::<Node>(a) {
        node.next = c;
    }
    let _ = writeln!(
        out,
        "after 3 allocs   live={} bytes={}",
        heap.live_count(),
        heap.live_bytes()
    );

    let rooted = Roots(alloc::vec![c]);
    heap.collect(&rooted);
    let _ = writeln!(out, "cycle rooted     live={}", heap.live_count());

    let empty = Roots(Vec::new());
    heap.collect(&empty);
    let _ = writeln!(out, "cycle unrooted   live={}", heap.live_count());

    // A stale handle is detected rather than dereferenced — the property that
    // motivated a handle table instead of raw pointers.
    let _ = writeln!(out, "stale handle     valid={}", heap.is_valid(a));

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
    let _ = writeln!(out, "filled to        {allocated} slots");
    let _ = writeln!(out, "refused past it  {refused}");

    let stats = heap.stats();
    let _ = writeln!(out, "collections      {}", stats.collections);
    let _ = writeln!(out, "total allocs     {}", stats.total_allocations);
    let _ = writeln!(out, "objects freed    {}", stats.total_objects_freed);
    let _ = writeln!(out, "peak live bytes  {}", stats.peak_live_bytes);
}
