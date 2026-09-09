//! **What widget a catalogue view is**, and with what on it.
//!
//! A waveform, a curve and a roll are the three pictures every editor in this
//! project draws, and each of them was assembled three times: once in the
//! Python client's `View.build`, once in the web client's, and — for the
//! waveform — once more in the standalone host's own tree. The three agreed
//! because they were written to agree, which is the agreement that stops being
//! true quietly: the *type* a builder emits (`waveform` is a `signal` drawn as
//! a `trace`, `bpf` is a `curve`, `pianoroll` is `notes`), the gesture plan a
//! sample editor offers, and the pitch window a roll falls back to are all
//! rules, and a rule written three times is three answers waiting to differ.
//!
//! So this module holds them once. A caller states the **facts** — the buffer,
//! the points, the notes, the axis chrome it wants — and gets back the widget's
//! props; the caller stamps the id, because ids are the caller's and nothing
//! here knows a widget's number.
//!
//! # What is here and what is not
//!
//! Only what is the *same* for every caller. A window title, a window's size,
//! the widgets a script appends beside the picture and which id a widget gets
//! are none of this module's business, and neither is where a number came from:
//! a curve's axis is [`crate::view`]'s caller's to keep between redraws
//! (`clausters_core::edit::curve_axis` widens it), and the unit a position is
//! measured in is the caller's bridge.
//!
//! What *is* here is the part a second implementation would have to guess:
//! which widget draws which structure, which props it is fed, and the two small
//! rules that go with them — the sample editor's three-gesture plan and the
//! roll's pitch window.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

/// The gesture plan a sample editor offers: a plain drag sweeps a selection,
/// `Alt` draws over the samples and `Ctrl` grabs one.
///
/// Three gestures rather than a mode, and the drawing one refuses out loud
/// below the zoom where a pixel is one sample — so a hand never silently paints
/// what the eye cannot check.
pub const SAMPLE_GESTURES: [(&str, &str); 3] =
    [("drag", "select"), ("alt", "draw"), ("ctrl", "sample")];

/// The pitch window a roll falls back to when it has no notes to fit, and the
/// margin it leaves around the ones it has.
pub const PITCH_FLOOR: f64 = 48.0;
/// The top of that fallback window.
pub const PITCH_CEIL: f64 = 84.0;
/// The semitones of air a roll keeps above and below its outermost notes.
pub const PITCH_PAD: f64 = 4.0;

/// A take's picture: the samples, and the axis they are drawn on.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Waveform {
    /// The server buffer the host reads the samples from.
    pub buffer: Option<i64>,
    /// Its interleaved channel count. Every channel is kept and drawn.
    pub channels: Option<u32>,
    /// What the picture measures, innermost last — `"peak"`, `"rms"`, or the
    /// two as one space-separated string.
    pub measure: String,
    /// What the ruler counts: `"time"` for a take on its own, `"samples"` for
    /// a pane beside a document that counts frames.
    pub ruler: String,
    /// The rate the axis reads its numbers with.
    pub sample_rate: f64,
    /// The tempo, for a ruler that counts beats.
    pub tempo: Option<f64>,
    /// What the header draws.
    pub label: String,
    /// A fixed height, for a pane stacked with others. `None` is elastic.
    pub height: Option<f64>,
    /// The sample the playhead is anchored at, for a view following a clock.
    pub playhead_at: Option<f64>,
}

/// The props a take's picture is drawn from — a `signal` shown as a `trace`.
#[must_use]
pub fn waveform(take: &Waveform) -> Map<String, Value> {
    let mut props = Map::new();
    props.insert("type".into(), json!("signal"));
    props.insert("view".into(), json!("trace"));
    if let Some(buffer) = take.buffer {
        props.insert("buffer".into(), json!(buffer));
    }
    if let Some(channels) = take.channels {
        props.insert("channels".into(), json!(channels.max(1)));
    }
    if !take.measure.is_empty() {
        props.insert("measure".into(), json!(take.measure));
    }
    if !take.label.is_empty() {
        props.insert("label".into(), json!(take.label));
    }
    if let Some(height) = take.height {
        props.insert("h".into(), json!(height));
    }
    let mut gestures = Map::new();
    for (key, verb) in SAMPLE_GESTURES {
        gestures.insert(key.into(), json!(verb));
    }
    props.insert("gestures".into(), Value::Object(gestures));

    let mut x = Map::new();
    if !take.ruler.is_empty() {
        x.insert("unit".into(), json!(take.ruler));
    }
    x.insert("sample_rate".into(), json!(take.sample_rate));
    if let Some(tempo) = take.tempo {
        x.insert("tempo".into(), json!(tempo));
    }
    if let Some(at) = take.playhead_at {
        x.insert("playhead_at".into(), json!(at));
    }
    axes(&mut props, x, Map::new());
    props
}

/// Puts the axis chrome under the one key the wire declares it with.
///
/// A ruler, a navigation window, a playhead and a value range describe the
/// **container's axes** rather than each element drawn against them, so they
/// ride nested and the host flattens them at its door. Flat props still land,
/// but nested is the spelling the protocol declares and both clients' builders
/// already write.
fn axes(props: &mut Map<String, Value>, x: Map<String, Value>, y: Map<String, Value>) {
    let mut pair = Map::new();
    if !x.is_empty() {
        pair.insert("x".into(), Value::Object(x));
    }
    if !y.is_empty() {
        pair.insert("y".into(), Value::Object(y));
    }
    if !pair.is_empty() {
        props.insert("axes".into(), Value::Object(pair));
    }
}

