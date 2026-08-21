# Memulai

*[English →](../getting-started.md)*

---

## Yang dibutuhkan

| | |
| --- | --- |
| Rust | 1.85 atau lebih baru (disarankan lewat `rustup`) |
| .NET SDK | 10 — RustCLR mengonsumsi IL, jadi Roslyn tetap diperlukan untuk mengompilasi C# |

Periksa keduanya:

```bash
cargo --version
dotnet --version
```

---

## Build runtime

```bash
cargo build --release
```

Perintah itu menghasilkan `target/release/rustnet`, biner toolchain-nya.
Letakkan di PATH Anda, atau gunakan path lengkapnya pada perintah di bawah.

Jalankan suite test untuk memastikan hasil build-nya sehat:

```bash
cargo test --workspace
```

Anda mestinya melihat 141 test lulus.

---

## Menjalankan program pertama di RustCLR

Repositori ini menyertakan fixture kecil:

```bash
cd tests/fixtures/HelloWorld
dotnet build -c Release
rustnet run bin/Release/net10.0/HelloWorld.dll
```

```
Hello from RustCLR
42
120
```

Keluaran itu berasal dari runtime Rust yang mengeksekusi IL hasil Roslyn.
Tambahkan `--stats` untuk melihat ongkosnya:

```bash
rustnet run bin/Release/net10.0/HelloWorld.dll --stats
```

---

## Menjalankan suite konformans

Ini bagian yang menarik — assembly yang sama di dua runtime:

```bash
cd tests/fixtures/Conformance
dotnet build -c Release

dotnet bin/Release/net10.0/Conformance.dll
rustnet run bin/Release/net10.0/Conformance.dll
```

Keduanya mencetak `checks=285 failures=0`. Kalau suatu saat berbeda, itu bug
runtime — dan pemeriksaan yang berbeda itu langsung menunjuk penyebabnya.
`tests/fixtures/ModernSyntax/` melakukan hal serupa untuk fitur C# modern dan
mencetak `checks=35 failures=0`.

---

## Menjalankan program Anda sendiri

Proyek C# apa pun yang berada dalam subset yang didukung akan berjalan. Cara
tercepat mengetahuinya adalah bertanya:

```bash
cd path/ke/proyek-anda
dotnet build -c Release
rustnet verify bin/Release/net10.0/AplikasiAnda.dll
```

`verify` mencantumkan setiap anggota yang dirujuk program Anda tetapi belum bisa
disediakan RustCLR, serta setiap method yang IL-nya gagal verifikasi. Laporan
bersih berarti program itu semestinya jalan:

```bash
rustnet run bin/Release/net10.0/AplikasiAnda.dll
```

Kalau `verify` melaporkan anggota framework yang hilang, itu wajar untuk apa pun
yang memakai `Span<T>`, TPL, atau tipe framework yang belum diimplementasikan
RustBCL — lihat [limitations.md](../limitations.md). Koleksi generic, LINQ, dan
`async`/`await` sudah jalan.

---

## Menjalankan CodeGen

```bash
dotnet run --project src/CodeGen
```

Saat pertama dibuka belum ada proyek dan Jack masih tidur — ia butuh penyedia
LLM.

**Membangunkan Jack.** Buka **Edit → Settings → Providers** lalu isi salah satu:

| Penyedia | Yang diisi |
| --- | --- |
| Claude | API key dari console.anthropic.com |
| OpenAI | API key dari platform.openai.com |
| Gemini | API key dari aistudio.google.com |
| Ollama | Tanpa key. Arahkan endpoint ke server lokal Anda, misalnya `http://localhost:11434/v1`, lalu pilih model yang sudah Anda unduh |

Pilih penyedia aktif di bagian atas panel chat; daftar modelnya menyesuaikan.

**Opsional: pencarian web.** Tambahkan API key Tavily di **Settings → Tools**
agar Jack punya `search_internet` dan `scrape_web_page`.

**Opsional: arahkan ke toolchain.** CodeGen mencari `rustnet` di
`target/release`, lalu `target/debug`, lalu PATH. Kalau milik Anda ada di tempat
lain, isi **Settings → Toolchain → rustnet path**.

Semua ini tersimpan di `app.config` di sebelah executable. Anda boleh menyunting
berkas itu langsung kalau lebih suka; dialog dan berkas itu adalah penyimpanan
yang sama.

---

## Membuat sesuatu

**File → New Project** menawarkan proyek console kosong atau salah satu dari
empat belas template. Pilih *Sensor Gateway* di kategori IoT — template itu
ditulis agar tetap berada dalam subset IL yang dijalankan RustCLR.

Lalu, di panel chat:

> Tambahkan rolling median di samping mean, lalu jalankan di RustCLR.

Jack akan membaca `Gateway.cs`, menyuntingnya, melakukan build, dan
menjalankannya. Alat yang ia pakai tercantum di bawah balasannya, keluaran build
muncul di panel log, dan status bar mengambil penghitung runtime dari eksekusi
tersebut.

---

## Pintasan papan ketik

| | |
| --- | --- |
| `Ctrl+S` / `Ctrl+Shift+S` | Simpan / Simpan semua |
| `Ctrl+G` | Lompat ke baris |
| `Ctrl+K` | Rapikan format kode |
| `Ctrl+B` | Build |
| `F5` / `Ctrl+F5` | Jalankan di .NET / Jalankan di RustCLR |
| `Ctrl+J` | Tampilkan atau sembunyikan panel chat |
| `Ctrl+Enter` | Kirim pesan ke Jack |

---

## Selanjutnya

- [Panduan CodeGen](codegen.md) — IDE secara rinci
- [Rujukan toolchain](../cli.md) — setiap perintah `rustnet`
- [Arsitektur](../architecture.md) — bagaimana runtime disusun
- [Fitur lanjutan C#](fitur-lanjutan.md) — matriks dukungan hasil pengukuran
- [Batasan](../limitations.md) — apa yang belum jalan, dan alasannya
