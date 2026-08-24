//! `rustnet` — the RustNet Toolchain.
//!
//! Build, run, inspect and verify .NET assemblies on RustCLR.

mod args;
mod commands;
mod dap;
mod json;
mod png;
mod serve;

use args::Command;
use std::process::ExitCode;

/// Machine stack reserved for the thread that runs managed code.
///
/// Compiled recursion runs on the *machine* stack rather than on a frame
/// budget the runtime can grow, so the depth a managed program can reach is
/// bounded by however much stack the host happened to give this thread — a
/// megabyte on Windows by default, which is less than the interpreter's own
/// 4096-frame budget needs. A program that ran interpreted would then fail
/// compiled, which is the one difference this runtime does not accept.
///
/// So the runtime asks for the stack rather than hoping for it. Reserved
/// address space, not committed memory: the cost of a large number here is
/// close to nothing until the pages are touched.
///
/// An embedder calling the interpreter directly is responsible for the same
/// thing; see `rustclr_jit`'s stack budget for the figure it must cover.
const MANAGED_STACK_BYTES: usize = 64 * 1024 * 1024;

fn main() -> ExitCode {
    match std::thread::Builder::new()
        .stack_size(MANAGED_STACK_BYTES)
        .name("rustnet".into())
        .spawn(run)
    {
        Ok(handle) => match handle.join() {
            Ok(code) => code,
            // The managed thread panicked; it has already printed why.
            Err(_) => ExitCode::from(70),
        },
        // No thread to be had. Running on this one is still better than not
        // running at all, and only deep compiled recursion would notice.
        Err(_) => run(),
    }
}

fn run() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();

    let command = match args::parse(&argv) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("rustnet: {e}");
            return ExitCode::from(2);
        }
    };

    let result = match command {
        Command::Help => {
            print!("{}", args::HELP);
            Ok(0)
        }
        Command::Version => {
            println!("rustnet {} (RustCLR runtime)", env!("CARGO_PKG_VERSION"));
            println!("built by Gravicode Studios, led by Kang Fadhil");
            Ok(0)
        }
        Command::Capabilities => commands::capabilities(),
        Command::Run {
            assembly,
            args,
            stats,
            trace,
            max_instructions,
            jit,
            jit_threshold,
            inline,
            minimal_bcl,
            board,
            capture_display,
        } => commands::run(
            &assembly,
            args,
            stats,
            trace,
            max_instructions,
            jit,
            jit_threshold,
            inline,
            minimal_bcl,
            board.as_deref(),
            capture_display.as_deref(),
        ),
        Command::Serve { address, image } => serve::serve(&address, image.as_deref()),
        Command::Info { assembly, verbose } => commands::info(&assembly, verbose),
        Command::Disasm { assembly, filter } => commands::disasm(&assembly, filter.as_deref()),
        Command::Verify { assembly } => commands::verify(&assembly),
        Command::Jit { assembly } => commands::jit(&assembly),
        Command::Profile { assembly, args, top, period_us, json, jit, board } => {
            commands::profile(&assembly, args, top, period_us, json, jit, board.as_deref())
        }
        Command::Debug { assembly, breakpoints, step, dap } => {
            commands::debug(&assembly, &breakpoints, step, dap)
        }
        Command::Build { project, configuration, run } => {
            commands::build(&project, &configuration, run)
        }
    };

    match result {
        Ok(code) => ExitCode::from(code as u8),
        Err(e) => {
            eprintln!("rustnet: {e}");
            ExitCode::FAILURE
        }
    }
}
