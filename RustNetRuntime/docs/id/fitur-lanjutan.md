# Fitur lanjutan C# di RustCLR

Fitur lanjutan C# mana yang berjalan di RustCLR — diukur, bukan diklaim.

*[English →](../advanced-features.md)*

Setiap baris di bawah berasal dari `tests/fixtures/AdvancedFeatures/`, yang
menguji tiap fitur dalam prosesnya sendiri lalu membandingkan keluaran RustCLR
dengan .NET. Sebuah fitur dihitung didukung hanya kalau keduanya menghasilkan
**keluaran yang identik** — berjalan tanpa crash saja tidak cukup, karena
jawaban yang salah lebih buruk daripada kegagalan yang jelas.

```bash
cd tests/fixtures/AdvancedFeatures
dotnet build -c Release
bash probe.sh
```

---

## Matriksnya

**10 dari 21 probe menghasilkan keluaran identik di kedua runtime.**

### Asynchronous & Parallel Programming

| Fitur | RustCLR | Alasan |
| --- | --- | --- |
| `async` / `await` | ❌ | State machine-nya butuh `AsyncTaskMethodBuilder<T>` dan `TaskAwaiter<T>` |
| Task Parallel Library (TPL) | ❌ | `Task<T>`, `Parallel.For` atas delegate generic |
| Threading, `lock`, `Interlocked` | ⚠️ **jalan, tapi diserialkan** | Lihat catatan di bawah |

### Memory & Resource Management

| Fitur | RustCLR | Alasan |
| --- | --- | --- |
| Garbage Collection | ✅ | Mark-sweep, menangani siklus, bisa diganti |
| `IDisposable` / `using` | ✅ | Dispatch interface menemukan `Dispose` yang konkret |
| `IAsyncDisposable` / `await using` | ❌ | Butuh `async` |
| `Span<T>`, `Memory<T>` | ❌ | Ref struct generic, dan `stackalloc` butuh `localloc` |

### Modern Language Features (C# 12–15)

