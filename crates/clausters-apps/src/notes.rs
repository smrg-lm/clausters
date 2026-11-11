//! **The notes editor**: an event sequence on a roll, edited note by note.
//!
//! What it opens is an [`EventSequence`] -- the document's, with an id per
//! event -- and it edits it **in place**: the sequence is shared with whoever
//! holds it (a client's handle, a session's source), so there is no copy to
//! write back and nothing to reconcile. A timeline is opened by rendering it
//! first, which is the client's `edit(timeline)`: what the roll edits is the
//! events the timeline produced, never the timeline.
//!
//! # The window
//!
//! One widget, the roll: the catalogue's `pianoroll` picture
//! ([`clausters_document::view::catalogue`]) over the sequence's notes on its
//! own beats, with the ids of the events beside them (`note_ids`) so every
//! gesture comes back naming the notes it touched. The Y axis is a domain
//! ([`clausters_editing::notes::YDomain`]); the first is MIDI notes.
//!
//! # A turn
//!
//! A gesture is read by [`clausters_editing::notes::intake`] into one edit in
//! the sequence's vocabulary, applied to the shared sequence, and recorded with
//! the sequence as it was as its inverse. The answer corrects the roll, since a
//! note the hand made has an id only once the sequence has given it one.
//!
//! # The dialog
//!
//! Entering notation over a sequence written as no notation asks how it is
//! read ([`dialogs`]): a meter, the shortest value and how the polyphony is
//! written, held as the sequence's reading says until the hand changes them.
//! A window composed without the dialog converts with the reading the
//! sequence stores.

pub mod dialogs;
pub mod display;
pub mod editor;
pub mod file;
pub mod panel;

use std::sync::{Arc, Mutex};

use serde_json::{Map, Value, json};

use clausters_document::EventSequence;
use clausters_document::view::catalogue::{self, Roll};
use clausters_editing::notes::{self, Axis, Editing, YDomain};
use clausters_editing::verbs;

/// A sequence the editor and its holder edit together.
pub type Shared = Arc<Mutex<EventSequence>>;

/// **Where a roll's marks get their outlines**: the engraver the application
/// holds, asked for the SMuFL codepoints a page writes marks with, answering
/// the outline of each it has. The application hands one in where it holds an
/// engraver; a roll without one writes its marks as text.
pub type OutlineSource =
    Arc<dyn Fn(&[&str]) -> std::collections::BTreeMap<String, String> + Send + Sync>;

/// **The marks a roll can write a symbol for, by name**: the articulations,
/// the breaths and the dynamics a page writes.
pub const MARK_NAMES: &[&str] = &[
    "stacc", "stacciss", "spicc", "acc", "marc", "ten", "stress", "breath", "caesura", "pp", "p",
    "mp", "mf", "f", "ff", "sf", "fp",
];

/// **The symbols a roll draws a note's marks with**: the engraver's outlines,
/// asked for once and only when the roll first shows a page's marks, since
/// asking loads the engraver -- the score editor's own symbols, so a roll and
/// a page write one accent (`score::palettes::symbol`). Without
/// them a mark is written as the text the palette labels it with.
#[derive(Clone, Default)]
pub struct Symbols {
    source: Option<OutlineSource>,
    held: Option<std::collections::BTreeMap<String, String>>,
}

impl std::fmt::Debug for Symbols {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Symbols")
            .field("source", &self.source.is_some())
            .field("held", &self.held.as_ref().map(|h| h.len()))
            .finish()
    }
}

impl Symbols {
    /// Symbols asked of `source`.
    pub fn with(source: Option<OutlineSource>) -> Self {
        Self { source, held: None }
    }

    /// Where it asks.
    pub fn source(&self) -> Option<OutlineSource> {
        self.source.clone()
    }

    /// **The outlines, asked for the first time**: `Some` once, with what the
    /// engraver had (nothing where there is no engraver or no mark has a
    /// symbol), and `None` every time after.
    pub fn ask(&mut self) -> Option<&std::collections::BTreeMap<String, String>> {
        if self.held.is_some() {
            return None;
        }
        let codes: Vec<&str> = MARK_NAMES.iter().filter_map(|name| code_of(name)).collect();
        let found = self
            .source
            .as_ref()
            .map(|source| source(&codes))
            .unwrap_or_default();
        self.held = Some(found);
        self.held.as_ref()
    }

    /// **How the mark `name` is drawn**: its symbol's character where the
    /// engraver gave its outline, else the text a palette labels it with,
    /// else its name.
    pub fn drawn(&self, name: &str) -> String {
        let held = self.held.as_ref();
        if let Some(code) = code_of(name)
            && held.is_some_and(|h| h.contains_key(code))
            && let Some(c) = u32::from_str_radix(code, 16).ok().and_then(char::from_u32)
        {
            return c.to_string();
        }
        text_of(name).unwrap_or(name).to_string()
    }
}

