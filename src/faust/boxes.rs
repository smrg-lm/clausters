//! JSON → Box API interpreter (F2).
//!
//! A Faust def arrives as a JSON tree where every node denotes a box
//! expression; the interpreter walks it and issues the corresponding `Cbox*`
//! calls. The schema mirrors the Box API one-to-one on purpose: the client's
//! instruction set is Faust's, not a UGen set of ours.
//!
//! # Schema
//!
//! Shorthands: a JSON **number** is a constant box (`int` if integral within
//! `i32`, `real` otherwise); the **strings** `"_"` and `"!"` are the wire and
//! cut primitives.
//!
//! Everything else is an object with an `"op"` field:
//!
//! | `op` | fields | Faust equivalent |
//! |---|---|---|
//! | `int`, `real` | `value` | constant |
//! | `wire`, `cut` | — | `_`, `!` |
//! | `seq`, `par`, `split`, `merge` | `in`: array of ≥ 2 boxes, folded left | `:` `,` `<:` `:>` |
//! | `rec` | `in`: exactly 2 boxes | `~` |
//! | `add` `sub` `mul` `div` `fmod` `pow` `min` `max` `atan2` `gt` `lt` `ge` `le` `eq` `ne` `and` `or` `xor` | `in`: exactly 2 boxes | binary operators |
//! | `sin` `cos` `tan` `asin` `acos` `atan` `exp` `exp10` `log` `log10` `sqrt` `abs` `floor` `ceil` `rint` `round` `intcast` `floatcast` | `in`: exactly 1 box | unary functions |
//! | `delay` | `in`: signal, delay length | `@` |
//! | `select2` | `in`: selector, then 2 branches | `select2` |
//! | `select3` | `in`: selector, then 3 branches | `select3` |
//! | `hslider`, `vslider`, `nentry` | `label`, `init`, `min`, `max`, `step` | named control |
//! | `button`, `checkbox` | `label` | named control (0/1) |
//! | `hgroup`, `vgroup` | `label`, `in`: exactly 1 box | control grouping |
//! | `faust` | `src` | escape hatch: a complete Faust program (`process = …`) compiled with `CDSPToBoxes`, giving access to the stdlib (`os.osc`, `fi.lowpass`, …) as a composable box |
//!
//! Example — `sin(2π·phasor(freq)) * 0.2` with `freq` as a named control:
//!
//! ```json
//! {"op": "mul", "in": [
//!   {"op": "sin", "in": [{"op": "mul", "in": [
//!     6.283185307179586,
//!     {"op": "rec", "in": [
//!       {"op": "seq", "in": [
//!         {"op": "add", "in": ["_", {"op": "div", "in": [
//!           {"op": "hslider", "label": "freq",
//!            "init": 440.0, "min": 20.0, "max": 20000.0, "step": 0.01},
//!           48000.0]}]},
//!         {"op": "split", "in": ["_",
//!           {"op": "sub", "in": ["_", {"op": "floor", "in": ["_"]}]}]}]},
//!       "_"]}]}]},
//!   0.2]}
//! ```
//!
//! # Errors
//!
//! Structural validation happens while building and every error carries the
//! path of the offending JSON node from the root `$` (e.g.
//! `at $.in[1].op: unknown op "mul3"`). Semantic errors (composition arity
//! mismatches, dangling inputs) are Faust's own and surface later, verbatim,
//! when the factory is created.

use std::ffi::{CStr, CString, c_char, c_int};
use std::fmt::Display;

use serde_json::{Map, Value};

use crate::faust::compiler::FaustArgs;
use crate::faust::ffi::{self, FaustBox};

fn err(path: &str, why: impl Display) -> String {
    format!("at {path}: {why}")
}

/// Builds the `process` box from the root of a JSON def.
///
/// `cstrings` keeps alive every C string handed to libfaust (labels); the
/// caller must hold it at least until the factory is created.
///
/// # Safety
/// Must run inside a `createLibContext`..`destroyLibContext` bracket while
/// holding [`crate::faust::compiler::ffi_lock`]; the returned box is an arena
/// pointer only valid inside that bracket.
pub unsafe fn build_process(
    root: &Value,
    cstrings: &mut Vec<CString>,
) -> Result<FaustBox, String> {
    let mut path = String::from("$");
    unsafe { build(root, &mut path, cstrings) }
}

