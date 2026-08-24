//! A Debug Adapter Protocol server, so VSCode and Rider can attach.
//!
//! DAP is a request/response protocol over a stream, framed with
//! `Content-Length:` headers exactly as LSP is. The editor asks for
//! breakpoints, stack frames and variables; the adapter answers, and sends
//! *events* — chiefly `stopped` — when the program does something the editor
//! did not ask about.
//!
//! # How it fits the runtime's hook
//!
//! [`rustclr_core::debug::Debugger::stopped`] is synchronous: it is called with
//! the program stopped and does not return until it says what to do next. That
//! is exactly the shape a debug adapter wants. When the program stops, this
//! sends a `stopped` event and then *keeps reading requests* — answering
//! `stackTrace`, `scopes`, `variables`, `setBreakpoints` — until the editor
//! sends a step or continue, which is what it returns.
//!
//! The consequence, stated rather than discovered: while the program is
//! **running**, no requests are serviced. An editor's pause button does nothing
//! until the next breakpoint. Servicing it would mean reading the stream from
//! another thread and interrupting the interpreter, which is a different design
//! from the one the hook has.
//!
//! # What is resolved up front
//!
//! Everything that needs the loader — which line has code, what a local is
//! called — is computed before the program starts, because the hook hands over
//! frames but not the loader. That also makes `setBreakpoints` answerable while
//! stopped, which an editor does constantly.

use crate::json::{parse, Json};
use rustclr_core::debug::{
    DebugCommand, DebugControl, DebugFrame, Debugger, StopReason,
};
use rustclr_core::{Interpreter, MethodId, MethodKind, Value};
use rustclr_core::host::Host;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::{Arc, Mutex};

/// Everything the adapter needs that only the loader could have told it.
#[derive(Default)]
pub struct SourceMap {
    /// Per method, the sequence points as (IL offset, source path, line).
    ///
    /// The path is the whole one the compiler recorded, not just the file
    /// name: it is what an editor opens. A build moved since compilation
    /// gets a path that does not resolve, which the editor reports — better
    /// than a bare `Program.cs` that could never resolve anywhere.
    lines: HashMap<MethodId, Vec<(u32, String, u32)>>,
    /// Per method, local slot to name.
    names: HashMap<MethodId, Vec<(u16, String)>>,
    /// Lower-cased file name to (line, method, IL offset).
    by_line: HashMap<String, Vec<(u32, MethodId, u32)>>,
}

impl SourceMap {
    /// Reads every method's debug information out of the loader.
    pub fn build(interp: &Interpreter) -> Self {
        let mut map = SourceMap::default();
        let methods: Vec<(MethodId, rustclr_core::AssemblyId, u32)> = interp
            .loader
            .registry
            .iter_methods()
            .filter(|m| matches!(m.kind, MethodKind::Il(_)))
            .map(|m| (m.id, m.assembly, m.token.row()))
            .collect();

        for (id, assembly, row) in methods {
            let Some(pdb) = interp.loader.debug_info(assembly) else { continue };
            let Some(debug) = pdb.method(row) else { continue };

            let mut points = Vec::new();
            for point in &debug.sequence_points {
                let Some(line) = point.start_line else { continue };
                let Some(document) = pdb.document(point.document) else { continue };
                points.push((point.il_offset, document.name.clone(), line));
                // Keyed by file name, so an editor's own path for the file
                // matches whatever directory the assembly was built in.
                map.by_line
                    .entry(file_name(&document.name).to_lowercase())
                    .or_default()
                    .push((line, id, point.il_offset));
            }
            if !points.is_empty() {
                map.lines.insert(id, points);
            }
            let names: Vec<(u16, String)> =
                debug.locals.iter().map(|l| (l.index, l.name.clone())).collect();
            if !names.is_empty() {
                map.names.insert(id, names);
            }
        }
        map
    }

    /// The source position covering an IL offset: the last point at or before
    /// it, as (path, line). `None` where the PDB says nothing, which an editor
    /// renders as a frame without a source location rather than as the wrong
    /// line.
    fn position(&self, method: MethodId, offset: u32) -> Option<(&str, u32)> {
        let points = self.lines.get(&method)?;
        let (_, path, line) = points.iter().take_while(|(at, _, _)| *at <= offset).last()?;
        Some((path.as_str(), *line))
    }

