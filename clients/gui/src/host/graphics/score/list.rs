//! **The display-list decode**: the client's engraved page, read off the wire.
//!
//! The host owns no notation. What arrives is a *drawing* the client engraved
//! (verovio, in `clausters.gui.notation`) plus the two indexes that make it
//! interactive — the `xml:id` on each primitive, and the timemap of cursors —
//! and this module turns that JSON into the typed page the rest of the module
//! draws, hits and sweeps.
//!
//! Malformed entries are **skipped rather than rejected**, so a partially
//! understood display list still draws what it can: the protocol's "an unknown
//! type is laid out and not painted" rule, applied within one widget.

use serde_json::{Map, Value};

use super::{Affine, Cursor, Prim, STEP, ScoreData};

impl ScoreData {
    /// Parse the `score` widget's display-list props sent by the client:
    /// `vb` = `[width, height]` page-unit viewBox, `glyphs` = an object mapping
    /// a hex SMuFL codepoint string to its outline path `d`, and `prims` = an
    /// array of `{k: "glyph"|"line"|"fill", ...}` primitives. Malformed entries
    /// are skipped rather than rejected, so a partially understood display list
    /// still draws what it can — the "unknown widget is laid out but not
    /// painted" spirit applied within the widget.
    pub fn parse(props: &Map<String, Value>) -> ScoreData {
        let mut data = ScoreData::default();
        if let Some(vb) = props.get("vb").and_then(Value::as_array) {
            data.vb_w = vb.first().and_then(Value::as_f64).unwrap_or(0.0) as f32;
            data.vb_h = vb.get(1).and_then(Value::as_f64).unwrap_or(0.0) as f32;
        }
        if let Some(glyphs) = props.get("glyphs").and_then(Value::as_object) {
            for (code, d) in glyphs {
                if let (Ok(cp), Some(d)) = (u32::from_str_radix(code, 16), d.as_str()) {
                    data.glyphs.insert(cp, d.to_string());
                }
            }
        }
        if let Some(prims) = props.get("prims").and_then(Value::as_array) {
            for p in prims {
                if let Some(prim) = parse_prim(p) {
                    data.prims.push(prim);
                }
            }
        }
        if let Some(cursors) = props.get("cursors").and_then(Value::as_array) {
            for c in cursors {
                if let Some(cur) = parse_cursor(c) {
                    data.cursors.push(cur);
                }
            }
            data.cursors
                .sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap_or(std::cmp::Ordering::Equal));
        }
        // A playhead is off unless the client sets a non-negative time (static)
        // or anchors one to the engine clock (sweeping).
        data.playhead = props
            .get("playhead")
            .and_then(Value::as_f64)
            .map(|f| f as f32)
            .unwrap_or(-1.0);
        data.playhead_at = props
            .get("playhead_at")
            .and_then(Value::as_f64)
            .unwrap_or(-1.0);
        data.playhead_loop_start = props
            .get("playhead_loop_start")
            .and_then(Value::as_f64)
            .map(|f| f as f32)
            .unwrap_or(0.0);
        data.playhead_loop_len = props
            .get("playhead_loop_len")
            .and_then(Value::as_f64)
            .map(|f| f as f32)
            .unwrap_or(0.0);
        data.sample_rate = props
            .get("sample_rate")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        data.selected = props
            .get("selected")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        data.step = props
            .get("step")
            .and_then(Value::as_f64)
            .map(|s| s as f32)
            .filter(|s| *s > 0.0)
            .unwrap_or(STEP);
        data.editable = props
            .get("editable")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        data.entry = props.get("entry").and_then(Value::as_bool).unwrap_or(false);
        // Which ids name a sounding element rather than the furniture that also
        // carries one. The client's walk is what knows; a renderer cannot
        // re-derive it, since to it an id is an id.
        data.elements = props
            .get("elements")
            .and_then(Value::as_array)
            .map(|xs| {
                xs.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        data.index();
        data
    }
}

/// Read a `[tx, ty, sx, sy]` transform array into an [`Affine`].
fn parse_xf(v: Option<&Value>) -> Option<Affine> {
    let a = v?.as_array()?;
    let n = |i: usize| a.get(i).and_then(Value::as_f64).map(|f| f as f32);
    Some(Affine {
        tx: n(0)?,
        ty: n(1)?,
        sx: n(2)?,
        sy: n(3)?,
    })
}

fn parse_cursor(v: &Value) -> Option<Cursor> {
    let o = v.as_object()?;
    let f = |k: &str| o.get(k).and_then(Value::as_f64).map(|x| x as f32);
    Some(Cursor {
        t: f("t")?,
        x: f("x")?,
        y0: f("y0")?,
        y1: f("y1")?,
    })
}

fn parse_prim(v: &Value) -> Option<Prim> {
    let obj = v.as_object()?;
    let id = obj.get("id").and_then(Value::as_str).map(str::to_string);
    match obj.get("k").and_then(Value::as_str)? {
        "glyph" => {
            let cp = u32::from_str_radix(obj.get("cp")?.as_str()?, 16).ok()?;
            Some(Prim::Glyph {
                cp,
                xf: parse_xf(obj.get("xf"))?,
                id,
            })
        }
        "line" => {
            let pts = obj.get("pts").and_then(Value::as_array)?;
            let pts: Vec<[f32; 2]> = pts
                .iter()
                .filter_map(|p| {
                    let a = p.as_array()?;
                    Some([a.first()?.as_f64()? as f32, a.get(1)?.as_f64()? as f32])
                })
                .collect();
            if pts.len() < 2 {
                return None;
            }
            Some(Prim::Line {
                pts,
                width: obj.get("w").and_then(Value::as_f64).unwrap_or(1.0) as f32,
                id,
            })
        }
        "fill" => Some(Prim::Fill {
            d: obj.get("d")?.as_str()?.to_string(),
            xf: parse_xf(obj.get("xf")).unwrap_or(Affine::IDENTITY),
            id,
        }),
        "text" => Some(Prim::Text {
            s: obj.get("s")?.as_str()?.to_string(),
            x: obj.get("x")?.as_f64()? as f32,
            y: obj.get("y")?.as_f64()? as f32,
            size: obj.get("size").and_then(Value::as_f64).unwrap_or(0.0) as f32,
            id,
        }),
        _ => None,
    }
}