type UnaryFn = unsafe extern "C" fn(FaustBox) -> FaustBox;
type BinaryFn = unsafe extern "C" fn(FaustBox, FaustBox) -> FaustBox;

fn unary_op(op: &str) -> Option<UnaryFn> {
    Some(match op {
        "sin" => ffi::CboxSinAux,
        "cos" => ffi::CboxCosAux,
        "tan" => ffi::CboxTanAux,
        "asin" => ffi::CboxAsinAux,
        "acos" => ffi::CboxAcosAux,
        "atan" => ffi::CboxAtanAux,
        "exp" => ffi::CboxExpAux,
        "exp10" => ffi::CboxExp10Aux,
        "log" => ffi::CboxLogAux,
        "log10" => ffi::CboxLog10Aux,
        "sqrt" => ffi::CboxSqrtAux,
        "abs" => ffi::CboxAbsAux,
        "floor" => ffi::CboxFloorAux,
        "ceil" => ffi::CboxCeilAux,
        "rint" => ffi::CboxRintAux,
        "round" => ffi::CboxRoundAux,
        "intcast" => ffi::CboxIntCastAux,
        "floatcast" => ffi::CboxFloatCastAux,
        _ => return None,
    })
}

fn binary_op(op: &str) -> Option<BinaryFn> {
    Some(match op {
        "add" => ffi::CboxAddAux,
        "sub" => ffi::CboxSubAux,
        "mul" => ffi::CboxMulAux,
        "div" => ffi::CboxDivAux,
        // "fmod" is special-cased in `build_op`: upstream bug.
        "pow" => ffi::CboxPowAux,
        "min" => ffi::CboxMinAux,
        "max" => ffi::CboxMaxAux,
        "atan2" => ffi::CboxAtan2Aux,
        "gt" => ffi::CboxGTAux,
        "lt" => ffi::CboxLTAux,
        "ge" => ffi::CboxGEAux,
        "le" => ffi::CboxLEAux,
        "eq" => ffi::CboxEQAux,
        "ne" => ffi::CboxNEAux,
        "and" => ffi::CboxANDAux,
        "or" => ffi::CboxORAux,
        "xor" => ffi::CboxXORAux,
        "delay" => ffi::CboxDelayAux,
        _ => return None,
    })
}

/// N-ary in the schema (folded left), binary in the C API.
fn fold_op(op: &str) -> Option<BinaryFn> {
    Some(match op {
        "seq" => ffi::CboxSeq,
        "par" => ffi::CboxPar,
        "split" => ffi::CboxSplit,
        "merge" => ffi::CboxMerge,
        _ => return None,
    })
}

unsafe fn build(
    node: &Value,
    path: &mut String,
    cstrings: &mut Vec<CString>,
) -> Result<FaustBox, String> {
    match node {
        Value::Number(n) => match n.as_i64().and_then(|i| c_int::try_from(i).ok()) {
            Some(i) => Ok(unsafe { ffi::CboxInt(i) }),
            None => Ok(unsafe { ffi::CboxReal(n.as_f64().unwrap_or(0.0)) }),
        },
        Value::String(s) => match s.as_str() {
            "_" => Ok(unsafe { ffi::CboxWire() }),
            "!" => Ok(unsafe { ffi::CboxCut() }),
            other => Err(err(
                path,
                format_args!("unknown shorthand {other:?} (expected \"_\" or \"!\")"),
            )),
        },
        Value::Object(obj) => {
            let Some(op_field) = obj.get("op") else {
                return Err(err(path, "missing \"op\" field"));
            };
            let Some(op) = op_field.as_str() else {
                return Err(err(path, "\"op\" must be a string"));
            };
            unsafe { build_op(op, obj, path, cstrings) }
        }
        _ => Err(err(
            path,
            "expected a box: number, \"_\", \"!\" or {\"op\": …} object",
        )),
    }
}

