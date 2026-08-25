//! Reading typed widget props out of a GuiDef node's generic JSON: the shared
//! wire-parsing helpers behind the schema ([`super`]) and its two long wire
//! matches ([`super::build`] and [`super::apply`]). Split out of the schema so
//! the enum and the construction/update matches read on their own. A helper
//! stays `pub(super)` — the `widget` module tree and nowhere else — until a
//! leaf that moved behind [`Element`](super::Element) needs it, since an
//! element in [`elements`](crate::host::elements) parses the same props from
//! outside the module; those are `pub(crate)`.

use std::sync::Arc;

use serde_json::Value;

use super::Rate;
use crate::spectrogram::FreqScale;

/// Coerce a `/gui_set` value that carries an array (either already a JSON array,
/// or an array encoded as a JSON string — the scalar-wire carrier `points`/
/// `notes`/`members` use) into a one-entry props map under `key`, for the
/// `parse_*` helpers to read.
pub(crate) fn as_array_props(key: &str, v: &Value) -> serde_json::Map<String, Value> {
    let value = match v {
        Value::String(s) => serde_json::from_str::<Value>(s).unwrap_or(Value::Null),
        other => other.clone(),
    };
    std::iter::once((key.to_string(), value)).collect()
}

/// Coerce a `/gui_set` value that carries a whole **props object** (the score's
/// `display_list`) into the map the widget's `parse` reads: an object as it
/// stands, or a JSON string parsed into one — OSC carries no objects, so the
/// wire form of a structural value is always a string. `None` for anything
/// else, so a malformed set is refused rather than applied as an empty page.
#[cfg(feature = "notation")]
pub(crate) fn as_props(v: &Value) -> Option<serde_json::Map<String, Value>> {
    match v {
        Value::String(s) => serde_json::from_str(s).ok(),
        Value::Object(o) => Some(o.clone()),
        _ => None,
    }
}

/// Parse a `piano`'s `voice_args` — a flat `[name, value, name, value, …]`
/// list of extra `/synth_new` control pairs (the `bind`-prefix posture: names are
/// strings, values numbers). A trailing partial pair is dropped.
pub(crate) fn voice_args(props: &serde_json::Map<String, Value>) -> Vec<(String, f32)> {
    let Some(Value::Array(items)) = props.get("voice_args") else {
        return Vec::new();
    };
    items
        .as_chunks::<2>()
        .0
        .iter()
        .filter_map(|c| Some((c[0].as_str()?.to_string(), c[1].as_f64()? as f32)))
        .collect()
}

/// The `freq_scale` property (`"linear"`/`"log"`/`"mel"`/`"bark"`), falling
/// back to the legacy `log_freq` boolean (default: log).
pub(super) fn parse_freq_scale(props: &serde_json::Map<String, Value>) -> FreqScale {
    if let Some(s) = props
        .get("freq_scale")
        .and_then(Value::as_str)
        .and_then(freq_scale_from_str)
    {
        return s;
    }
    if props.get("log_freq").and_then(truthy) == Some(false) {
        FreqScale::Linear
    } else {
        FreqScale::Log
    }
}

/// A frequency-scale name as the widget schema spells it.
pub(crate) fn freq_scale_from_str(s: &str) -> Option<FreqScale> {
    Some(match s {
        "linear" | "lin" => FreqScale::Linear,
        "log" => FreqScale::Log,
        "mel" => FreqScale::Mel,
        "bark" => FreqScale::Bark,
        _ => return None,
    })
}

/// A non-negative integer dimension property, defaulted when absent.
pub(super) fn dimension(props: &serde_json::Map<String, Value>, key: &str, default: u32) -> u32 {
    props
        .get(key)
        .and_then(Value::as_u64)
        .map(|n| n.clamp(1, u32::MAX as u64) as u32)
        .unwrap_or(default)
}

/// An integer property, defaulted when absent or non-integer.
pub(crate) fn int_prop(props: &serde_json::Map<String, Value>, key: &str, default: i32) -> i32 {
    props
        .get(key)
        .and_then(Value::as_i64)
        .map(|n| n as i32)
        .unwrap_or(default)
}

