# RustCLR on ESP32 hardware

Firmware that runs RustCLR's metadata reader and garbage collector on real
silicon. Verified on **two boards with different cores**, from one source file:

| Board | Core | Target | Toolchain | Log |
| --- | --- | --- | --- | --- |
| ESP32-WROOM-32 rev v1.0 | Xtensa LX6 | `xtensa-esp32-none-elf` | forked (`espup`) | [log](../../docs/logs/esp32-wroom32.log) |
| ESP32-C3 rev v0.3 | RISC-V | `riscv32imc-unknown-none-elf` | **stable** | [log](../../docs/logs/esp32c3.log) |

Their output is **byte-identical**. That is the point: the runtime is meant to
be portable across exactly this kind of gap, and nothing in `main.rs` is
core-specific beyond the banner.

It does two things on the chip:

| | |
| --- | --- |
| Reads a real assembly | `HelloWorld.dll`, built by Roslyn for `net10.0`, embedded in flash and parsed by `rustclr-metadata` |
| Runs the collector | Allocates, builds a reference cycle, drops the root, collects, and fills a fixed heap to its ceiling |

**It executes IL**, and this is the board it was verified on. On the C3 the
loader builds a type registry, RustBCL registers all 766 of its native
bindings, and the interpreter runs `HelloWorld.Main` — printing the same bytes
`dotnet` prints on a desktop, CRLF included, with the same instruction and call
counts. Capture: [docs/logs/esp32c3-interpreter.log](../../docs/logs/esp32c3-interpreter.log).

The old note here said `rustclr-core` needed "a hash map, a clock, and a way to
read an assembly". Two of those three were shallow — the maps are keyed by
types that are all `Ord`, so `BTreeMap` serves, and the clock was already
behind the `Host` trait. Only reading a file was real, and an assembly on a
chip arrives as bytes from flash.

**The two chips reach the memory budget differently.** The C3 has one
contiguous DRAM segment and 288 KB fits in it. The WROOM-32's `dram_seg` tops
out at 176 KB, which is below even the reduced binding set — what rescues it is
a second bank of 98,768 bytes past the ROM's data and stacks, which esp-hal
exposes as `#[ram(reclaimed)]`. `esp-alloc` takes regions rather than one
arena, so the firmware adds both.

---

## Why this is its own workspace

The runtime crates have **no external dependencies at all**, and that is a
property worth keeping. This firmware needs a HAL. Excluding it from the root
workspace is what lets both stay true — `cargo test --workspace` at the repo
root never sees `esp-hal`.

---

## ESP32-C3 (RISC-V) — the easy one

`riscv32imc-unknown-none-elf` is an upstream Rust target, so stable Rust builds
it with prebuilt `core` and `alloc`, and no forked toolchain is involved.

```bash
cd embedded/esp32-demo
cargo build --release --no-default-features \
      --features esp32c3 --target riscv32imc-unknown-none-elf

espflash flash --port COM18 --chip esp32c3 \
  target/riscv32imc-unknown-none-elf/release/rustclr-esp32-demo
```

The CH340 bridge's auto-reset works, so flashing needs no button press.

---

## ESP32-WROOM-32 (Xtensa) — the awkward one

Xtensa LX6 is not an upstream target. It needs the forked toolchain from
[espup](https://github.com/esp-rs/espup) and `core`/`alloc` built from source:

```bash
cargo install espup espflash
espup install                 # installs the `esp` rustup toolchain

cargo +esp build --release --no-default-features \
      --features esp32 --target xtensa-esp32-none-elf -Z build-std=core,alloc

espflash flash --port COM4 --chip esp32 \
  target/xtensa-esp32-none-elf/release/rustclr-esp32-demo
```

**This board needs the BOOT button.** If `espflash` reports
`Wrong boot mode detected (0x13)`, its auto-reset circuit is not pulling GPIO0
low. Hold **BOOT**, tap **EN/RST**, keep BOOT held while the flash connects,
then release.

---

## Reading the output

```bash
espflash monitor --port COM18 --chip esp32c3
```

`espflash monitor` also tries to enter download mode; if it refuses, open the
port directly at 115200 and pulse RTS to reset the chip into normal boot.

---

## Three things that had to be fixed to get here

**`rustclr-metadata` did not build without `std`.** Its `use alloc::…` lived in
`lib.rs` and reached no submodule, so every module failed the moment `std` went
away. A shared prelude fixed it; `tests/embedded.sh` now checks four upstream bare-metal
targets so it cannot regress silently. Xtensa is checked by building this
firmware, since that target needs the forked toolchain.

**`Image` was gated on `std` in its entirety**, when only `from_file` and
`path()` actually need a filesystem. Everything else works from a byte slice —
which is exactly what lets a microcontroller read an assembly out of flash.

**The image needed an ESP-IDF app descriptor.** `esp-bootloader-esp-idf` 0.3
emitted nothing the second-stage bootloader could find; 0.5 places
`esp_app_desc` at `0x3f400020`, where it is looked for.

---

## Memory

| | |
| --- | --- |
| `HEAP_BYTES` | 64 KB given to the allocator (the WROOM-32 has 520 KB SRAM, the C3 400 KB) |
| `MANAGED_SLOTS` | 128 — a **ceiling**, not a hint |

`Heap::embedded(n)` refuses to allocate past `n` rather than growing. On a
device whose RAM was budgeted up front, a heap that quietly grows has not been
bounded at all. The firmware demonstrates this: it fills the heap to exactly 128
slots and prints `refused past it  true`.
