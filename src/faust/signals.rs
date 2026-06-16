//! JSON → Signal API interpreter (the lower-level Faust API).
//!
//! Where [`crate::faust::boxes`] maps a box-composition algebra, this maps the
//! **Signal API** (`libfaust-signal-c.h`): every node is one signal (one
//! output), inputs are **explicit** (`{"op":"input","index":n}`), delays are
//! explicit (`delay`/`delay1`), and feedback is **explicit** —
//! `{"op":"recursion","in":[body]}` with `{"op":"self"}` inside the body, the
//! `CsigSelf()`/`CsigRecursion()` pair (one implicit sample of delay). This is
//! the sample-accurate feedback fused into one node.
//!
//! # Schema
//!
//! The root is `{"signals": [ <node>, … ]}` — one node per DSP **output**
//! (this is also how a signal def declares >1 output). A bare JSON **number**
//! is a constant (`int` if integral within `i32`, `real` otherwise).
//!
//! | `op` | fields | Signal API |
//! |---|---|---|
//! | `int`, `real` | `value` | `CsigInt` / `CsigReal` |
//! | `input` | `index` | `CsigInput` |
//! | `delay` | `in`: signal, delay | `CsigDelay` |
//! | `delay1` | `in`: 1 signal | `CsigDelay1` |
//! | `recursion` | `in`: 1 body (uses `self`) | `CsigRecursion` |
//! | `self` | — | `CsigSelf` (only valid inside a `recursion` body) |
//! | `add` `sub` `mul` `div` `rem` `fmod` `remainder` `pow` `min` `max` `atan2` `gt` `lt` `ge` `le` `eq` `ne` `and` `or` `xor` `lsh` `rsh` | `in`: 2 signals | binary ops |
//! | `sin` `cos` `tan` `asin` `acos` `atan` `exp` `exp10` `log` `log10` `sqrt` `abs` `floor` `ceil` `rint` `intcast` `floatcast` | `in`: 1 signal | unary functions |
//! | `select2` | `in`: selector, 2 signals | `CsigSelect2` |
//! | `select3` | `in`: selector, 3 signals | `CsigSelect3` |
//! | `hslider` `vslider` `nentry` | `label`, `init`, `min`, `max`, `step` | named control |
//! | `button` `checkbox` | `label` | named control (0/1) |
//! | `hbargraph` `vbargraph` | `label`, `min`, `max`, `in`: 1 signal | passive monitor (passes the signal through) |
//! | `waveform` | `values`: non-empty array of numbers | `CsigWaveform` (its size is `int(len)`) |
//! | `rdtable` | `in`: size, init, ridx | `CsigReadOnlyTable` |
//! | `rwtable` | `in`: size, init, widx, wsig, ridx | `CsigWriteReadTable` |
//!
//! Differences from the box schema: no implicit wire/cut (`"_"`, `"!"`) — the
//! Signal API has no point-free composition; no `seq`/`par`/`split`/`merge`,
//! `hgroup`/`vgroup` or the `faust` source escape hatch (those are box/UI-tree
//! concepts); `round` is absent upstream (`rint` rounds). N-ary mutual
//! recursion (`CsigSelfN`/`CsigRecursionN`) is not exposed: like the box `~`,
//! single recursion is the surface.
//!
//! Errors carry the path of the offending node (`at $.signals[0].in[1]: …`);
//! semantic errors (dangling inputs, a `self` outside a recursion) are Faust's
//! own and surface verbatim at factory creation.

use std::ffi::{CString, c_int};

use serde_json::{Map, Value};

use crate::faust::ffi::{self, FaustSignal};
use crate::faust::json_util::{err, inputs, label_field, num_field};

/// Builds the output-signal vector from the root of a JSON def.
///
/// # Safety
/// Must run inside a `createLibContext`..`destroyLibContext` bracket while
/// holding [`crate::faust::compiler::ffi_lock`]; the returned signals are
/// arena pointers only valid inside that bracket.
pub unsafe fn build_signals(
    root: &Value,
    cstrings: &mut Vec<CString>,
) -> Result<Vec<FaustSignal>, String> {
    let mut path = String::from("$");
    let obj = root
        .as_object()
        .ok_or_else(|| err(&path, "a signal def must be a {\"signals\": [...]} object"))?;
    let Some(field) = obj.get("signals") else {
        return Err(err(&path, "missing \"signals\" array"));
    };
    let Some(items) = field.as_array() else {
        return Err(err(&path, "\"signals\" must be an array"));
    };
    if items.is_empty() {
        return Err(err(&path, "\"signals\" must list at least one output"));
    }
    let mut outputs = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        let parent = path.len();
        path.push_str(&format!(".signals[{i}]"));
        let sig = unsafe { build(item, &mut path, cstrings) };
        path.truncate(parent);
        outputs.push(sig?);
    }
    Ok(outputs)
}

