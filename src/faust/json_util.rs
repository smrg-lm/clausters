//! Validation helpers shared by the JSON→Box ([`crate::faust::boxes`]) and
//! JSON→Signal ([`crate::faust::signals`]) interpreters: arity/field checks
//! and error messages that carry the path of the offending node from the
//! root `$` (e.g. `at $.in[1]: …`).

use std::ffi::{CString, c_char};
use std::fmt::Display;

use serde_json::{Map, Value};

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
    let Some(label) = obj.get("label").and_then(Value::as_str) else {
        return Err(err(path, "needs a string \"label\""));
    };
    let label_c = CString::new(label).map_err(|_| err(path, "NUL byte in \"label\""))?;
    cstrings.push(label_c);
    Ok(cstrings.last().unwrap().as_ptr())
}
