//! Picks the linker's `MEMORY` block from the board feature.
//!
//! `cortex-m-rt`'s `link.x` pulls `MEMORY` from a `memory.x` anywhere on the
//! link search path. The two parts differ in both flash and RAM — and the
//! F427VI's layout is not just "the same but bigger", it puts `.bss` and the
//! stack in a different physical memory — so choosing the wrong one produces a
//! firmware that links and then misbehaves at run time. Selecting it here from
//! the feature means it cannot be chosen wrongly.

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let nucleo = env::var_os("CARGO_FEATURE_NUCLEO_F401RE").is_some();
    let netduino = env::var_os("CARGO_FEATURE_NETDUINO3_F427VI").is_some();

    let source = match (nucleo, netduino) {
        (true, false) => "memory-f401re.x",
        (false, true) => "memory-f427vi.x",
        (true, true) => panic!("select exactly one board feature, not both"),
        (false, false) => {
            panic!("select a board feature: nucleo-f401re or netduino3-f427vi")
        }
    };

    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    fs::write(out.join("memory.x"), fs::read(source).unwrap()).unwrap();

    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed=memory-f401re.x");
    println!("cargo:rerun-if-changed=memory-f427vi.x");
    println!("cargo:rerun-if-changed=build.rs");
}