type UnaryFn = unsafe extern "C" fn(FaustSignal) -> FaustSignal;
type BinaryFn = unsafe extern "C" fn(FaustSignal, FaustSignal) -> FaustSignal;

fn unary_op(op: &str) -> Option<UnaryFn> {
    Some(match op {
        "sin" => ffi::CsigSin,
        "cos" => ffi::CsigCos,
        "tan" => ffi::CsigTan,
        "asin" => ffi::CsigAsin,
        "acos" => ffi::CsigAcos,
        "atan" => ffi::CsigAtan,
        "exp" => ffi::CsigExp,
        "exp10" => ffi::CsigExp10,
        "log" => ffi::CsigLog,
        "log10" => ffi::CsigLog10,
        "sqrt" => ffi::CsigSqrt,
        "abs" => ffi::CsigAbs,
        "floor" => ffi::CsigFloor,
        "ceil" => ffi::CsigCeil,
        "rint" => ffi::CsigRint,
        "intcast" => ffi::CsigIntCast,
        "floatcast" => ffi::CsigFloatCast,
        "delay1" => ffi::CsigDelay1,
        _ => return None,
    })
}

fn binary_op(op: &str) -> Option<BinaryFn> {
    Some(match op {
        "add" => ffi::CsigAdd,
        "sub" => ffi::CsigSub,
        "mul" => ffi::CsigMul,
        "div" => ffi::CsigDiv,
        "rem" => ffi::CsigRem,
        "fmod" => ffi::CsigFmod,
        "remainder" => ffi::CsigRemainder,
        "pow" => ffi::CsigPow,
        "min" => ffi::CsigMin,
        "max" => ffi::CsigMax,
        "atan2" => ffi::CsigAtan2,
        "gt" => ffi::CsigGT,
        "lt" => ffi::CsigLT,
        "ge" => ffi::CsigGE,
        "le" => ffi::CsigLE,
        "eq" => ffi::CsigEQ,
        "ne" => ffi::CsigNE,
        "and" => ffi::CsigAND,
        "or" => ffi::CsigOR,
        "xor" => ffi::CsigXOR,
        "lsh" => ffi::CsigLeftShift,
        "rsh" => ffi::CsigARightShift,
        "delay" => ffi::CsigDelay,
        _ => return None,
    })
}

unsafe fn build(
    node: &Value,
    path: &mut String,
    cstrings: &mut Vec<CString>,
) -> Result<FaustSignal, String> {
    match node {
        Value::Number(n) => Ok(unsafe { number_sig(n) }),
        Value::Object(obj) => {
            let Some(op_field) = obj.get("op") else {
                return Err(err(path, "missing \"op\" field"));
            };
            let Some(op) = op_field.as_str() else {
                return Err(err(path, "\"op\" must be a string"));
            };
            unsafe { build_op(op, obj, path, cstrings) }
        }
        _ => Err(err(path, "expected a signal: number or {\"op\": …} object")),
    }
}

unsafe fn number_sig(n: &serde_json::Number) -> FaustSignal {
    match n.as_i64().and_then(|i| c_int::try_from(i).ok()) {
        Some(i) => unsafe { ffi::CsigInt(i) },
        None => unsafe { ffi::CsigReal(n.as_f64().unwrap_or(0.0)) },
    }
}

