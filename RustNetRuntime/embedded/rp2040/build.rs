use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    // cortex-m-rt's link.x takes MEMORY from a memory.x on the link search
    // path, so it has to be copied where the linker will find it.
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    fs::write(out.join("memory.x"), include_bytes!("memory.x")).unwrap();
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed=memory.x");
}
