//! **The score editor**: a symbolic score on its engraved page, edited by hand.
//!
//! What it opens is a [`Score`] -- the core's engraver-driven document, its
//! model a [`clausters_core::notation::Sheet`] -- and it edits it **in place**:
//! the score is shared with whoever holds it (a client's `Score` handle), so a
//! script reads every edit through the handle it already has. The engraver is
//! a port the caller hands in ([`AnyEngraver`]): libverovio natively, a JS
//! object in a page. This crate links neither.
//!
//! # The window
//!
//! The engraved page, in a scroll as tall as the page is at the window's
//! width, and a status line under it saying what is selected. The page takes
//! pitch edits and note entry; what a hand may do to each element is the
//! page's own (`kinds`, read against the core's `admits`), so a slur is
//! selected and never dragged.
//!
//! # A turn
//!
//! A gesture on the page -- a press naming an element, a drag naming the staff
//! position a note reaches, a press on empty staff naming a place -- is read
//! into one model operation, applied to the shared score, and recorded with
//! the MEI before it as its inverse: a state rather than a step, so a walk
//! through the history loads it back whole. A verb the client calls (a mark, a
//! length, a voice) is the same: an operation over what is selected, built
//! here ([`verbs`]) rather than in each client.

pub mod editor;
pub mod verbs;

use std::sync::{Arc, Mutex};

use serde_json::{Map, Value, json};

use clausters_core::notation::{AnyEngraver, Page, Score};

/// A score the editor and its holder edit together.
pub type Shared = Arc<Mutex<Score<AnyEngraver>>>;

/// **The window's widget ids**, which are the caller's: the page, the scroll
/// it sits in, and the status line under it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ids {
    /// The `score` widget.
    pub page: i32,
    /// The scroll the page sits in, which grows with it.
    pub scroll: Option<i32>,
    /// The line saying what is selected.
    pub status: Option<i32>,
}

/// How tall the status line under the page is.
pub const STATUS_H: f64 = 22.0;

/// **The page as the host draws it**: everything the engraving produced except
/// the notes that sound, which are the caller's own layer.
///
/// Derived from the page rather than listed, so a layer the walk adds reaches
/// the host with nothing to update here.
pub fn drawing(page: &Page) -> Map<String, Value> {
    let mut out = match serde_json::to_value(page) {
        Ok(Value::Object(map)) => map,
        _ => Map::new(),
    };
    out.remove("notes");
    out
}

/// **How tall the page is drawn at `width`**, in its own aspect: what the
/// `score` widget and the scroll's content are sized to, so an edit that adds a
/// system grows the page rather than shrinking the engraving to fit.
pub fn drawn_height(page: &Page, width: f64) -> f64 {
    let [w, h] = page.draw.vb;
    if w > 0.0 {
        (width * h / w * 10.0).round() / 10.0
    } else {
        width
    }
}

/// The page's width in a window of `size`, the margins taken off.
pub fn page_width(size: (i64, i64)) -> f64 {
    (size.0 as f64 - 16.0).max(1.0)
}

/// **The window**, as a GuiDef rooted at a `window` node: the page under
/// `ids.page`, in a scroll, with the status line under it saying `status`. A
/// script's own widgets are the client's to append, as in every application
/// here.
pub fn window(page: &Page, ids: Ids, title: &str, size: (i64, i64), status: &str) -> Value {
    let width = page_width(size);
    let height = drawn_height(page, width);
    let mut picture = drawing(page);
    picture.insert("type".into(), json!("score"));
    picture.insert("id".into(), json!(ids.page));
    picture.insert("editable".into(), json!(true));
    picture.insert("entry".into(), json!(true));
    for (key, value) in [("x", 0.0), ("y", 0.0), ("w", width), ("h", height)] {
        picture.insert(key.into(), json!(value));
    }
    let mut scroll = json!({
        "type": "scroll",
        "axis": "y",
        "content_w": width,
        "content_h": height,
        "children": [Value::Object(picture)],
    });
    if let (Some(id), Some(map)) = (ids.scroll, scroll.as_object_mut()) {
        map.insert("id".into(), json!(id));
    }
    let mut line = json!({"type": "label", "text": status, "h": STATUS_H});
    if let (Some(id), Some(map)) = (ids.status, line.as_object_mut()) {
        map.insert("id".into(), json!(id));
    }
    json!({
        "type": "window",
        "title": title,
        "w": size.0,
        "h": size.1,
        "flow": "col",
        "children": [scroll, line],
    })
}

/// **What the window is corrected with** after the score changed: the page
/// replaced in place and sized again, and the scroll grown with it -- each
/// `(widget, props)`.
pub fn correction(page: &Page, ids: Ids, size: (i64, i64)) -> Vec<(i32, Value)> {
    let height = drawn_height(page, page_width(size));
    let mut out = vec![(
        ids.page,
        json!({"display_list": Value::Object(drawing(page)), "h": height}),
    )];
    if let Some(scroll) = ids.scroll {
        out.push((scroll, json!({"content_h": height})));
    }
    out
}
