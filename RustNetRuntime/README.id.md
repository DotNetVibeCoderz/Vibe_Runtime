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
checks=134 failures=0

$ rustnet run Conformance.dll --stats
checks=134 failures=0

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

Ke-38 pemeriksaan itu mencakup aritmatika dan overflow, matematika `long`, alur
kontrol, string, array, pewarisan kelas, dispatch virtual dan interface,
property, struct, enum, boxing dan casting, `try`/`catch`/`finally`, handler
bersarang, pembagian nol, delegate, serta alokasi di bawah tekanan GC. Suite
kedua, `tests/fixtures/ModernSyntax/`, melakukan hal sama untuk 35 fitur C#
modern. Keduanya proyek C# biasa yang bisa Anda baca dan kembangkan.

`cargo test --workspace` menjalankan 141 test di delapan crate.

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

## Kecepatan

Assembly yang sama di kedua runtime. Terbaik dari tiga kali jalan, wall clock
termasuk waktu start proses; .NET dibangun dengan tiered compilation dimatikan
agar JIT-nya berjalan penuh.

| Beban kerja | .NET | RustCLR | Rasio |
| --- | ---: | ---: | ---: |
| Start proses | 126 ms | **65 ms** | **0,5x** |
| Exception (50rb throw) | 125 ms | **128 ms** | **1,0x** |
| String (20rb concat) | 134 ms | **176 ms** | **1,3x** |
| Rekursi (fib 27) | 119 ms | 427 ms | 3,6x |
| Alokasi (300rb objek) | 119 ms | 992 ms | 8,3x |
| Sieve (1 juta) | 114 ms | 1.187 ms | 10,4x |
| Perkalian matriks (120 kuadrat) | 110 ms | 1.305 ms | 11,9x |
| Panggilan virtual (2 juta) | 110 ms | 1.632 ms | 14,8x |
| Quicksort (200rb) | 120 ms | 2.242 ms | 18,7x |
| Akses field (3 juta) | 144 ms | 2.764 ms | 19,2x |

Kurangi dulu baris start proses sebelum menyimpulkan: baris itu adalah sebagian
besar angka .NET pada beban kerja pendek, sehingga rasio *komputasi* sebenarnya
lebih buruk daripada yang ditunjukkan wall clock, sekitar 100x pada perulangan
paling ketat. Itulah ongkos menafsirkan alih-alih mengompilasi, dan itulah yang
dituju [Milestone 4](Plan.md).

Dua baris justru berbalik, dan keduanya karena alasan nyata. **RustCLR start dua
kali lebih cepat** (tanpa JIT, tanpa pemanasan), yang penting untuk perkakas CLI
berumur pendek dan untuk mikrokontroler yang tak punya ruang bagi cache kode.
**Exception dan string nyaris setara** karena pekerjaannya terjadi di Rust native
di dalam RustBCL, bukan di IL yang ditafsirkan.

Checksum setiap baris dibandingkan antar-runtime sebelum diukur; kalau berbeda,
yang dicetak `MISMATCH`, bukan angka.

```bash
cd benchmarks && bash run.sh
```

Rincian: [docs/benchmarks.md](docs/benchmarks.md).

---

## Instalasi

```bash
./packaging/build.sh              # untuk mesin ini
./packaging/build.sh linux-arm64  # atau Raspberry Pi, win-x64, osx-arm64
```

Lalu, dari paket yang sudah diekstrak:

```bash
./install.sh          # Linux dan macOS, ke ~/.local, tanpa root
install.ps1           # Windows, ke %LOCALAPPDATA%, tanpa elevasi
```

Keduanya menerima `--uninstall`. Sebuah paket berisi toolchain, CodeGen
self-contained, dokumentasi, dan sample: semuanya kecuali .NET SDK, yang tetap
Anda butuhkan untuk *mengompilasi* C#.

Panduan lengkap: [docs/installation.md](docs/installation.md).

---

## Memulai

**Prasyarat:** Rust 1.85+ dan .NET SDK 10.

