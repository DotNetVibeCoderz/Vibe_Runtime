//! `System.IO.Path`: the string functions over file names and paths. Pure
//! text work (nothing touches a file system), so they behave the same in the
//! browser and on a board. Both '/' and '\\' separate directories, as on
//! Windows .NET; a null path gives null back, as .NET does.

use rustclr_core::{ExecResult, Interpreter, Value};

#[allow(unused_imports)]
use crate::prelude::*;
use crate::support::*;

fn is_sep(c: char) -> bool {
    c == '/' || c == '\\'
}

/// Argument `i` as a string, or `None` for null.
fn opt_string(interp: &mut Interpreter, args: &[Value], i: usize) -> ExecResult<Option<String>> {
    match arg(interp, args, i)? {
        Value::Null => Ok(None),
        _ => Ok(Some(arg_string_or_empty(interp, args, i)?)),
    }
}

fn opt_result(interp: &mut Interpreter, s: Option<String>) -> Value {
    match s {
        Some(s) => string_value(interp, &s),
        None => Value::Null,
    }
}

/// The part after the last directory separator (or drive colon).
pub fn file_name(path: &str) -> &str {
    match path.rfind(|c: char| is_sep(c) || c == ':') {
        Some(at) => &path[at + 1..],
        None => path,
    }
}

/// The extension with its dot, or "" when the name has none (or ends in a dot).
pub fn extension(path: &str) -> &str {
    let name = file_name(path);
    match name.rfind('.') {
        Some(at) if at + 1 < name.len() => &name[at..],
        _ => "",
    }
}

pub fn without_extension(path: &str) -> &str {
    let name = file_name(path);
    match name.rfind('.') {
        Some(at) => &name[..at],
        None => name,
    }
}

/// Everything before the file name, without the trailing separator; None at a root.
pub fn directory_name(path: &str) -> Option<String> {
    if path.is_empty() {
        return None;
    }
    let at = path.rfind(is_sep)?;
    let dir = path[..at].trim_end_matches(is_sep);
    if dir.is_empty() {
        // "/file": the root itself has no directory name.
        return None;
    }
    Some(dir.to_string())
}

pub fn combine(parts: &[String]) -> String {
    let mut out = String::new();
    for p in parts {
        if p.is_empty() {
            continue;
        }
        // A rooted part starts again, as Path.Combine does.
        if p.starts_with(is_sep) || p.chars().nth(1) == Some(':') {
            out.clear();
        } else if !out.is_empty() && !out.ends_with(is_sep) {
            out.push('/');
        }
        out.push_str(p);
    }
    out
}

pub fn join(parts: &[String]) -> String {
    let mut out = String::new();
    for p in parts {
        if p.is_empty() {
            continue;
        }
        if !out.is_empty() && !out.ends_with(is_sep) && !p.starts_with(is_sep) {
            out.push('/');
        }
        out.push_str(p);
    }
    out
}

/// Combine refuses a null part, as .NET does; Join treats it as empty.
fn parts(interp: &mut Interpreter, args: &[Value], n: usize, strict: bool) -> ExecResult<Option<Value>> {
    let mut list = Vec::with_capacity(n);
    for k in 0..n {
        list.push(if strict { arg_string(interp, args, k)? } else { arg_string_or_empty(interp, args, k)? });
    }
    let joined = if strict { combine(&list) } else { join(&list) };
    Ok(Some(string_value(interp, &joined)))
}

pub fn register(interp: &mut Interpreter) {
    interp.register_native("System.IO.Path::GetFileName(string)", |i, a| {
        let s = opt_string(i, a, 0)?.map(|p| file_name(&p).to_string());
        Ok(Some(opt_result(i, s)))
    });
    interp.register_native("System.IO.Path::GetFileNameWithoutExtension(string)", |i, a| {
        let s = opt_string(i, a, 0)?.map(|p| without_extension(&p).to_string());
        Ok(Some(opt_result(i, s)))
    });
    interp.register_native("System.IO.Path::GetExtension(string)", |i, a| {
        let s = opt_string(i, a, 0)?.map(|p| extension(&p).to_string());
        Ok(Some(opt_result(i, s)))
    });
    interp.register_native("System.IO.Path::HasExtension(string)", |i, a| {
        let s = opt_string(i, a, 0)?.unwrap_or_default();
        Ok(Some(Value::I32(!extension(&s).is_empty() as i32)))
    });
    interp.register_native("System.IO.Path::GetDirectoryName(string)", |i, a| {
        let s = opt_string(i, a, 0)?.and_then(|p| directory_name(&p));
        Ok(Some(opt_result(i, s)))
    });
    interp.register_native("System.IO.Path::ChangeExtension(string,string)", |i, a| {
        let Some(path) = opt_string(i, a, 0)? else { return Ok(Some(Value::Null)) };
        let ext = opt_string(i, a, 1)?;
        let name = file_name(&path);
        let stem_len = path.len() - name.len() + name.rfind('.').unwrap_or(name.len());
        let mut out = path[..stem_len].to_string();
        if let Some(ext) = ext {
            if !ext.starts_with('.') {
                out.push('.');
            }
            out.push_str(&ext);
        }
        Ok(Some(string_value(i, &out)))
    });
    interp.register_native("System.IO.Path::Combine(string,string)", |i, a| parts(i, a, 2, true));
    interp.register_native("System.IO.Path::Combine(string,string,string)", |i, a| parts(i, a, 3, true));
    interp.register_native("System.IO.Path::Combine(string,string,string,string)", |i, a| parts(i, a, 4, true));
    interp.register_native("System.IO.Path::Join(string,string)", |i, a| parts(i, a, 2, false));
    interp.register_native("System.IO.Path::Join(string,string,string)", |i, a| parts(i, a, 3, false));
    interp.register_native("System.IO.Path::Join(string,string,string,string)", |i, a| parts(i, a, 4, false));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_extensions() {
        assert_eq!(file_name("data/kecamatan.geojson"), "kecamatan.geojson");
        assert_eq!(file_name(r"C:\fakepath\a.csv"), "a.csv");
        assert_eq!(without_extension("a/b/report.v2.json"), "report.v2");
        assert_eq!(without_extension("noext"), "noext");
        assert_eq!(extension("a.tar.gz"), ".gz");
        assert_eq!(extension("dot."), "");
        assert_eq!(extension(".gitignore"), ".gitignore");
        assert_eq!(directory_name("a/b/c.txt").as_deref(), Some("a/b"));
        assert_eq!(directory_name("c.txt"), None);
        assert_eq!(directory_name("/c.txt"), None);
    }

    #[test]
    fn combining() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(combine(&s(&["data", "maps", "a.json"])), "data/maps/a.json");
        assert_eq!(combine(&s(&["data/", "a.json"])), "data/a.json");
        assert_eq!(combine(&s(&["data", "/abs/a.json"])), "/abs/a.json");
        assert_eq!(join(&s(&["data", "/abs/a.json"])), "data/abs/a.json");
    }
}
