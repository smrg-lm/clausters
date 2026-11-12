//! The axis a break-point curve is drawn against, and a segment's level.

#[cfg(target_arch = "wasm32")]
use clausters_core::envshape;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

/// JS face: the axis a break-point curve is **drawn** against, as `[lo, hi]` --
/// its values' range with a tenth of headroom, and a flat curve still gets a
/// band to be dragged in.
///
/// Pass the axis already in hand as `keptLo`/`keptHi` and it is held, widened
/// only where the data stopped fitting inside it: recomputing the range on
/// every redraw makes an edit rescale the picture, so dragging one point
/// visibly moves every other one. Omit them for a curve drawn for the first
/// time.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = curveAxis)]
pub fn curve_axis(values: &[f64], kept_lo: Option<f64>, kept_hi: Option<f64>) -> Vec<f64> {
    let kept = match (kept_lo, kept_hi) {
        (Some(lo), Some(hi)) => Some((lo, hi)),
        _ => None,
    };
    let (lo, hi) = envshape::curve_axis(values, kept);
    vec![lo, hi]
}

/// JS face: one envelope segment's level -- shape `shape` (SuperCollider's
/// numbers), curvature `c`, from `a` to `b`, at the position `t` in `[0, 1)`.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = shapeValue)]
pub fn shape_value(shape: i32, c: f32, a: f32, b: f32, t: f32) -> f32 {
    envshape::shape_value(shape, c, a, b, t)
}
