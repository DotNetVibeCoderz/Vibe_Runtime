//! Every intrinsic agrees with the IL it stands in for.
//!
//! An intrinsic (`crates/rustclr-bcl/src/intrinsics.rs`) is a second
//! implementation of a method whose IL stays the definition, so the check is
//! direct: run a workload that reaches every shape the intrinsics take — and
//! every shape they decline — with them on and with them off, and compare.
//! The workload is `ChannelRingChecksum` in the Conformance fixture, whose
//! expected value `dotnet` also checks.

use rustclr_core::CaptureHost;
use rustclr_core::{Interpreter, MethodId, Value};

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/Conformance/bin/Release/net10.0/Conformance.dll");

fn checksum(intrinsics: bool, tamper: Option<rustclr_core::IntrinsicFn>) -> (Option<Value>, u64) {
    let mut interp = Interpreter::with_host(Box::new(CaptureHost::new()));
    rustclr_bcl::install(&mut interp);
    if let Some(f) = tamper {
        interp.register_intrinsic("System.Threading.Channels.BufferedChannel`1::PopFront()", f);
    }
    interp.intrinsics_enabled = intrinsics;
    interp.loader.load_from_file(FIXTURE).expect("fixture loads");
    let method = (0..interp.loader.registry.method_count())
        .map(|n| MethodId(n as u32))
        .find(|m| interp.loader.registry.method(*m).name == "ChannelRingChecksum")
        .expect("the fixture declares ChannelRingChecksum");
    let value = interp.invoke(method, vec![Value::I32(64)]).expect("runs");
    (value, interp.stats.intrinsic_calls)
}

#[test]
fn intrinsics_agree_with_the_il_they_replace() {
    if !std::path::Path::new(FIXTURE).exists() {
        eprintln!("skipping: conformance fixture not built");
        return;
    }
    let (with, used) = checksum(true, None);
    let (without, unused) = checksum(false, None);
    assert_eq!(unused, 0, "switched off, no intrinsic may run");
    assert!(used > 0, "the workload must actually reach the intrinsics");
    assert_eq!(with, without, "an intrinsic disagrees with its IL body");
    assert_eq!(with, Some(Value::I64(-2_829_831_623_697_927_011)), "and both with dotnet");
}

/// The comparison above has teeth: a `PopFront` wrong for one item out of
/// several hundred is caught.
#[test]
fn a_native_that_disagrees_with_its_il_is_caught() {
    if !std::path::Path::new(FIXTURE).exists() {
        eprintln!("skipping: conformance fixture not built");
        return;
    }
    // The real native, wrong by one for a single item: 3, the second int
    // the unbounded channel carries.
    fn wrong(interp: &mut Interpreter, args: &[Value]) -> rustclr_core::ExecResult<Option<Option<Value>>> {
        Ok(match rustclr_bcl::intrinsics::pop_front(interp, args)? {
            Some(Some(Value::I32(3))) => Some(Some(Value::I32(4))),
            other => other,
        })
    }
    let (good, _) = checksum(false, None);
    let (bad, used) = checksum(true, Some(wrong));
    assert!(used > 0);
    assert_ne!(good, bad, "the differential check must notice a wrong intrinsic");
}