/// An `f64` property, defaulted when absent or non-numeric — for sample
/// positions and clock values, where `f32` would lose sample accuracy on
/// buffers past a few minutes.
pub(crate) fn number_f64(props: &serde_json::Map<String, Value>, key: &str, default: f64) -> f64 {
    props.get(key).and_then(Value::as_f64).unwrap_or(default)
}

/// Sets an `f64` slot from a numeric JSON value, reporting whether it applied.
pub(crate) fn set_f64(slot: &mut f64, v: &Value) -> bool {
    match v.as_f64() {
        Some(x) => {
            *slot = x;
            true
        }
        None => false,
    }
}

/// A float property, defaulted when absent or non-numeric.
pub(crate) fn number(props: &serde_json::Map<String, Value>, key: &str, default: f32) -> f32 {
    props
        .get(key)
        .and_then(Value::as_f64)
        .map(|n| n as f32)
        .unwrap_or(default)
}

/// A fixed-size `[f32; N]` from a JSON array property, taking the first `N`
/// numbers and padding the rest with `default`.
pub(crate) fn f32_array<const N: usize>(
    props: &serde_json::Map<String, Value>,
    key: &str,
    default: f32,
) -> [f32; N] {
    let mut out = [default; N];
    if let Some(Value::Array(items)) = props.get(key) {
        for (slot, v) in out.iter_mut().zip(items) {
            if let Some(x) = v.as_f64() {
                *slot = x as f32;
            }
        }
    }
    out
}

/// A fixed-size `[i32; N]` from a JSON array property, taking the first `N`
/// integers and padding the rest with `default`.
pub(crate) fn i32_array<const N: usize>(
    props: &serde_json::Map<String, Value>,
    key: &str,
    default: i32,
) -> [i32; N] {
    let mut out = [default; N];
    if let Some(Value::Array(items)) = props.get(key) {
        for (slot, v) in out.iter_mut().zip(items) {
            if let Some(n) = v.as_i64() {
                *slot = n as i32;
            }
        }
    }
    out
}

/// The integer suffix of `key` after `prefix` (e.g. `"param2"` -> `2`), if `key`
/// is exactly `prefix` followed by digits.
pub(crate) fn index_suffix(key: &str, prefix: &str) -> Option<usize> {
    key.strip_prefix(prefix).and_then(|s| s.parse().ok())
}

/// The `text_size` property: the glyph scale text draws at (font-pixels per
/// cell pixel over the embedded 5x7 font), clamped to a legible range.
pub(crate) fn text_size(props: &serde_json::Map<String, Value>) -> f32 {
    clamp_text_size(number(props, "text_size", crate::host::font::DEFAULT_SIZE))
}

/// Clamped to a legible range, and quantized to what the face this build draws
/// with can actually render evenly — half-steps of the cell for the bitmap,
/// the number itself once a typeface is loaded (see
/// [`font::quantize_size`](crate::host::font::quantize_size)).
pub(crate) fn clamp_text_size(s: f32) -> f32 {
    crate::host::font::quantize_size(s.clamp(1.0, 16.0))
}

/// Sets a `text_size` slot from a numeric JSON value, clamped.
pub(crate) fn set_size(slot: &mut f32, v: &Value) -> bool {
    match v.as_f64() {
        Some(x) => {
            *slot = clamp_text_size(x as f32);
            true
        }
        None => false,
    }
}

