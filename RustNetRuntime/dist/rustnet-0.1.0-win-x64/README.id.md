# RustNetRuntime

**C# di atas runtime yang ditulis ulang dengan Rust.**

*[Read in English →](README.md)*

RustNetRuntime menggantikan CoreCLR. C# tetap menjadi bahasa yang Anda tulis; di
bawahnya, runtime-nya — garbage collector, loader, mesin eksekusi, interop —
seluruhnya Rust. Ini bukan porting dari C++ milik CoreCLR, melainkan
implementasi ulang yang dibangun untuk memanfaatkan model keamanan memori dan
konkurensi Rust, bukan untuk menyalin aslinya baris demi baris.

Berdampingan dengan runtime ada **CodeGen**, sebuah IDE desktop dengan asisten —
*Jack, The Code Bender* — yang membuat kerangka proyek, menulis kode, dan
menjalankannya di kedua runtime tanpa keluar dari jendela.

---

## Sudah menjalankan assembly sungguhan

Inilah pengujian yang menentukan. Program C# yang dikompilasi Roslyn — dengan
pewarisan, interface, struct, enum, delegate, dan penanganan exception —
menghasilkan keluaran yang identik di kedua runtime:

```console
$ dotnet Conformance.dll
checks=37 failures=0

$ rustnet run Conformance.dll --stats
checks=37 failures=0

─── execution ──────────────────────────────
  wall clock                  10.505 ms
  IL instructions              19,617
  throughput                 1,867,343 instr/s
  managed calls                 2,167
  native calls                    103
  peak frame depth                 16
─── heap ───────────────────────────────────
  collector               mark-sweep
  allocations                     194
  live bytes                   10,023
```

Ke-37 pemeriksaan itu mencakup aritmatika dan overflow, matematika `long`, alur
kontrol, string, array, pewarisan kelas, dispatch virtual dan interface,
property, struct, enum, boxing dan casting, `try`/`catch`/`finally`, handler
bersarang, pembagian nol, serta delegate. Suite-nya ada di
`tests/fixtures/Conformance/` — proyek C# biasa yang bisa Anda baca dan
kembangkan.

`cargo test --workspace` menjalankan 108 test di delapan crate.

---

## CodeGen

![CodeGen menyunting proyek, dengan keluaran RustCLR dan penghitung runtime langsung](docs/images/codegen-main.png)

Ruang kerja tiga panel: penjelajah berkas, editor dengan pewarnaan sintaks dan
nomor baris, serta asisten di sebelah kanan. Status bar menampilkan hal yang
justru menjadi inti proyek ini — apa yang dikerjakan runtime pada eksekusi
terakhir: `IL 4.812 · HEAP 3,1 KB · GC 0`.

![Jack mengubah kode sesuai permintaan, memperlihatkan alat yang ia pakai](docs/images/codegen-chat.png)

Jack bukan sekadar jendela obrolan yang ditempelkan ke editor. Ia punya alat: ia
membaca dan menulis berkas di proyek yang terbuka, menyunting blok yang presisi
alih-alih menulis ulang seluruh berkas, melakukan build, menjalankan di kedua
runtime, membongkar IL, mencari di web, dan menghitung. Baris di bawah setiap
balasan mencantumkan alat yang benar-benar ia panggil.

Empat penyedia, satu antarmuka — **OpenAI**, **Claude**, **Gemini**, dan
**Ollama**. Semuanya lewat Semantic Kernel, sehingga alat asisten bekerja sama
persis apa pun pilihan Anda.

![Dialog New Project, menampilkan template dengan pratinjau langsung](docs/images/codegen-new-project.png)

Empat belas template yang mencakup proyek console, web, desktop, mobile, IoT,
dan library, lintas bidang bisnis, sains, edukasi, dan game. Template bertanda
*runs on RustCLR* berada dalam subset IL yang sudah dieksekusi runtime saat ini.

![Settings, dengan setiap nilai tersimpan di app.config](docs/images/codegen-settings.png)

Setiap pengaturan tersimpan di `app.config` dan bisa diubah di sini — model,
API key, endpoint, temperature, system prompt, path toolchain, preferensi
editor, dan tata letak. Tidak ada tempat penyimpanan konfigurasi kedua.

---

## Memulai

**Prasyarat:** Rust 1.85+ dan .NET SDK 9 atau 10.

```bash
# Build runtime dan toolchain
cargo build --release

# Kompilasi program C# dengan .NET SDK, lalu jalankan di RustCLR
cd tests/fixtures/HelloWorld
dotnet build -c Release
../../../target/release/rustnet run bin/Release/net9.0/HelloWorld.dll
```

```bash
# Jalankan IDE
dotnet run --project src/CodeGen
```

Isi API key di **Settings → Providers** untuk mengaktifkan Jack. Ollama tidak
butuh key; arahkan saja ke server lokal Anda dan pilih model.