/// The SMuFL codepoint the mark `name` is written with.
#[cfg(feature = "notation")]
fn code_of(name: &str) -> Option<&'static str> {
    crate::score::palettes::symbol(name).map(|(code, _)| code)
}

/// The text the mark `name` reads as without its symbol.
#[cfg(feature = "notation")]
fn text_of(name: &str) -> Option<&'static str> {
    crate::score::palettes::symbol(name).map(|(_, text)| text)
}

#[cfg(not(feature = "notation"))]
fn code_of(_: &str) -> Option<&'static str> {
    None
}

#[cfg(not(feature = "notation"))]
fn text_of(_: &str) -> Option<&'static str> {
    None
}

/// **The props the roll is drawn with**: the notes, their ids and the markers,
/// on the sequence's beats through its tempo map at `rate` samples a second,
/// as `editing` draws them; read-only when `editable` is `false`.
pub fn props(
    sequence: &EventSequence,
    domain: &YDomain,
    rate: f64,
    editable: bool,
    editing: &Editing,
    symbols: &Symbols,
) -> Map<String, Value> {
    let axis = Axis::of(sequence, rate);
    let drawn = notes::project(sequence, domain, &axis, editing);
    let mut beside = Map::new();
    written(&mut beside, &drawn, symbols);
    let mut out = catalogue::pianoroll(&Roll {
        notes: drawn.notes,
        osc: drawn.osc,
        ruler: "beats".into(),
        tempo_map: serde_json::to_string(&axis.map).ok(),
        sample_rate: axis.rate,
        editable,
        ..Roll::default()
    });
    out.insert("note_ids".into(), json!(drawn.note_ids));
    out.insert("curves".into(), json!(drawn.curves));
    out.insert("layers".into(), json!(drawn.layers));
    out.insert("points".into(), json!(drawn.points));
    out.insert("midi".into(), json!(drawn.midi));
    out.insert("joins".into(), json!(drawn.joins));
    out.insert("lines".into(), json!(drawn.lines));
    out.extend(beside);
    out.remove("type");
    // **The window is the reader's**, as the multitrack's is: a content change
    // in an editor is mostly the reader's own edit, or a mode drawing the same
    // notes at their written values, so the axis neither re-frames nor pages
    // forward on one.
    out.insert("autofit".into(), json!(false));
    let notes: Vec<f64> = out
        .get("notes")
        .and_then(Value::as_array)
        .map(|n| n.iter().filter_map(Value::as_f64).collect())
        .unwrap_or_default();
    y_axis(&mut out, &notes, domain);
    // **The play cursor is the transport's position**: anchored at 0, the
    // counter the window's playheads read (the playback's transport) puts the
    // line on the sample the lane is playing, stopped or rolling.
    if let Some(Value::Object(x)) = out.get_mut("axes").and_then(|axes| axes.get_mut("x")) {
        x.insert("playhead_at".into(), json!(0.0));
        // the bars are the axis' own, as its tempo map is
        x.insert("bars".into(), json!(drawn.bars));
    }
    out
}

/// **The Y axis a roll navigates, and the window it opens on.**
///
/// The compass is the domain's whole range -- MIDI notes 0 to 127, or the
/// hertz the domain spans, which reach past the highest MIDI note -- so the
/// hand scrolls to any octave; and the window is the notes with some air
/// around them, as a box's roll is fitted (`catalogue::pitch_window`), sent as
/// the slice of the compass the view starts on. A roll in hertz says so, and
/// its compass is in hertz; its window is fitted in pitch, where the air
/// around the notes is semitones, since a log frequency is a linear pitch.
fn y_axis(props: &mut Map<String, Value>, notes: &[f64], domain: &YDomain) {
    use clausters_core::scale::hz_to_midi;

    let hz = domain.ruler == "hz";
    let row = |value: f64| {
        if hz {
            hz_to_midi(value.max(1e-3))
        } else {
            value
        }
    };
    let (floor, ceiling) = (row(domain.min), row(domain.max));
    let span = (ceiling - floor).max(1.0);
    let pitches: Vec<f64> = notes
        .as_chunks::<5>()
        .0
        .iter()
        .flat_map(|n| [n[0], n[1], row(n[2]), n[3], n[4]])
        .collect();
    let (low, high) = catalogue::pitch_window(&pitches);
    let (low, high) = (low.clamp(floor, ceiling), high.clamp(floor, ceiling));
    let mut y = Map::new();
    if hz {
        y.insert("unit".into(), json!("hz"));
    }
    y.insert("min".into(), json!(domain.min));
    y.insert("max".into(), json!(domain.max));
    y.insert("start".into(), json!((low - floor) / span));
    y.insert("len".into(), json!(((high - low) / span).max(1.0 / span)));
    if let Some(axes) = props
        .entry("axes")
        .or_insert_with(|| json!({}))
        .as_object_mut()
    {
        axes.insert("y".into(), Value::Object(y));
    }
}

