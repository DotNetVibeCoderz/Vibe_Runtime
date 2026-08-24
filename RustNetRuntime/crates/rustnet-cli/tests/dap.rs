//! The debug adapter, driven the way an editor drives it.
//!
//! Spawns `rustnet debug --dap` and speaks the protocol to it over a pipe:
//! `initialize`, `setBreakpoints`, `configurationDone`, then answers to the
//! `stopped` event. Nothing here mocks the adapter — the point is that a real
//! process, framed messages and all, does what VSCode would expect.

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/Conformance/bin/Release/net10.0/Conformance.dll"
);
const SOURCE: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/Conformance/Program.cs");

const HELLO: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/HelloWorld/bin/Release/net10.0/HelloWorld.dll"
);

fn binary() -> std::path::PathBuf {
    // The test binary lives beside the CLI it was built with.
    let mut path = std::env::current_exe().expect("test binary path");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join(if cfg!(windows) { "rustnet.exe" } else { "rustnet" })
}

struct Session {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    seq: i64,
}

impl Session {
    fn start() -> Option<Self> {
        Session::start_on(FIXTURE)
    }

    fn start_on(fixture: &str) -> Option<Self> {
        let binary = binary();
        if !std::path::Path::new(fixture).exists() || !binary.exists() {
            eprintln!("skipping: fixture or rustnet binary not built");
            return None;
        }
        let mut child = Command::new(binary)
            .args(["debug", "--dap", fixture])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("rustnet starts");
        let input = child.stdin.take().unwrap();
        let output = BufReader::new(child.stdout.take().unwrap());
        Some(Session { child, input, output, seq: 1 })
    }

    fn request(&mut self, command: &str, arguments: &str) {
        let seq = self.seq;
        self.seq += 1;
        let body = format!(
            r#"{{"seq":{seq},"type":"request","command":"{command}","arguments":{arguments}}}"#
        );
        write!(self.input, "Content-Length: {}\r\n\r\n{}", body.len(), body).unwrap();
        self.input.flush().unwrap();
    }