    /// Every place a breakpoint on this file and line belongs.
    ///
    /// All of them, not the first: a lambda and its enclosing method share
    /// lines, and setting only one would silently miss the other.
    fn places(&self, file: &str, line: u32) -> Vec<(MethodId, u32)> {
        let key = file_name(file).to_lowercase();
        let mut found: Vec<(MethodId, u32)> = self
            .by_line
            .get(&key)
            .into_iter()
            .flatten()
            .filter(|(at, _, _)| *at == line)
            .map(|(_, method, offset)| (*method, *offset))
            .collect();
        found.sort();
        found.dedup();
        found
    }

    fn local_name(&self, method: MethodId, slot: usize) -> Option<&str> {
        self.names
            .get(&method)?
            .iter()
            .find(|(index, _)| *index as usize == slot)
            .map(|(_, name)| name.as_str())
    }
}

fn file_name(path: &str) -> String {
    path.rsplit(['/', '\\']).next().unwrap_or(path).to_string()
}

/// The one place anything is written to the editor.
///
/// The adapter and the debuggee's console write to the same stream, so they
/// share a writer and a sequence counter. Each message is framed and flushed
/// while the lock is held, which is what keeps a `Console.WriteLine` from
/// landing in the middle of a `stopped` event.
pub struct Wire<W: Write> {
    out: W,
    seq: i64,
}

impl<W: Write> Wire<W> {
    pub fn new(out: W) -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self { out, seq: 1 }))
    }

    fn send(&mut self, message: Json) {
        let body = message.write();
        let _ = write!(self.out, "Content-Length: {}\r\n\r\n{}", body.len(), body);
        let _ = self.out.flush();
    }

    fn next_seq(&mut self) -> i64 {
        let seq = self.seq;
        self.seq += 1;
        seq
    }

    fn respond(&mut self, request: &Json, body: Json) {
        let seq = self.next_seq();
        self.send(
            Json::obj()
                .set("seq", Json::num(seq as f64))
                .set("type", Json::str("response"))
                .set(
                    "request_seq",
                    Json::num(request.get("seq").and_then(Json::as_i64).unwrap_or(0) as f64),
                )
                .set("success", Json::Bool(true))
                .set(
                    "command",
                    Json::str(request.get("command").and_then(Json::as_str).unwrap_or("")),
                )
                .set("body", body),
        );
    }

    fn event(&mut self, name: &str, body: Json) {
        let seq = self.next_seq();
        self.send(
            Json::obj()
                .set("seq", Json::num(seq as f64))
                .set("type", Json::str("event"))
                .set("event", Json::str(name))
                .set("body", body),
        );
    }
}

/// The debuggee's console, once stdout belongs to the protocol.
///
/// A `Console.WriteLine` written to stdout under `--dap` would land inside a
/// framed message and the editor would drop the connection, so output becomes
/// `output` events instead — which is where an editor shows it anyway, and it
/// arrives live rather than at the next stop.
///
/// Reading answers end-of-input. Stdin carries the editor's requests, and
/// handing the program a line of protocol would be worse than telling it there
/// is no console to read: `Console.ReadLine()` returns `null`, exactly as it
/// does under a redirected empty stdin.
pub struct DapHost<W: Write> {
    wire: Arc<Mutex<Wire<W>>>,
    args: Vec<String>,
    start: std::time::Instant,
    exit_code: Option<i32>,
}

impl<W: Write> DapHost<W> {
    pub fn new(wire: Arc<Mutex<Wire<W>>>, args: Vec<String>) -> Self {
        Self { wire, args, start: std::time::Instant::now(), exit_code: None }
    }

    fn output(&mut self, category: &str, text: &str) {
        if let Ok(mut wire) = self.wire.lock() {
            wire.event(
                "output",
                Json::obj()
                    .set("category", Json::str(category))
                    .set("output", Json::str(text)),
            );
        }
    }
}

impl<W: Write + Send> Host for DapHost<W> {
    fn write_out(&mut self, text: &str) {
        self.output("stdout", text);
    }