Panduan lengkap: [docs/id/memulai.md](docs/id/memulai.md).

---

## Isi paket

| Komponen | Perannya |
| --- | --- |
| **RustCLR** | Runtime inti: garbage collector, sistem tipe, loader assembly, mesin eksekusi IL, penanganan exception |
| **RustBCL** | Base class library, diimplementasikan native dengan Rust — `Console`, `String`, `Math`, `Convert`, `StringBuilder`, `Array`, `GC`, dan lainnya |
| **RustNet Toolchain** | `rustnet` — build, run, inspeksi, disassembly, dan verifikasi assembly |
| **Interop Bridge** | P/Invoke ke library native, dengan marshalling dan pembungkus handle yang aman |
| **CodeGen** | IDE Avalonia beserta asistennya |

Delapan crate Rust:

```
rustclr-metadata   Pembaca PE/COFF dan ECMA-335
rustclr-gc         Managed heap, collector yang bisa diganti
rustclr-core       Sistem tipe, loader, interpreter IL
rustclr-bcl        Base class library native
rustclr-sched      Antrean lock-free, channel, thread pool
rustclr-interop    P/Invoke dan marshalling
rustclr-jit        Antarmuka kompilasi dan verifier IL
rustnet-cli        Biner toolchain
```

Arsitektur lengkap: [docs/architecture.md](docs/architecture.md).

---

## Toolchain

```bash
rustnet run <assembly> [--stats] [--trace]   # jalankan di RustCLR
rustnet info <assembly> [--verbose]          # ringkasan metadata
rustnet disasm <assembly> [filter]           # bongkar ke IL
rustnet verify <assembly>                    # laporkan apa yang tidak ter-resolve
rustnet build [proyek] [--run]               # kompilasi, lalu jalankan di sini
rustnet capabilities                         # yang sudah diimplementasikan runtime
```

`verify` adalah perintah pertama yang layak dipakai saat memindahkan program:
ia menyebutkan setiap anggota yang dirujuk program tetapi belum bisa disediakan
RustCLR — sebelum Anda menjalankannya.

Rujukan: [docs/cli.md](docs/cli.md).

---

## Yang sudah jalan, dan yang belum

Berterus terang soal ini lebih berguna daripada daftar fitur.

**Sudah jalan.** Kelas, interface, pewarisan, dispatch virtual dan interface,
value type, enum, delegate (unicast dan multicast), array, string dengan
semantik UTF-16 yang benar, boxing, casting, `try`/`catch`/`finally`,
constructor statis, P/Invoke, dan garbage collector yang menangani siklus.

**Belum jalan.** Generic di-*erase* menjadi `object`, bukan diinstansiasi.
Exception filter (`catch when`) belum dievaluasi. State machine `async`/`await`
belum dijalankan scheduler. Reflection masih minimal. Belum ada penghasil kode
native — `rustclr-jit` menyediakan antarmuka kompilasi dan verifier IL,
sedangkan eksekusinya masih interpretasi.

`rustnet capabilities` mencetak daftar ini langsung dari runtime, jadi tidak
bisa melenceng dari kenyataan. Rincian: [docs/limitations.md](docs/limitations.md).

---

## Target

Pembaca metadata mengenali x86, x64, Arm, Arm64, RISC-V 32, dan RISC-V 64. Crate
inti ditulis agar ramah `no_std`, dan collector punya profil `embedded` dengan
pemicu alokasi kecil untuk target mikrokontroler (ESP32, STM32, RISC-V). Profil
Cargo `embedded` melakukan build yang dioptimalkan untuk ukuran.

---

## Dokumentasi

- [Memulai](docs/id/memulai.md) · [English](docs/getting-started.md)
- [Arsitektur](docs/architecture.md)
- [Runtime secara mendalam](docs/runtime.md)
- [Panduan CodeGen](docs/id/codegen.md) · [English](docs/codegen.md)
- [Rujukan toolchain](docs/cli.md)
- [Template](docs/templates.md)
- [Batasan](docs/limitations.md)
- [Peta jalan](Plan.md) · [Progres](Progress.md)

---

## Berkontribusi

Test adalah kontraknya. `cargo test --workspace` harus tetap hijau, dan fixture
konformans harus terus melaporkan `failures=0` di kedua runtime. Ketika Anda
menambah kemampuan runtime, tambahkan pemeriksaan di
`tests/fixtures/Conformance/Program.cs` yang gagal tanpa kemampuan itu.

Screenshot di README ini dihasilkan otomatis, bukan diambil manual:

```bash
dotnet run --project src/CodeGen -c Release -- --screenshot docs/images
```

---

## Kredit

Dibuat oleh **Gravicode Studios**, dipimpin oleh **Kang Fadhil**.

Lisensi MIT.