| Fitur | RustCLR | Alasan |
| --- | --- | --- |
| Primary Constructors (C# 12) | ✅ | Dikompilasi jadi constructor dan field biasa |
| Collection Expressions — array | ✅ | `[1, 2, 3]` adalah `newarr` plus `InitializeArray` |
| Collection Expressions — spread | ❌ | `[..a, b]` diturunkan lewat `Span<T>` |
| Collection Expressions — span | ❌ | `ReadOnlySpan<char> x = ['a']` butuh `Span<T>` |
| Extension Members (C# 14) | ✅ | Metode statis dengan parameter penerima |
| Interceptors | ✅ | Penulisan ulang saat kompilasi; runtime hanya melihat IL biasa |
| Union Types | **belum ada di .NET 10** | Compiler mengurai sintaksnya; `System.Runtime.CompilerServices.IUnion` tidak ada |
| Closed Hierarchies | **belum ada di .NET 10** | Sama — `IsClosedTypeAttribute` tidak ada di BCL |
| Extension Indexers | ⚠️ | Bagian dari extension members C# 14; bentuk property dan metode terverifikasi, indexer belum |

### Advanced Interop

| Fitur | RustCLR | Alasan |
| --- | --- | --- |
| P/Invoke | ✅ | Pemuatan dinamis sungguhan; probe-nya membaca process id sendiri |
| Type Marshalling | ❌ | `Marshal.SizeOf<T>` / `PtrToStructure<T>` bersifat generic |
| Unsafe Code, pointer | ❌ | Managed pointer di sini bersifat struktural dan tidak punya alamat |

### High-Level Abstractions

| Fitur | RustCLR | Alasan |
| --- | --- | --- |
| LINQ | ❌ | `IEnumerable<T>`, `Where`, `Select` — semuanya generic |
| Pattern Matching, switch expression | ✅ | Pattern tipe, relasional, logis, dan property |
| Records | ❌ | `Equals` bawaannya memakai `EqualityComparer<T>` |
| Source Generators | ✅ | Saat kompilasi; runtime hanya melihat IL biasa |

---

## Satu penyebab di balik hampir semua ❌

Sembilan dari sebelas kegagalan berasal dari satu celah yang sama: **tipe
generic di-*erase*, bukan diinstansiasi.** `Span<T>`, `Task<T>`, `List<T>`,
`EqualityComparer<T>`, dan `IEnumerable<T>` semuanya generic, sehingga apa pun
yang dibangun di atasnya tidak bisa di-resolve.

Itulah [Milestone 2](../../Plan.md), dan itu pekerjaan tersisa dengan nilai
tertinggi — ini bukan sebelas masalah terpisah, melainkan satu masalah dengan
sebelas gejala.

Metode generic **sudah** bekerja: sebuah instansiasi terikat berdasarkan
argumen tipenya, dan itulah yang memungkinkan interpolasi string, tuple, range,
serta `Nullable<T>`. Yang belum ada adalah **tipe** generic.

---

## Thread diserialkan

`Thread.Start()` menjalankan delegate **secara sinkron di thread pemanggil**,
dan `Join()` langsung kembali karena pekerjaannya sudah selesai. `lock` menjadi
no-op dengan alasan yang sama: tanpa eksekusi bersamaan, tidak ada yang perlu
dikecualikan.

Ini benar untuk pola start-lalu-join yang lazim, dan untuk kode yang memakai
thread guna menata pekerjaan, bukan untuk mengejar paralelisme. Ini **salah**
untuk program yang bergantung pada dua thread berjalan bersamaan — consumer yang
menunggu producer yang dimulai belakangan akan menggantung.

Alternatifnya adalah menolak `Thread` sama sekali. Menyerialkannya membuat lebih
banyak program berjalan, jadi ia ditawarkan dengan batasan yang dinyatakan di
sini, di `rustnet capabilities`, dan di kode sumbernya — bukan dibiarkan untuk
ditemukan sendiri.

`rustclr-sched` sudah memiliki substrat aslinya — antrean lock-free, channel,
dan thread pool, semuanya teruji. Yang belum ada adalah interpreter re-entrant
yang bisa digerakkan beberapa thread OS sekaligus. Itu datang bersama
[Milestone 3](../../Plan.md).

---

## Fitur saat kompilasi tidak butuh dukungan runtime

**Source generators** dan **interceptors** bekerja, dan alasannya layak
dinyatakan terang-terangan: keduanya berjalan di dalam compiler. Saat RustCLR
melihat assembly-nya, kode yang dihasilkan dan call site yang ditulis ulang
sudah menjadi IL biasa.

Fixture-nya membuktikan hal ini, bukan mengasumsikannya. `Generator/` adalah
incremental generator sungguhan yang meng-*emit* sebuah kelas dengan isi yang
dihitung dari kompilasinya, dan secara terpisah menulis ulang satu call site
dengan `InterceptsLocationAttribute`. Kedua probe lolos di RustCLR dengan
keluaran yang sama dengan .NET.

Konsekuensi praktisnya: pustaka apa pun yang dibangun di atas source generation
— banyak serializer, mapper, dan DI container — punya peluang bagus untuk
berjalan, asalkan apa yang *dihasilkannya* tetap berada dalam subset yang
didukung.

---

## Dua fitur yang belum ada di mana pun

**Union types** dan **closed hierarchies** masih berupa proposal C#. Compiler di
.NET 10 mengurai sintaksnya, tetapi tipe runtime yang dibutuhkannya
(`IUnion`, `IsClosedTypeAttribute`) tidak ada di BCL, sehingga gagal dikompilasi
bahkan di .NET sendiri:

```
error CS0518: Predefined type 'System.Runtime.CompilerServices.IUnion' is not defined
error CS0656: Missing compiler required member 'IsClosedTypeAttribute..ctor'
```

Keduanya tidak bisa didukung runtime mana pun sampai BCL merilisnya.

---

## Memeriksa program Anda sendiri

```bash
dotnet build -c Release
rustnet verify bin/Release/net10.0/AplikasiAnda.dll
```

`verify` menyebutkan setiap anggota yang dirujuk program Anda tetapi tidak bisa
disediakan RustCLR, dan setiap metode yang IL-nya gagal verifikasi — sebelum
Anda menjalankannya. Baris bertuliskan `<generic instantiation>` adalah celah
yang dijelaskan di atas.