/// **What the roll is corrected with** after the sequence changed: the notes,
/// their ids, the markers, the curves and their points, the pitch lines and the tempo map, each stated even when empty -- a
/// roll whose last note was removed is told so. The pitch window is left as
/// the hand has it.
pub fn correction(
    sequence: &EventSequence,
    domain: &YDomain,
    rate: f64,
    editing: &Editing,
    symbols: &Symbols,
) -> Map<String, Value> {
    let axis = Axis::of(sequence, rate);
    let drawn = notes::project(sequence, domain, &axis, editing);
    let mut out = Map::new();
    out.insert("notes".into(), json!(drawn.notes));
    out.insert("note_ids".into(), json!(drawn.note_ids));
    out.insert("osc".into(), json!(drawn.osc));
    out.insert("curves".into(), json!(drawn.curves));
    out.insert("layers".into(), json!(drawn.layers));
    out.insert("points".into(), json!(drawn.points));
    out.insert("midi".into(), json!(drawn.midi));
    out.insert("joins".into(), json!(drawn.joins));
    out.insert("lines".into(), json!(drawn.lines));
    written(&mut out, &drawn, symbols);
    out.insert("bars".into(), json!(drawn.bars));
    if let Ok(map) = serde_json::to_string(&axis.map) {
        out.insert("tempo_map".into(), json!(map));
    }
    out
}

/// **What the notation mode draws beside the notes**, each list stated --
/// empty in MIDI mode, which takes them away. A note's marks are drawn with
/// `symbols`: its articulations over the box (`marks`), its breath at its
/// end (`breaths`) and its dynamic in it (`labels`).
fn written(out: &mut Map<String, Value>, drawn: &notes::Projection, symbols: &Symbols) {
    out.insert("held".into(), json!(drawn.held));
    out.insert("voices".into(), json!(drawn.voices));
    out.insert("dimmed".into(), json!(drawn.dimmed));
    let labels: Vec<String> = drawn
        .labels
        .iter()
        .map(|dynamic| {
            if dynamic.is_empty() {
                String::new()
            } else {
                symbols.drawn(dynamic)
            }
        })
        .collect();
    out.insert("labels".into(), json!(labels));
    out.insert("spelled".into(), json!(drawn.spelled));
    let breath = |name: &&str| matches!(*name, "breath" | "caesura");
    let (mut marks, mut breaths) = (Vec::new(), Vec::new());
    for names in &drawn.marks {
        let names: Vec<&str> = names.split_whitespace().collect();
        // one by one, a space between, for the roll to lay them out
        marks.push(
            names
                .iter()
                .filter(|n| !breath(n))
                .map(|n| symbols.drawn(n))
                .collect::<Vec<String>>()
                .join(" "),
        );
        breaths.push(
            names
                .iter()
                .find(|n| breath(n))
                .map(|n| symbols.drawn(n))
                .unwrap_or_default(),
        );
    }
    out.insert("tints".into(), json!(drawn.tints));
    out.insert("slurs".into(), json!(drawn.slurs));
    out.insert("marks".into(), json!(marks));
    out.insert("breaths".into(), json!(breaths));
}

/// **The window**, as a GuiDef rooted at a `window` node: the roll under
/// `widget`, with the title and the size given. A script's own widgets are the
/// client's to append, as in every application here.
pub fn window(roll: Map<String, Value>, widget: i32, title: &str, size: (i64, i64)) -> Value {
    let mut picture = roll;
    picture.insert("type".into(), json!("notes"));
    picture.insert("id".into(), json!(widget));
    json!({
        "type": "window",
        "title": title,
        "w": size.0,
        "h": size.1,
        "flow": "col",
        // **The space bar is the application's**: it plays the sequence
        // through the editor's own playback, so the host's monitor stays out.
        "plays": true,
        // the roll's own verbs, and the scope of the key table they are in
        "keys": [verbs::NOTES_SCOPE],
        "verbs": verbs::declaration(&[(verbs::NOTES_SCOPE, verbs::NOTES)]),
        "children": [Value::Object(picture)],
    })
}
