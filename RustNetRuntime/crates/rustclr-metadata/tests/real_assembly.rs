//! End-to-end checks against a real assembly produced by the C# compiler.
//!
//! The fixture is built by `tests/fixtures/HelloWorld`. If it has not been
//! built the tests skip rather than fail, so `cargo test` works on a clean
//! checkout without the .NET SDK.

use rustclr_metadata::{Image, TableId, SignatureParser, TypeSig};

fn fixture() -> Option<Image> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/HelloWorld/bin/Release/net9.0/HelloWorld.dll"
    );
    if !std::path::Path::new(path).exists() {
        eprintln!("skipping: fixture not built ({path})");
        return None;
    }
    Some(Image::from_file(path).expect("fixture should parse"))
}

macro_rules! image_or_skip {
    () => {
        match fixture() {
            Some(i) => i,
            None => return,
        }
    };
}

#[test]
fn parses_the_pe_container() {
    let image = image_or_skip!();
    let pe = image.pe();
    assert!(pe.is_il_only(), "C# output should be IL-only");
    assert!(!pe.sections.is_empty());
    assert!(pe.sections.iter().any(|s| s.name_str() == ".text"));
    assert_ne!(pe.cli_header.metadata.rva, 0);
}

#[test]
fn reads_the_assembly_identity() {
    let image = image_or_skip!();
    assert_eq!(image.assembly_name(), "HelloWorld");
    let md = image.metadata();
    assert!(md.version.starts_with("v4.0.30319"), "got {}", md.version);
    let asm = md.assembly(1).unwrap();
    assert_eq!(asm.name, "HelloWorld");
}

#[test]
fn finds_the_program_type_and_its_methods() {
    let image = image_or_skip!();
    let md = image.metadata();

    let mut program_row = None;
    for row in 1..=md.row_count(TableId::TypeDef) {
        let td = md.type_def(row).unwrap();
        if td.name == "Program" {
            program_row = Some(row);
            assert_eq!(td.namespace, "HelloWorld");
        }
    }
    let program_row = program_row.expect("Program type should exist");

    let methods: Vec<String> = md
        .methods_of(program_row)
        .unwrap()
        .map(|m| md.method_def(m).unwrap().name.to_string())
        .collect();

    for expected in ["Add", "Factorial", "Main"] {
        assert!(methods.contains(&expected.to_string()), "missing {expected} in {methods:?}");
    }
}

#[test]
fn decodes_a_method_signature() {
    let image = image_or_skip!();
    let md = image.metadata();

    let add = (1..=md.row_count(TableId::MethodDef))
        .map(|r| md.method_def(r).unwrap())
        .find(|m| m.name == "Add")
        .expect("Add should exist");

    let sig = SignatureParser::new(add.signature).parse_method().unwrap();
    assert!(!sig.has_this, "Add is static");
    assert_eq!(sig.return_type, TypeSig::I4);
    assert_eq!(sig.params, vec![TypeSig::I4, TypeSig::I4]);
}

#[test]
fn reads_a_method_body() {
    let image = image_or_skip!();
    let md = image.metadata();

    let add_row = (1..=md.row_count(TableId::MethodDef))
        .find(|r| md.method_def(*r).unwrap().name == "Add")
        .expect("Add should exist");

    let body = image.method_body(add_row).unwrap().expect("Add has a body");
    // ldarg.0; ldarg.1; add; ret
    assert_eq!(body.il, &[0x02, 0x03, 0x58, 0x2A]);
}

#[test]
fn resolves_the_entry_point_to_main() {
    let image = image_or_skip!();
    let token = image.entry_point().expect("executable should have an entry point");
    assert_eq!(token.table(), Some(TableId::MethodDef));
    let method = image.metadata().method_def(token.row()).unwrap();
    assert_eq!(method.name, "Main");
}

#[test]
fn lists_referenced_assemblies() {
    let image = image_or_skip!();
    let md = image.metadata();
    let refs: Vec<String> = (1..=md.row_count(TableId::AssemblyRef))
        .map(|r| md.assembly_ref(r).unwrap().name.to_string())
        .collect();
    assert!(
        refs.iter().any(|r| r == "System.Runtime" || r == "System.Console"),
        "expected a framework reference, got {refs:?}"
    );
}

#[test]
fn every_type_row_decodes_without_error() {
    let image = image_or_skip!();
    let md = image.metadata();
    for row in 1..=md.row_count(TableId::TypeDef) {
        let td = md.type_def(row).expect("type row decodes");
        assert!(!td.name.is_empty());
        for f in md.fields_of(row).unwrap() {
            md.field(f).expect("field row decodes");
        }
        for m in md.methods_of(row).unwrap() {
            let method = md.method_def(m).expect("method row decodes");
            SignatureParser::new(method.signature)
                .parse_method()
                .unwrap_or_else(|e| panic!("signature of {} failed: {e}", method.name));
        }
    }
}

#[test]
fn every_user_string_decodes() {
    let image = image_or_skip!();
    let md = image.metadata();
    // The literal from Console.WriteLine must be reachable in the #US heap.
    let mut found = false;
    let mut offset = 1u32;
    while let Ok(s) = md.user_string(offset) {
        if s == "Hello from RustCLR" {
            found = true;
            break;
        }
        if s.is_empty() {
            break;
        }
        // Advance past this entry: compressed length prefix + payload.
        offset += (s.len() as u32 * 2) + 2;
    }
    assert!(found, "expected the greeting literal in the #US heap");
}
