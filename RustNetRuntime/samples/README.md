# Samples

Sample data and worked examples for RustNetRuntime.

---

## Data

`samples/data/` holds realistic fixtures you can point templates and experiments
at. Names and email addresses are fictional; the domain is `example`, which is
reserved and cannot resolve.

| File | Rows | What it is |
| --- | --- | --- |
| `users.json` | 15 | The sample user directory — id, username, display name, role, team, email, active flag, join date |
| `users.csv` | 15 | The same directory as CSV, for templates that parse flat files |
| `products.csv` | 12 | A stock ledger: SKU, name, category, quantity on hand, reorder point, unit price in rupiah |
| `sensor-readings.csv` | 12 | Raw and calibrated readings from two sensors, five seconds apart |
| `courses.json` | 5 | Courses with capacity and enrolment, matching the Course Catalog template |

The users span eight roles across six teams, with two suspended accounts and
join dates in both 2024 and 2025 — enough variety that grouping, filtering and
date arithmetic all have something to report.

---

## UserDirectory

A console program that reports over the sample users: totals, counts by team and
by role, who joined in 2025, and which accounts are suspended.

```bash
cd samples/UserDirectory
dotnet build -c Release

dotnet bin/Release/net10.0/UserDirectory.dll
rustnet run bin/Release/net10.0/UserDirectory.dll
```

**The output is byte-for-byte identical on both runtimes.** That is the point of
the sample — it is small enough to read in one sitting and exercises strings,
arrays, object fields, boolean logic and integer parsing, all of which are
places a runtime can quietly disagree.

```
GRAVICODE STUDIOS - USER DIRECTORY
==================================
Total accounts: 15
Active:         13
Suspended:      2

BY TEAM
  Runtime     4
  Tooling     3
  Quality     2
  Product     3
  Embedded    2
  Platform    1
...
```

### Why the data is embedded

The rows live in `Users.cs` rather than being read from `data/users.json`.
RustBCL does not implement `System.IO` yet, so a sample that read from disk
would run on .NET and fail on RustCLR — which would make it useless as a
comparison. The JSON and CSV files are there for templates and tools that do
have file access.

### Why no LINQ

The program uses explicit loops and arrays because, when it was written, generic
collections and LINQ did not run on RustCLR. **They do now** — see
[Milestone 2](../Plan.md) — so the constraint no longer applies, and the
comments in the source marking "LINQ would be the obvious choice here" are a
record of what changed rather than a live restriction.

The sample is left as it is on purpose. It exercises strings, arrays, object
fields, boolean logic and integer parsing directly, which is exactly what makes
it useful as a byte-for-byte comparison between the two runtimes; rewriting it
in LINQ would move that coverage into the LINQ implementation and out of the
sample. A LINQ variant alongside it is the better addition.

---

## Using the data with templates

The **Inventory API** template (web) and **Course Catalog API** template seed
themselves with a couple of rows. Point them at `products.csv` and
`courses.json` for a fuller dataset — they run on .NET, where file access works.

The **Sensor Gateway** template (IoT) generates its own readings in a loop.
`sensor-readings.csv` holds the calibrated values that generator produces, so
you can check the calibration arithmetic independently.

---

## Adding a sample

A good sample here does one thing: it makes some runtime behaviour observable.
If it cannot run on RustCLR, say so in this file and explain what it needs —
that is more useful than quietly shipping something that only works on .NET.