unsafe fn build_op(
    op: &str,
    obj: &Map<String, Value>,
    path: &mut String,
    cstrings: &mut Vec<CString>,
) -> Result<FaustBox, String> {
    if let Some(f) = fold_op(op) {
        let items = inputs(obj, op, path, 2, usize::MAX)?;
        let boxes = unsafe { build_children(items, path, cstrings) }?;
        return Ok(boxes
            .into_iter()
            .reduce(|a, b| unsafe { f(a, b) })
            .expect("inputs() guarantees at least two"));
    }
    if let Some(f) = binary_op(op) {
        let items = inputs(obj, op, path, 2, 2)?;
        let boxes = unsafe { build_children(items, path, cstrings) }?;
        return Ok(unsafe { f(boxes[0], boxes[1]) });
    }
    if let Some(f) = unary_op(op) {
        let items = inputs(obj, op, path, 1, 1)?;
        let boxes = unsafe { build_children(items, path, cstrings) }?;
        return Ok(unsafe { f(boxes[0]) });
    }
    match op {
        "int" => {
            let n = obj
                .get("value")
                .and_then(Value::as_i64)
                .and_then(|i| c_int::try_from(i).ok());
            match n {
                Some(i) => Ok(unsafe { ffi::CboxInt(i) }),
                None => Err(err(path, "`int` needs an integer \"value\"")),
            }
        }
        "real" | "float" => match obj.get("value").and_then(Value::as_f64) {
            Some(x) => Ok(unsafe { ffi::CboxReal(x) }),
            None => Err(err(path, format_args!("`{op}` needs a numeric \"value\""))),
        },
        "wire" => Ok(unsafe { ffi::CboxWire() }),
        "cut" => Ok(unsafe { ffi::CboxCut() }),
        "rec" => {
            let items = inputs(obj, op, path, 2, 2)?;
            let boxes = unsafe { build_children(items, path, cstrings) }?;
            Ok(unsafe { ffi::CboxRec(boxes[0], boxes[1]) })
        }
        // libfaust bug (2.81.10, still on master-dev): `boxFmod()` returns
        // the `abs` primitive (`gGlobal->gAbsPrim->box()`), so `CboxFmodAux`
        // builds `(a, b) : abs` and fails with an arity error. A one-line
        // fragment yields the genuine 2-input fmod primitive instead.
        "fmod" => {
            let items = inputs(obj, op, path, 2, 2)?;
            let boxes = unsafe { build_children(items, path, cstrings) }?;
            let prim = unsafe { dsp_to_boxes("process = fmod;", path) }?;
            Ok(unsafe { ffi::CboxSeq(ffi::CboxPar(boxes[0], boxes[1]), prim) })
        }
        "select2" => {
            let items = inputs(obj, op, path, 3, 3)?;
            let b = unsafe { build_children(items, path, cstrings) }?;
            Ok(unsafe { ffi::CboxSelect2Aux(b[0], b[1], b[2]) })
        }
        "select3" => {
            let items = inputs(obj, op, path, 4, 4)?;
            let b = unsafe { build_children(items, path, cstrings) }?;
            Ok(unsafe { ffi::CboxSelect3Aux(b[0], b[1], b[2], b[3]) })
        }
        "hslider" | "vslider" | "nentry" => {
            let label = label_field(obj, path, cstrings)?;
            let init = num_field(obj, op, "init", path)?;
            let min = num_field(obj, op, "min", path)?;
            let max = num_field(obj, op, "max", path)?;
            let step = num_field(obj, op, "step", path)?;
            let f = match op {
                "hslider" => ffi::CboxHSlider,
                "vslider" => ffi::CboxVSlider,
                _ => ffi::CboxNumEntry,
            };
            Ok(unsafe {
                f(
                    label,
                    ffi::CboxReal(init),
                    ffi::CboxReal(min),
                    ffi::CboxReal(max),
                    ffi::CboxReal(step),
                )
            })
        }
        "button" => Ok(unsafe { ffi::CboxButton(label_field(obj, path, cstrings)?) }),
        "checkbox" => Ok(unsafe { ffi::CboxCheckbox(label_field(obj, path, cstrings)?) }),
        "hgroup" | "vgroup" => {
            let label = label_field(obj, path, cstrings)?;
            let items = inputs(obj, op, path, 1, 1)?;
            let boxes = unsafe { build_children(items, path, cstrings) }?;
            let f = if op == "hgroup" {
                ffi::CboxHGroup
            } else {
                ffi::CboxVGroup
            };
            Ok(unsafe { f(label, boxes[0]) })
        }
        "faust" => unsafe { faust_fragment(obj, path, cstrings) },
        other => Err(err(path, format_args!("unknown op {other:?}"))),
    }
}

