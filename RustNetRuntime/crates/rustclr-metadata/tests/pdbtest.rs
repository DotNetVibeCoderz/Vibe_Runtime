//! Reading the portable PDB the .NET SDK produced beside the fixture.
//!
//! Decoding sequence points is all deltas — line, column and IL offset are each
//! relative to the point before — so a decoder that is subtly wrong still
//! produces plausible-looking numbers. These check the decoded lines against
//! the *source file*, which is the only independent thing to check them against.

use rustclr_metadata::pdb::Pdb;

const PDB: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/Conformance/bin/Release/net10.0/Conformance.pdb"
);
const SOURCE: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/Conformance/Program.cs");

fn pdb() -> Option<Pdb> {
    if !std::path::Path::new(PDB).exists() {
        eprintln!("skipping: conformance fixture not built");
        return None;
    }
    Some(Pdb::parse(&std::fs::read(PDB).unwrap()).expect("the SDK's PDB parses"))
}

#[test]
fn documents_name_the_source_files() {
    let Some(pdb) = pdb() else { return };
    assert!(!pdb.documents.is_empty(), "a PDB with no documents describes nothing");
    assert!(
        pdb.documents.iter().any(|d| d.name.ends_with("Program.cs")),
        "no document named Program.cs: {:?}",
        pdb.documents.iter().map(|d| &d.name).collect::<Vec<_>>()
    );
    // The name blob is a separator byte plus parts; a decoder that dropped the
    // separator would produce a run-together path.
    let program = pdb.documents.iter().find(|d| d.name.ends_with("Program.cs")).unwrap();
    assert!(
        program.name.contains('\\') || program.name.contains('/'),
        "the document name lost its separators: {}",
        program.name
    );
}

#[test]
fn sequence_points_land_on_the_lines_they_claim() {
    let Some(pdb) = pdb() else { return };
    let source: Vec<String> =
        std::fs::read_to_string(SOURCE).unwrap().lines().map(String::from).collect();

    // Every visible sequence point must name a line that exists, and the line
    // must not be blank — the compiler does not emit code for nothing. This is
    // the check that catches a delta decoder that has drifted: a wrong line is
    // usually a blank one or past the end of the file.
    let mut checked = 0usize;
    for row in 1..4000u32 {
        let Some(debug) = pdb.method(row) else { continue };
        for point in &debug.sequence_points {
            let Some(line) = point.start_line else { continue };
            let Some(document) = pdb.document(point.document) else { continue };
            if !document.name.ends_with("Program.cs") {
                continue;
            }
            assert!(
                line as usize <= source.len(),
                "sequence point claims line {line}, but the file has {} lines",
                source.len()
            );
            let text = source[line as usize - 1].trim();
            assert!(
                !text.is_empty(),
                "sequence point claims line {line}, which is blank — the deltas have drifted"
            );
            checked += 1;
        }
    }
    assert!(checked > 500, "only {checked} points checked; the fixture has far more");
}

#[test]
fn a_known_method_maps_to_its_own_source() {
    let Some(pdb) = pdb() else { return };
    let source = std::fs::read_to_string(SOURCE).unwrap();
    let lines: Vec<&str> = source.lines().collect();

    // `Triple` is one line: `static int Triple(int v) { return v * 3 + (v >> 1); }`
    let declared = lines
        .iter()
        .position(|l| l.contains("static int Triple(int v)"))
        .expect("the fixture defines Triple") as u32
        + 1;

    // Find the method whose points all sit within a couple of lines of that
    // declaration. Row-to-name needs the assembly, which this test does not
    // load, so it identifies the method by where its code is instead.
    let found = (1..4000u32).find(|row| {
        pdb.method(*row).is_some_and(|d| {
            !d.sequence_points.is_empty()
                && d.sequence_points.iter().all(|p| {
                    p.start_line.is_none_or(|l| l >= declared && l <= declared + 2)
                })
        })
    });
    let debug = pdb.method(found.expect("a method sitting on Triple's lines")).unwrap();

    // Its first instruction maps to a line, and asking that line back gives an
    // offset — the round trip a breakpoint depends on.
    let first = debug.sequence_points[0];
    let (document, line) = debug.line_for(first.il_offset).expect("offset 0 has a line");
    assert_eq!(
        debug.offset_for_line(document, line),
        Some(first.il_offset),
        "line and offset do not round-trip"
    );
}

#[test]
fn locals_are_named() {
    let Some(pdb) = pdb() else { return };
    let mut named = 0usize;
    let mut seen: Vec<String> = Vec::new();
    for row in 1..4000u32 {
        if let Some(debug) = pdb.method(row) {
            for local in &debug.locals {
                assert!(!local.name.is_empty(), "a local was named with an empty string");
                seen.push(local.name.clone());
                named += 1;
            }
        }
    }
    assert!(named > 100, "only {named} named locals; the fixture has far more");
    // Names the source actually uses as *locals*, so this cannot pass on
    // garbage that happens to be non-empty. `checks` and `failures` look like
    // obvious candidates and are static fields, which a PDB never names here —
    // the first version of this test asserted on them and failed for that
    // reason rather than for a decoding one.
    for expected in ["total", "carried", "celsius"] {
        assert!(
            seen.iter().any(|n| n == expected),
            "no local named `{expected}` among {named}"
        );
    }
}

#[test]
fn a_line_with_no_code_has_no_offset() {
    let Some(pdb) = pdb() else { return };
    // Line 1 of the fixture is a `using` directive or a comment; either way the
    // compiler emits no code for it. Answering `None` rather than the nearest
    // line is what stops a breakpoint landing somewhere the user did not ask.
    let program = pdb
        .document_by_suffix("Program.cs")
        .expect("Program.cs resolves unambiguously");
    let any_at_line_one = (1..4000u32)
        .filter_map(|row| pdb.method(row))
        .any(|d| d.offset_for_line(program, 1).is_some());
    assert!(!any_at_line_one, "something claimed to have code on line 1");
}

#[test]
fn a_document_is_found_by_the_tail_of_its_path() {
    let Some(pdb) = pdb() else { return };
    // An editor sends the path it knows; the compiler recorded the path the
    // build machine knew. Matching on the tail is what bridges them.
    assert!(pdb.document_by_suffix("Program.cs").is_some());
    assert!(pdb.document_by_suffix("Conformance/Program.cs").is_some());
    assert!(
        pdb.document_by_suffix("NoSuchFile.cs").is_none(),
        "a file that is not there must not resolve to one that is"
    );
}
