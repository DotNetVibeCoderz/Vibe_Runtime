# Instalasi

Paket untuk Windows, Linux, dan macOS, dibangun dari sumber dengan satu perintah.

*[English →](../installation.md)*

---

## Isi sebuah paket

| | |
| --- | --- |
| `bin/rustnet` | Toolchain — run, inspeksi, disassembly, verifikasi |
| `codegen/` | IDE-nya, self-contained: tidak butuh .NET runtime untuk dijalankan |
| `docs/`, `samples/` | Dokumentasi, sample data, contoh siap pakai |
| `install.sh`, `install.ps1` | Installer-nya |

**Satu hal tidak ada di dalam paket: .NET SDK.** RustCLR mengeksekusi IL; ia
tidak mengompilasi C#. Itu tugas Roslyn, yang ikut dengan SDK. Installer
memeriksa keberadaannya dan memberi tahu Anda, alih-alih membiarkan build
pertama Anda gagal dengan pesan yang membingungkan.

---

## Memasang

### Linux dan macOS

```bash
tar xzf rustnet-0.1.0-linux-x64.tar.gz
cd rustnet-0.1.0-linux-x64
./install.sh
```

Terpasang ke `~/.local` — tanpa root, tidak ada yang ditulis di luar prefix.
Skripnya mencetak baris persis yang perlu Anda tambahkan ke profil shell kalau
`~/.local/bin` belum ada di `PATH`.

| | |
| --- | --- |
| `./install.sh` | Per-pengguna, ke `~/.local` (bawaan) |
| `sudo ./install.sh --system` | Untuk semua pengguna, ke `/usr/local` |
| `./install.sh --prefix /opt/rustnet` | Ke lokasi tertentu |
| `./install.sh --uninstall` | Mencopot kembali |

Di Linux, sebuah desktop entry ditambahkan sehingga CodeGen muncul di menu
aplikasi Anda.

### Windows

```powershell
Expand-Archive rustnet-0.1.0-win-x64.zip
cd rustnet-0.1.0-win-x64
.\install.ps1
```

Terpasang ke `%LOCALAPPDATA%\RustNetRuntime`, menambahkan `bin` ke `PATH`
pengguna, dan membuat pintasan CodeGen di Start Menu. Tidak perlu elevasi.

| | |
| --- | --- |
| `.\install.ps1` | Per-pengguna (bawaan) |
| `.\install.ps1 -System` | Semua pengguna, ke `%ProgramFiles%` — perlu shell elevated |
| `.\install.ps1 -Prefix D:\Tools\RustNet` | Ke lokasi tertentu |
| `.\install.ps1 -Uninstall` | Mencopot kembali |

Buka terminal baru sesudahnya agar perubahan `PATH` berlaku.

### Verifikasi

```bash
rustnet capabilities
rustnet run <prefix>/samples/UserDirectory/bin/Release/net9.0/UserDirectory.dll
```

---

## Membangun paket

```bash
./packaging/build.sh                 # untuk mesin ini
./packaging/build.sh linux-arm64     # untuk Raspberry Pi
./packaging/build.sh win-x64
```

Skripnya membangun toolchain dengan `cargo build --release`, mem-*publish*
CodeGen self-contained untuk runtime identifier yang diminta, menyusun pohon
berkasnya, lalu menghasilkan `dist/rustnet-<versi>-<rid>.tar.gz` — atau `.zip`
untuk target Windows.

### Runtime identifier yang didukung

| Runtime id | Target Rust | Catatan |
| --- | --- | --- |
| `win-x64` | `x86_64-pc-windows-msvc` | |
| `win-arm64` | `aarch64-pc-windows-msvc` | |
| `linux-x64` | `x86_64-unknown-linux-gnu` | |
| `linux-arm64` | `aarch64-unknown-linux-gnu` | Raspberry Pi 4/5 |
| `linux-arm` | `armv7-unknown-linux-gnueabihf` | Raspberry Pi 2/3 |
| `linux-riscv64` | `riscv64gc-unknown-linux-gnu` | |
| `osx-x64` | `x86_64-apple-darwin` | |
| `osx-arm64` | `aarch64-apple-darwin` | Apple silicon |

### Kompilasi silang

Tambahkan dulu target Rust-nya:

```bash
rustup target add aarch64-unknown-linux-gnu
./packaging/build.sh linux-arm64
```

Kalau target itu belum terpasang, skrip mengatakannya dan membangun untuk host
alih-alih gagal diam-diam — baca keluarannya sebelum mengirimkan hasilnya.

Kompilasi silang ke Linux dari host non-Linux juga memerlukan linker untuk
target tersebut (`gcc-aarch64-linux-gnu` dan sejenisnya). Jawaban yang lazim
adalah membangun paket tiap platform di platform itu sendiri, di CI.

Ketika .NET SDK tidak bisa mem-*publish* untuk suatu runtime identifier, skrip
memberi peringatan dan mengirimkan paket **toolchain saja**: `rustnet` tanpa
CodeGen. Itu justru hasil yang benar untuk target embedded dan headless, di mana
IDE Avalonia tidak punya tempat menggambar.

---

## Membangun dari sumber saja

Anda tidak wajib memakai paket:

```bash
git clone <repositori>
cd RustNetRuntime
cargo build --release            # target/release/rustnet
dotnet run --project src/CodeGen # IDE-nya
```

`cargo test --workspace` semestinya melaporkan 111 test lulus.

---

## Mencopot

Kedua installer menerima `--uninstall` / `-Uninstall`. Keduanya menghapus biner,
entri `PATH`, dan pintasan menu.

Keduanya sengaja **tidak** menyentuh proyek Anda, maupun `CodeGen.dll.config`
yang menyimpan API key Anda — itu milik Anda. Kalau Anda ingin key-nya ikut
hilang, hapus prefix instalasinya secara manual setelah mencopot.
