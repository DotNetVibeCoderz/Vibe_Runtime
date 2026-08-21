//! Command implementations.

use rustclr_core::{
    metadata::{Machine, TableId},
    opcode::{decode_all, Operand},
    ExecutionError, Interpreter, MethodKind, SystemHost, TypeKind,
};
use std::error::Error;
use std::path::Path;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// Builds an interpreter with the full runtime installed.
fn runtime(args: Vec<String>) -> Interpreter {
    runtime_with_bcl(args, false)
}

/// The runtime, with either the whole BCL or the subset a small board can hold.
///
/// `minimal` is not a smaller *runtime* — the loader and interpreter are
/// identical. It registers 314 native bindings instead of 821, which is what a
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
) -> Result<i32> {
    let mut interp = runtime_with_bcl(args, minimal_bcl);
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
        if let Some(e) = interp.heap.get_as::<rustclr_core::ClrException>(*object) {
            for frame in &e.stack_trace {
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
    if let Some(i) = info.signature.params.iter().position(|p| !is_integer_sig(p)) {
        return format!("parameter {i} is not an integer");
    }
    if let Some(i) = body.locals.iter().position(|l| !is_integer_sig(l)) {
        return format!("local {i} is not an integer");
    }
    match rustclr_core::opcode::decode_all(&body.il) {
        Ok(instructions) => {
            let mut unsupported: Vec<String> = instructions
                .iter()
                .filter(|i| !rustclr_jit::x64::is_supported(i.op))
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
    run(&assembly.to_string_lossy(), Vec::new(), false, false, None, true, None, true, false)
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
    println!("  native JIT backend           x86-64, INTEGER METHODS ONLY:");
    println!("                               integer arithmetic, comparison, branching,");
    println!("                               arguments and locals. No allocation, exception");
    println!("                               handling, floating point or object access.");
    println!("                               Everything else is interpreted, and");
    println!("                               `rustnet jit <assembly>` says which is which.");
    println!("  code memory                  write-xor-execute; never both at once");
    println!("  tiering                      compile after 32 calls; --no-jit disables");
    println!("  inlining                     yes - branch-free static callees, one level");
    println!("                               deep; --no-inline disables");
    println!("  AArch64, RISC-V backends     EMIT ONLY, NEVER EXECUTED. Both encode the");
    println!("                               same IL the x86-64 backend does and are");
    println!("                               checked by disassembly, but no compiled");
    println!("                               method has ever run on either architecture.");
    println!("                               Only x86-64 is dispatched to at runtime.");
    println!("  managed call depth limit     {}", interp.limits.max_frames);
    println!("\nMemory");
    println!("  collector                    {}", interp.heap.collector_name());
    println!("  pluggable collectors         yes");
    println!("  object pinning for interop   yes");
    println!("\nType system");
    println!("  classes, interfaces          yes");
    println!("  virtual and interface calls  yes");
    println!("  value types, enums           yes");
    println!("  delegates                    yes (unicast and multicast)");
    println!("  generics                     erased to object");
    println!("  generic methods              instantiations bind by type argument");
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
    println!("                               Assembly.Load is NOT here: it needs a search");
    println!("                               path, and a chip has no filesystem.");
    println!("                               Constructing a generic type at run time is");
    println!("                               blocked by erasure, not by reflection.");
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
    println!("  async / await                yes, but SYNCHRONOUS:");
    println!("                               a task runs to completion where it is");
    println!("                               created, so results, ordering and exception");
    println!("                               propagation are correct but nothing ever");
    println!("                               overlaps. Code relying on two tasks");
    println!("                               interleaving will not interleave.");
    println!("  Task, Task<T>, WhenAll       yes");
    println!("  TaskCompletionSource         yes - a real suspend and resume");
    println!("  Task Parallel Library        no - Parallel.For is unimplemented");
    println!("  Thread, lock, Interlocked    yes, but SERIALISED:");
    println!("                               Thread.Start runs the body on the calling");
    println!("                               thread, and Join returns at once. Correct");
    println!("                               for start-then-join; a program that needs");
    println!("                               two threads to progress together will hang.");

    println!("\nMemory and resources");
    println!("  IDisposable, using           yes");
    println!("  IAsyncDisposable             no - await using is unimplemented");
    println!("  Span<T>, Memory<T>           no - generic ref structs. Slicing,");
    println!("                               indexing and stackalloc all refuse.");
    println!("                               ONE PATH IS IMPLEMENTED: `string + char`");
    println!("                               lowers through ReadOnlySpan<char> on");
    println!("                               .NET 10, so op_Implicit, the one-element");
    println!("                               ctor and Concat over spans exist and a");
    println!("                               span over chars is the string it stands");
    println!("                               for. That is a representation for string");
    println!("                               building, not a span type.");
    println!("  stackalloc, unsafe pointers  no - localloc is unimplemented, and managed");
    println!("                               references here are structural, not addresses");

    println!("\nCompile-time features");
    println!("  source generators            yes - the runtime only ever sees the IL");
    println!("  interceptors                 yes - same reason");
    println!("\nExceptions");
    println!("  try / catch / finally        yes");
    println!("  exception filters            yes - `catch when` runs mid-unwind;");
    println!("                               a throwing filter declines");
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
    println!("  IL EXECUTION ON A CHIP       yes - verified on an ESP32-C3 (RISC-V,");
    println!("                               400 KB SRAM, no OS). Loader, interpreter and");
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
    println!("  run on real hardware         ESP32-C3 executes IL. The Xtensa and");
    println!("                               Cortex-M7 boards were last flashed before");
    println!("                               the interpreter landed, so their captures");
    println!("                               show metadata and GC only. The K210, both");
    println!("                               STM32F4 boards and the Pico build but were");
    println!("                               never flashed - no board was connected.");
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