    fn write_err(&mut self, text: &str) {
        self.output("stderr", text);
    }

    fn read_line(&mut self) -> Option<String> {
        None
    }

    fn monotonic_millis(&mut self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    fn wall_clock_millis(&mut self) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }

    fn args(&self) -> &[String] {
        &self.args
    }

    fn exit(&mut self, code: i32) {
        self.exit_code = Some(code);
    }
}

/// The adapter. Owns the streams it talks to the editor over.
pub struct DapAdapter<R: Read, W: Write> {
    input: BufReader<R>,
    wire: Arc<Mutex<Wire<W>>>,
    map: SourceMap,
    /// Breakpoints the editor asked for, by file. Kept so a later
    /// `setBreakpoints` for the same file can replace them wholesale, which is
    /// how the protocol works — an editor sends the complete set for a file
    /// every time any one of them changes.
    by_file: HashMap<String, Vec<(MethodId, u32)>>,
    /// Frames of the current stop, flattened for `stackTrace` and `variables`.
    frames: Vec<FrameSnapshot>,
    stopped_once: bool,
}

/// One frame, copied out so it can be answered about after `stopped` returns
/// its borrow. Values are rendered eagerly because rendering needs nothing the
/// snapshot does not already have.
struct FrameSnapshot {
    method: MethodId,
    name: String,
    il_offset: u32,
    arguments: Vec<(String, String)>,
    locals: Vec<(String, String)>,
}

impl<R: Read, W: Write> DapAdapter<R, W> {
    pub fn new(input: R, wire: Arc<Mutex<Wire<W>>>, map: SourceMap) -> Self {
        Self {
            input: BufReader::new(input),
            wire,
            map,
            by_file: HashMap::new(),
            frames: Vec::new(),
            stopped_once: false,
        }
    }

    /// Reads one message. `None` at end of stream, which is how an editor
    /// disconnecting reaches us.
    fn read_message(&mut self) -> Option<Json> {
        let mut length = None;
        loop {
            let mut line = String::new();
            if self.input.read_line(&mut line).ok()? == 0 {
                return None;
            }
            let trimmed = line.trim_end();
            if trimmed.is_empty() {
                break;
            }
            if let Some(value) = trimmed.strip_prefix("Content-Length:") {
                length = value.trim().parse::<usize>().ok();
            }
        }
        let mut body = vec![0u8; length?];
        self.input.read_exact(&mut body).ok()?;
        parse(&String::from_utf8(body).ok()?)
    }

    fn respond(&mut self, request: &Json, body: Json) {
        if let Ok(mut wire) = self.wire.lock() {
            wire.respond(request, body);
        }
    }

    fn event(&mut self, name: &str, body: Json) {
        if let Ok(mut wire) = self.wire.lock() {
            wire.event(name, body);
        }
    }

    /// Handles the requests an editor sends before the program starts.
    ///
    /// Returns false if the editor disconnected instead of launching.
    pub fn handshake(&mut self) -> bool {
        loop {
            let Some(request) = self.read_message() else { return false };
            let command = request.get("command").and_then(Json::as_str).unwrap_or("").to_string();
            match command.as_str() {
                "initialize" => {
                    // Only what is actually implemented is advertised. Claiming
                    // a capability an adapter does not have produces an editor
                    // that silently does nothing, which is worse than one that
                    // greys the button out.
                    let body = Json::obj()
                        .set("supportsConfigurationDoneRequest", Json::Bool(true))
                        .set("supportsTerminateRequest", Json::Bool(true));
                    self.respond(&request, body);
                    self.event("initialized", Json::obj());
                }
                "setBreakpoints" => self.set_breakpoints(&request),
                "setExceptionBreakpoints" => self.respond(&request, Json::obj()),
                "launch" | "attach" => self.respond(&request, Json::obj()),
                "configurationDone" => {
                    self.respond(&request, Json::obj());
                    return true;
                }
                "disconnect" | "terminate" => {
                    self.respond(&request, Json::obj());
                    return false;
                }
                _ => self.respond(&request, Json::obj()),
            }
        }
    }

