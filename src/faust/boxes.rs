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
//! | `fconst`, `fvar` | `ctype`: `"int"`/`"real"`, `name`, `file` (optional) | `CboxFConst`/`CboxFVar` (runtime scalar, e.g. `fSamplingFreq` behind `ma.SR`) |
//! | `hgroup`, `vgroup` | `label`, `in`: exactly 1 box | control grouping |
//! | `waveform` | `values`: non-empty array of numbers | `waveform{…}` — outputs the (size, content) pair |
//! | `rdtable` | `in`: size, init, ridx — or 2 boxes when a `waveform` stands in for (size, init) | `rdtable` |
//! | `rwtable` | `in`: size, init, widx, wsig, ridx — or 4 boxes starting with a `waveform` | `rwtable` |
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

use serde_json::{Map, Value};

use crate::faust::compiler::FaustArgs;
use crate::faust::ffi::{self, FaustBox};
use crate::faust::json_util::{err, foreign_args, inputs, label_field, num_field};

/// Builds the `process` box from the root of a JSON def.
///
/// `cstrings` keeps alive every C string handed to libfaust (labels); the
/// caller must hold it at least until the factory is created.
///
/// # Safety
/// Must run inside a `createLibContext`..`destroyLibContext` bracket while
/// holding [`crate::faust::compiler::ffi_lock`]; the returned box is an arena
/// pointer only valid inside that bracket.
pub unsafe fn build_process(root: &Value, cstrings: &mut Vec<CString>) -> Result<FaustBox, String> {
    let mut path = String::from("$");
    unsafe { build(root, &mut path, cstrings) }
}

type UnaryFn = unsafe extern "C" fn(FaustBox) -> FaustBox;
type BinaryFn = unsafe extern "C" fn(FaustBox, FaustBox) -> FaustBox;

fn unary_op(op: &str) -> Option<UnaryFn> {
    Some(match op {
        "sin" => ffi::CboxSinAux,
        // "cos" is special-cased in `build_op`: upstream bug (`boxCos()`
        // returns the abs primitive), same as fmod.
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
        Value::Number(n) => Ok(unsafe { number_box(n) }),
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

/// A constant box: `int` if the number is integral within `c_int`, `real`
/// otherwise (same rule as the bare-number shorthand).
unsafe fn number_box(n: &serde_json::Number) -> FaustBox {
    match n.as_i64().and_then(|i| c_int::try_from(i).ok()) {
        Some(i) => unsafe { ffi::CboxInt(i) },
        None => unsafe { ffi::CboxReal(n.as_f64().unwrap_or(0.0)) },
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
        // libfaust bug (2.81.10, still in 2.85.5): in `box_signal_api.cpp`,
        // `boxFmod()` and `boxCos()` both return the `abs` primitive
        // (`gGlobal->gAbsPrim->box()`), so `CboxFmodAux`/`CboxCosAux` silently
        // compute `abs` (fmod also fails with an arity error). A one-line
        // source fragment yields the genuine primitive instead. The Signal
        // API (`sigCos`) is unaffected.
        "fmod" => {
            let items = inputs(obj, op, path, 2, 2)?;
            let boxes = unsafe { build_children(items, path, cstrings) }?;
            let prim = unsafe { dsp_to_boxes("process = fmod;", path) }?;
            Ok(unsafe { ffi::CboxSeq(ffi::CboxPar(boxes[0], boxes[1]), prim) })
        }
        "cos" => {
            let items = inputs(obj, op, path, 1, 1)?;
            let boxes = unsafe { build_children(items, path, cstrings) }?;
            let prim = unsafe { dsp_to_boxes("process = cos;", path) }?;
            Ok(unsafe { ffi::CboxSeq(boxes[0], prim) })
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
        "fconst" => {
            let (ty, name, file) = foreign_args(obj, op, path, cstrings)?;
            Ok(unsafe { ffi::CboxFConst(ty, name, file) })
        }
        "fvar" => {
            let (ty, name, file) = foreign_args(obj, op, path, cstrings)?;
            Ok(unsafe { ffi::CboxFVar(ty, name, file) })
        }
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
        "waveform" => {
            let Some(field) = obj.get("values") else {
                return Err(err(path, "`waveform` needs a \"values\" array of numbers"));
            };
            let Some(values) = field.as_array() else {
                return Err(err(path, "`waveform` \"values\" must be an array"));
            };
            if values.is_empty() {
                return Err(err(path, "`waveform` \"values\" must not be empty"));
            }
            let mut boxes: Vec<FaustBox> = Vec::with_capacity(values.len() + 1);
            for (i, v) in values.iter().enumerate() {
                let Value::Number(n) = v else {
                    return Err(err(
                        path,
                        format_args!("`waveform` values[{i}] must be a number"),
                    ));
                };
                boxes.push(unsafe { number_box(n) });
            }
            boxes.push(std::ptr::null_mut()); // CboxWaveform wants a NULL terminator
            Ok(unsafe { ffi::CboxWaveform(boxes.as_mut_ptr()) })
        }
        // The table primitives take (size, init, read index) — rdtable — and
        // (size, init, write index, write signal, read index) — rwtable. A
        // `waveform` box outputs the (size, init) pair itself, so each op
        // also accepts the form with one box less up front.
        "rdtable" => unsafe { table_op(obj, op, path, cstrings, 2, 3, ffi::CboxReadOnlyTable) },
        "rwtable" => unsafe { table_op(obj, op, path, cstrings, 4, 5, ffi::CboxWriteReadTable) },
        "faust" => unsafe { faust_fragment(obj, path, cstrings) },
        other => Err(err(path, format_args!("unknown op {other:?}"))),
    }
}

/// `seq(par(inputs…), primitive)` — how upstream's own `Cbox*TableAux`
/// helpers apply the 0-argument table primitives. Faust checks the summed
/// output arity against the primitive's inputs at compile time.
unsafe fn table_op(
    obj: &Map<String, Value>,
    op: &str,
    path: &mut String,
    cstrings: &mut Vec<CString>,
    min: usize,
    max: usize,
    primitive: unsafe extern "C" fn() -> FaustBox,
) -> Result<FaustBox, String> {
    let items = inputs(obj, op, path, min, max)?;
    let boxes = unsafe { build_children(items, path, cstrings) }?;
    let pars = boxes
        .into_iter()
        .reduce(|a, b| unsafe { ffi::CboxPar(a, b) })
        .expect("inputs() guarantees at least two");
    Ok(unsafe { ffi::CboxSeq(pars, primitive()) })
}

/// The `faust` escape hatch: compiles a complete Faust program into a box
/// with `CDSPToBoxes`, importing from the stdlib directories (see
/// [`FaustArgs::defaults`]).
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
    let args = FaustArgs::defaults();
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
