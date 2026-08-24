//! Command implementations.

use rustclr_core::{
    metadata::{Machine, TableId},
    opcode::{decode_all, Operand},
    ExecutionError, Interpreter, MethodKind, SystemHost, TypeKind,
};
use std::error::Error;
use std::path::Path;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// Installs the board `RustNet.Hal` will talk to.
///
/// Named rather than detected: which board is underneath is a fact about the
/// hardware, and a runtime that guessed would drive the wrong pins. An
/// unknown name lists what there is instead of falling back to the simulator,
/// because a program that quietly ran against a simulation when it was meant
/// to drive a relay has done something worse than fail.
fn install_board(interp: &mut Interpreter, name: &str) -> Result<()> {
    match name {
        "host" | "sim" | "simulator" => {
            interp.board = Some(Box::new(rustclr_hal_host::HostBoard::new()));
            Ok(())
        }
        "none" => Ok(()),
        other => Err(format!(
            "unknown board `{other}`. This build has: host (the simulator), none.
A real chip installs its own board from its firmware, not from this flag."
        )
        .into()),
    }
}

/// Builds an interpreter with the full runtime installed.
fn runtime(args: Vec<String>) -> Interpreter {
    runtime_with_bcl(args, false)
}

/// The runtime, with either the whole BCL or the subset a small board can hold.
///
/// `minimal` is not a smaller *runtime* — the loader and interpreter are
/// identical. It registers 314 native bindings instead of 826, which is what a
/// board with 192 KB of RAM can afford. Running a program this way on a desktop
/// is how to find out it calls `List<T>` before flashing it to something that
/// cannot answer.
fn runtime_with_bcl(args: Vec<String>, minimal: bool) -> Interpreter {
    let mut interp = Interpreter::with_host(Box::new(SystemHost::with_args(args)));
    if minimal {
        rustclr_bcl::install_minimal(&mut interp);
    } else {
        rustclr_bcl::install(&mut interp);
    }
    interp
}

#[allow(clippy::too_many_arguments)]
/// Write the canvas to `path` as a PNG, or say why there is nothing to write.
///
/// The *canvas*, not the board's panel. Drawing needs no board — that is how a
/// device UI is developed headlessly — so this captures what the program drew
/// whether or not a panel was ever attached, which is exactly what a
/// documentation screenshot wants.
fn write_display_png(interp: &Interpreter, path: &str) -> Result<(u32, u32)> {
    let Some(canvas) = interp.canvas.as_ref() else {
        // Not an error the run fails on, but not silence either: a screenshot
        // that did not happen should say so rather than leave a stale file
        // from a previous run looking current.
        return Err(
            "the program never opened a display, so there is no screen to capture".into(),
        );
    };
    let png = crate::png::rgb565_to_png(&canvas.pixels, canvas.width, canvas.height);
    std::fs::write(path, png).map_err(|e| format!("could not write {path}: {e}"))?;
    Ok((canvas.width, canvas.height))
}

pub fn run(
    assembly: &str,
    args: Vec<String>,
    show_stats: bool,
    trace: bool,
    max_instructions: Option<u64>,
    jit: bool,
    jit_threshold: Option<u32>,
    inline: bool,
    minimal_bcl: bool,
    board: Option<&str>,
    capture_display: Option<&str>,
) -> Result<i32> {
    let mut interp = runtime_with_bcl(args, minimal_bcl);
    if let Some(name) = board {
        install_board(&mut interp, name)?;
    }
    if minimal_bcl {
        eprintln!(
            "[rustnet] --bcl minimal: {} native bindings, the set a 192 KB board can hold",
            interp.native_count()
        );
    }
    if let Some(budget) = max_instructions {
        interp.limits.max_instructions = Some(budget);
    }
    if jit {
        // Methods the backend declines keep running in the interpreter, so
        // enabling this can only change how fast a program runs, never what it
        // prints. `--no-jit` exists to make that checkable.
        let mut tier = match jit_threshold {
            Some(t) => rustclr_jit::JitTier::with_threshold(t),
            None => rustclr_jit::JitTier::new(),
        };
        // Same reasoning as above one level down: inlining widens what the
        // backend accepts without changing any answer, and `--no-inline` is how
        // that claim gets checked rather than asserted.
        tier.set_inline(inline);
        interp.native_tier = Some(Box::new(tier));
    }

    let id = interp.loader.load_from_file(assembly)?;
    // Native declarations can only be wired once the assembly is loaded.
    let pinvokes = rustclr_interop::install(&mut interp);
    if trace {
        eprintln!(
            "[rustnet] loaded {} ({} types, {} methods, {} P/Invoke declarations)",
            interp.loader.assembly(id).name,
            interp.loader.registry.type_count(),
            interp.loader.registry.method_count(),
            pinvokes
        );
    }

    let started = std::time::Instant::now();
    let outcome = interp.run_entry_point(id);
    let elapsed = started.elapsed();

    // Before the outcome is examined, deliberately: a program that ended with
    // an exception has still drawn something, and what was on screen when it
    // failed is usually the most useful thing about it. Paired with
    // `--max-instructions`, this is also how a program that loops forever —
    // which is most device programs — gets photographed at all.
    if let Some(path) = capture_display {
        match write_display_png(&interp, path) {
            Ok((w, h)) => eprintln!("[rustnet] wrote {path} ({w}x{h})"),
            Err(e) => eprintln!("[rustnet] --capture-display: {e}"),
        }
    }

    let exit_code = match outcome {
        Ok(code) => code,
        Err(e) => {
            report_failure(&interp, &e);
            if show_stats {
                print_stats(&interp, elapsed);
            }
            return Ok(1);
        }
    };

    if show_stats {
        print_stats(&interp, elapsed);
    }
    Ok(exit_code)
}

/// Prints an unhandled runtime failure the way a .NET host would.
fn report_failure(interp: &Interpreter, error: &ExecutionError) {
    eprintln!("\nUnhandled exception. {error}");

    if let ExecutionError::Exception { object, .. } = error {
        let trace = interp
            .heap
            .with::<rustclr_core::ClrException, _>(*object, |e| e.stack_trace.clone());
        if let Some(frames) = trace {
            for frame in &frames {
                eprintln!("{frame}");
            }
            return;
        }
    }
    for frame in interp.stack_trace() {
        eprintln!("{frame}");
    }
}

fn print_stats(interp: &Interpreter, elapsed: std::time::Duration) {
    let s = interp.stats;
    let heap = interp.heap.stats();
    let ips = if elapsed.as_secs_f64() > 0.0 {
        s.instructions_executed as f64 / elapsed.as_secs_f64()
    } else {
        0.0
    };

    eprintln!("\n─── execution ──────────────────────────────");
    eprintln!("  wall clock          {:>14.3} ms", elapsed.as_secs_f64() * 1000.0);
    eprintln!("  IL instructions     {:>14}", s.instructions_executed);
    eprintln!("  throughput          {:>14.0} instr/s", ips);
    eprintln!("  managed calls       {:>14}", s.calls);
    eprintln!("  native calls        {:>14}", s.native_calls);
    if let Some(tier) = interp.native_tier.as_ref() {
        let (methods, bytes) = tier.stats();
        eprintln!("  compiled calls      {:>14}", s.native_tier_calls);
        eprintln!("  methods compiled    {:>14}", methods);
        eprintln!("  code emitted        {:>14} bytes", bytes);
    }
    eprintln!("  peak frame depth    {:>14}", s.max_frame_depth);
    eprintln!("─── heap ───────────────────────────────────");
    eprintln!("  collector           {:>14}", interp.heap.collector_name());
    eprintln!("  allocations         {:>14}", heap.total_allocations);
    eprintln!("  bytes allocated     {:>14}", heap.total_bytes_allocated);
    eprintln!("  collections         {:>14}", heap.collections);
    eprintln!("  objects reclaimed   {:>14}", heap.total_objects_freed);
    eprintln!("  live objects        {:>14}", interp.heap.live_count());
    eprintln!("  live bytes          {:>14}", interp.heap.live_bytes());
    eprintln!("  peak live bytes     {:>14}", heap.peak_live_bytes);
}

