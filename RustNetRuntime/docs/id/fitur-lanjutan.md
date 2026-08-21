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

**21 dari 21 probe menghasilkan keluaran identik di kedua runtime.**

Itu pernyataan tentang *keluaran*, bukan tentang kemampuan. Baris `async`,
threading dan TPL ditandai ⚠️ karena probe tidak bisa membedakannya: ketiganya
memberi jawaban yang sama dengan .NET, tanpa konkurensi sama sekali. Probe yang
mengukur tumpang tindih waktu nyata akan gagal.

### Asynchronous & Parallel Programming

| Fitur | RustCLR | Alasan |
| --- | --- | --- |
| `async` / `await` | ⚠️ **jalan, sinkron** | Builder dan awaiter-nya sudah ada; lihat catatan di bawah |
| `Task`, `Task<T>`, `WhenAll`, `TaskCompletionSource` | ✅ | Hasil, urutan, dan propagasi exception sama dengan .NET |
| Task Parallel Library (TPL) | ❌ | `Parallel.For` belum diimplementasikan |
| Threading, `lock`, `Interlocked` | ⚠️ **jalan, tapi diserialkan** | Lihat catatan di bawah |

### Memory & Resource Management

| Fitur | RustCLR | Alasan |
| --- | --- | --- |
| Garbage Collection | ✅ | Mark-sweep, menangani siklus, bisa diganti |
| `IDisposable` / `using` | ✅ | Dispatch interface menemukan `Dispose` yang konkret |
| `IAsyncDisposable` / `await using` | ✅ | Jalan, dengan `ValueTask` di bawahnya. Dispose berjalan setelah body |
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
| LINQ | ⚠️ **jalan, eager** | ~40 operator `Enumerable` secara native; lihat catatan di bawah |
| Koleksi generic | ✅ | `List`, `Dictionary`, `HashSet`, `Queue`, `Stack`, secara native |
| `foreach` atas `IEnumerable<T>` | ✅ | Termasuk iterator `yield return` dan enumerator buatan pengguna |
| Pattern Matching, switch expression | ✅ | Pattern tipe, relasional, logis, dan property |
| Records | ✅ | Butuh `EqualityComparer<T>.Default`, yang kini sudah ada |
| Source Generators | ✅ | Saat kompilasi; runtime hanya melihat IL biasa |

---

## Apa yang masih dibayar akibat *erasure*

Argumen tipe generic di-*erase*: `List<int>` dan `List<string>` adalah satu tipe
runtime yang sama. Dulu itu memblokir semua yang ada di halaman ini. Sekarang
tidak lagi, karena koleksinya diimplementasikan secara native di atas
penyimpanan yang mendeskripsikan dirinya sendiri — sebuah nilai runtime sudah
tahu apakah ia memuat bilangan bulat atau referensi, sehingga satu implementasi
melayani setiap `T`.

Yang masih terhalang adalah hal yang benar-benar membutuhkan argumennya saat
runtime. `Task<T>` ternyata juga tidak membutuhkan apa pun dari *erasure* —
sebuah task membawa hasilnya sebagai nilai runtime, jadi satu tipe `Task`
melayani setiap `T`. Yang masih terhalang adalah `Span<T>`, sebuah ref struct
yang harus dimodelkan runtime, dan `Marshal.SizeOf<T>` yang butuh tata letak
untuk `T` yang tidak dimilikinya.

Untuk kode generic buatan pengguna, dampak *erasure* yang terukur — `typeof(T)`,
`is T`, static per instansiasi — ada di
[limitations.md](../limitations.md).

---

## LINQ bersifat eager

Setiap operator langsung mematerialkan hasilnya, bukan mengembalikan iterator
yang malas. Tiga perilaku berbeda dari .NET: efek samping di dalam predikat
terjadi saat pemanggilan LINQ, bukan saat konsumsi; sequence tak hingga tidak
pernah berhenti; dan sumber yang diubah setelah pemanggilan tidak tercermin di
hasilnya.

Pengurutan membandingkan angka dan string. Tipe kunci lain **ditolak** dengan
pesan yang jelas alih-alih diurutkan sembarangan, dan argumen `IComparer<T>`
kustom diterima tetapi diabaikan.

---

## async bersifat sinkron

`await` sudah jalan, dan hasil, urutan, serta propagasi exception sebuah metode
async sama persis dengan .NET — termasuk exception yang dilempar melintasi
`await` lalu ditangkap pemanggilnya. Yang tidak terjadi adalah *tumpang tindih*:
sebuah task berjalan sampai selesai di titik ia dibuat, karena hanya ada satu
thread interpreter. `Task.Run` langsung memanggil delegate-nya; `Task.Delay`
tidur.

Jalur suspend-and-resume-nya nyata, bukan dilewati: sebuah
`TaskCompletionSource` yang diselesaikan setelah awaiter-nya menggantung
benar-benar memarkir state machine di heap dan melanjutkannya saat selesai.
Itulah yang diuji conformance check `resumed continuation`.

Biayanya adalah setiap program yang bergantung pada dua task berjalan bersamaan,
dan setiap percepatan wall-clock dari paralelisme. Itu datang bersama
interpreter re-entrant — lihat catatan tentang thread di bawah, penyebabnya sama.

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
yang bisa digerakkan beberapa thread OS sekaligus. Itulah satu-satunya bagian
yang ditunggu baik oleh ini maupun oleh `async`.

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
