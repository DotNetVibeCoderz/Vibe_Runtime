//! `rustnet` — the RustNet Toolchain.
//!
//! Build, run, inspect and verify .NET assemblies on RustCLR.

mod args;
mod commands;

use args::Command;
use std::process::ExitCode;

fn main() -> ExitCode {
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
        } => commands::run(
            &assembly,
            args,
            stats,
            trace,
            max_instructions,
            jit,
            jit_threshold,
            inline,
        ),
        Command::Info { assembly, verbose } => commands::info(&assembly, verbose),
        Command::Disasm { assembly, filter } => commands::disasm(&assembly, filter.as_deref()),
        Command::Verify { assembly } => commands::verify(&assembly),
        Command::Jit { assembly } => commands::jit(&assembly),
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
