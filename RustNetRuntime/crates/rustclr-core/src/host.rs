//! The seam between the runtime and its embedder.
//!
//! Console output, standard input and the clock all go through this trait so
//! that a hosting application — the CodeGen IDE, a test harness, an embedded
//! target with no OS console — can redirect them.

use std::io::Write;

/// Services the runtime needs from its host.
pub trait Host: Send {
    fn write_out(&mut self, text: &str);
    fn write_err(&mut self, text: &str);
    /// Returns `None` at end of input.
    fn read_line(&mut self) -> Option<String>;
    /// Milliseconds since an arbitrary fixed origin, for `Stopwatch`.
    fn monotonic_millis(&mut self) -> u64;
    /// Unix epoch milliseconds, for `DateTime.Now`.
    fn wall_clock_millis(&mut self) -> i64;
    /// Command-line arguments visible to the program.
    fn args(&self) -> &[String] {
        &[]
    }
    /// Requests process exit with a code; the runtime honours it at the next
    /// safe point.
    fn exit(&mut self, code: i32) {
        let _ = code;
    }

    /// Everything written to stdout so far, for hosts that buffer it.
    ///
    /// Streaming hosts return `None`; there is nothing to replay once bytes
    /// have gone to a terminal.
    fn captured_output(&self) -> Option<&str> {
        None
    }

    /// Everything written to stderr so far, for hosts that buffer it.
    fn captured_error(&self) -> Option<&str> {
        None
    }
}

/// The default host: real stdio and the system clock.
pub struct SystemHost {
    args: Vec<String>,
    start: std::time::Instant,
    pub exit_code: Option<i32>,
}

impl SystemHost {
    pub fn new() -> Self {
        Self {
            args: Vec::new(),
            start: std::time::Instant::now(),
            exit_code: None,
        }
    }

    pub fn with_args(args: Vec<String>) -> Self {
        Self { args, ..Self::new() }
    }
}

impl Default for SystemHost {
    fn default() -> Self {
        Self::new()
    }
}

impl Host for SystemHost {
    fn write_out(&mut self, text: &str) {
        let mut out = std::io::stdout().lock();
        let _ = out.write_all(text.as_bytes());
        let _ = out.flush();
    }

    fn write_err(&mut self, text: &str) {
        let mut err = std::io::stderr().lock();
        let _ = err.write_all(text.as_bytes());
        let _ = err.flush();
    }

    fn read_line(&mut self) -> Option<String> {
        let mut buf = String::new();
        match std::io::stdin().read_line(&mut buf) {
            Ok(0) => None,
            Ok(_) => Some(buf.trim_end_matches(['\r', '\n']).to_string()),
            Err(_) => None,
        }
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

/// A host that buffers everything, for tests and for the IDE's output pane.
#[derive(Debug, Default)]
pub struct CaptureHost {
    pub out: String,
    pub err: String,
    pub input: std::collections::VecDeque<String>,
    pub args: Vec<String>,
    pub exit_code: Option<i32>,
    clock: u64,
}

impl CaptureHost {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_input(lines: impl IntoIterator<Item = String>) -> Self {
        Self {
            input: lines.into_iter().collect(),
            ..Self::default()
        }
    }

    /// Everything written to stdout so far.
    pub fn output(&self) -> &str {
        &self.out
    }

    /// Output split into lines, with the trailing empty line removed.
    pub fn lines(&self) -> Vec<&str> {
        let mut v: Vec<&str> = self.out.split('\n').map(|l| l.trim_end_matches('\r')).collect();
        if v.last().is_some_and(|l| l.is_empty()) {
            v.pop();
        }
        v
    }
}

impl Host for CaptureHost {
    fn write_out(&mut self, text: &str) {
        self.out.push_str(text);
    }

    fn write_err(&mut self, text: &str) {
        self.err.push_str(text);
    }

    fn read_line(&mut self) -> Option<String> {
        self.input.pop_front()
    }

    fn monotonic_millis(&mut self) -> u64 {
        // Deterministic clock so tests that measure elapsed time are stable.
        self.clock += 1;
        self.clock
    }

    fn wall_clock_millis(&mut self) -> i64 {
        1_700_000_000_000
    }

    fn args(&self) -> &[String] {
        &self.args
    }

    fn exit(&mut self, code: i32) {
        self.exit_code = Some(code);
    }

    fn captured_output(&self) -> Option<&str> {
        Some(&self.out)
    }

    fn captured_error(&self) -> Option<&str> {
        Some(&self.err)
    }
}
