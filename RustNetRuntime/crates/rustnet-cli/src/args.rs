//! Argument parsing.
//!
//! Hand-rolled rather than pulled from a crate: the CLI has a small, stable
//! surface and this keeps the toolchain dependency-free, which matters when
//! cross-compiling it for the embedded targets the runtime supports.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Execute an assembly's entry point.
    Run {
        assembly: String,
        args: Vec<String>,
        stats: bool,
        trace: bool,
        max_instructions: Option<u64>,
        /// Compile hot methods to machine code. On by default.
        jit: bool,
        /// Calls before a method is compiled; `None` uses the backend default.
        jit_threshold: Option<u32>,
        /// Splice small static callees into their callers. On by default.
        inline: bool,
    },
    /// Summarise an assembly's metadata.
    Info { assembly: String, verbose: bool },
    /// Report what the code generator can compile.
    Jit { assembly: String },
    /// Disassemble methods to IL.
    Disasm {
        assembly: String,
        /// Substring filter on `Type.Method`.
        filter: Option<String>,
    },
    /// Load an assembly and report anything that will not resolve at runtime.
    Verify { assembly: String },
    /// Compile a C# project with the .NET SDK, then run it on RustCLR.
    Build { project: String, configuration: String, run: bool },
    /// Print the runtime's capabilities.
    Capabilities,
    Help,
    Version,
}

#[derive(Debug)]
pub struct ParseError(pub String);

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

pub fn parse(argv: &[String]) -> Result<Command, ParseError> {
    let Some(first) = argv.first().map(String::as_str) else {
        return Ok(Command::Help);
    };

    match first {
        "help" | "-h" | "--help" => Ok(Command::Help),
        "version" | "-V" | "--version" => Ok(Command::Version),
        "capabilities" => Ok(Command::Capabilities),

        "run" => {
            let mut assembly = None;
            let mut args = Vec::new();
            let mut stats = false;
            let mut trace = false;
            let mut max_instructions = None;
            let mut jit = true;
            let mut jit_threshold = None;
            let mut inline = true;
            let mut rest = argv[1..].iter();

            while let Some(a) = rest.next() {
                match a.as_str() {
                    "--stats" => stats = true,
                    "--trace" => trace = true,
                    "--no-jit" => jit = false,
                    "--no-inline" => inline = false,
                    "--jit-threshold" => {
                        let v = rest.next().ok_or_else(|| {
                            ParseError("--jit-threshold needs a value".into())
                        })?;
                        jit_threshold = Some(
                            v.parse()
                                .map_err(|_| ParseError(format!("`{v}` is not a number")))?,
                        );
                    }
                    "--max-instructions" => {
                        let v = rest.next().ok_or_else(|| {
                            ParseError("--max-instructions needs a value".into())
                        })?;
                        max_instructions = Some(
                            v.parse()
                                .map_err(|_| ParseError(format!("`{v}` is not a number")))?,
                        );
                    }
                    "--" => {
                        args.extend(rest.cloned());
                        break;
                    }
                    other if assembly.is_none() => assembly = Some(other.to_string()),
                    other => args.push(other.to_string()),
                }
            }

            Ok(Command::Run {
                assembly: assembly.ok_or_else(|| ParseError("run needs an assembly path".into()))?,
                args,
                stats,
                trace,
                max_instructions,
                jit,
                jit_threshold,
                inline,
            })
        }

        "jit" => {
            let assembly = argv[1..]
                .iter()
                .find(|a| !a.starts_with('-'))
                .cloned()
                .ok_or_else(|| ParseError("jit needs an assembly path".into()))?;
            Ok(Command::Jit { assembly })
        }

        "info" => {
            let verbose = argv.iter().any(|a| a == "--verbose" || a == "-v");
            let assembly = argv[1..]
                .iter()
                .find(|a| !a.starts_with('-'))
                .cloned()
                .ok_or_else(|| ParseError("info needs an assembly path".into()))?;
            Ok(Command::Info { assembly, verbose })
        }

        "disasm" => {
            let positional: Vec<&String> =
                argv[1..].iter().filter(|a| !a.starts_with('-')).collect();
            let assembly = positional
                .first()
                .map(|s| (*s).clone())
                .ok_or_else(|| ParseError("disasm needs an assembly path".into()))?;
            Ok(Command::Disasm {
                assembly,
                filter: positional.get(1).map(|s| (*s).clone()),
            })
        }

        "verify" => {
            let assembly = argv
                .get(1)
                .cloned()
                .ok_or_else(|| ParseError("verify needs an assembly path".into()))?;
            Ok(Command::Verify { assembly })
        }

        "build" => {
            let run = argv.iter().any(|a| a == "--run");
            let configuration = argv
                .iter()
                .position(|a| a == "-c" || a == "--configuration")
                .and_then(|i| argv.get(i + 1))
                .cloned()
                .unwrap_or_else(|| "Release".into());
            let project = argv[1..]
                .iter()
                .find(|a| !a.starts_with('-'))
                .cloned()
                .unwrap_or_else(|| ".".into());
            Ok(Command::Build { project, configuration, run })
        }

        other => Err(ParseError(format!(
            "unknown command `{other}`; try `rustnet help`"
        ))),
    }
}

pub const HELP: &str = "\
rustnet — the RustNet Toolchain for RustCLR

USAGE
    rustnet <command> [options]

COMMANDS
    run <assembly> [--stats] [--trace] [--max-instructions N]
                   [--no-jit] [--no-inline] [--jit-threshold N] [-- args...]
        Execute an assembly's entry point on RustCLR. Hot methods the code
        generator can take are compiled to machine code; --no-jit interprets
        everything and --no-inline compiles without splicing small callees.
        All three must produce identical output.

    info <assembly> [--verbose]
        Summarise types, methods and references.

    disasm <assembly> [filter]
        Disassemble method bodies to IL. `filter` matches Type.Method.

    verify <assembly>
        Load the assembly and report every member that will not resolve.

    jit <assembly>
        Report which methods the native code generator compiles, and why it
        declines the rest. A declined method is interpreted, not a failure.

    build [project] [-c Release] [--run]
        Compile a C# project with the .NET SDK, then optionally run it here.

    capabilities
        Report which runtime features are implemented.

    help | version
";
