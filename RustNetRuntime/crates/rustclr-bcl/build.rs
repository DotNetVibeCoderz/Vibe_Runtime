//! Builds the parts of RustBCL written in C# and embeds them.
//!
//! `managed/RustBCL.Concurrent` defines `System.Collections.Concurrent`,
//! `System.Threading.Channels`, `SemaphoreSlim` and `ThreadLocal<T>` as
//! ordinary managed code. With the `managed` feature on, this compiles it with
//! the .NET SDK and sets `RUSTBCL_CONCURRENT_DLL` for `include_bytes!`.
//!
//! Without an SDK the build still succeeds: it warns, and the runtime runs
//! without those types — a program that names one reports it as unresolved,
//! which is what it did before they existed. `RUSTBCL_MANAGED_DIR` points at a
//! directory holding a prebuilt `RustBCL.Concurrent.dll` instead, for a
//! machine (a CI cross-build, say) that has no SDK.

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo::rustc-check-cfg=cfg(managed_bcl)");
    println!("cargo:rerun-if-env-changed=RUSTBCL_MANAGED_DIR");
    if std::env::var_os("CARGO_FEATURE_MANAGED").is_none() {
        return;
    }

    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let project = manifest.join("managed").join("RustBCL.Concurrent");
    for entry in std::fs::read_dir(&project).into_iter().flatten().flatten() {
        let path = entry.path();
        if matches!(path.extension().and_then(|e| e.to_str()), Some("cs" | "csproj")) {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }

    let found = prebuilt().or_else(|| build(&project));
    match found {
        Some(dll) => {
            println!("cargo:rustc-env=RUSTBCL_CONCURRENT_DLL={}", dll.display());
            println!("cargo:rustc-cfg=managed_bcl");
        }
        None => println!(
            "cargo:warning=RustBCL.Concurrent was not built (no .NET SDK on PATH and no RUSTBCL_MANAGED_DIR): \
             System.Collections.Concurrent and System.Threading.Channels will be unavailable"
        ),
    }
}

fn prebuilt() -> Option<PathBuf> {
    let dir = std::env::var_os("RUSTBCL_MANAGED_DIR")?;
    let dll = Path::new(&dir).join("RustBCL.Concurrent.dll");
    dll.is_file().then_some(dll)
}

fn build(project: &Path) -> Option<PathBuf> {
    let out = PathBuf::from(std::env::var("OUT_DIR").ok()?).join("managed");
    let status = Command::new("dotnet")
        .arg("build")
        .arg(project.join("RustBCL.Concurrent.csproj"))
        .args(["-c", "Release", "--nologo", "-v", "q", "-o"])
        .arg(&out)
        .status()
        .ok()?;
    if !status.success() {
        println!("cargo:warning=dotnet build of RustBCL.Concurrent failed");
        return None;
    }
    let dll = out.join("RustBCL.Concurrent.dll");
    dll.is_file().then_some(dll)
}
