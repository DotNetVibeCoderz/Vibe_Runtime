# Documentation

*Dokumentasi RustNetRuntime — [Bahasa Indonesia di bawah](#bahasa-indonesia)*

---

## English

**Start here**

- [Getting started](getting-started.md) — build the runtime, run your first
  program on it, set up CodeGen

**The runtime**

- [Architecture](architecture.md) — how a `.dll` becomes a running program, and
  the decision that makes C# work without CoreLib
- [The runtime in depth](runtime.md) — metadata, the collector, values, calls,
  exceptions, and the behaviour that is easy to get subtly wrong
- [Toolchain reference](cli.md) — every `rustnet` command
- [Limitations](limitations.md) — what does not work yet, and why

**CodeGen**

- [CodeGen guide](codegen.md) — the IDE, Jack's tools, providers, configuration
- [Templates](templates.md) — all fourteen, and how to add one

**Project**

- [Roadmap](../Plan.md) — milestones, in the order that unblocks the most code
- [Progress](../Progress.md) — what is done, with evidence
- [Samples](../samples/README.md) — sample data, sample users, worked examples

---

## Bahasa Indonesia

**Mulai di sini**

- [Memulai](id/memulai.md) — build runtime, jalankan program pertama Anda di
  atasnya, siapkan CodeGen
- [Panduan CodeGen](id/codegen.md) — IDE, alat-alat Jack, penyedia LLM,
  konfigurasi

Dokumen teknis lain — arsitektur, runtime mendalam, rujukan toolchain, dan
batasan — tersedia dalam bahasa Inggris, sejalan dengan komentar di kode:

- [Architecture](architecture.md) · [Runtime](runtime.md) · [CLI](cli.md) ·
  [Limitations](limitations.md) · [Templates](templates.md)

Kalau salah satu di antaranya sering Anda rujuk dan lebih nyaman dalam Bahasa
Indonesia, terjemahannya layak ditambahkan — pola berkasnya sudah ada di `id/`.

---

## Screenshots

Every image under `images/` is rendered from the real application, headlessly:

```bash
dotnet run --project src/CodeGen -c Release -- --screenshot docs/images
```

Re-run it after any UI change. A screenshot that cannot be regenerated goes
stale the first time the layout moves.

| | |
| --- | --- |
| `codegen-main.png` | The three-pane workspace with a project open |
| `codegen-chat.png` | Jack responding, with the tools he used |
| `codegen-new-project.png` | The template picker |
| `codegen-settings.png` | Settings, backed by `app.config` |

---

Built by **Gravicode Studios**, led by **Kang Fadhil**.
