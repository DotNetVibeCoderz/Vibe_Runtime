//! The on-chip demonstration, shared by every board.
//!
//! Four firmwares now run this — Xtensa, RISC-V 32, Arm Cortex-M7 and
//! Cortex-M0+ — and the whole point is that they print the *same* report. That
//! only stays true if there is one copy of it, so this crate holds the report
//! and each board supplies a `core::fmt::Write` to receive it.
//!
//! It does two things on the chip:
//!
//! 1. **Parses a real assembly.** A Roslyn-built `HelloWorld.dll`, supplied by
//!    the caller from flash, read by `rustclr-metadata` — PE header, CLI
//!    header, metadata tables, string heap. The same reader the desktop
//!    runtime uses, unchanged.
//!
//! 2. **Runs the collector.** `rustclr-gc` allocates into a heap with a hard
//!    slot ceiling, builds a reference cycle, drops the root, and collects.
//!
//! It does **not** execute IL: `rustclr-core` still needs `std`.

#![no_std]

extern crate alloc;

use alloc::vec::Vec;
use core::any::Any;
use core::fmt::Write;

use rustclr_gc::{GcObject, Handle, Heap, RootSet, Tracer};
use rustclr_metadata::{Image, TableId};

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

/// The banner, the metadata report and the collector report.
///
/// `board` names the hardware; `detail` is anything the board wants to record
/// about how it got there — a clock source, a console — or empty.
pub fn run<W: Write>(out: &mut W, board: &str, detail: &str, assembly: &[u8]) {
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
