//! Resolves which assembly the firmware embeds.
//!
//! The ESP32 firmware needs no linker script — esp-hal supplies its own — so
//! this build script exists solely for the indirection below.

use std::env;
use std::path::PathBuf;

fn main() {

    // ── the assembly the firmware carries ────────────────────────────────
    //
    // `include_bytes!` needs a literal path, which would mean editing this
    // crate to put a different program on the board. Resolving it here instead
    // lets a host — CodeGen's Deploy, or a shell one-liner — point the build at
    // any assembly:
    //
    //     RUSTCLR_APP=/path/to/MyApp.dll cargo build --release
    //
    // Defaults to the `HelloWorld.dll` checked in beside `main.rs`, so a plain
    // `cargo build` is unchanged.
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let app = match env::var_os("RUSTCLR_APP") {
        Some(path) => PathBuf::from(path),
        None => manifest.join("src").join("HelloWorld.dll"),
    };
    if !app.is_file() {
        panic!("RUSTCLR_APP does not name a readable file: {}", app.display());
    }
    println!("cargo:rustc-env=RUSTCLR_APP_PATH={}", app.display());
    println!("cargo:rerun-if-env-changed=RUSTCLR_APP");
    println!("cargo:rerun-if-changed={}", app.display());
}
