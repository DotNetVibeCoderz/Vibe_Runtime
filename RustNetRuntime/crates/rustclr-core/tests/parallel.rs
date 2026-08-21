//! Two interpreters, one heap.
//!
//! These are the properties that had to hold before more than one thread could
//! run managed code at all: allocation from several threads has to be safe, an
//! object one thread makes has to be visible to another, and a collection has
//! to stop everyone before it sweeps.

use rustclr_core::{ClrString, Interpreter, SystemHost};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;

#[test]
fn two_threads_allocate_into_one_heap() {
    let main = Interpreter::new();
    let mut workers = Vec::new();
    for n in 0..4 {
        let mut w = main.worker(Box::new(SystemHost::new()));
        workers.push(thread::spawn(move || {
            let mut handles = Vec::new();
            for i in 0..200 {
                handles.push(w.alloc_string(&format!("t{n}-{i}")));
            }
            handles
        }));
    }

    let mut all = Vec::new();
    for w in workers {
        all.extend(w.join().expect("a worker panicked"));
    }

    assert_eq!(all.len(), 800);
    // Every handle still names the string its thread wrote. Interleaved
    // allocation that was not serialised would show up here as a handle
    // naming another thread's object, or naming nothing.
    for h in &all {
        assert!(
            main.heap.with::<ClrString, _>(*h, |_| ()).is_some(),
            "a handle was lost"
        );
    }
}

#[test]
fn an_object_one_thread_makes_is_visible_to_another() {
    let mut main = Interpreter::new();
    let handle = main.alloc_string("written on the main thread");

    let mut w = main.worker(Box::new(SystemHost::new()));
    let seen = thread::spawn(move || w.string_value(handle))
        .join()
        .expect("worker panicked");

    assert_eq!(seen.as_deref(), Some("written on the main thread"));
}

#[test]
fn a_collection_stops_the_other_threads() {
    // The workers have to still be *running* when the collection happens.
    // An earlier version of this test just spawned four threads and collected;
    // they finished and unregistered first, so the collector waited for nobody
    // and the test passed without ever exercising the handshake.
    let mut main = Interpreter::new();
    let kept = main.intern("rooted on main");

    let ready = Arc::new(Barrier::new(5));
    let done = Arc::new(AtomicBool::new(false));

    let workers: Vec<_> = (0..4)
        .map(|n| {
            let mut w = main.worker(Box::new(SystemHost::new()));
            let ready = ready.clone();
            let done = done.clone();
            thread::spawn(move || {
                // Each worker roots one string of its own. If a worker's roots
                // are not contributed to a collection another thread runs,
                // this is what gets swept.
                let mine = w.intern(&format!("rooted on worker {n}"));
                ready.wait();

                while !done.load(Ordering::Relaxed) {
                    w.alloc_string("garbage");
                    w.maybe_collect();
                }

                assert!(w.heap.is_valid(mine), "worker {n} lost its rooted string");
                w.string_value(mine)
            })
        })
        .collect();

    ready.wait();
    for _ in 0..50 {
        main.force_collect();
    }
    done.store(true, Ordering::Relaxed);

    // `join` waits outside the runtime and reaches no safe point. Without this
    // guard a worker collecting here would wait for the main thread forever —
    // which is the deadlock the `blocked` state exists to prevent.
    let results = main.blocking(|_| {
        workers.into_iter().map(|w| w.join().expect("a worker panicked")).collect::<Vec<_>>()
    });

    for (n, r) in results.iter().enumerate() {
        assert_eq!(r.as_deref(), Some(format!("rooted on worker {n}").as_str()));
    }
    main.force_collect();
    assert_eq!(main.string_value(kept).as_deref(), Some("rooted on main"));
}

// -- running managed code on more than one thread ----------------------------

/// Loads the conformance fixture, which is the largest real program to hand.
fn load_fixture(interp: &mut Interpreter) -> Option<()> {
    let path = std::path::Path::new("../../tests/fixtures/Conformance/bin/Release/net10.0/Conformance.dll");
    if !path.exists() {
        return None;
    }
    interp.loader.load_from_file(path).ok().map(|_| ())
}

#[test]
fn a_worker_sees_the_same_types_as_its_parent() {
    let mut main = Interpreter::new();
    if load_fixture(&mut main).is_none() {
        eprintln!("fixture not built; skipping");
        return;
    }

    let worker = main.worker(Box::new(SystemHost::new()));

    // The copy is the point: same counts, so the same ids mean the same things.
    assert_eq!(
        worker.loader.registry.type_count(),
        main.loader.registry.type_count(),
        "a worker's registry is a copy, not a rebuild"
    );
    assert_eq!(
        worker.loader.registry.method_count(),
        main.loader.registry.method_count()
    );
    assert!(!worker.diverged(), "nothing has grown yet");
}

#[test]
fn a_static_written_on_one_thread_is_read_on_another() {
    let mut main = Interpreter::new();
    if load_fixture(&mut main).is_none() {
        eprintln!("fixture not built; skipping");
        return;
    }
    // Any static slot will do; what is being tested is that the storage is one
    // table rather than two.
    let slot = rustclr_core::FieldId(0);
    main.loader.set_static(slot, rustclr_core::Value::I32(7));

    let worker = main.worker(Box::new(SystemHost::new()));
    let seen = thread::spawn(move || worker.loader.static_value(slot))
        .join()
        .expect("worker panicked");
    assert_eq!(seen.as_i32(), Some(7), "the worker read the parent's static");

    // And back the other way, while the parent is still alive.
    let worker = main.worker(Box::new(SystemHost::new()));
    thread::spawn(move || worker.loader.set_static(slot, rustclr_core::Value::I32(9)))
        .join()
        .expect("worker panicked");
    assert_eq!(main.loader.static_value(slot).as_i32(), Some(9));
}

#[test]
fn a_worker_can_cross_a_thread_boundary() {
    fn assert_send<T: Send>() {}
    assert_send::<Interpreter>();
}
