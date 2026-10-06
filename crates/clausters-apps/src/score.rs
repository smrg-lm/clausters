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
//! A menu bar holding every action the application has ([`menu`]); a toolbar
//! with what a hand reaches for while it writes ([`tools`]); the palettes of
//! what can be written, beside the page ([`palettes`]); the forms a menu
//! entry opens over it ([`dialogs`]); the engraved score in a scroll that pans and zooms -- every page of the paper
//! one under another, or one system as long as the music -- and a status line
//! under it saying what is selected. The page takes
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

pub mod dialogs;
pub mod editor;
pub mod entry;
pub mod icons;
pub mod menu;
pub mod palettes;
pub mod selection;
pub mod tools;
pub mod verbs;

use std::sync::{Arc, Mutex};

use serde_json::{Map, Value, json};

use clausters_core::notation::{AnyEngraver, Page, PageSetup, Score};

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

/// **The chrome's widget ids**, which are the caller's too: each tool of the
/// toolbar, each widget of the dialogs and each entry of the palettes, by the
/// name the crate gives it ([`tools::TOOLS`], [`dialogs::names`],
/// [`palettes::names`]). What is left empty is chrome the window does not
/// have.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Chrome {
    /// The toolbar's tools.
    pub tools: tools::Ids,
    /// The dialogs' widgets: all of them, or the window has no dialogs.
    pub dialogs: dialogs::Ids,
    /// The palettes' entries.
    pub palettes: palettes::Ids,
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

/// **How big the drawing is in the window**, `(width, height)`: the page's
/// own units times `scale`, pixels per page unit.
///
/// The scale is fixed by the paper and the window it opened in
/// ([`scale_for`]), not by the drawing -- so a staff is the same size in a
/// page view and in a continuous one, and an edit that adds a page makes the
/// drawing longer rather than the engraving smaller.
pub fn drawn_size(page: &Page, scale: f64) -> (f64, f64) {
    let [w, h] = page.draw.vb;
    let round = |v: f64| (v * scale * 10.0).round() / 10.0;
    (round(w).max(1.0), round(h).max(1.0))
}

/// **The pixels a page unit is drawn at**: what makes a page of `setup`'s
/// width fill a window of `size`, the margins taken off. A drawing's units are
/// ten to the engraver's, which are tenths of a millimetre.
pub fn scale_for(setup: &PageSetup, size: (i64, i64)) -> f64 {
    let width = (size.0 as f64 - 16.0).max(1.0);
    width / (f64::from(setup.width.max(1)) * 10.0)
}

/// The gap between two pages of a page view, in page units: a centimetre.
pub const PAGE_GAP: f64 = 1000.0;

/// What the window is composed from.
pub struct Window<'a> {
    /// The drawing.
    pub page: &'a Page,
    /// The caller's widget ids.
    pub ids: Ids,
    /// The window's title and its size.
    pub title: &'a str,
    pub size: (i64, i64),
    /// What the status line says.
    pub status: &'a str,
    /// Whether the page is in note entry.
    pub entry: bool,
    /// Where note entry writes next, as the page's `edit_cursor` prop, or
    /// the empty string outside the mode.
    pub edit_cursor: Value,
    /// The key table's scopes in force in the window, as its `keys` prop.
    pub keys: Value,
    /// How big the drawing is ([`scale_for`]).
    pub scale: f64,
    /// The menu bar ([`menu::menu`]).
    pub menu: Value,
    /// The toolbar ([`tools::toolbar`]), when the caller numbered its tools.
    pub toolbar: Option<Value>,
    /// The dialogs ([`dialogs::stack`]), when the caller numbered them.
    pub dialogs: Option<Value>,
    /// The palettes ([`palettes::column`]), when the caller numbered any.
    pub palettes: Option<Value>,
    /// The outlines of the symbols the chrome is labelled with, by codepoint
    /// ([`tools::Outlines`]): the window's `glyphs`, which is what lets the
    /// host draw a character of a music font it has no face for.
    pub glyphs: &'a tools::Outlines,
}

/// **The window**, as a GuiDef rooted at a `window` node: the menu bar; the
/// toolbar; the palettes beside the drawing, which is under `ids.page` in a
/// scroll that pans both ways and zooms; the status line under them; and the
/// dialogs. A script's own widgets are the client's to
/// append, as in every application here.
pub fn window(w: Window<'_>) -> Value {
    let Window {
        page,
        ids,
        title,
        size,
        status,
        entry,
        edit_cursor,
        keys,
        scale,
        menu,
        toolbar,
        dialogs,
        palettes,
        glyphs,
    } = w;
    let (width, height) = drawn_size(page, scale);
    let mut picture = drawing(page);
    picture.insert("type".into(), json!("score"));
    picture.insert("id".into(), json!(ids.page));
    picture.insert("editable".into(), json!(true));
    picture.insert("entry".into(), json!(entry));
    picture.insert("edit_cursor".into(), edit_cursor);
    // **The play cursor is anchored at the score's start**: the page draws it
    // over the engraver's timemap from whatever counter the window's head
    // clock names, which is the position of the transport the score plays on
    // -- so it needs no anchor of its own and no message per frame.
    picture.insert("playhead_at".into(), json!(0));
    for (key, value) in [("x", 0.0), ("y", 0.0), ("w", width), ("h", height)] {
        picture.insert(key.into(), json!(value));
    }
    // **A drag that starts on no staff and no element pans**: the page
    // declines that press, and the scroll it sits in takes it. Both axes,
    // since a continuous view is as long as the music and a page zoomed in is
    // wider than the window.
    let mut scroll = json!({
        "type": "plane",
        "axis": "both",
        // the wheel turns the pages, and Ctrl with it zooms
        "zoom": "ctrl",
        "bars": true,
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
    // **The palettes stand beside the page**, with a divider a drag moves
    // between them; a window without them gives the page the whole width.
    let work = match palettes {
        Some(palettes) => json!({
            "type": "layout",
            "flow": "row",
            "margin": 0,
            "split": true,
            "weight": 1,
            "children": [palettes, scroll],
        }),
        None => scroll,
    };
    let children: Vec<Value> = toolbar
        .into_iter()
        .chain([work, line])
        .chain(dialogs)
        .collect();
    let mut window = json!({
        "type": "window",
        "title": title,
        "w": size.0,
        "h": size.1,
        "flow": "col",
        "menu": menu,
        // **The space bar is the application's**: it plays the score through
        // the editor's own playback, so the host's monitor stays out.
        "plays": true,
        // **The keys are the editor's in its scopes**: `N` everywhere in the
        // window, and the letters, the arrows and the digits in note entry.
        "keys": keys,
        "children": children,
    });
    if let (false, Some(map)) = (glyphs.is_empty(), window.as_object_mut()) {
        map.insert("glyphs".into(), json!(glyphs));
    }
    window
}

/// **What the window is corrected with** after the score changed: the drawing
/// replaced in place and sized again, and the scroll's content with it -- each
/// `(widget, props)`.
pub fn correction(page: &Page, ids: Ids, scale: f64) -> Vec<(i32, Value)> {
    let (width, height) = drawn_size(page, scale);
    let mut out = vec![(
        ids.page,
        json!({"display_list": Value::Object(drawing(page)), "w": width, "h": height}),
    )];
    if let Some(scroll) = ids.scroll {
        out.push((scroll, json!({"content_w": width, "content_h": height})));
    }
    out
}