pub fn info(assembly: &str, verbose: bool) -> Result<i32> {
    let image = rustclr_core::metadata::Image::from_file(assembly)?;
    let pe = image.pe();
    let md = image.metadata();

    println!("Assembly     {}", image.assembly_name());
    if md.row_count(TableId::Assembly) > 0 {
        let a = md.assembly(1)?;
        println!("Version      {}", a.version_string());
        if !a.culture.is_empty() {
            println!("Culture      {}", a.culture);
        }
    }
    println!("Runtime      {}", md.version);
    println!(
        "Machine      {} ({})",
        pe.machine.name(),
        if pe.is_pe32_plus { "PE32+" } else { "PE32" }
    );
    println!("IL only      {}", if pe.is_il_only() { "yes" } else { "no" });
    println!(
        "Entry point  {}",
        match image.entry_point() {
            Some(t) => md
                .method_def(t.row())
                .map(|m| m.name.to_string())
                .unwrap_or_else(|_| t.to_string()),
            None => "(library)".into(),
        }
    );

    println!("\nMetadata tables");
    for (label, table) in [
        ("types", TableId::TypeDef),
        ("methods", TableId::MethodDef),
        ("fields", TableId::Field),
        ("properties", TableId::Property),
        ("events", TableId::Event),
        ("type refs", TableId::TypeRef),
        ("member refs", TableId::MemberRef),
        ("assembly refs", TableId::AssemblyRef),
        ("P/Invoke", TableId::ImplMap),
    ] {
        println!("  {label:<14} {}", md.row_count(table));
    }

    if md.row_count(TableId::AssemblyRef) > 0 {
        println!("\nReferences");
        for row in 1..=md.row_count(TableId::AssemblyRef) {
            let r = md.assembly_ref(row)?;
            println!("  {} {}", r.name, r.version_string());
        }
    }

    if verbose {
        println!("\nTypes");
        for row in 1..=md.row_count(TableId::TypeDef) {
            let t = md.type_def(row)?;
            let kind = if t.is_interface() {
                "interface"
            } else if t.is_abstract() {
                "abstract"
            } else if t.is_sealed() {
                "sealed"
            } else {
                "class"
            };
            println!("  [{kind}] {}", t.full_name());
            for m in md.methods_of(row)? {
                if m > md.row_count(TableId::MethodDef) {
                    break;
                }
                let method = md.method_def(m)?;
                let modifier = if method.is_static() { "static " } else { "" };
                println!("      {modifier}{}", method.name);
            }
        }
    }

    Ok(0)
}

pub fn disasm(assembly: &str, filter: Option<&str>) -> Result<i32> {
    let mut interp = runtime(Vec::new());
    let id = interp.loader.load_from_file(assembly)?;

    let mut printed = 0usize;
    let types: Vec<_> = interp
        .loader
        .registry
        .iter_types()
        .filter(|t| t.assembly == id)
        .map(|t| (t.id, t.full_name()))
        .collect();

    for (type_id, type_name) in types {
        let methods = interp.loader.registry.ty(type_id).methods.clone();
        for method_id in methods {
            let info = interp.loader.registry.method(method_id);
            let qualified = format!("{type_name}.{}", info.name);
            if let Some(f) = filter {
                if !qualified.to_lowercase().contains(&f.to_lowercase()) {
                    continue;
                }
            }

            let MethodKind::Il(body) = &info.kind else {
                println!("\n.method {qualified}  // {}", describe_kind(&info.kind));
                printed += 1;
                continue;
            };

            println!(
                "\n.method {qualified}  // maxstack {}, {} locals, {} EH clauses",
                body.max_stack,
                body.locals.len(),
                body.exception_clauses.len()
            );

            let instructions = decode_all(&body.il)?;
            for ins in &instructions {
                let operand = render_operand(&interp, &ins.operand);
                println!("  IL_{:04X}:  {:<16}{}", ins.offset, ins.op.name(), operand);
            }
            printed += 1;
        }
    }

    if printed == 0 {
        eprintln!("rustnet: no methods matched");
        return Ok(1);
    }
    Ok(0)
}

fn describe_kind(kind: &MethodKind) -> &'static str {
    match kind {
        MethodKind::Il(_) => "IL",
        MethodKind::InternalCall => "internal call (RustBCL)",
        MethodKind::PInvoke { .. } => "P/Invoke",
        MethodKind::RuntimeProvided => "runtime provided",
        MethodKind::Abstract => "abstract",
    }
}

/// Renders an operand, resolving tokens to names where possible.
fn render_operand(interp: &Interpreter, operand: &Operand) -> String {
    match operand {
        Operand::None => String::new(),
        Operand::I32(v) => v.to_string(),
        Operand::I64(v) => v.to_string(),
        Operand::F64(v) => v.to_string(),
        Operand::Var(v) => v.to_string(),
        Operand::Target(t) => format!("IL_{t:04X}"),
        Operand::Targets(ts) => {
            let list: Vec<String> = ts.iter().map(|t| format!("IL_{t:04X}")).collect();
            format!("({})", list.join(", "))
        }
        Operand::Token(t) => {
            // Naming a token needs the defining assembly, which the caller of a
            // standalone disassembly does not track per-instruction; show the
            // raw token alongside its table for orientation.
            let _ = interp;
            match t.table() {
                Some(table) => format!("{:?} /* {} */", t, table.name()),
                None if t.is_user_string() => format!("{t} /* string literal */"),
                None => t.to_string(),
            }
        }
    }
}

/// Reports what the native code generator can and cannot take.
///
/// A declined method is not a failure — it is interpreted, exactly as before.
/// The point of listing them is that the *reasons* are actionable: they are the
/// backend's to-do list, in the order a real program cares about.
pub fn jit(assembly: &str) -> Result<i32> {
    let mut interp = runtime(Vec::new());
    let id = interp.loader.load_from_file(assembly)?;
    rustclr_interop::install(&mut interp);

    println!("Code generation for {}", interp.loader.assembly(id).name);
    println!("Backend: x86-64 baseline
");

    let methods: Vec<rustclr_core::MethodId> = interp
        .loader
        .registry
        .iter_methods()
        .filter(|m| m.assembly == id && matches!(m.kind, MethodKind::Il(_)))
        .map(|m| m.id)
        .collect();

    use rustclr_jit::Compiler as _;
    let mut backend = rustclr_jit::X64Backend::new();
    let mut compiled = 0usize;
    let mut declined: Vec<(String, String)> = Vec::new();

    for method in methods {
        let name = interp.loader.registry.method(method).qualified_name.clone();
        if !backend.can_compile(&interp.loader.registry, method) {
            declined.push((name, describe_decline(&interp, method)));
            continue;
        }
        match backend.compile(&interp.loader, method) {
            Ok(code) => {
                println!("  JIT  {name}  ({} bytes)", code.bytes.len());
                compiled += 1;
            }
            Err(e) => declined.push((name, e.to_string())),
        }
    }

    if !declined.is_empty() {
        println!();
        for (name, reason) in &declined {
            println!("  --   {name}: {reason}");
        }
    }

    println!(
        "
{compiled} compiled, {} interpreted, {} bytes emitted.",
        declined.len(),
        backend.bytes_emitted
    );
    Ok(0)
}

/// Why the baseline backend turned a method down, in the order it checks.
fn describe_decline(interp: &Interpreter, method: rustclr_core::MethodId) -> String {
    let info = interp.loader.registry.method(method);
    let MethodKind::Il(body) = &info.kind else {
        return "no IL body".into();
    };
    if !body.exception_clauses.is_empty() {
        return "has exception handling".into();
    }
    if info.signature.has_this {
        return "is an instance method".into();
    }
    if !info.returns_void() && !is_integer_sig(&info.signature.return_type) {
        return "returns a non-integer type".into();
    }
    // An `int[]` parameter is accepted — the backend takes arrays that arrive
    // as arguments. This explanation used to test integers only and reported
    // "parameter 0 is not an integer" for a method the backend declined for a
    // different reason entirely, which is worse than no explanation.
    if let Some(i) = info
        .signature
        .params
        .iter()
        .position(|p| !is_integer_sig(p) && !is_supported_array(p))
    {
        return format!("parameter {i} is neither a number nor an array the backend takes");
    }
    // Arrays are allowed as locals now that compiled code can allocate; this
    // explanation still said otherwise, and a stale explanation is worse than
    // none — it sends a reader looking for the wrong thing.
    if let Some(i) = body.locals.iter().position(|l| !is_integer_sig(l) && !is_supported_array(l)) {
        return format!("local {i} is neither a number nor an array the backend takes");
    }
    match rustclr_core::opcode::decode_all(&body.il) {
        Ok(instructions) => {
            let mut unsupported: Vec<String> = instructions
                .iter()
                .filter(|i| {
                    !rustclr_jit::x64::is_supported(i.op)
                        && !rustclr_jit::translate::is_array_op(i.op)
                })
                .map(|i| i.op.name().to_string())
                .collect();
            unsupported.sort();
            unsupported.dedup();
            if unsupported.is_empty() {
                "declined by the backend".into()
            } else {
                format!("uses {}", unsupported.join(", "))
            }
        }
        Err(e) => format!("undecodable IL: {e}"),
    }
}

fn is_integer_sig(sig: &rustclr_core::metadata::TypeSig) -> bool {
    use rustclr_core::metadata::TypeSig as T;
    matches!(
        sig.unwrap_modifiers(),
        T::Boolean | T::Char | T::I1 | T::U1 | T::I2 | T::U2 | T::I4 | T::U4 | T::I8 | T::U8
    )
}

pub fn verify(assembly: &str) -> Result<i32> {
    let mut interp = runtime(Vec::new());
    let id = interp.loader.load_from_file(assembly)?;

    println!("Verifying {}", interp.loader.assembly(id).name);

    // 1. IL verification.
    let failures = rustclr_jit::verify_all(&interp.loader);
    for (method, error) in &failures {
        let info = interp.loader.registry.method(*method);
        println!("  IL   {}: {error}", info.qualified_name);
    }

    // 2. References the loader could not bind at all. These fail at run time
    //    with "could not resolve token", and are invisible to the IL pass
    //    because they never became methods.
    //
    //    Only those some IL actually names are worth reporting: every assembly
    //    references attribute constructors it never executes, and listing those
    //    would bury the findings that matter.
    let reached = member_refs_reached_by_il(&interp, id);
    let mut unbound: Vec<String> = interp
        .loader
        .assembly(id)
        .unresolved_members
        .iter()
        .filter(|(row, _)| reached.contains(row))
        .map(|(_, name)| name.clone())
        .collect();
    unbound.sort();
    unbound.dedup();
    for name in &unbound {
        println!("  REF  cannot resolve {name} — its declaring type is not available");
    }

    // 3. Members that resolved but have no native implementation.
    let mut missing: Vec<String> = Vec::new();
    for m in interp.loader.registry.iter_methods() {
        if !matches!(m.kind, MethodKind::InternalCall) {
            continue;
        }
        // A method declared on an interface is meant to be implemented by the
        // caller's own type, and virtual dispatch finds it there. Reporting it
        // as a missing native would flag every `IDisposable` in the program.
        if interp.loader.registry.ty(m.declaring_type).kind == TypeKind::Interface {
            continue;
        }
        let keys = interp.loader.native_keys(m.id);
        if !keys.iter().any(|k| interp.has_native(k)) {
            missing.push(m.qualified_name.clone());
        }
    }
    missing.sort();
    missing.dedup();
    for name in &missing {
        println!("  BCL  no native implementation for {name}");
    }

    let total = failures.len() + missing.len() + unbound.len();
    if total == 0 {
        println!("  OK — every method verifies and every referenced member resolves.");
        Ok(0)
    } else {
        println!("\n{total} problem(s) found.");
        Ok(1)
    }
}

/// `MemberRef` rows named by an operand somewhere in this assembly's IL.
fn member_refs_reached_by_il(
    interp: &Interpreter,
    assembly: rustclr_core::AssemblyId,
) -> std::collections::HashSet<u32> {
    use rustclr_core::metadata::TableId;

    let mut reached = std::collections::HashSet::new();
    for method in interp.loader.registry.iter_methods() {
        if method.assembly != assembly {
            continue;
        }
        let MethodKind::Il(body) = &method.kind else { continue };
        let Ok(instructions) = decode_all(&body.il) else { continue };

        for instruction in instructions {
            if let Operand::Token(token) = instruction.operand {
                if token.table() == Some(TableId::MemberRef) {
                    reached.insert(token.row());
                }
            }
        }
    }
    reached
}

pub fn build(project: &str, configuration: &str, then_run: bool) -> Result<i32> {
    let project_path = Path::new(project);
    println!("Compiling {project} ({configuration}) with the .NET SDK…");

    let status = std::process::Command::new("dotnet")
        .args(["build", "-c", configuration, "--nologo"])
        .arg(project_path)
        .status()
        .map_err(|e| format!("could not run `dotnet`: {e}. Install the .NET SDK to build C# sources."))?;

    if !status.success() {
        return Ok(status.code().unwrap_or(1));
    }

    if !then_run {
        return Ok(0);
    }

    // Locate the produced assembly.
    let bin = project_path.join("bin").join(configuration);
    let assembly = find_assembly(&bin)
        .ok_or("build succeeded but no output assembly was found under bin/")?;

    println!("Running {} on RustCLR…\n", assembly.display());
    run(&assembly.to_string_lossy(), Vec::new(), false, false, None, true, None, true, false, None, None)
}

/// Finds the first `.dll` under a build output directory.
fn find_assembly(root: &Path) -> Option<std::path::PathBuf> {
    let entries = std::fs::read_dir(root).ok()?;
    let mut directories = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            directories.push(path);
        } else if path.extension().is_some_and(|e| e == "dll") {
            return Some(path);
        }
    }
    directories.into_iter().find_map(|d| find_assembly(&d))
}

