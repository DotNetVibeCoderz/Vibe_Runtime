use std::env;
use std::fs;
use std::path::PathBuf;

/// How many referenced assemblies a firmware image can carry.
///
/// `include_bytes!` needs a literal path per file, so each slot is a written-out
/// line in `main.rs` rather than a loop — which is why this is a small fixed
/// number and not "as many as you like". Eight is what the `Kiosk` sample needs
/// (Core, Graphics, Hal, IO, Media, Net, Serialization, UI); a program wanting
/// more is refused here, at build time, rather than failing to resolve a token
/// on a board with no debugger attached.
const MAX_LIBRARIES: usize = 8;

fn main() {
    // riscv-rt's link.x takes MEMORY and the REGION_ALIAS lines from a
    // memory.x on the link search path.
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    fs::write(out.join("memory.x"), include_bytes!("memory.x")).unwrap();
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed=memory.x");

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

    // ── the libraries it references ──────────────────────────────────────
    //
    // A board has no filesystem, so an `AssemblyRef` cannot be resolved by
    // looking beside the program the way `rustnet run` does. Whatever the
    // program references has to be carried too, and named in dependency order:
    //
    //     RUSTCLR_APP=MyApp.dll     //     RUSTCLR_LIB1=RustNet.Core.dll     //     RUSTCLR_LIB2=RustNet.Graphics.dll cargo build --release
    //
    // The ceiling is [`MAX_LIBRARIES`], and a program needing more says so at
    // build time rather than failing to resolve a token on the board.
    for n in 1..=MAX_LIBRARIES {
        println!("cargo::rustc-check-cfg=cfg(rustclr_lib{n})");
    }
    for n in 1..=MAX_LIBRARIES {
        let var = format!("RUSTCLR_LIB{n}");
        let var = var.as_str();
        println!("cargo:rerun-if-env-changed={var}");
        if let Some(path) = env::var_os(var) {
            let path = PathBuf::from(path);
            if !path.is_file() {
                panic!("{var} does not name a readable file: {}", path.display());
            }
            println!("cargo:rustc-env={var}_PATH={}", path.display());
            println!("cargo:rustc-cfg=rustclr_lib{n}");
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
}