    fn set_breakpoints(&mut self, request: &Json) {
        let path = request
            .path("arguments.source.path")
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_string();
        let wanted: Vec<i64> = request
            .path("arguments.breakpoints")
            .and_then(Json::as_array)
            .map(|items| {
                items.iter().filter_map(|b| b.get("line").and_then(Json::as_i64)).collect()
            })
            .unwrap_or_default();

        // The editor sends the complete set for a file every time, so the
        // previous set for this file is replaced rather than added to.
        let mut placed = Vec::new();
        let mut verified = Vec::new();
        for line in &wanted {
            let places = self.map.places(&path, *line as u32);
            // `verified: false` is the honest answer for a line with no code —
            // the editor shows the breakpoint hollow rather than pretending it
            // is armed somewhere the user did not point at.
            verified.push(
                Json::obj()
                    .set("verified", Json::Bool(!places.is_empty()))
                    .set("line", Json::num(*line as f64)),
            );
            placed.extend(places);
        }
        self.by_file.insert(file_name(&path).to_lowercase(), placed);
        self.respond(request, Json::obj().set("breakpoints", Json::Arr(verified)));
    }

    /// Every breakpoint currently wanted, across all files.
    fn all_breakpoints(&self) -> Vec<(MethodId, u32)> {
        self.by_file.values().flatten().copied().collect()
    }

    fn stack_trace(&mut self, request: &Json) {
        let frames: Vec<Json> = self
            .frames
            .iter()
            .enumerate()
            .map(|(index, frame)| {
                let mut json = Json::obj()
                    .set("id", Json::num(index as f64))
                    .set("name", Json::str(frame.name.clone()))
                    .set("line", Json::num(0))
                    .set("column", Json::num(0));
                if let Some((path, line)) = self.map.position(frame.method, frame.il_offset) {
                    json = json
                        .set("line", Json::num(line as f64))
                        .set("column", Json::num(1))
                        .set(
                            "source",
                            Json::obj()
                                .set("name", Json::str(file_name(path)))
                                .set("path", Json::str(path)),
                        );
                }
                json
            })
            .collect();
        let total = frames.len();
        self.respond(
            request,
            Json::obj()
                .set("stackFrames", Json::Arr(frames))
                .set("totalFrames", Json::num(total as f64)),
        );
    }

    fn scopes(&mut self, request: &Json) {
        let frame = request.path("arguments.frameId").and_then(Json::as_i64).unwrap_or(0);
        // Two scopes per frame, encoded in the reference so `variables` can
        // tell them apart without holding state.
        let arguments = frame * 2 + 1;
        let locals = frame * 2 + 2;
        self.respond(
            request,
            Json::obj().set(
                "scopes",
                Json::Arr(vec![
                    Json::obj()
                        .set("name", Json::str("Locals"))
                        .set("variablesReference", Json::num(locals as f64))
                        .set("expensive", Json::Bool(false)),
                    Json::obj()
                        .set("name", Json::str("Arguments"))
                        .set("variablesReference", Json::num(arguments as f64))
                        .set("expensive", Json::Bool(false)),
                ]),
            ),
        );
    }

