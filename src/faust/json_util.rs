//! Validation helpers shared by the JSON→Box ([`crate::faust::boxes`]) and
//! JSON→Signal ([`crate::faust::signals`]) interpreters: arity/field checks
//! and error messages that carry the path of the offending node from the
//! root `$` (e.g. `at $.in[1]: …`).

use std::ffi::{CString, c_char};
use std::fmt::Display;

use serde_json::{Map, Value};

use crate::faust::ffi::SType;

pub(crate) fn err(path: &str, why: impl Display) -> String {
    format!("at {path}: {why}")
}

/// Validates the `"in"` array of `op`: present, an array, with length in
/// `min..=max` (`max == usize::MAX` reads as "at least `min`").
pub(crate) fn inputs<'a>(
    obj: &'a Map<String, Value>,
    op: &str,
    path: &str,
    min: usize,
    max: usize,
) -> Result<&'a [Value], String> {
    let Some(field) = obj.get("in") else {
        return Err(err(path, format_args!("`{op}` needs an \"in\" array")));
    };
    let Some(items) = field.as_array() else {
        return Err(err(path, format_args!("`{op}` \"in\" must be an array")));
    };
    if items.len() < min || items.len() > max {
        let expected = if min == max {
            min.to_string()
        } else if max == usize::MAX {
            format!("at least {min}")
        } else {
            format!("{min} to {max}")
        };
        return Err(err(
            path,
            format_args!("`{op}` takes {expected} in \"in\", got {}", items.len()),
        ));
    }
    Ok(items)
}

pub(crate) fn num_field(
    obj: &Map<String, Value>,
    op: &str,
    key: &str,
    path: &str,
) -> Result<f64, String> {
    obj.get(key)
        .and_then(Value::as_f64)
        .ok_or_else(|| err(path, format_args!("`{op}` needs a numeric \"{key}\"")))
}

/// Returns the label as a C pointer kept alive in `cstrings`.
pub(crate) fn label_field(
    obj: &Map<String, Value>,
    path: &str,
    cstrings: &mut Vec<CString>,
) -> Result<*const c_char, String> {
    str_field(obj, "label", path, cstrings)
}

/// Returns the required string field `key` as a C pointer kept alive in
/// `cstrings`.
pub(crate) fn str_field(
    obj: &Map<String, Value>,
    key: &str,
    path: &str,
    cstrings: &mut Vec<CString>,
) -> Result<*const c_char, String> {
    let Some(s) = obj.get(key).and_then(Value::as_str) else {
        return Err(err(path, format_args!("needs a string \"{key}\"")));
    };
    cstr(s, key, path, cstrings)
}

/// Interns `s` as a C string in `cstrings` and returns its pointer.
fn cstr(
    s: &str,
    key: &str,
    path: &str,
    cstrings: &mut Vec<CString>,
) -> Result<*const c_char, String> {
    let c = CString::new(s).map_err(|_| err(path, format_args!("NUL byte in \"{key}\"")))?;
    cstrings.push(c);
    Ok(cstrings.last().unwrap().as_ptr())
}

/// Parses the `ctype`/`name`/`file` fields shared by `fconst`/`fvar` (foreign
/// constant/variable). `ctype` is `"int"` or `"real"`; `file` (the include
/// where the symbol is declared) is optional and defaults to empty.
pub(crate) fn foreign_args(
    obj: &Map<String, Value>,
    op: &str,
    path: &str,
    cstrings: &mut Vec<CString>,
) -> Result<(SType, *const c_char, *const c_char), String> {
    let ty = match obj.get("ctype").and_then(Value::as_str) {
        Some("int") => SType::Int,
        Some("real") => SType::Real,
        _ => {
            return Err(err(
                path,
                format_args!("`{op}` needs \"ctype\": \"int\" or \"real\""),
            ));
        }
    };
    let name = str_field(obj, "name", path, cstrings)?;
    let file = cstr(
        obj.get("file").and_then(Value::as_str).unwrap_or(""),
        "file",
        path,
        cstrings,
    )?;
    Ok((ty, name, file))
}