```bash
# Build runtime dan toolchain
cargo build --release

# Kompilasi program C# dengan .NET SDK, lalu jalankan di RustCLR
cd tests/fixtures/HelloWorld
dotnet build -c Release
../../../target/release/rustnet run bin/Release/net10.0/HelloWorld.dll
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

**C# modern juga jalan.** Interpolasi string, tuple dan dekonstruksi, range dan
index (`a[^1]`, `a[1..4]`), nullable value type, property `init`-only, record,
pattern matching, switch expression, `new` bertipe-target, local function, dan
variabel `out`. `tests/fixtures/ModernSyntax/` menguji 35 di antaranya dan
melaporkan `failures=0` di kedua runtime.

**Koleksi dan LINQ.** `List<T>`, `Dictionary<K,V>`, `HashSet<T>`, `Queue<T>`,
dan `Stack<T>` diimplementasikan secara native, begitu pula LINQ — sekitar empat
puluh operator `Enumerable`, termasuk `GroupBy` dan `OrderBy`/`ThenBy`.

```csharp
var totals = orders
    .Where(o => o.Paid)
    .GroupBy(o => o.Region)
    .OrderBy(g => g.Key)
    .ToDictionary(g => g.Key, g => g.Sum(o => o.Amount));
```

Itu jalan, byte demi byte sama dengan .NET. Begitu pula `foreach` atas
`IEnumerable<T>`, atas iterator `yield return`, dan atas tipe apa pun yang
mengikuti pola enumerator.

Dua catatan, keduanya disebutkan `rustnet capabilities`: **LINQ bersifat eager**,
bukan lazy, sehingga efek samping di dalam predikat terjadi saat pemanggilan,
bukan saat konsumsi; dan pengurutan membandingkan angka dan string, serta
menolak tipe kunci lain alih-alih mengurutkannya secara sembarang.

**async dan await.** `Task`, `Task<T>`, `TaskCompletionSource`, `Task.Run`,
`Task.WhenAll`, dan pola awaiter sudah diimplementasikan, sehingga metode
`async` berjalan — termasuk exception yang dilempar melintasi `await` lalu
ditangkap pemanggilnya.

```csharp
static async Task<int> Chain(int n)
{
    int a = await Doubled(n);
    int b = await Doubled(a);
    return a + b;
}
```

Metode `async` bukan sesuatu yang istimewa bagi runtime: Roslyn menurunkannya
menjadi struct biasa ditambah panggilan ke sebuah *builder*, dan
mengimplementasikan builder itulah keseluruhan dukungan `await`. Catatannya sama
dengan yang dibawa `Thread` — **tidak ada tumpang tindih**. Sebuah task berjalan
sampai selesai di titik ia dibuat, jadi hasil dan urutannya benar, tetapi tidak
ada yang berjalan paralel.

**Reflection bekerja di atas objek `Type` yang sungguhan.** `typeof(T)`,
`GetType()`, base type, `IsAssignableFrom`, enumerasi anggota,
`MethodInfo.Invoke`, get dan set `FieldInfo`, serta `Activator.CreateInstance`.
Objek Type di-*intern* satu per tipe runtime, jadi `typeof(int) == typeof(int)`
adalah kesetaraan referensi. Custom attribute juga sudah didekode — argumen
konstruktor, named field, dan named property. `typeof(T)` atas parameter
generic yang sudah di-*erase* melempar exception alih-alih menjawab
`System.Object`.

**Sebagian metode dikompilasi menjadi kode mesin.** `rustclr-jit` menghasilkan
x86-64 ke dalam halaman write-xor-execute untuk metode *leaf* yang mengerjakan
aritmetika bilangan bulat, setelah 32 panggilan membuktikan metode itu layak
dikompilasi. Pada benchmark `kernels` hasilnya **10,7× lebih cepat** daripada
interpretasi — 232 ms berbanding 2.484 ms, yaitu 1,6× .NET, bukan 17,5×.

Jangkauannya sempit, dan `rustnet jit <assembly>` menyebutkan persis seberapa
sempit: apa pun yang memakai array, pemanggilan, alokasi, atau exception
handling tetap diinterpretasi. `rustnet run --no-jit` menginterpretasi
semuanya, dan harus mencetak byte yang sama — ada differential test yang
menjaminnya.

**Fitur lanjutan C#.** 13 dari 21 fitur yang diuji menghasilkan keluaran identik
di kedua runtime: garbage collection, `IDisposable`/`using`, `async`/`await`,
threading dengan `lock` dan `Interlocked` (keduanya diserialkan — lihat
dokumennya), primary constructor, collection expression atas array, extension
members, P/Invoke, pattern matching, record, LINQ, source generator, dan
interceptor.

Celah yang tersisa adalah `Span<T>` dan marshalling struct, yang membutuhkan
argumen tipe generic yang sudah di-*erase*; TPL dan `await using`, yang memang
belum diimplementasikan; serta pointer unsafe, yang tidak bisa diekspresikan
referensi terkelola yang struktural. Union types dan closed hierarchies belum
ada di .NET 10 sama sekali — compiler mengurainya, tetapi tipe BCL yang
dibutuhkannya belum ada.

Matriks lengkapnya, beserta alasan tiap baris:
[docs/id/fitur-lanjutan.md](docs/id/fitur-lanjutan.md).


**Belum jalan.** Tidak ada yang berjalan konkuren: task `async` maupun badan
`Thread` sama-sama dieksekusi inline, jadi hasilnya benar tetapi tidak ada
paralelisme. *Argumen* tipe generic di-*erase*, sehingga kode generic buatan
pengguna yang membaca `T` saat runtime — `typeof(T)`, `is T`, static field per
instansiasi — tidak berperilaku benar, dan comparer kustom diabaikan.
Exception filter (`catch when`) belum dievaluasi.
Penghasil kode native hanya menangani metode *leaf* bilangan bulat — 10,7× lebih
cepat di tempat ia berlaku, tetapi menolak apa pun yang memakai array atau
pemanggilan, yang mencakup sebagian besar program nyata.

`rustnet capabilities` mencetak daftar ini langsung dari runtime, jadi tidak
bisa melenceng dari kenyataan. Rincian: [docs/limitations.md](docs/limitations.md).

---

## Target

Pembaca metadata mengenali x86, x64, Arm, Arm64, RISC-V 32, dan RISC-V 64.
`rustclr-metadata` dan `rustclr-gc` **bisa dibangun tanpa `std`** untuk
`thumbv7em-none-eabihf`, `thumbv6m-none-eabi`, `riscv32imc-unknown-none-elf`,
dan `riscv64gc-unknown-none-elf` — `bash tests/embedded.sh` memeriksa keempatnya.

**Dan keduanya berjalan di perangkat keras sungguhan — di tiga arsitektur.**
Sebuah ESP32-WROOM-32 (Xtensa LX6), sebuah ESP32-C3 (RISC-V), dan sebuah
Meadow F7 Micro (STM32F777, Arm Cortex-M7), dengan keluaran yang identik byte
demi byte. Di tiap chip, pembaca metadata mengurai assembly hasil Roslyn
langsung dari flash, dan collector-nya mereklamasi sebuah siklus referensi:

```
assembly         HelloWorld
metadata version v4.0.30319
entry point      Main
cycle unrooted   live=0
refused past it  true
```

Firmware: [ESP32](embedded/esp32-demo) · [Meadow F7](embedded/meadow-f7).
Rekaman lengkap: [Xtensa](docs/logs/esp32-wroom32.log) ·
[RISC-V](docs/logs/esp32c3.log) · [Arm](docs/logs/meadow-f7.log).

**IL belum dieksekusi di chip.** Itu butuh interpreter, dan `rustclr-core` masih
memerlukan `std` — hash map, jam, dan akses berkas. Papan itu bisa membaca
assembly .NET dan mengelola heap, tetapi belum bisa menjalankan sebuah metode.

`Heap::embedded(n)` adalah batas keras, bukan sekadar petunjuk: alokasi
melampauinya gagal alih-alih tumbuh — satu-satunya jenis batas yang berarti di
perangkat yang RAM-nya sudah dianggarkan di muka. Profil Cargo `embedded`
melakukan build yang dioptimalkan untuk ukuran.

---

## Dokumentasi

- [Memulai](docs/id/memulai.md) · [English](docs/getting-started.md)
- [Instalasi](docs/id/instalasi.md) · [English](docs/installation.md)
- [Benchmark](docs/benchmarks.md)
- [Fitur lanjutan C#](docs/id/fitur-lanjutan.md) · [English](docs/advanced-features.md)
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
konformans harus terus melaporkan `failures=0` di kedua runtime.

**Koleksi dan LINQ.** `List<T>`, `Dictionary<K,V>`, `HashSet<T>`, `Queue<T>`,
dan `Stack<T>` diimplementasikan secara native, begitu pula LINQ — sekitar empat
puluh operator `Enumerable`, termasuk `GroupBy` dan `OrderBy`/`ThenBy`.

```csharp
var totals = orders
    .Where(o => o.Paid)
    .GroupBy(o => o.Region)
    .OrderBy(g => g.Key)
    .ToDictionary(g => g.Key, g => g.Sum(o => o.Amount));
