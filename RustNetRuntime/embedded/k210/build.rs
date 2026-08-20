use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    // riscv-rt's link.x takes MEMORY and the REGION_ALIAS lines from a
    // memory.x on the link search path.
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    fs::write(out.join("memory.x"), include_bytes!("memory.x")).unwrap();
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed=memory.x");
}