/// The `label` property as an owned string, if present.
pub(crate) fn label(props: &serde_json::Map<String, Value>) -> Option<String> {
    props
        .get("label")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The `options` property as a list of strings (for a menu).
pub(crate) fn options(props: &serde_json::Map<String, Value>) -> Vec<String> {
    match props.get("options") {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

/// A JSON value as a boolean: real bool, or a number where non-zero is true.
/// A container's arrangement, as the wire names it. The model spends the word
/// `layout` on the container type itself, so the arrangement is `flow` — on
/// every container that has one, a `window` and a `plane` included.
pub(super) fn flow(props: &serde_json::Map<String, Value>) -> Option<&str> {
    props.get("flow").and_then(Value::as_str)
}

pub(crate) fn truthy(v: &Value) -> Option<bool> {
    match v {
        Value::Bool(b) => Some(*b),
        Value::Number(n) => n.as_f64().map(|x| x != 0.0),
        _ => None,
    }
}

/// Sets `slot` from a numeric JSON value, reporting whether it applied.
pub(crate) fn set_f(slot: &mut f32, v: &Value) -> bool {
    match v.as_f64() {
        Some(x) => {
            *slot = x as f32;
            true
        }
        None => false,
    }
}

/// Sets a data view's rate live (`/gui_set rate "control"`), so one widget can
/// be retuned between watching an audio bus and a control bus.
pub(crate) fn set_rate(slot: &mut Rate, v: &Value) -> bool {
    match v.as_str() {
        Some(s) => {
            *slot = Rate::parse(Some(s));
            true
        }
        None => false,
    }
}

/// An optional f32 prop: `None` when absent (the plot's auto-fit sides).
pub(super) fn opt_number(props: &serde_json::Map<String, Value>, key: &str) -> Option<f32> {
    props.get(key).and_then(Value::as_f64).map(|n| n as f32)
}

/// Sets an optional f32 from a number, or clears it from the string `"auto"`.
pub(crate) fn set_opt_f(slot: &mut Option<f32>, v: &Value) -> bool {
    if v.as_str() == Some("auto") {
        *slot = None;
        return true;
    }
    match v.as_f64() {
        Some(n) => {
            *slot = Some(n as f32);
            true
        }
        None => false,
    }
}

/// Sets an optional label from a string JSON value.
pub(crate) fn set_label(slot: &mut Option<String>, v: &Value) -> bool {
    match v.as_str() {
        Some(s) => {
            *slot = Some(s.to_string());
            true
        }
        None => false,
    }
}

/// Resolves a sample-view widget's inline samples: inline `"data": [f32…]`, or
/// `"blob": <index>` into the OSC blobs carried with the def (raw little-endian
/// `f32`). Shared by `waveform` and `plot`; `kind` names the widget in errors.
pub(super) fn inline_samples(
    kind: &str,
    id: Option<i32>,
    props: &serde_json::Map<String, Value>,
    blobs: &[Vec<u8>],
) -> Result<Arc<[f32]>, String> {
    let label = id.map_or_else(|| kind.to_string(), |i| format!("{kind} {i}"));
    if let Some(Value::Array(items)) = props.get("data") {
        let samples: Vec<f32> = items
            .iter()
            .map(|v| v.as_f64().map(|x| x as f32))
            .collect::<Option<Vec<f32>>>()
            .ok_or_else(|| format!("{label}: `data` must be an array of numbers"))?;
        return Ok(samples.into());
    }
    if let Some(index) = props.get("blob").and_then(Value::as_u64) {
        let blob = blobs.get(index as usize).ok_or_else(|| {
            format!(
                "{label}: `blob` {index} out of range ({} sent)",
                blobs.len()
            )
        })?;
        if blob.len() % 4 != 0 {
            return Err(format!(
                "{label}: blob length {} is not a multiple of 4",
                blob.len()
            ));
        }
        let samples: Vec<f32> = blob
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f32::from_le_bytes(*c))
            .collect();
        return Ok(samples.into());
    }
    // A `buffer` (audio-server fetch) or a `path`/`cache` (mapped local
    // resource) is loaded later by the windowed front; start empty.
    Ok(Arc::from([] as [f32; 0]))
}

/// The **stretch of its container's time axis** a node declares, from `at` and
/// `dur` — `None` when it declares neither, which is a node filling whatever
/// the container gives it.
///
/// A node that names one and not the other still gets a span: `at` alone runs
/// to the end of the container (a segment that finishes it), `dur` alone starts
/// at zero. A negative length is no span at all rather than a backwards one.
pub(crate) fn span(props: &serde_json::Map<String, Value>) -> Option<(f64, f64)> {
    if !props.contains_key("at") && !props.contains_key("dur") {
        return None;
    }
    let at = number_f64(props, "at", 0.0).max(0.0);
    let dur = number_f64(props, "dur", f64::INFINITY);
    (dur > 0.0).then_some((at, dur))
}