```

Itu jalan, byte demi byte sama dengan .NET. Begitu pula `foreach` atas
`IEnumerable<T>`, atas iterator `yield return`, dan atas tipe apa pun yang
mengikuti pola enumerator.

Dua catatan, keduanya disebutkan `rustnet capabilities`: **LINQ bersifat eager**,
bukan lazy, sehingga efek samping di dalam predikat terjadi saat pemanggilan,
bukan saat konsumsi; dan pengurutan membandingkan angka dan string, serta
menolak tipe kunci lain alih-alih mengurutkannya secara sembarang.

**async dan await.** `Task`, `Task<T>`, `TaskCompletionSource`, `Task.Run`,
`Task.WhenAll`, dan pola awaiter sudah diimplementasikan, sehingga metode
`async` berjalan — termasuk exception yang dilempar melintasi `await` lalu
ditangkap pemanggilnya.

```csharp
static async Task<int> Chain(int n)
{
    int a = await Doubled(n);
    int b = await Doubled(a);
    return a + b;
}
```

Metode `async` bukan sesuatu yang istimewa bagi runtime: Roslyn menurunkannya
menjadi struct biasa ditambah panggilan ke sebuah *builder*, dan
mengimplementasikan builder itulah keseluruhan dukungan `await`. Catatannya sama
dengan yang dibawa `Thread` — **tidak ada tumpang tindih**. Sebuah task berjalan
sampai selesai di titik ia dibuat, jadi hasil dan urutannya benar, tetapi tidak
ada yang berjalan paralel.

**Reflection bekerja di atas objek `Type` yang sungguhan.** `typeof(T)`,
`GetType()`, base type, `IsAssignableFrom`, enumerasi anggota,
`MethodInfo.Invoke`, get dan set `FieldInfo`, serta `Activator.CreateInstance`.
Objek Type di-*intern* satu per tipe runtime, jadi `typeof(int) == typeof(int)`
adalah kesetaraan referensi. Custom attribute juga sudah didekode — argumen
konstruktor, named field, dan named property. `typeof(T)` atas parameter
generic yang sudah di-*erase* melempar exception alih-alih menjawab
`System.Object`.

**Sebagian metode dikompilasi menjadi kode mesin.** `rustclr-jit` menghasilkan
x86-64 ke dalam halaman write-xor-execute untuk metode *leaf* yang mengerjakan
aritmetika bilangan bulat, setelah 32 panggilan membuktikan metode itu layak
dikompilasi. Pada benchmark `kernels` hasilnya **10,7× lebih cepat** daripada
interpretasi — 232 ms berbanding 2.484 ms, yaitu 1,6× .NET, bukan 17,5×.

Jangkauannya sempit, dan `rustnet jit <assembly>` menyebutkan persis seberapa
sempit: apa pun yang memakai array, pemanggilan, alokasi, atau exception
handling tetap diinterpretasi. `rustnet run --no-jit` menginterpretasi
semuanya, dan harus mencetak byte yang sama — ada differential test yang
menjaminnya.

**Fitur lanjutan C#.** 13 dari 21 fitur yang diuji menghasilkan keluaran identik
di kedua runtime: garbage collection, `IDisposable`/`using`, `async`/`await`,
threading dengan `lock` dan `Interlocked` (keduanya diserialkan — lihat
dokumennya), primary constructor, collection expression atas array, extension
members, P/Invoke, pattern matching, record, LINQ, source generator, dan
interceptor.

Celah yang tersisa adalah `Span<T>` dan marshalling struct, yang membutuhkan
argumen tipe generic yang sudah di-*erase*; TPL dan `await using`, yang memang
belum diimplementasikan; serta pointer unsafe, yang tidak bisa diekspresikan
referensi terkelola yang struktural. Union types dan closed hierarchies belum
ada di .NET 10 sama sekali — compiler mengurainya, tetapi tipe BCL yang
dibutuhkannya belum ada.

Matriks lengkapnya, beserta alasan tiap baris:
[docs/id/fitur-lanjutan.md](docs/id/fitur-lanjutan.md).
 Ketika Anda
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