pub fn capabilities() -> Result<i32> {
    let interp = runtime(Vec::new());

    println!("RustCLR runtime capabilities\n");
    println!("Execution");
    println!("  IL interpreter               yes");
    println!("  native JIT backend           x86-64. SCALAR ARITHMETIC AND int[]:");
    println!("                               integer AND double arithmetic, comparison,");
    println!("                               branching, negation, conversion between the");
    println!("                               two, arguments, locals, and element access");
    println!("                               on an int[], double[] or bool[] - in a");
    println!("                               parameter or a local - bounds-checked.");
    println!("                               IL's `add` is `add` for both kinds, so a");
    println!("                               pass infers which from the types that reach");
    println!("                               it and DECLINES the method when it cannot");
    println!("                               follow the stream - never guesses.");
    println!("                               ALLOCATION works: new int[n], new double[n]");
    println!("                               and new bool[n] inside a compiled method,");
    println!("                               because allocating does not itself collect");
    println!("                               and a compiled method cannot return a");
    println!("                               reference - so nothing it allocates");
    println!("                               outlives the call.");
    println!("                               SAFE POINTS: a compiled method polls at every");
    println!("                               loop back-edge, so it can be stopped mid-body.");
    println!("                               Two reasons to stop: another thread wants to");
    println!("                               collect, or this call has allocated enough to");
    println!("                               want one itself. Both are needed - with a");
    println!("                               single thread nothing else ever asks.");
    println!("                               That is what makes ALLOCATION IN A LOOP legal:");
    println!("                               the frame's live arrays and references are");
    println!("                               described by offsets fixed at compile time, so");
    println!("                               a collection can see them, and everything the");
    println!("                               loop has finished with is released there. A");
    println!("                               loop's memory is flat, not proportional to its");
    println!("                               trip count.");
    println!("                               A back-edge reached with anything still on the");
    println!("                               evaluation stack gets NO safe point, and such a");
    println!("                               method may not allocate in a loop - the old");
    println!("                               rule, kept for the case the map cannot describe.");
    println!("                               DECLINED: float[]; `rem` on doubles;");
    println!("                               exception handling.");
    println!("                               FIELD ACCESS on a heap object goes through a");
    println!("                               HELPER CALL, never an inline load. The heap's");
    println!("                               value representation is a Rust enum with boxed");
    println!("                               payloads and no layout the language promises,");
    println!("                               so emitting code that read it would fail as a");
    println!("                               WRONG NUMBER the day a compiler reordered the");
    println!("                               variants. A call costs more and cannot be wrong");
    println!("                               that way. int, long, double and reference");
    println!("                               fields cross; a struct field is REFUSED.");
    println!("                               The declared kind is CHECKED, not just used to");
    println!("                               convert: a reference stored as a number would");
    println!("                               round-trip perfectly and be invisible to the");
    println!("                               collector.");
    println!("                               SCALAR REPLACEMENT: an object that provably");
    println!("                               never leaves the method that made it stops");
    println!("                               existing - each field becomes a local, and");
    println!("                               ldfld/stfld become ldloc/stloc. Compiled code");
    println!("                               never reads a field out of a heap object,");
    println!("                               because that would make the Rust layout of");
    println!("                               Value part of this backend's ABI and fail as");
    println!("                               a wrong number rather than a crash.");
    println!("                               The proof is narrow ON PURPOSE: newobj outside");
    println!("                               any loop, a constructor that only chains to");
    println!("                               Object's, scalar fields only, one local, and");
    println!("                               used ONLY by ldfld/stfld/dup. Passing it to a");
    println!("                               method, returning, boxing, locking or taking a");
    println!("                               field's address all REFUSE.");
    println!("                               GUARDED DEVIRTUALISATION: a callvirt whose");
    println!("                               every possible answer is known becomes a type");
    println!("                               compare and a branch - no call at all.");
    println!("                               Trusted ONLY when the declaring type is not");
    println!("                               visible outside its assembly, so nothing");
    println!("                               loaded later can override it; a public");
    println!("                               hierarchy is REFUSED even when it looks ideal.");
    println!("                               Each override must fold to a constant.");
    println!("                               The receiver's type comes from a helper, never");
    println!("                               from reading the object - compiled code holds");
    println!("                               only a handle, an opaque token it cannot");
    println!("                               dereference. A receiver no guard matches TRAPS:");
    println!("                               there is no on-stack replacement, so a body");
    println!("                               that already allocated cannot return to the");
    println!("                               interpreter, and a wrong proof must say so.");
    println!("                               A null receiver throws NullReferenceException,");
    println!("                               told apart from that trap on purpose.");
    println!("                               RECURSION compiles: a method that calls");
    println!("                               ITSELF gets a real calling convention, which");
    println!("                               the inliner cannot provide - splicing a");
    println!("                               method into itself does not terminate.");
    println!("                               A call to ANOTHER compiled method goes through");
    println!("                               a TABLE of entry addresses the runtime fills in");
    println!("                               before the caller runs - the callee's address");
    println!("                               does not exist when the caller is compiled, and");
    println!("                               two methods may call each other. Each entry");
    println!("                               names the callee AND the callee's own table,");
    println!("                               because a callee reads its table from the");
    println!("                               argument array its caller builds.");
    println!("                               Compiled calls run on the MACHINE stack, so the");
    println!("                               runtime counts the BYTES they occupy and");
    println!("                               refuses past a budget rather than crashing.");
    println!("                               Bytes, not frames: frames differ in size, so a");
    println!("                               frame count is too generous for a large one and");
    println!("                               too mean for a small one. The CLI reserves a");
    println!("                               large stack for managed code rather than hoping");
    println!("                               the host gave it one.");
    println!("                               TIERING: a method whose body LOOPS compiles");
    println!("                               on its first call; a loop-free one waits for");
    println!("                               the call count. A single call that runs for");
    println!("                               seconds never crossed a count, and the body");
    println!("                               is the only signal available beforehand.");
    println!("                               Not on-stack replacement: a method already");
    println!("                               running cannot be swapped.");
    println!("                               Everything else is interpreted, and");
    println!("                               `rustnet jit <assembly>` says which is which.");
    println!("  array bounds                 checked. Compiled code cannot throw, so a");
    println!("                               failure flags and returns and the tier raises");
    println!("                               IndexOutOfRangeException; stores already made");
    println!("                               stay made, as on .NET.");
    println!("  code memory                  write-xor-execute; never both at once");
    println!("  tiering                      compile after 32 calls; --no-jit disables");
    println!("  inlining                     yes - branch-free static callees. A callee");
    println!("                               that is ITSELF built from smaller callees is");
    println!("                               flattened before it is spliced, three levels");
    println!("                               deep: one level only handles a caller whose");
    println!("                               callees are leaves, and code is written with");
    println!("                               helpers built out of helpers. Depth AND the");
    println!("                               size of the result are both bounded. A callee");
    println!("                               with any branch, or any instance method, is");
    println!("                               REFUSED. --no-inline disables");
    println!("  AArch64, RISC-V backends     EMIT ONLY, NEVER EXECUTED. Both encode the");
    println!("                               same IL the x86-64 backend does and are");
    println!("                               checked by disassembly, but no compiled");
    println!("                               method has ever run on either architecture.");
    println!("                               Only x86-64 is dispatched to at runtime.");
    println!("  managed call depth limit     {}", interp.limits.max_frames);
    println!("  debugger hooks               yes - breakpoints by (method, IL offset),");
    println!("                               step into/over/out, and a view of every");
    println!("                               frame's arguments, locals and evaluation");
    println!("                               stack while stopped. `rustnet debug` runs");
    println!("                               a program through them.");
    println!("                               A debugger may set breakpoints WHILE stopped");
    println!("                               and they take effect on the same run.");
    println!("  source-line breakpoints      yes, where a portable PDB sits beside the");
    println!("                               assembly - `--break File.cs:line` resolves to");
    println!("                               EVERY place that line has code. A line with");
    println!("                               no code is refused, not moved to a nearby one.");
    println!("                               Locals are named from the PDB where it names");
    println!("                               them; a compiler-generated slot is shown by");
    println!("                               number instead of an invented name. No PDB");
    println!("                               means IL offsets only, and it says so.");
    println!("  debug adapter (DAP)          yes - `rustnet debug --dap` speaks the Debug");
    println!("                               Adapter Protocol over stdin/stdout, so VSCode");
    println!("                               and Rider can attach. Breakpoints, stepping,");
    println!("                               call stack, arguments and locals. The");
    println!("                               program's console arrives as `output` events.");
    println!("                               While the program RUNS no requests are");
    println!("                               serviced: an editor's pause button does");
    println!("                               nothing until the next breakpoint, because");
    println!("                               the hook is synchronous by design.");
    println!("                               ONE thread is debugged: the one that runs");
    println!("                               Main. A thread the program starts runs with");
    println!("                               no debugger, so a breakpoint inside it never");
    println!("                               fires. Its console is still redirected.");
    println!("                               The hook costs nothing when nothing is");
    println!("                               attached - measured, not assumed.");
    println!("  profiler                     yes - `rustnet profile` samples between IL");
    println!("                               instructions and reports self and inclusive");
    println!("                               time per method, exact call counts, and what");
    println!("                               was allocated by type. It samples ITSELF:");
    println!("                               no sampling thread, so it works on a board.");
    println!("                               Measured elapsed time is charged rather than");
    println!("                               one tick per sample, so a late sample does");
    println!("                               not shorten the run it reports.");
    println!("                               WALL CLOCK, not CPU: time blocked in a wait");
    println!("                               counts against the method that waited.");
    println!("                               ONE thread is profiled and everything is");
    println!("                               interpreted by default - compiled code runs");
    println!("                               no IL, so `--jit` charges its time and its");
    println!("                               allocations to whoever called it.");
    println!("                               Costs nothing when nothing is attached -");
    println!("                               measured, not assumed.");
    println!("  diagnostics a program reads  GC.CollectionCount, GetTotalMemory,");
    println!("                               GetTotalAllocatedBytes, Stopwatch,");
    println!("                               Environment.TickCount - answered from the");
    println!("                               same counters the profiler reports, so a");
    println!("                               program and its profile cannot disagree.");
    println!("\nDevice");
    println!("  board seam                   yes - `RustNet.Hal` reaches whatever board the");
    println!("                               firmware installed: GPIO, ADC, PWM, I2C, SPI,");
    println!("                               UART, watchdog, delay. The board is a");
    println!("                               swappable module like the collector and the");
    println!("                               scheduler, not something wired in.");
    println!("                               `run --board host` installs a simulator so");
    println!("                               hardware code is testable on a desktop.");
    println!("  no board                     an ERROR, not a no-op. A program that writes");
    println!("                               a pin on a machine that has none is told so.");
    println!("                               Only `Board.Name` and `Board.IsPresent`");
    println!("                               answer without a board.");
    println!("  storage                      yes - `RustNet.IO.Storage` over a log-");
    println!("                               structured store on the board's external");
    println!("                               memory. Files are read and written WHOLE:");
    println!("                               NOR flash cannot set bits, so a seekable");
    println!("                               stream would erase a sector per write.");
    println!("  crypto                       SHA-256, HMAC-SHA256, AES-128/256-CTR,");
    println!("                               CRC-32 - written out, not depended on, and");
    println!("                               checked against the vectors FIPS 180-4,");
    println!("                               FIPS 197, SP 800-38A and RFC 4231 publish.");
    println!("                               NO RSA or elliptic curves: a hand-rolled");
    println!("                               bignum with a side channel is worse than no");
    println!("                               signature check, because it reports success.");
    println!("                               NO random number generator: entropy belongs");
    println!("                               to the chip, and a board without one says so");
    println!("                               rather than returning a counter.");
    println!("  graphics                     yes - one RGB565 canvas, lines, rects,");
    println!("                               circles, text in a built-in 8x8 font, and");
    println!("                               `Present` to the board panel. Drawing needs");
    println!("                               no board, which is how a UI is tested");
    println!("                               headlessly. ONE canvas: 240x240 is 115 KB,");
    println!("                               and on most of these boards that is the");
    println!("                               whole heap.");
    println!("  camera                       yes - `RustNet.Media.Camera` over the board");
    println!("                               seam: a colour-bar test sensor on `--board");
    println!("                               host`, and a Maix Go's OV2640 through the");
    println!("                               K210's DVP. A board with no sensor THROWS");
    println!("                               rather than returning a blank frame, which");
    println!("                               a covered lens is indistinguishable from.");
    println!("                               NO exposure, gain or white-balance control.");
    println!("  signed images                yes - `RCSB` containers verified before a");
    println!("                               byte of payload runs, with the chip family");
    println!("                               checked so an ESP32 image cannot brick an");
    println!("                               STM32. A MAC, NOT a signature: the verifying");
    println!("                               key can also sign, so extracting it from one");
    println!("                               device compromises every device sharing it.");
    println!("                               No rollback protection yet.");
    println!("  over-the-air updates         yes - A/B slots, so a failed write costs a");
    println!("                               reboot rather than a probe. New firmware");
    println!("                               must CONFIRM itself; three silent boots roll");
    println!("                               back. Silence is treated as failure, which");
    println!("                               is the safe direction.");
    println!("  device crates                MQTT codec, USB descriptors, a record store,");
    println!("                               structured logging - all bare-metal. HTTP,");
    println!("                               Modbus and the web server are host-only:");
    println!("                               their transport is `std` sockets.");
    println!("  text encodings               UTF-8 and ASCII. Every wire format a device");
    println!("                               meets is one of the two, and .NET's code-page");
    println!("                               tables are larger than the heap on the boards");
    println!("                               this targets.");
    println!("\nMemory");
    println!("  collector                    {}", interp.heap.collector_name());
    println!("  pluggable collectors         yes");
    println!("  object pinning for interop   yes");
    println!("\nType system");
    println!("  classes, interfaces          yes");
    println!("  virtual and interface calls  yes");
    println!("  value types, enums           yes");
    println!("  delegates                    yes (unicast and multicast)");
    println!("  generic types                USER types get one runtime type per closed");
    println!("                               construction: Cell<int> and Cell<string> are");
    println!("                               two types, each with its own type arguments");
    println!("                               and its own static slots. A class type");
    println!("                               parameter is answered through the receiver.");
    println!("                               Framework generics stay erased by choice -");
    println!("                               native bindings are keyed by declaring type");
    println!("                               name, and List<int> having its own name would");
    println!("                               put List`1::Add out of reach.");
    println!("                               A class parameter in a STATIC method also");
    println!("                               answers: there is no receiver, so the frame");
    println!("                               carries the construction the CALL SITE named.");
    println!("                               A call site naming the open definition still");
    println!("                               refuses - nothing there knows the argument.");
    println!("  generic methods              type arguments ARE known: typeof(T),");
    println!("                               default(T) and `x is T` all answer inside");
    println!("                               M<T>, because each call site\'s MethodSpec");
    println!("                               carries the arguments and the instantiation");
    println!("                               records them.");
    println!("  custom comparers             yes - List<T>.Sort takes a Comparison<T>");
    println!("                               lambda or an IComparer<T>, and calls back");
    println!("                               into managed code for each comparison.");
    println!("                               The sort is a stable merge sort, where");
    println!("                               .NET's List<T>.Sort is an unstable");
    println!("                               introsort - equal elements can end up in a");
    println!("                               different order there, which .NET does not");
    println!("                               promise either way. OrderBy is stable on");
    println!("                               both and agrees exactly.");
    println!("  reflection                   System.Type is a real object:");
    println!("                               name, namespace, base type, IsValueType and");
    println!("                               friends, IsAssignableFrom, GetMethods,");
    println!("                               GetFields, GetProperties, GetProperty,");
    println!("                               MethodInfo.Invoke, MethodBase.GetParameters,");
    println!("                               FieldInfo get/set, PropertyInfo get/set,");
    println!("                               Activator.CreateInstance.");
    println!("                               Parameter names come from the Param table;");
    println!("                               a method with no rows there reports argN");
    println!("                               rather than inventing one.");
    println!("                               Assembly and Module: GetExecutingAssembly,");
    println!("                               GetEntryAssembly, GetTypes, GetType(name),");
    println!("                               GetName, Type.Assembly and Type.Module.");
    println!("                               Assembly.Load works on a host and RESOLVES");
    println!("                               DIFFERENTLY from .NET: it probes beside the");
    println!("                               loaded assemblies and the search paths, where");
    println!("                               .NET reads deps.json and the load context. A");
    println!("                               referenced assembly behaves identically; an");
    println!("                               unreferenced DLL beside the app loads here and");
    println!("                               does not on .NET. Without std it refuses.");
    println!("                               MakeGenericType WORKS, and returns the same");
    println!("                               instance as typeof: it calls the loader path a");
    println!("                               TypeSpec calls and shares its cache. So do");
    println!("                               IsGenericType, IsGenericTypeDefinition,");
    println!("                               ContainsGenericParameters and");
    println!("                               GetGenericTypeDefinition.");
    println!("                               GetGenericArguments on an OPEN definition is");
    println!("                               REFUSED: .NET returns the parameter T and this");
    println!("                               runtime records only a definition's arity, so");
    println!("                               an empty array would quietly disagree.");
    println!("                               typeof(T) on a generic parameter is REFUSED:");
    println!("                               the argument was erased, so there is no type");
    println!("                               to name and a guess would look plausible.");
    println!("  custom attributes            yes - constructor arguments, named fields and");
    println!("                               named properties. An argument shape this");
    println!("                               runtime cannot decode (arrays, Type, boxed)");
    println!("                               omits that attribute rather than inventing it.");

    println!("
Collections and LINQ");
    println!("  List, Dictionary, HashSet    yes");
    println!("  Queue, Stack, KeyValuePair   yes");
    println!("  foreach over IEnumerable<T>  yes");
    println!("  LINQ                         yes, but EAGER:");
    println!("                               every operator materialises its result at");
    println!("                               once. Side effects in a predicate happen at");
    println!("                               the call, not at consumption, and an");
    println!("                               infinite sequence never terminates.");
    println!("  ordering keys                numbers and strings; other key types are");
    println!("                               refused rather than ordered arbitrarily");
    println!("  custom comparers             no - IEqualityComparer<T> arguments ignored");
    println!("\nModern C#");
    println!("  string interpolation         yes");
    println!("  tuples                       yes (ValueTuple 1-8)");
    println!("  ranges and indices           yes (a[^1], a[1..4])");
    println!("  nullable value types         yes");
    println!("  init-only properties         yes");
    println!("  primary constructors         yes");
    println!("  collection expressions       arrays only (spread needs Span<T>)");
    println!("  extension members (C# 14)    yes");
    println!("  pattern matching, switch     yes");
    println!("  records                      yes");
    println!("  generic collections, LINQ    yes - see below");

    println!("\nAsync and threading");
    println!("  async / await                yes, and await SUSPENDS. An async method");
    println!("                               awaiting a pending task copies its state");
    println!("                               machine to the heap, queues it on that");
    println!("                               task and returns; the thread that");
    println!("                               completes the task resumes it.");
    println!("                               Where work STARTS is what decides whether");
    println!("                               it overlaps: two Task.Run started and then");
    println!("                               awaited run at once; awaiting in a loop is");
    println!("                               sequential, as it is on .NET.");
    println!("                               Task.Run and Parallel.* run on a POOL of one");
    println!("                               worker per core; Task.Delay arms a timer.");
    println!("                               A thread waiting on a task runs queued work");
    println!("                               rather than idling, so a task awaiting a");
    println!("                               task cannot deadlock the pool.");
    println!("                               Thread.Start still gets its own thread.");
    println!("                               The pool GROWS when workers block, up to a");
    println!("                               ceiling, so queued work is not stuck behind");
    println!("                               a worker that is only sleeping.");
    println!("  Task, Task<T>, WhenAll       yes");
    println!("  TaskCompletionSource         yes - a real suspend and resume");
    println!("  Task Parallel Library        yes - see Parallel.* below");
    println!("  Thread, lock, Interlocked    yes - REAL THREADS. Thread.Start spawns an");
    println!("                               OS thread and Join waits for it. lock");
    println!("                               genuinely excludes; Interlocked does not");
    println!("                               lose updates. A thread started later can");
    println!("                               unblock one already waiting.");
    println!("                               A spawned thread shares the heap, static");
    println!("                               storage and the bindings, and gets its own");
    println!("                               frames and an identical COPY of the loader -");
    println!("                               which is why no lock sits on the path that");
    println!("                               runs every instruction.");
    println!("                               Task.Run and Parallel.* spawn too.");
    println!("  Monitor.Wait / Pulse         yes - Wait releases the monitor at the");
    println!("                               depth it was held, waits, and retakes it,");
    println!("                               so producer/consumer queues work.");
    println!("                               Pulse wakes EVERY waiter rather than one:");
    println!("                               a weaker promise, which a Wait written in");
    println!("                               the required re-check loop already");
    println!("                               tolerates.");
    println!("                               Wait(obj, ms) honours its timeout, reports");
    println!("                               whether it was pulsed, and comes back");
    println!("                               HOLDING the monitor either way.");

    println!("\nMemory and resources");
    println!("  IDisposable, using           yes");
    println!("  IAsyncDisposable             yes - await using works, with ValueTask");
    println!("                               underneath. Disposal runs after the body.");
    println!("  IAsyncEnumerable, await      yes - an async iterator runs, including");
    println!("  foreach                      yield break, an empty sequence, and break");
    println!("                               out of the loop. Still no overlap: the body");
    println!("                               runs to the next yield before MoveNextAsync");
    println!("                               returns, so the sequence is produced eagerly");
    println!("                               one element at a time rather than awaited.");
    println!("  Span<T>, Memory<T>           yes - over an array, a string, or");
    println!("                               stackalloc memory. Length, IsEmpty,");
    println!("                               indexing, Slice, CopyTo, ToArray, foreach,");
    println!("                               AsSpan, AsMemory and Memory<T>.Span.");
    println!("                               Indexing yields a REFERENCE, so a write");
    println!("                               through a span reaches what it is over.");
    println!("                               Both collection-expression forms work:");
    println!("                               [..xs, 4] and ReadOnlySpan<char> x =");
    println!("                               ['a','b'], the latter via CreateSpan.");
    println!("                               For raw memory the element width comes");
    println!("                               from the CALL SITE - framework generics");
    println!("                               are erased, but the TypeSpec still names");
    println!("                               its arguments.");
    println!("                               REFUSED: slicing a span that STANDS FOR a");
    println!("                               string - the string is the whole");
    println!("                               representation and has no offset in it.");
    println!("  Task.WaitAll(a, b) in C#     yes - .NET 10 lowers it through an");
    println!("                               InlineArray2<Task> and a ReadOnlySpan,");
    println!("                               and both are implemented. It still says");
    println!("                               NOTHING about concurrency: both tasks ran");
    println!("                               to completion before WaitAll saw them.");
    println!("  Unsafe.As / Unsafe.Add       yes - a managed reference here is a path");
    println!("                               to a slot, so As keeps the path and Add");
    println!("                               walks it to the nth element or field.");
    println!("  MemoryMarshal.CreateSpan     yes, but it COPIES rather than making a");
    println!("                               view onto the caller's storage. Nothing");
    println!("                               that reaches it can observe the");
    println!("                               difference; a genuine view is not served.");
    println!("  Marshal (blittable structs)  yes - SizeOf<T>, AllocHGlobal, FreeHGlobal,");
    println!("                               StructureToPtr and PtrToStructure<T> for a");
    println!("                               struct of primitive fields; widths survive.");
    println!("                               AllocHGlobal uses the MANAGED heap, so");
    println!("                               FreeHGlobal is a no-op and the pointer");
    println!("                               cannot be handed to native code.");
    println!("                               A struct with a reference field is refused:");
    println!("                               its field is a handle, not bytes.");
    println!("  stackalloc, unsafe pointers  yes - stackalloc, fixed, arithmetic,");
    println!("                               comparison and dereference all run, and so");
    println!("                               do cpblk and initblk.");
    println!("                               A POINTER IS NOT AN ADDRESS: it is a buffer");
    println!("                               on the managed heap plus a byte offset, so");
    println!("                               it cannot name memory the runtime does not");
    println!("                               own, and it roots its buffer. The access");
    println!("                               width comes from the instruction, not the");
    println!("                               pointer.");
    println!("                               REFUSED: an unaligned pointer, a pointer");
    println!("                               into an array of references, cpblk between");
    println!("                               anything but byte buffers, and Span<T> over");
    println!("                               stackalloc - its element width lives in T.");

    println!("\nCompile-time features");
    println!("  source generators            yes - the runtime only ever sees the IL");
    println!("  interceptors                 yes - same reason");
    println!("\nExceptions");
    println!("  try / catch / finally        yes");
    println!("  Parallel.For/ForEach/Invoke  yes, REAL - one thread per core, capped by");
    println!("                               the iteration count, contiguous chunks.");
    println!("                               Iteration ORDER IS NOT PRESERVED, which is");
    println!("                               the contract a parallel loop already has.");
    println!("  Task.Run                     yes, REAL - starts the delegate on another");
    println!("                               thread. Result, Wait, WaitAll and an");
    println!("                               awaiter all wait for it first.");
    println!("  Task.WaitAll from C#         yes, and it genuinely waits. .NET 10 lowers");
    println!("                               it through an InlineArray2<Task> and a");
    println!("                               ReadOnlySpan, so it reads a span as well as");
    println!("                               an array.");
    println!("  exception filters            yes - `catch when` runs mid-unwind;");
    println!("                               a throwing filter declines");
    println!("  calli                        yes - a function pointer names a method");
    println!("                               rather than an address, so an indirect call");
    println!("                               needs no code map. It does not survive being");
    println!("                               stored in integer-shaped storage such as a");
    println!("                               delegate*[] element; that refuses with a");
    println!("                               message saying why.");
    println!("  localloc, cpblk, initblk     no - they address byte ranges, and a managed");
    println!("                               pointer here is a path to a slot rather than");
    println!("                               an address. Needs a raw-pointer value kind.");
    println!("  arglist                      no - varargs");
    println!("\nInterop");
    println!("  P/Invoke                     yes, up to {} arguments", rustclr_interop::MAX_PINVOKE_ARGS);
    println!("  string marshalling           UTF-8 only");
    println!("  struct marshalling           no - Marshal.SizeOf<T> is generic");
    println!("\nBase class library");
    println!("  native bindings registered   {}", interp.native_count());

    println!("\nEmbedded targets");
    println!("  whole runtime without std    yes - metadata, gc, core AND bcl, for");
    println!("                               thumbv7em, thumbv6m, riscv32imc, riscv64gc,");
    println!("                               and xtensa-esp32 via the esp fork.");
    println!("                               tests/embedded.sh checks all of them.");
    println!("  what changed to get there    maps become BTreeMap (every key is Ord),");
    println!("                               Arc becomes Rc (riscv32imc has no atomics),");
    println!("                               float maths comes from libm (core has none).");
    println!("                               Only the filesystem stayed std-only, so");
    println!("                               load_from_file is gated and bytes are not.");
    println!("  fixed-size heap              yes - Heap::embedded(n) is a hard ceiling");
    println!("  IL EXECUTION ON A CHIP       yes - verified on TWO architectures from one");
    println!("                               source: an ESP32-C3 (RISC-V 32) and an");
    println!("                               M5Stack Tough (Xtensa LX6), both with no OS.");
    println!("                               Loader, interpreter and");
    println!("                               all {} native bindings; HelloWorld.Main", interp.native_count());
    println!("                               printed the same bytes dotnet prints, with");
    println!("                               the same instruction and call counts.");
    println!("  memory needed to do it       260,702 bytes with every binding, or");
    println!("                               192,045 with console, strings and maths");
    println!("                               only. Measured, not estimated. A firmware");
    println!("                               picks a tier from its heap budget and says");
    println!("                               plainly when neither fits.");
    println!("  ahead-of-time compilation    no - needs Arm and RISC-V backends, which");
    println!("                               emit code but have never executed any");
    println!("  board firmwares              seven, one shared demonstration:");
    println!("                               ESP32-WROOM-32 (Xtensa), ESP32-C3 (RV32),");
    println!("                               Meadow F7 (M7), Maix Go K210 (RV64),");
    println!("                               Netduino 3 WiFi / STM32F427VI (M4F),");
    println!("                               Pico (M0+), Nucleo-F401RE (M4F).");
    println!("                               tests/firmware.sh builds all of them.");
    println!("  below the floor              the Nucleo-F401RE has 96 KB and cannot load");
    println!("                               the runtime at any tier. It reports that");
    println!("                               and runs metadata + GC. Its image is 21 KB");
    println!("                               of .text against 282 KB for the same source");
    println!("                               on the F427VI: the tier is a const fn over a");
    println!("                               constant, so LTO strips what cannot run.");
    println!("  run on real hardware         ESP32-C3 (RISC-V) and M5Stack Tough (Xtensa)");
    println!("                               both execute IL. The Xtensa board needs two");
    println!("                               heap regions to fit the full binding set:");
    println!("                               176 KB of dram_seg plus the 96 KB bank past");
    println!("                               the ROM data, 278,528 against 260,702 needed.");
    println!("                               The WROOM-32 and Meadow F7 were last flashed");
    println!("                               before the interpreter landed, so their");
    println!("                               captures show metadata and GC only. The K210,");
    println!("                               both STM32F4 boards and the Pico build but");
    println!("                               were never flashed - no board was connected.");
    println!("                               Captured runs: docs/logs/.");

    println!("\nArchitectures recognised in PE headers");
    for m in [
        Machine::I386,
        Machine::Amd64,
        Machine::Arm,
        Machine::Arm64,
        Machine::RiscV32,
        Machine::RiscV64,
    ] {
        println!("  {}", m.name());
    }

    println!("\nMeasured, not claimed: tests/fixtures/AdvancedFeatures/probe.sh runs each");
    println!("feature on both runtimes and compares. Detail: docs/advanced-features.md");

    println!("\nBuilt by Gravicode Studios, led by Kang Fadhil.");
    Ok(0)
}

/// Whether the backend takes an array of this type in a slot.
fn is_supported_array(sig: &rustclr_core::metadata::TypeSig) -> bool {
    rustclr_jit::translate::is_int_array(sig)
        || rustclr_jit::translate::is_bool_array(sig)
        || rustclr_jit::translate::is_double_array(sig)
}

/// `rustnet debug` — run under the debugger hooks.
///
/// Two front ends over the same hooks. Without `--dap` it prints every stop —
/// where execution stopped, in what method, at what source line, and what the
/// frames held — which is the smallest thing that exercises the hooks end to
/// end. With `--dap` it speaks the Debug Adapter Protocol over stdin and
/// stdout instead, and an editor drives it.
///
/// Breakpoints are `File.cs:line` where a portable PDB sits beside the
/// assembly, and `Method:offset` always. A line with no code is refused rather
/// than moved to a nearby line that does.
pub fn debug(
    path: &str,
    breakpoints: &[(String, u32)],
    step: bool,
    dap: bool,
) -> Result<i32> {
    use rustclr_core::debug::{DebugCommand, DebugControl, DebugFrame, Debugger, StopReason};

    // Under `--dap` stdout carries the protocol and nothing else may write to
    // it — not this command, and not the program being debugged. Its console
    // becomes `output` events, which is where an editor shows it anyway.
    let wire = if dap { Some(crate::dap::Wire::new(std::io::stdout())) } else { None };
    let mut interp = match &wire {
        Some(wire) => Interpreter::with_host(Box::new(crate::dap::DapHost::new(
            wire.clone(),
            Vec::new(),
        ))),
        None => Interpreter::with_host(Box::new(rustclr_core::SystemHost::new())),
    };
    if let Some(wire) = &wire {
        // Every thread the program starts, not just this one. A `Task.Run` or a
        // `Thread.Start` that printed to the real stdout would land inside a
        // framed message and the editor would drop the connection.
        let wire = wire.clone();
        interp.host_factory = Some(std::sync::Arc::new(move || {
            Box::new(crate::dap::DapHost::new(wire.clone(), Vec::new()))
        }));
    }
    rustclr_bcl::install(&mut interp);
    let assembly = interp.loader.load_from_file(path)?;
    // Source information is optional; say so rather than silently offering
    // breakpoints by line that could never match.
    let has_source = interp.loader.load_debug_info(assembly);
    if !has_source {
        // Over stderr under `--dap`: stdout is the protocol's.
        let note = "no portable PDB beside this assembly — breakpoints by line are unavailable";
        if dap {
            eprintln!("{note}");
        } else {
            println!("{note}");
        }
    }

    // The adapter takes the process over from here: it owns the conversation,
    // and the breakpoints come from the editor rather than the command line.
    if let Some(wire) = wire {
        let map = crate::dap::SourceMap::build(&interp);
        let mut adapter = crate::dap::DapAdapter::new(std::io::stdin(), wire, map);
        // An editor that disconnects during the handshake gets no program run.
        if !adapter.handshake() {
            return Ok(0);
        }
        interp.attach_debugger(Box::new(adapter));
        let outcome = interp.run_entry_point(assembly);
        // `terminated` is what ends the editor's session; without it the debug
        // view keeps spinning after the program has stopped.
        let code = match &outcome {
            Ok(code) => *code,
            Err(_) => 1,
        };
        if let Some(mut debugger) = interp.debugger.take() {
            debugger.finished(match &outcome {
                Ok(code) => Ok(*code),
                Err(e) => Err(format!("{e}")),
            });
        }
        return Ok(code);
    }

    // Resolve each name to a method before running, so a typo is an error now
    // rather than a breakpoint that silently never fires.
    for (name, offset) in breakpoints {
        // `File.cs:line` when the name looks like a file, which is what an
        // editor speaks. Resolved to *every* place that line has code: a
        // lambda and its enclosing method share lines, and setting only one
        // would silently miss the other.
        if name.ends_with(".cs") {
            let places = interp.loader.breakpoints_for_line(name, *offset);
            if places.is_empty() {
                return Err(format!(
                    "no code on {name}:{offset}{}",
                    if has_source { "" } else { " (and no PDB was found)" }
                )
                .into());
            }
            for (method, il) in places {
                interp.breakpoints.add(method, il);
                println!(
                    "breakpoint: {}:{} -> {} at IL_{:04X}",
                    name,
                    offset,
                    interp.loader.registry.method(method).qualified_name,
                    il
                );
            }
            continue;
        }

        // Exact name first, then qualified name. A substring match is
        // deliberately *not* a fallback: asking for `Triple` and silently
        // getting `TripledAsync` is a plausible wrong answer, which is the one
        // kind of failure this runtime refuses everywhere else.
        let candidates: Vec<rustclr_core::MethodId> = (0..interp.loader.registry.method_count())
            .map(|i| rustclr_core::MethodId(i as u32))
            .filter(|&id| {
                let m = interp.loader.registry.method(id);
                m.name == *name || m.qualified_name == *name
            })
            .collect();
        let found = match candidates.as_slice() {
            [one] => *one,
            [] => {
                return Err(format!(
                    "no method named `{name}` in this assembly; `rustnet info {path} --verbose` lists them"
                )
                .into())
            }
            many => {
                // Overloads, or the same name on two types. Naming one at
                // random would put the breakpoint somewhere the user did not
                // ask for, so this asks rather than guesses.
                let names: Vec<&str> = many
                    .iter()
                    .map(|id| interp.loader.registry.method(*id).qualified_name.as_str())
                    .collect();
                return Err(format!(
                    "`{name}` is ambiguous — {} methods match:
  {}",
                    many.len(),
                    names.join("
  ")
                )
                .into());
            }
        };
        interp.breakpoints.add(found, *offset);
        println!(
            "breakpoint: {} at IL_{:04X}",
            interp.loader.registry.method(found).qualified_name,
            offset
        );
    }
    if breakpoints.is_empty() && !step {
        println!("no breakpoints and no --step: the program will run to completion");
    }

    // Source lines and local names need the loader, and the loader lives
    // inside the interpreter the debugger is attached to. Rather than hand the
    // debugger a borrow it cannot have, what it needs is captured up front:
    // both are fixed once the assembly is loaded.
    let mut source_of: std::collections::HashMap<rustclr_core::MethodId, Vec<(u32, String, u32)>> =
        Default::default();
    let mut names_of: std::collections::HashMap<rustclr_core::MethodId, Vec<(u16, String)>> =
        Default::default();
    if has_source {
        let methods: Vec<(rustclr_core::MethodId, rustclr_core::AssemblyId, u32)> = interp
            .loader
            .registry
            .iter_methods()
            .filter(|m| matches!(m.kind, MethodKind::Il(_)))
            .map(|m| (m.id, m.assembly, m.token.row()))
            .collect();
        for (id, assembly, row) in methods {
            let Some(pdb) = interp.loader.debug_info(assembly) else { continue };
            let Some(debug) = pdb.method(row) else { continue };
            let lines: Vec<(u32, String, u32)> = debug
                .sequence_points
                .iter()
                .filter_map(|point| {
                    let line = point.start_line?;
                    let name = &pdb.document(point.document)?.name;
                    let file = name.rsplit(['/', '\\']).next().unwrap_or(name);
                    Some((point.il_offset, file.to_string(), line))
                })
                .collect();
            if !lines.is_empty() {
                source_of.insert(id, lines);
            }
            let names: Vec<(u16, String)> =
                debug.locals.iter().map(|l| (l.index, l.name.clone())).collect();
            if !names.is_empty() {
                names_of.insert(id, names);
            }
        }
    }

    struct Tracer {
        step: bool,
        stops: usize,
        source: std::collections::HashMap<rustclr_core::MethodId, Vec<(u32, String, u32)>>,
        names: std::collections::HashMap<rustclr_core::MethodId, Vec<(u16, String)>>,
    }
    impl Tracer {
        /// The last sequence point at or before this offset — the one whose
        /// span covers it. `None` where the PDB says nothing, which is shown
        /// as an IL offset rather than as the nearest line.
        fn where_in_source(&self, method: rustclr_core::MethodId, offset: u32) -> Option<String> {
            let points = self.source.get(&method)?;
            let (_, file, line) = points.iter().take_while(|(at, _, _)| *at <= offset).last()?;
            Some(format!("{file}:{line}"))
        }
    }
    impl Debugger for Tracer {
        fn stopped(
            &mut self,
            reason: StopReason,
            frames: &[DebugFrame<'_>],
            _control: &mut DebugControl<'_>,
        ) -> DebugCommand {
            self.stops += 1;
            if let Some(top) = frames.first() {
                let at = self
                    .where_in_source(top.method, top.il_offset)
                    .unwrap_or_else(|| format!("IL_{:04X}", top.il_offset));
                println!(
                    "\n─── {:?} · {} · {} · {} frame(s)",
                    reason,
                    top.method_name,
                    at,
                    frames.len()
                );
                if !top.arguments.is_empty() {
                    println!("  args    {}", render(top.arguments));
                }
                if !top.locals.is_empty() {
                    // Named where the PDB names them, by index where it does
                    // not — rather than inventing a name for a compiler
                    // temporary that never had one.
                    let names = self.names.get(&top.method);
                    let rendered: Vec<String> = top
                        .locals
                        .iter()
                        .enumerate()
                        .map(|(i, value)| {
                            match names
                                .and_then(|n| n.iter().find(|(slot, _)| *slot as usize == i))
                            {
                                Some((_, name)) => format!("{name}={value:?}"),
                                None => format!("[{i}]={value:?}"),
                            }
                        })
                        .collect();
                    println!("  locals  {}", rendered.join(", "));
                }
                if !top.stack.is_empty() {
                    println!("  stack   {}", render(top.stack));
                }
                for frame in frames.iter().skip(1).take(8) {
                    let at = self
                        .where_in_source(frame.method, frame.il_offset)
                        .unwrap_or_else(|| format!("IL_{:04X}", frame.il_offset));
                    println!("     at {} · {}", frame.method_name, at);
                }
            }
            // A step budget, so `--step` on a real program terminates rather
            // than printing until the heat death of the universe.
            if self.step && self.stops < 64 {
                DebugCommand::StepInto
            } else if self.step {
                println!("\n(step budget reached; running to completion)");
                DebugCommand::Detach
            } else {
                DebugCommand::Run
            }
        }
    }

    interp.attach_debugger(Box::new(Tracer {
        step,
        stops: 0,
        source: source_of,
        names: names_of,
    }));
    let code = interp.run_entry_point(assembly)?;
    Ok(code)
}

/// A short rendering of a slice of values, for the trace above.
fn render(values: &[rustclr_core::Value]) -> String {
    values
        .iter()
        .map(|v| format!("{v:?}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `rustnet profile` — run under the sampling profiler and report the result.
///
/// **Everything is interpreted unless `--jit` is given.** The profiler samples
/// between IL instructions, and compiled code does not execute IL: with the
/// code generator on, the time a compiled method spends is charged to whatever
/// interpreted frame called into it, which is a plausible-looking wrong answer
/// rather than a missing one. Interpreting instead changes the absolute numbers
/// — a profile is a shape, not a benchmark — but keeps them attributable.
pub fn profile(
    path: &str,
    args: Vec<String>,
    top: usize,
    period_us: u64,
    json: bool,
    jit: bool,
    board: Option<&str>,
) -> Result<i32> {
    let mut interp = runtime(args);
    if let Some(name) = board {
        install_board(&mut interp, name)?;
    }
    if jit {
        // Said plainly rather than left in the report's footnotes: a profile
        // taken this way attributes compiled time to the wrong method.
        eprintln!(
            "[rustnet] --jit: methods the code generator takes do not execute IL, so their"
        );
        eprintln!(
            "[rustnet] time is charged to the interpreted frame that called them."
        );
        interp.native_tier = Some(Box::new(rustclr_jit::JitTier::new()));
    }
    let id = interp.loader.load_from_file(path)?;
    rustclr_interop::install(&mut interp);

    interp.profiler = Some(rustclr_core::profile::Profiler::new(
        &interp.loader.registry,
        period_us.saturating_mul(1_000),
    ));

    let started = std::time::Instant::now();
    let outcome = interp.run_entry_point(id);
    let elapsed = started.elapsed();
    let now = interp.host.monotonic_nanos();
    if let Some(profiler) = interp.profiler.as_mut() {
        profiler.finish(now);
    }

    let exit_code = match outcome {
        Ok(code) => code,
        Err(e) => {
            report_failure(&interp, &e);
            1
        }
    };

    let report = interp
        .profiler
        .as_ref()
        .expect("the profiler was installed")
        .report(&interp.loader.registry);

    if json {
        println!("{}", profile_json(&report, &interp, elapsed));
    } else {
        print_profile(&report, &interp, elapsed, top);
    }
    Ok(exit_code)
}

/// Nanoseconds as a human reads them.
fn duration(nanos: u64) -> String {
    if nanos >= 1_000_000_000 {
        format!("{:.2} s", nanos as f64 / 1e9)
    } else if nanos >= 1_000_000 {
        format!("{:.1} ms", nanos as f64 / 1e6)
    } else if nanos >= 1_000 {
        format!("{:.1} us", nanos as f64 / 1e3)
    } else {
        format!("{nanos} ns")
    }
}

/// A count with thousands separators — `4,000,031` reads, `4000031` does not.
fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// A proportion as a short bar, so the shape is readable without arithmetic.
fn bar(percent: f64) -> String {
    let filled = ((percent / 100.0) * 12.0).round().clamp(0.0, 12.0) as usize;
    let mut out = String::new();
    for i in 0..12 {
        out.push(if i < filled { '#' } else { '.' });
    }
    out
}

fn print_profile(
    report: &rustclr_core::profile::Report,
    interp: &Interpreter,
    elapsed: std::time::Duration,
    top: usize,
) {
    let heap = interp.heap.stats();

    println!("\n─── where the time went ────────────────────────────────────────────");
    if report.samples == 0 {
        // Better than an empty table: the reason is always the same one.
        println!(
            "  no samples — the program finished inside one sampling period ({}).",
            duration(report.period_nanos)
        );
        println!("  Lower it with --period-us.");
    } else {
        println!("  {:<44} {:>9}  {:>7}  {:>13}", "method", "self", "share", "calls");
        for row in report.methods.iter().take(top) {
            let percent = report.percent(row.profile.self_nanos);
            println!(
                "  {:<44} {:>9}  {:>6.1}%  {:>13}",
                truncate(&row.name, 44),
                duration(row.profile.self_nanos),
                percent,
                grouped(row.profile.calls),
            );
        }
        if report.methods.len() > top {
            println!("  ... {} more methods", report.methods.len() - top);
        }

        // Inclusive time answers a different question — "what is this call
        // tree costing me" — and the two orders rarely agree.
        let mut by_inclusive: Vec<_> = report.methods.iter().collect();
        by_inclusive.sort_by(|a, b| b.profile.inclusive_nanos.cmp(&a.profile.inclusive_nanos));
        println!("\n  by inclusive time (the method and everything it called)");
        for row in by_inclusive.iter().take(top.min(8)) {
            let percent = report.percent(row.profile.inclusive_nanos);
            println!(
                "  {:<44} {:>9}  {:>6.1}%  {}",
                truncate(&row.name, 44),
                duration(row.profile.inclusive_nanos),
                percent,
                bar(percent),
            );
        }
    }

    println!("\n─── what was allocated ─────────────────────────────────────────────");
    let (total_count, total_bytes) = report.total_allocations();
    if total_count == 0 {
        println!("  nothing was allocated");
    } else {
        println!("  {:<44} {:>11}  {:>7}  {:>9}", "type", "bytes", "share", "objects");
        for row in report.types.iter().take(top) {
            let percent = if total_bytes == 0 {
                0.0
            } else {
                row.profile.bytes as f64 * 100.0 / total_bytes as f64
            };
            println!(
                "  {:<44} {:>11}  {:>6.1}%  {:>9}",
                truncate(&row.name, 44),
                bytes(row.profile.bytes),
                percent,
                grouped(row.profile.count),
            );
        }
        if report.types.len() > top {
            println!("  ... {} more types", report.types.len() - top);
        }
    }

    println!("\n─── memory ─────────────────────────────────────────────────────────");
    println!("  collector             {:>18}", interp.heap.collector_name());
    println!("  allocated             {:>18}", bytes(total_bytes));
    println!("  objects               {:>18}", grouped(total_count));
    println!("  peak live             {:>18}", bytes(heap.peak_live_bytes as u64));
    println!("  live at exit          {:>18}", bytes(interp.heap.live_bytes() as u64));
    println!("  collections           {:>18}", grouped(report.gc.collections));
    if report.gc.collections > 0 {
        let share = if elapsed.as_nanos() > 0 {
            report.gc.total_nanos as f64 * 100.0 / elapsed.as_nanos() as f64
        } else {
            0.0
        };
        println!(
            "  time collecting       {:>18}   ({share:.1}% of the run)",
            duration(report.gc.total_nanos)
        );
        println!("  longest pause         {:>18}", duration(report.gc.longest_nanos));
        println!("  reclaimed             {:>18}", grouped(heap.total_objects_freed));
    }

    println!("\n─── run ────────────────────────────────────────────────────────────");
    println!("  wall clock            {:>18}", duration(elapsed.as_nanos() as u64));
    println!("  sampled               {:>18}", duration(report.wall_nanos));
    println!(
        "  samples               {:>18}   (period {})",
        grouped(report.samples),
        duration(report.period_nanos)
    );
    println!("  IL instructions       {:>18}", grouped(interp.stats.instructions_executed));
}

/// The same report as JSON, for an IDE or a script.
///
/// Hand-written like the debug adapter's, and for the same reason: the
/// workspace has no external dependencies.
fn profile_json(
    report: &rustclr_core::profile::Report,
    interp: &Interpreter,
    elapsed: std::time::Duration,
) -> String {
    let mut out = String::from("{\n");
    out.push_str(&format!("  \"wallNanos\": {},\n", elapsed.as_nanos() as u64));
    out.push_str(&format!("  \"sampledNanos\": {},\n", report.wall_nanos));
    out.push_str(&format!("  \"samples\": {},\n", report.samples));
    out.push_str(&format!("  \"periodNanos\": {},\n", report.period_nanos));
    out.push_str(&format!("  \"instructions\": {},\n", interp.stats.instructions_executed));
    out.push_str("  \"methods\": [\n");
    for (i, row) in report.methods.iter().enumerate() {
        out.push_str(&format!(
            "    {{\"name\": {}, \"selfNanos\": {}, \"inclusiveNanos\": {}, \"calls\": {}}}{}\n",
            crate::json::Json::str(row.name.clone()).write(),
            row.profile.self_nanos,
            row.profile.inclusive_nanos,
            row.profile.calls,
            if i + 1 == report.methods.len() { "" } else { "," },
        ));
    }
    out.push_str("  ],\n  \"types\": [\n");
    for (i, row) in report.types.iter().enumerate() {
        out.push_str(&format!(
            "    {{\"name\": {}, \"bytes\": {}, \"count\": {}}}{}\n",
            crate::json::Json::str(row.name.clone()).write(),
            row.profile.bytes,
            row.profile.count,
            if i + 1 == report.types.len() { "" } else { "," },
        ));
    }
    out.push_str("  ],\n");
    out.push_str(&format!(
        "  \"gc\": {{\"collections\": {}, \"totalNanos\": {}, \"longestNanos\": {}}},\n",
        report.gc.collections, report.gc.total_nanos, report.gc.longest_nanos
    ));
    let heap = interp.heap.stats();
    out.push_str(&format!(
        "  \"heap\": {{\"peakLiveBytes\": {}, \"liveBytesAtExit\": {}, \"objectsFreed\": {}}}\n",
        heap.peak_live_bytes,
        interp.heap.live_bytes(),
        heap.total_objects_freed
    ));
    out.push('}');
    out
}

/// Keeps a long qualified name inside a column, cutting the *front* — the
/// distinguishing part of `Namespace.Type::Method` is at the end.
fn truncate(name: &str, width: usize) -> String {
    if name.chars().count() <= width {
        return name.to_string();
    }
    let kept: String = name.chars().skip(name.chars().count() - (width - 1)).collect();
    format!("~{kept}")
}