unsafe fn build_op(
    op: &str,
    obj: &Map<String, Value>,
    path: &mut String,
    cstrings: &mut Vec<CString>,
) -> Result<FaustSignal, String> {
    if let Some(f) = binary_op(op) {
        let items = inputs(obj, op, path, 2, 2)?;
        let s = unsafe { build_children(items, path, cstrings) }?;
        return Ok(unsafe { f(s[0], s[1]) });
    }
    if let Some(f) = unary_op(op) {
        let items = inputs(obj, op, path, 1, 1)?;
        let s = unsafe { build_children(items, path, cstrings) }?;
        return Ok(unsafe { f(s[0]) });
    }
    match op {
        "int" => match obj
            .get("value")
            .and_then(Value::as_i64)
            .and_then(|i| c_int::try_from(i).ok())
        {
            Some(i) => Ok(unsafe { ffi::CsigInt(i) }),
            None => Err(err(path, "`int` needs an integer \"value\"")),
        },
        "real" | "float" => match obj.get("value").and_then(Value::as_f64) {
            Some(x) => Ok(unsafe { ffi::CsigReal(x) }),
            None => Err(err(path, format_args!("`{op}` needs a numeric \"value\""))),
        },
        "input" => match obj
            .get("index")
            .and_then(Value::as_i64)
            .and_then(|i| c_int::try_from(i).ok())
        {
            Some(i) if i >= 0 => Ok(unsafe { ffi::CsigInput(i) }),
            _ => Err(err(path, "`input` needs a non-negative integer \"index\"")),
        },
        "self" => Ok(unsafe { ffi::CsigSelf() }),
        "recursion" => {
            let items = inputs(obj, op, path, 1, 1)?;
            let s = unsafe { build_children(items, path, cstrings) }?;
            Ok(unsafe { ffi::CsigRecursion(s[0]) })
        }
        "select2" => {
            let items = inputs(obj, op, path, 3, 3)?;
            let s = unsafe { build_children(items, path, cstrings) }?;
            Ok(unsafe { ffi::CsigSelect2(s[0], s[1], s[2]) })
        }
        "select3" => {
            let items = inputs(obj, op, path, 4, 4)?;
            let s = unsafe { build_children(items, path, cstrings) }?;
            Ok(unsafe { ffi::CsigSelect3(s[0], s[1], s[2], s[3]) })
        }
        "hslider" | "vslider" | "nentry" => {
            let label = label_field(obj, path, cstrings)?;
            let init = num_field(obj, op, "init", path)?;
            let min = num_field(obj, op, "min", path)?;
            let max = num_field(obj, op, "max", path)?;
            let step = num_field(obj, op, "step", path)?;
            let f = match op {
                "hslider" => ffi::CsigHSlider,
                "vslider" => ffi::CsigVSlider,
                _ => ffi::CsigNumEntry,
            };
            Ok(unsafe {
                f(
                    label,
                    ffi::CsigReal(init),
                    ffi::CsigReal(min),
                    ffi::CsigReal(max),
                    ffi::CsigReal(step),
                )
            })
        }
        "button" => Ok(unsafe { ffi::CsigButton(label_field(obj, path, cstrings)?) }),
        "checkbox" => Ok(unsafe { ffi::CsigCheckbox(label_field(obj, path, cstrings)?) }),
        "hbargraph" | "vbargraph" => {
            let label = label_field(obj, path, cstrings)?;
            let min = num_field(obj, op, "min", path)?;
            let max = num_field(obj, op, "max", path)?;
            let items = inputs(obj, op, path, 1, 1)?;
            let s = unsafe { build_children(items, path, cstrings) }?;
            let f = if op == "hbargraph" {
                ffi::CsigHBargraph
            } else {
                ffi::CsigVBargraph
            };
            Ok(unsafe { f(label, ffi::CsigReal(min), ffi::CsigReal(max), s[0]) })
        }
        "waveform" => unsafe { waveform(obj, path) },
        "rdtable" => {
            let items = inputs(obj, op, path, 3, 3)?;
            let s = unsafe { build_children(items, path, cstrings) }?;
            Ok(unsafe { ffi::CsigReadOnlyTable(s[0], s[1], s[2]) })
        }
        "rwtable" => {
            let items = inputs(obj, op, path, 5, 5)?;
            let s = unsafe { build_children(items, path, cstrings) }?;
            Ok(unsafe { ffi::CsigWriteReadTable(s[0], s[1], s[2], s[3], s[4]) })
        }
        other => Err(err(path, format_args!("unknown op {other:?}"))),
    }
}

/// `waveform`: a NULL-terminated array of constant signals. Its size signal is
/// `int(values.len())`, provided separately as the `rdtable` size input.
unsafe fn waveform(obj: &Map<String, Value>, path: &str) -> Result<FaustSignal, String> {
    let Some(field) = obj.get("values") else {
        return Err(err(path, "`waveform` needs a \"values\" array of numbers"));
    };
    let Some(values) = field.as_array() else {
        return Err(err(path, "`waveform` \"values\" must be an array"));
    };
    if values.is_empty() {
        return Err(err(path, "`waveform` \"values\" must not be empty"));
    }
    let mut sigs: Vec<FaustSignal> = Vec::with_capacity(values.len() + 1);
    for (i, v) in values.iter().enumerate() {
        let Value::Number(n) = v else {
            return Err(err(path, format_args!("`waveform` values[{i}] must be a number")));
        };
        sigs.push(unsafe { number_sig(n) });
    }
    sigs.push(std::ptr::null_mut()); // NULL terminator
    Ok(unsafe { ffi::CsigWaveform(sigs.as_mut_ptr()) })
}

unsafe fn build_children(
    items: &[Value],
    path: &mut String,
    cstrings: &mut Vec<CString>,
) -> Result<Vec<FaustSignal>, String> {
    let mut sigs = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        let parent = path.len();
        path.push_str(&format!(".in[{i}]"));
        let result = unsafe { build(item, path, cstrings) };
        path.truncate(parent);
        sigs.push(result?);
    }
    Ok(sigs)
}
