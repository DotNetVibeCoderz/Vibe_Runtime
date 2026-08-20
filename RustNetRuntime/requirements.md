name: RustNetRuntime
goal: C# dengan Low Level Runtime dengan Rust
deskripsi: membuat C# dengan low-level runtime berbasis Rust (bukan C++), re-implementasi CLR (Common Language Runtime) dan ekosistem dasar .NET dengan Rust sebagai fondasi. Jadi bukan sekadar porting, tapi mendefinisikan ulang runtime agar tetap kompatibel dengan C# sambil memanfaatkan safety dan concurrency model dari Rust.

---

 🧩 Layer Requirement

- Core Runtime  
  - Garbage Collector (GC) ditulis dengan Rust, memanfaatkan ownership model untuk safety.  
  - JIT/AOT compiler interface: bridging IL → native code.  
  - Threading, async/await scheduler, dan task runtime.  
  - Memory safety + concurrency primitives (channel, lock-free queue).  

- Interop Layer  
  - P/Invoke dan FFI ke native library.  
  - ABI compatibility dengan OS target (Windows, Linux, macOS).  
  - Safe wrapper untuk pointer dan handle.  

- Base Class Library  
  - Koleksi (List, Dictionary, Queue) dengan Rust generics.  
  - IO (File, Stream, Network) dengan async Rust.  
  - Numerics, DateTime, Regex, dll.  

- Language Integration  
  - Compiler C# output → IL → Rust runtime loader.  
  - Metadata reader (assembly, reflection).  
  - Roslyn integration untuk tooling.  

- Toolchain  
  - CLI untuk build, run, deploy.  
  - Debugger hooks (VSCode, Rider).  
  - Profiler + diagnostics (perf, memory).  

---

 🚀 Strategic Goals

- Safety-first runtime: Rust menggantikan C++ untuk mengurangi bug memory/segfault.  
- Cross-platform: Target OS + microcontroller (ESP32, STM32, RISC-V).  
- Extensible: Modular runtime (GC bisa diganti, scheduler bisa di-tweak).  
- Interop-friendly: Tetap bisa panggil library C/C++ bila perlu.  
- Backward compatibility: C# code tetap jalan tanpa modifikasi besar.  

---

 📌 Deliverables

- RustCLR → runtime inti.  
- RustBCL → library dasar.  
- RustNet Toolchain → compiler, CLI, debugger.  
- Interop Bridge → FFI layer.  
- Testing Suite → unit test + stress test.  

---
Tools

- Tambahkan tools berupa aplikasi dengan Avalonia UI Bernama CodeGen bentuknya seperti code editor yang memiliki fungsi generate app with prompt dengan bantuan LLM menggunakan library semantic kernel, LLM yang disupport: OpenAI, Claude, Gemini, Ollama, settingnya (model, api key, endpoint, temperature, system prompt) disimpan di app.config. 
- Nama AI Assistant: Jack - The Code Bender
- Buatkan kernel functions yang diperlukan agar assisten AI-nya bisa membuatkan aplikasi dengan benar baik UI dan Backend Code-nya, kasih common functions juga untuk SearchInternet (tavily), ScrapeWebPage, MathCalculation, Check Date and Time, dan fungsi lain yang diperlukan untuk membuat project, membuatkan/mengubah code, menjalankan aplikasi, debug, compile. 
- Panel chat ada di sebelah kanan code editor, bisa attach gambar, bisa di resize width-nya dan hide/show, send chat bisa dengan Ctrl+Enter atau klik button send, ada button untuk clear chat thread, Model LLM bisa dipilih dibagian atas Chat Panel 
- Di tengah ada code editor, lengkap dengan line number, code highlight
- Di panel kiri ada code explorer seperti VSCode
- Pada menu dan toolbar terdapat fungsi: New Project (Folder), Open Project/File, Close Project, Go To Line Number, Format Code, Build, Run, Deploy, Exit. 
- Create new project ada 2 pilihan: Blank dan From Template (buatkan berbagai template jenis aplikasi (web, desktop multi-platform, console, mobile, IoT) dengan use case bermacam-macam dari bisnis/industri, science, edukasi sampai games). 
- Terdapat status bar dan logs panel di bagian bawah untuk memantau proses dan output. 
- Show/hide line number pada code editor. 
- Buatkan dengan UI dan UX modern dengan skill frontend-design. Semua konfigurasi disimpan di app.config dan bisa di ubah di UI. 
---
Summary:
RustCLR sebagai pengganti CoreCLR. Jadi C# tetap jadi bahasa utama, tapi runtime-nya full Rust.  
---
Notes:
- Tarik kode existing yang bisa digunakan dari official repo C#, tulis dan modifikasi untuk layer bawah dengan Rust, sehingga tetap bisa kompatibel dengan ekosistem yang sudah ada
- Tambahkan readme.md (English dan Bahasa Indonesia), sertakan screenshot
- Tambahkan dokumentasi lengkap di folder docs (sertakan screenshot)
- Buatkan banyak sample data, dan user
- optimasi kode agar aplikasi cepat, hemat memory dan ringan
- Tambahkan info di dokumentasi dan app: dibuat oleh Gravicode Studios dipimpin oleh Kang Fadhil
- Plan.md untuk roadmap pengembangan, Progress.md untuk tracking development
- Support: x86, x64, Arm, Arm64, RiscV