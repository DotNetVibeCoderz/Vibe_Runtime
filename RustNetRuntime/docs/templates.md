# Templates

Fourteen project templates, reachable from **File → New Project** in CodeGen or
from Jack via `list_templates` and `create_project`.

Every template compiles as written. Those marked **RustCLR** also run on the
Rust runtime today. They were written to stay inside the IL subset RustCLR
supported at the time — arrays rather than generic collections, explicit loops
rather than LINQ. Both of those now run (see [Milestone 2](../Plan.md)), so the
constraint on new templates is looser than the existing ones suggest.

---

## Console

| Template | Field | Runs on | What it is |
| --- | --- | --- | --- |
| **Blank** | — | RustCLR | A project file and an entry point. Nothing else. |
| **Invoice Calculator** | Business | RustCLR | Line items, banded tax rates, a printed total. A small billing core. |
| **Numerical Methods** | Science | RustCLR | Bisection root-finding and composite Simpson integration, with a convergence report. |
| **Quiz Engine** | Education | RustCLR | Multiple-choice questions, scoring, per-topic breakdown. Reads from the console. |
| **Text Adventure** | Games | RustCLR | Rooms, exits, an inventory and a win condition. A complete game loop. |

The numerical methods template is a good first thing to run on both runtimes:
it is float-heavy and loop-heavy, so a discrepancy would show up immediately.

---

## Web

| Template | Field | Runs on | What it is |
| --- | --- | --- | --- |
| **Inventory API** | Business | .NET | Minimal API with Swagger over an in-memory stock ledger. Stock adjustments refuse to go negative. |
| **Sensor Telemetry API** | Science | .NET | Ingests readings, returns rolling statistics including a correct sample standard deviation. |
| **Course Catalog API** | Education | .NET | Courses, enrolment and capacity checks. |

Web templates target ASP.NET Core, which RustCLR does not host. They run on
.NET; the marker in the dialog says so rather than letting you find out at run
time.

---

## Desktop

| Template | Field | Runs on | What it is |
| --- | --- | --- | --- |
| **Point of Sale** | Business | .NET | Avalonia till: item entry, running total. |
| **Flashcards** | Education | .NET | Avalonia study app with a cut-down SM-2 spaced-repetition schedule. |

---

## Mobile

| Template | Field | Runs on | What it is |
| --- | --- | --- | --- |
| **Field Logger** | Business | .NET | Touch-first Avalonia layout for site observations. Every touch target is at least 48 px. |

This produces a desktop-hosted Avalonia app sized to a phone viewport. Add the
Avalonia Android or iOS heads to deploy to a device — the template's summary
says this plainly rather than implying a one-click mobile build.

---

## IoT

| Template | Field | Runs on | What it is |
| --- | --- | --- | --- |
| **Sensor Gateway** | Science | RustCLR | Polls sensors, applies offset-and-scale calibration, batches readings and publishes a mean. |
| **Thermostat Controller** | Business | RustCLR | A hysteresis control loop with a minimum dwell time, so the relay does not chatter around the setpoint. |

Both are written for the constrained end: fixed-size arrays, no allocation in
the control loop, no generic collections. They are the templates to reach for
when targeting microcontrollers.

---

## Library

| Template | Field | Runs on | What it is |
| --- | --- | --- | --- |
| **Class Library** | Runtime | RustCLR | A reusable library with a worked example — `Add` and a `Factorial` that throws on negative input. |

---

## How substitution works

Two placeholders are replaced when a template is written out:

| | |
| --- | --- |
| `{NAME}` | The project name, sanitised — invalid path characters removed |
| `{NAMESPACE}` | The same name as a valid C# namespace — segments capitalised, a leading digit prefixed with `_` |

So a project called `my-sensor-app` produces `my-sensor-app.csproj` and
`namespace MySensorApp;`.

---

## Adding a template

Templates live in `src/CodeGen/Services/TemplateCatalog.cs` as data — an id, a
name, a summary, a category, a field, and a list of files with their contents.
Add an entry to `BuildAll()` and it appears in the dialog and in Jack's
`list_templates` output.

Set `RunsOnRustClr = false` if the template uses framework surface RustCLR does
not implement. The dialog shows that marker, and it is better to be honest in
the picker than to have the run fail later.