/// The `faust` escape hatch: compiles a complete Faust program into a box
/// with `CDSPToBoxes`, importing from the stdlib directories (see
/// [`FaustArgs::stdlib`]).
unsafe fn faust_fragment(
    obj: &Map<String, Value>,
    path: &str,
    _cstrings: &mut [CString],
) -> Result<FaustBox, String> {
    let Some(src) = obj.get("src").and_then(Value::as_str) else {
        return Err(err(path, "`faust` needs a \"src\" string of Faust source"));
    };
    unsafe { dsp_to_boxes(src, path) }
}

/// Faust source → box, inside the current lib context. The source is fully
/// consumed by the call, so its C string can die with this frame.
unsafe fn dsp_to_boxes(src: &str, path: &str) -> Result<FaustBox, String> {
    let src_c = CString::new(src).map_err(|_| err(path, "NUL byte in Faust source"))?;
    let name_c = CString::new("fragment").unwrap();
    let args = FaustArgs::stdlib();
    let mut error_msg = [0 as c_char; ffi::ERROR_MSG_SIZE];
    let (mut num_inputs, mut num_outputs) = (0 as c_int, 0 as c_int);
    let fragment = unsafe {
        ffi::CDSPToBoxes(
            name_c.as_ptr(),
            src_c.as_ptr(),
            args.argc(),
            args.argv(),
            &mut num_inputs,
            &mut num_outputs,
            error_msg.as_mut_ptr(),
        )
    };
    if fragment.is_null() {
        let msg = unsafe { CStr::from_ptr(error_msg.as_ptr()) };
        Err(err(path, msg.to_string_lossy().trim()))
    } else {
        Ok(fragment)
    }
}

/// Validates the `"in"` array of `op`: present, an array, and with the right
/// length (`min..=max`; `max == usize::MAX` reads as "at least `min`").
fn inputs<'a>(
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
            format_args!("`{op}` takes {expected} boxes in \"in\", got {}", items.len()),
        ));
    }
    Ok(items)
}

unsafe fn build_children(
    items: &[Value],
    path: &mut String,
    cstrings: &mut Vec<CString>,
) -> Result<Vec<FaustBox>, String> {
    let mut boxes = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        let parent_len = path.len();
        path.push_str(&format!(".in[{i}]"));
        let result = unsafe { build(item, path, cstrings) };
        path.truncate(parent_len);
        boxes.push(result?);
    }
    Ok(boxes)
}

fn num_field(obj: &Map<String, Value>, op: &str, key: &str, path: &str) -> Result<f64, String> {
    obj.get(key)
        .and_then(Value::as_f64)
        .ok_or_else(|| err(path, format_args!("`{op}` needs a numeric \"{key}\"")))
}

/// Returns the label as a C pointer kept alive in `cstrings`.
fn label_field(
    obj: &Map<String, Value>,
    path: &str,
    cstrings: &mut Vec<CString>,
) -> Result<*const c_char, String> {
    let Some(label) = obj.get("label").and_then(Value::as_str) else {
        return Err(err(path, "needs a string \"label\""));
    };
    let label_c =
        CString::new(label).map_err(|_| err(path, "NUL byte in \"label\""))?;
    cstrings.push(label_c);
    Ok(cstrings.last().unwrap().as_ptr())
}
