//! `System.Console`.
//!
//! All output goes through the [`Host`](rustclr_core::Host) so an embedder —
//! the CodeGen IDE, a test harness — can capture it instead of writing to a
//! terminal.
//!
//! Native methods are plain `fn` pointers, so the renderers below are free
//! functions rather than closures; a closure that captured anything could not
//! coerce to [`NativeFn`](rustclr_core::NativeFn).

use crate::support::*;
use rustclr_core::{ExecResult, Interpreter, Value};

#[allow(unused_imports)]
use crate::prelude::*;


/// Renders argument 0 the way `Object.ToString` would.
fn render_display(i: &mut Interpreter, a: &[Value]) -> ExecResult<String> {
    let v = arg(i, a, 0)?;
    Ok(display(i, &v))
}

fn render_char(i: &mut Interpreter, a: &[Value]) -> ExecResult<String> {
    let c = arg_i32(i, a, 0)?;
    Ok(char::from_u32(c as u32).map(String::from).unwrap_or_default())
}

fn render_bool(i: &mut Interpreter, a: &[Value]) -> ExecResult<String> {
    Ok(bool_text(arg_bool(i, a, 0)?))
}

fn render_single(i: &mut Interpreter, a: &[Value]) -> ExecResult<String> {
    Ok(format_single(arg_f64(i, a, 0)? as f32))
}

fn render_nothing(_i: &mut Interpreter, _a: &[Value]) -> ExecResult<String> {
    Ok(String::new())
}

pub fn register(interp: &mut Interpreter) {
    macro_rules! line {
        ($key:literal, $render:path) => {
            interp.register_native($key, |i, a| {
                let text = $render(i, a)?;
                i.host.write_out(&text);
                i.host.write_out(NEWLINE);
                Ok(None)
            });
        };
    }
    macro_rules! inline {
        ($key:literal, $render:path) => {
            interp.register_native($key, |i, a| {
                let text = $render(i, a)?;
                i.host.write_out(&text);
                Ok(None)
            });
        };
    }

    line!("System.Console::WriteLine()", render_nothing);
    line!("System.Console::WriteLine(string)", render_display);
    line!("System.Console::WriteLine(object)", render_display);
    line!("System.Console::WriteLine(int)", render_display);
    line!("System.Console::WriteLine(uint)", render_display);
    line!("System.Console::WriteLine(long)", render_display);
    line!("System.Console::WriteLine(ulong)", render_display);
    line!("System.Console::WriteLine(double)", render_display);
    line!("System.Console::WriteLine(decimal)", render_display);
    line!("System.Console::WriteLine(char)", render_char);
    line!("System.Console::WriteLine(bool)", render_bool);
    line!("System.Console::WriteLine(float)", render_single);

    inline!("System.Console::Write(string)", render_display);
    inline!("System.Console::Write(object)", render_display);
    inline!("System.Console::Write(int)", render_display);
    inline!("System.Console::Write(uint)", render_display);
    inline!("System.Console::Write(long)", render_display);
    inline!("System.Console::Write(double)", render_display);
    inline!("System.Console::Write(char)", render_char);
    inline!("System.Console::Write(bool)", render_bool);
    inline!("System.Console::Write(float)", render_single);

    // Composite-format overloads.
    interp.register_native("System.Console::WriteLine(string,object)", |i, a| {
        let fmt = arg_string_or_empty(i, a, 0)?;
        let v = arg(i, a, 1)?;
        let text = crate::strings::format_composite(&fmt, &[display(i, &v)]);
        i.host.write_out(&text);
        i.host.write_out(NEWLINE);
        Ok(None)
    });
    interp.register_native("System.Console::WriteLine(string,object,object)", |i, a| {
        let fmt = arg_string_or_empty(i, a, 0)?;
        let mut rendered = Vec::new();
        for k in 1..3 {
            let v = arg(i, a, k)?;
            rendered.push(display(i, &v));
        }
        let text = crate::strings::format_composite(&fmt, &rendered);
        i.host.write_out(&text);
        i.host.write_out(NEWLINE);
        Ok(None)
    });
    interp.register_native("System.Console::WriteLine(string,object[])", |i, a| {
        let fmt = arg_string_or_empty(i, a, 0)?;
        let array = arg_handle(i, a, 1)?;
        let rendered: Vec<String> = array_values(i, array)
            .iter()
            .map(|v| {
                let v = v.clone();
                display(i, &v)
            })
            .collect();
        let text = crate::strings::format_composite(&fmt, &rendered);
        i.host.write_out(&text);
        i.host.write_out(NEWLINE);
        Ok(None)
    });
    interp.register_native("System.Console::Write(string,object)", |i, a| {
        let fmt = arg_string_or_empty(i, a, 0)?;
        let v = arg(i, a, 1)?;
        let text = crate::strings::format_composite(&fmt, &[display(i, &v)]);
        i.host.write_out(&text);
        Ok(None)
    });

    interp.register_native("System.Console::ReadLine()", |i, _a| {
        Ok(Some(match i.host.read_line() {
            Some(line) => {
                let h = i.alloc_string(&line);
                Value::Obj(h)
            }
            None => Value::Null,
        }))
    });
    interp.register_native("System.Console::ReadKey()", |i, _a| {
        Ok(Some(Value::I32(i.host.read_line().and_then(|s| s.chars().next()).map_or(0, |c| c as i32))))
    });

    // Terminal styling has no meaning behind an abstract host; accept and
    // ignore rather than fail a program that only wants coloured output.
    interp.register_native("System.Console::Clear()", |_i, _a| Ok(None));
    interp.register_native("System.Console::ResetColor()", |_i, _a| Ok(None));
    interp.register_native("System.Console::set_ForegroundColor(#0)", |_i, _a| Ok(None));
    interp.register_native("System.Console::set_BackgroundColor(#0)", |_i, _a| Ok(None));
    interp.register_native("System.Console::set_Title(string)", |_i, _a| Ok(None));
}

/// .NET renders booleans with a leading capital.
fn bool_text(b: bool) -> String {
    if b { "True".into() } else { "False".into() }
}