/// A break-point curve's picture: the points, and the axis they are held
/// against.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Curve {
    /// The flat `time value shape curve` quads.
    pub points: Vec<f64>,
    /// The value axis, which the caller keeps between redraws — a curve that
    /// refits while a point is dragged moves every other point on screen.
    pub min: f64,
    /// The top of that axis.
    pub max: f64,
    /// How far the time axis reaches. Zero or less lets the widget fit it.
    pub duration: f64,
    /// What the header draws.
    pub label: String,
}

/// The props a curve's picture is drawn from.
#[must_use]
pub fn bpf(curve: &Curve) -> Map<String, Value> {
    let mut props = Map::new();
    props.insert("type".into(), json!("curve"));
    props.insert("points".into(), json!(curve.points));
    if curve.duration > 0.0 {
        props.insert("duration".into(), json!(curve.duration));
    }
    if !curve.label.is_empty() {
        props.insert("label".into(), json!(curve.label));
    }
    let mut y = Map::new();
    y.insert("min".into(), json!(curve.min));
    y.insert("max".into(), json!(curve.max));
    axes(&mut props, Map::new(), y);
    props
}

/// A timeline's picture: the notes on the beat grid, and the markers beside
/// them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Roll {
    /// The flat `start dur pitch velocity channel` quintuples.
    pub notes: Vec<f64>,
    /// The flat marker lane.
    pub osc: Vec<f64>,
    /// What the ruler counts.
    pub ruler: String,
    /// The tempo the grid is drawn at, in beats per second.
    pub tempo: f64,
    /// The rate the axis reads its numbers with.
    pub sample_rate: f64,
    /// Whether a hand may write here. A roll over what a generator produced
    /// has nothing to write onto, and saying so refuses the press instead of
    /// offering a drag that will be unwound. A caller that says nothing is
    /// taken to mean yes, since refusing is the exception.
    #[serde(default = "yes")]
    pub editable: bool,
}

impl Default for Roll {
    fn default() -> Self {
        Self {
            notes: Vec::new(),
            osc: Vec::new(),
            ruler: String::new(),
            tempo: 0.0,
            sample_rate: 0.0,
            editable: true,
        }
    }
}

fn yes() -> bool {
    true
}

/// The props a timeline's picture is drawn from — `notes`, with the pitch
/// window fitted to what it holds.
#[must_use]
pub fn pianoroll(roll: &Roll) -> Map<String, Value> {
    let mut props = Map::new();
    props.insert("type".into(), json!("notes"));
    let mut y = Map::new();
    if !roll.notes.is_empty() {
        props.insert("notes".into(), json!(roll.notes));
        let (low, high) = pitch_window(&roll.notes);
        y.insert("min".into(), json!(low));
        y.insert("max".into(), json!(high));
    }
    if !roll.osc.is_empty() {
        props.insert("osc".into(), json!(roll.osc));
    }
    if !roll.editable {
        props.insert("notes_editable".into(), json!(false));
    }
    let mut x = Map::new();
    x.insert(
        "unit".into(),
        json!(if roll.ruler.is_empty() {
            "beats"
        } else {
            &roll.ruler
        }),
    );
    x.insert("tempo".into(), json!(roll.tempo));
    x.insert("sample_rate".into(), json!(roll.sample_rate));
    axes(&mut props, x, y);
    props
}

/// The pitch window a roll of these notes is drawn in: the outermost pitches
/// with [`PITCH_PAD`] of air, and never so far from middle C that the fallback
/// window is out of sight — the bottom is at most [`PITCH_CEIL`] and the top at
/// least [`PITCH_FLOOR`], so a piece written high still shows where the
/// ordinary range was. Notes at all is what makes a window: with none, the
/// fallback is the whole answer.
#[must_use]
pub fn pitch_window(notes: &[f64]) -> (f64, f64) {
    let pitches: Vec<f64> = notes
        .as_chunks::<5>()
        .0
        .iter()
        .map(|note| note[2])
        .collect();
    let Some(low) = pitches.iter().copied().reduce(f64::min) else {
        return (PITCH_FLOOR, PITCH_CEIL);
    };
    let high = pitches.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    (
        (low - PITCH_PAD).min(PITCH_CEIL),
        (high + PITCH_PAD).max(PITCH_FLOOR),
    )
}

/// One catalogue view by name, from the facts written in its own shape —
/// the door a binding crosses, since a caller there holds JSON and not a
/// struct.
///
/// `None` for a kind this crate does not draw, or facts that will not read as
/// that kind's: a caller is told it asked for nothing rather than handed an
/// empty picture.
#[must_use]
pub fn props(kind: &str, facts: &Value) -> Option<Map<String, Value>> {
    match kind {
        "waveform" => serde_json::from_value(facts.clone())
            .ok()
            .map(|f: Waveform| waveform(&f)),
        "bpf" => serde_json::from_value(facts.clone())
            .ok()
            .map(|f: Curve| bpf(&f)),
        "pianoroll" => serde_json::from_value(facts.clone())
            .ok()
            .map(|f: Roll| pianoroll(&f)),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