    fn variables(&mut self, request: &Json) {
        let reference = request
            .path("arguments.variablesReference")
            .and_then(Json::as_i64)
            .unwrap_or(0);
        let frame_index = ((reference - 1) / 2) as usize;
        let wants_locals = reference % 2 == 0;

        let variables: Vec<Json> = self
            .frames
            .get(frame_index)
            .map(|frame| if wants_locals { &frame.locals } else { &frame.arguments })
            .map(|values| {
                values
                    .iter()
                    .map(|(name, value)| {
                        Json::obj()
                            .set("name", Json::str(name.clone()))
                            .set("value", Json::str(value.clone()))
                            .set("variablesReference", Json::num(0))
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.respond(request, Json::obj().set("variables", Json::Arr(variables)));
    }
}

impl<R: Read + Send, W: Write + Send> Debugger for DapAdapter<R, W> {
    fn stopped(
        &mut self,
        reason: StopReason,
        frames: &[DebugFrame<'_>],
        control: &mut DebugControl<'_>,
    ) -> DebugCommand {
        // The entry stop exists so an adapter can arm itself before anything
        // runs. An editor did not ask for it and should not see it, so the
        // breakpoints it requested are installed and the program continues.
        if reason == StopReason::Entry && !self.stopped_once {
            self.stopped_once = true;
            control.breakpoints.clear();
            for (method, offset) in self.all_breakpoints() {
                control.breakpoints.add(method, offset);
            }
            return DebugCommand::Run;
        }

        self.frames = frames
            .iter()
            .map(|frame| FrameSnapshot {
                method: frame.method,
                name: frame.method_name.to_string(),
                il_offset: frame.il_offset,
                arguments: frame
                    .arguments
                    .iter()
                    .enumerate()
                    .map(|(i, v)| (format!("arg{i}"), render(v)))
                    .collect(),
                locals: frame
                    .locals
                    .iter()
                    .enumerate()
                    .map(|(i, v)| {
                        let name = self
                            .map
                            .local_name(frame.method, i)
                            .map(String::from)
                            .unwrap_or_else(|| format!("local{i}"));
                        (name, render(v))
                    })
                    .collect(),
            })
            .collect();

        self.event(
            "stopped",
            Json::obj()
                .set(
                    "reason",
                    Json::str(match reason {
                        StopReason::Breakpoint => "breakpoint",
                        StopReason::Step => "step",
                        StopReason::Entry => "entry",
                    }),
                )
                .set("threadId", Json::num(1))
                .set("allThreadsStopped", Json::Bool(true)),
        );

        // Answer questions until told to move. This is the whole reason the
        // hook is synchronous: the frames above are valid for exactly as long
        // as this call, and an editor asking about them later would be asking
        // about a program that had moved on.
        loop {
            let Some(request) = self.read_message() else {
                return DebugCommand::Detach;
            };
            let command =
                request.get("command").and_then(Json::as_str).unwrap_or("").to_string();
            match command.as_str() {
                "threads" => {
                    let body = Json::obj().set(
                        "threads",
                        Json::Arr(vec![Json::obj()
                            .set("id", Json::num(1))
                            .set("name", Json::str("main"))]),
                    );
                    self.respond(&request, body);
                }
                "stackTrace" => self.stack_trace(&request),
                "scopes" => self.scopes(&request),
                "variables" => self.variables(&request),
                "setBreakpoints" => {
                    self.set_breakpoints(&request);
                    // Take effect now, on this same run.
                    control.breakpoints.clear();
                    for (method, offset) in self.all_breakpoints() {
                        control.breakpoints.add(method, offset);
                    }
                }
                "continue" => {
                    self.respond(
                        &request,
                        Json::obj().set("allThreadsContinued", Json::Bool(true)),
                    );
                    return DebugCommand::Run;
                }
                "next" => {
                    self.respond(&request, Json::obj());
                    return DebugCommand::StepOver;
                }
                "stepIn" => {
                    self.respond(&request, Json::obj());
                    return DebugCommand::StepInto;
                }
                "stepOut" => {
                    self.respond(&request, Json::obj());
                    return DebugCommand::StepOut;
                }
                "disconnect" | "terminate" => {
                    self.respond(&request, Json::obj());
                    return DebugCommand::Detach;
                }
                // Anything else is answered emptily rather than ignored: an
                // editor waiting on a response it never gets simply hangs.
                _ => self.respond(&request, Json::obj()),
            }
        }
    }

    fn finished(&mut self, outcome: Result<i32, String>) {
        match outcome {
            Ok(code) => {
                self.event("exited", Json::obj().set("exitCode", Json::num(code as f64)))
            }
            Err(message) => self.event(
                "output",
                Json::obj()
                    .set("category", Json::str("stderr"))
                    .set("output", Json::str(format!("{message}\n"))),
            ),
        }
        self.event("terminated", Json::obj());
    }
}

/// A value as an editor should show it.
fn render(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::I32(v) => v.to_string(),
        Value::I64(v) | Value::NativeInt(v) => v.to_string(),
        Value::F(v) => v.to_string(),
        // Deliberately not dereferenced. Showing an object's fields means
        // reading the heap, which this does not have while stopped — and a
        // handle is at least honest about being one.
        other => format!("{other:?}"),
    }
}
