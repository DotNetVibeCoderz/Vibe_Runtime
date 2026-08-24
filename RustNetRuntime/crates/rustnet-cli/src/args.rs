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
        /// Install only the bindings a small board can hold.
        minimal_bcl: bool,
        /// Which board `RustNet.Hal` talks to. `None` means no hardware, and
        /// a program that reaches for a pin is told so rather than ignored.
        board: Option<String>,
    },
    /// Summarise an assembly's metadata.
    Info { assembly: String, verbose: bool },
    /// Report what the code generator can compile.
    Jit { assembly: String },
    /// Run under the sampling profiler and report where the time went.
    Profile {
        assembly: String,
        args: Vec<String>,
        /// Rows to print in each table.
        top: usize,
        /// Sampling period in microseconds.
        period_us: u64,
        /// Emit the report as JSON instead of tables.
        json: bool,
        /// Let the code generator compile hot methods. Off by default: the
        /// profiler cannot see inside compiled code.
        jit: bool,
    },
    /// Run under the debugger hooks, tracing every stop.
    Debug { assembly: String, breakpoints: Vec<(String, u32)>, step: bool, dap: bool },
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
            let mut minimal_bcl = false;
            let mut board = None;
            let mut rest = argv[1..].iter();

            while let Some(a) = rest.next() {
                match a.as_str() {
                    "--stats" => stats = true,
                    "--trace" => trace = true,
                    "--no-jit" => jit = false,
                    "--no-inline" => inline = false,
                    "--bcl" => {
                        let v = rest.next().ok_or_else(|| {
                            ParseError("--bcl needs `full` or `minimal`".into())
                        })?;
                        match v.as_str() {
                            "minimal" => minimal_bcl = true,
                            "full" => minimal_bcl = false,
                            other => {
                                return Err(ParseError(format!(
                                    "--bcl takes `full` or `minimal`, not `{other}`"
                                )))
                            }
                        }
                    }
                    "--board" => {
                        let v = rest.next().ok_or_else(|| {
                            ParseError("--board needs a name; `host` is the simulator".into())
                        })?;
                        board = Some(v.to_string());
                    }
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
                minimal_bcl,
                board,
            })
        }

        "profile" => {
            let mut assembly = None;
            let mut program_args = Vec::new();
            let mut top = 15usize;
            let mut period_us = 1_000u64;
            let json = argv.iter().any(|a| a == "--json");
            let jit = argv.iter().any(|a| a == "--jit");
            let mut rest = argv[1..].iter().peekable();
            while let Some(arg) = rest.next() {
                match arg.as_str() {
                    "--" => {
                        program_args.extend(rest.map(|a| a.to_string()));
                        break;
                    }
                    "--top" => {
                        let value = rest
                            .next()
                            .ok_or_else(|| ParseError("--top needs a count".into()))?;
                        top = value
                            .parse()
                            .map_err(|_| ParseError(format!("`{value}` is not a count")))?;
                    }
                    "--period-us" => {
                        let value = rest
                            .next()
                            .ok_or_else(|| ParseError("--period-us needs microseconds".into()))?;
                        period_us = value.parse().map_err(|_| {
                            ParseError(format!("`{value}` is not a number of microseconds"))
                        })?;
                        if period_us == 0 {
                            return Err(ParseError(
                                "--period-us 0 would sample every instruction; use 1 or more"
                                    .into(),
                            ));
                        }
                    }
                    "--json" | "--jit" => {}
                    other if !other.starts_with('-') && assembly.is_none() => {
                        assembly = Some(other.to_string());
                    }
                    other => {
                        return Err(ParseError(format!("unknown option `{other}` for profile")))
                    }
                }
            }
            let assembly = assembly
                .ok_or_else(|| ParseError("profile needs an assembly path".into()))?;
            Ok(Command::Profile { assembly, args: program_args, top, period_us, json, jit })
        }

        "jit" => {
            let assembly = argv[1..]
                .iter()
                .find(|a| !a.starts_with('-'))
                .cloned()
                .ok_or_else(|| ParseError("jit needs an assembly path".into()))?;
            Ok(Command::Jit { assembly })
        }

        "debug" => {
            let mut breakpoints = Vec::new();
            let mut assembly = None;
            let step = argv.iter().any(|a| a == "--step");
            let dap = argv.iter().any(|a| a == "--dap");
            let mut rest = argv[1..].iter().peekable();
            while let Some(arg) = rest.next() {
                match arg.as_str() {
                    "--break" => {
                        // `Method:offset` or `File.cs:line`; both split on
                        // the last colon, and which one it is falls out of
                        // whether the name ends in `.cs`. A path with a drive
                        // letter therefore still works — `C:\src\P.cs:12`
                        // splits at the colon before `12`, not the one after
                        // `C`.
                        let spec = rest
                            .next()
                            .ok_or_else(|| ParseError("--break needs Method:offset".into()))?;
                        let (name, offset) = spec.rsplit_once(':').ok_or_else(|| {
                            ParseError(format!("`{spec}` is not Method:offset"))
                        })?;
                        let offset: u32 = offset.parse().map_err(|_| {
                            ParseError(format!("`{offset}` is not an IL offset"))
                        })?;
                        breakpoints.push((name.to_string(), offset));
                    }
                    "--step" | "--dap" => {}
                    other if !other.starts_with('-') && assembly.is_none() => {
                        assembly = Some(other.to_string());
                    }
                    other => return Err(ParseError(format!("unknown option `{other}`"))),
                }
            }
            let assembly =
                assembly.ok_or_else(|| ParseError("debug needs an assembly path".into()))?;
            Ok(Command::Debug { assembly, breakpoints, step, dap })
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
                   [--no-jit] [--no-inline] [--bcl full|minimal]
                   [--jit-threshold N] [--board NAME] [-- args...]
        Execute an assembly's entry point on RustCLR. Hot methods the code
        generator can take are compiled to machine code; --no-jit interprets
        everything and --no-inline compiles without splicing small callees.
        All three must produce identical output. --bcl minimal installs only
        the bindings a small board can hold, so a program can be checked
        against those limits before it is flashed. --board says which board
        `RustNet.Hal` reaches; `host` is the simulator, and without it a
        program that touches a pin is told there is none.

    info <assembly> [--verbose]
        Summarise types, methods and references.

    disasm <assembly> [filter]
        Disassemble method bodies to IL. `filter` matches Type.Method.

    verify <assembly>
        Load the assembly and report every member that will not resolve.

    jit <assembly>
        Report which methods the native code generator compiles, and why it
        declines the rest. A declined method is interpreted, not a failure.

    profile <assembly> [--top N] [--period-us N] [--json] [--jit] [-- args...]
        Run under the sampling profiler: where the time went, by method, and
        what was allocated, by type. Everything is interpreted unless --jit is
        given, because the profiler cannot see inside compiled code.

    debug <assembly> [--break File.cs:LINE | --break Method:OFFSET] [--step] [--dap]
        Run under the debugger hooks. --dap speaks the Debug Adapter Protocol
        over stdin and stdout, so VSCode and Rider can attach.

    build [project] [-c Release] [--run]
        Compile a C# project with the .NET SDK, then optionally run it here.

    capabilities
        Report which runtime features are implemented.

    help | version
";