    /// Reads one framed message, returning its raw text.
    fn read(&mut self) -> Option<String> {
        let mut length = None;
        loop {
            let mut line = String::new();
            if self.output.read_line(&mut line).ok()? == 0 {
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
        self.output.read_exact(&mut body).ok()?;
        Some(String::from_utf8(body).ok()?)
    }

    /// Reads until a message contains `needle`, returning it.
    fn read_until(&mut self, needle: &str) -> Option<String> {
        for _ in 0..200 {
            let message = self.read()?;
            if message.contains(needle) {
                return Some(message);
            }
        }
        None
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn line_of(needle: &str) -> u32 {
    std::fs::read_to_string(SOURCE)
        .unwrap()
        .lines()
        .position(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("the fixture contains `{needle}`")) as u32
        + 1
}

#[test]
fn an_editor_can_initialize_set_a_breakpoint_and_read_the_stack() {
    let Some(mut session) = Session::start() else { return };

    session.request("initialize", r#"{"adapterID":"rustnet"}"#);
    let response = session.read_until(r#""command":"initialize""#).expect("initialize answered");
    assert!(
        response.contains("supportsConfigurationDoneRequest"),
        "the adapter must say which capabilities it has: {response}"
    );
    session.read_until(r#""event":"initialized""#).expect("initialized event");

    // A breakpoint on `Triple`, by source line, exactly as an editor sends it.
    let line = line_of("static int Triple(int v)");
    session.request(
        "setBreakpoints",
        &format!(
            r#"{{"source":{{"path":"{}"}},"breakpoints":[{{"line":{line}}}]}}"#,
            SOURCE.replace('\\', "\\\\")
        ),
    );
    let response = session.read_until(r#""command":"setBreakpoints""#).expect("answered");
    assert!(
        response.contains(r#""verified":true"#),
        "a line with code must verify: {response}"
    );

    session.request("configurationDone", "{}");
    session.read_until(r#""command":"configurationDone""#).expect("answered");
    session.request("launch", "{}");

    // The program runs and stops at the breakpoint.
    let stopped = session.read_until(r#""event":"stopped""#).expect("stopped event");
    assert!(
        stopped.contains(r#""reason":"breakpoint""#),
        "stopped for the wrong reason: {stopped}"
    );

    // The stack must name the method and the line the breakpoint was set on.
    session.request("stackTrace", r#"{"threadId":1}"#);
    let stack = session.read_until(r#""command":"stackTrace""#).expect("stack answered");
    assert!(stack.contains("Triple"), "the stack does not name Triple: {stack}");
    assert!(
        stack.contains(&format!(r#""line":{line}"#)),
        "the stack does not report line {line}: {stack}"
    );
    assert!(
        stack.contains("Program.cs"),
        "the stack has no source file: {stack}"
    );

    // Arguments are readable, and `Triple(int v)` was called with something.
    session.request("scopes", r#"{"frameId":0}"#);
    let scopes = session.read_until(r#""command":"scopes""#).expect("scopes answered");
    assert!(scopes.contains("Arguments") && scopes.contains("Locals"), "{scopes}");

    session.request("variables", r#"{"variablesReference":1}"#);
    let variables = session.read_until(r#""command":"variables""#).expect("variables answered");
    assert!(
        variables.contains("arg0"),
        "the argument was not reported: {variables}"
    );

    session.request("continue", r#"{"threadId":1}"#);
    session.read_until(r#""command":"continue""#).expect("continue answered");
}

#[test]
fn a_line_with_no_code_is_reported_unverified() {
    let Some(mut session) = Session::start() else { return };
    session.request("initialize", r#"{"adapterID":"rustnet"}"#);
    session.read_until(r#""event":"initialized""#).expect("initialized");

    // Line 1 is a using directive or a comment. An editor should see the
    // breakpoint refused rather than silently moved to a line with code.
    session.request(
        "setBreakpoints",
        &format!(
            r#"{{"source":{{"path":"{}"}},"breakpoints":[{{"line":1}}]}}"#,
            SOURCE.replace('\\', "\\\\")
        ),
    );
    let response = session.read_until(r#""command":"setBreakpoints""#).expect("answered");
    assert!(
        response.contains(r#""verified":false"#),
        "a line with no code must not verify: {response}"
    );
}

#[test]
fn stepping_moves_one_line_at_a_time() {
    let Some(mut session) = Session::start() else { return };
    session.request("initialize", r#"{"adapterID":"rustnet"}"#);
    session.read_until(r#""event":"initialized""#).expect("initialized");

    let line = line_of("static int TwoLevels(int v)");
    session.request(
        "setBreakpoints",
        &format!(
            r#"{{"source":{{"path":"{}"}},"breakpoints":[{{"line":{line}}}]}}"#,
            SOURCE.replace('\\', "\\\\")
        ),
    );
    session.read_until(r#""command":"setBreakpoints""#).expect("answered");
    session.request("configurationDone", "{}");
    session.read_until(r#""command":"configurationDone""#).expect("answered");

    session.read_until(r#""event":"stopped""#).expect("stopped at the breakpoint");

    // Step in: the next stop must be reported as a step, and the stack must
    // still be answerable — the frames are rebuilt at every stop, and one that
    // was not would answer about the previous one.
    session.request("stepIn", r#"{"threadId":1}"#);
    session.read_until(r#""command":"stepIn""#).expect("stepIn answered");
    let stopped = session.read_until(r#""event":"stopped""#).expect("stopped after the step");
    assert!(stopped.contains(r#""reason":"step""#), "{stopped}");

    session.request("stackTrace", r#"{"threadId":1}"#);
    let stack = session.read_until(r#""command":"stackTrace""#).expect("stack answered");
    assert!(stack.contains("stackFrames"), "{stack}");
    assert!(
        stack.contains("Program.cs"),
        "the frames lost their source after a step: {stack}"
    );
}

#[test]
fn the_programs_console_becomes_output_events() {
    // Under `--dap` stdout carries the protocol. A `Console.WriteLine` that
    // went there directly would land inside a framed message and the reader
    // below would fail to parse the stream at all — so this test failing to
    // *read* is itself the symptom it is looking for.
    let Some(mut session) = Session::start_on(HELLO) else { return };
    session.request("initialize", r#"{"adapterID":"rustnet"}"#);
    session.read_until(r#""event":"initialized""#).expect("initialized");
    session.request("configurationDone", "{}");
    session.read_until(r#""command":"configurationDone""#).expect("answered");

    let output = session
        .read_until(r#""event":"output""#)
        .expect("the program's console reaches the editor");
    assert!(
        output.contains(r#""category":"stdout""#),
        "console output must be categorised: {output}"
    );
    assert!(
        output.contains("Hello"),
        "the program printed something else entirely: {output}"
    );

    // And the session ends by saying so, or the editor keeps spinning.
    session.read_until(r#""event":"terminated""#).expect("terminated event");
}

#[test]
fn the_whole_conformance_fixture_runs_under_the_adapter() {
    // The check that matters, over the protocol: a real Roslyn assembly runs
    // to completion with an editor attached, and prints what it prints on
    // `dotnet`. A stream corrupted anywhere along the way fails to frame, so
    // this also fails if anything writes to stdout that should not.
    let Some(mut session) = Session::start() else { return };
    session.request("initialize", r#"{"adapterID":"rustnet"}"#);
    session.read_until(r#""event":"initialized""#).expect("initialized");
    session.request("configurationDone", "{}");
    session.read_until(r#""command":"configurationDone""#).expect("answered");

    let mut printed = String::new();
    for _ in 0..2000 {
        let Some(message) = session.read() else {
            panic!("the stream ended before the program did:\n{printed}")
        };
        printed.push_str(&message);
        if message.contains(r#""event":"terminated""#) {
            assert!(
                printed.contains("checks=345 failures=0"),
                "the fixture did not report a clean run:\n{printed}"
            );
            return;
        }
    }
    panic!("the program never terminated:\n{printed}");
}
