//! **The points editor, one turn at a time**: what a gesture on the curve does
//! to it, the entry it leaves, and what the window is corrected with.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use clausters_core::tempoclock::{samples_to_secs, secs_to_samples};
use clausters_document::history::Editable;
use clausters_document::multitrack::Automation;
use clausters_document::points::{self as vocabulary, POINTS, Points};
use clausters_document::{Applied, Opaque};
use clausters_editing::conversation::{self, Answer, Conversation, Correction};
use clausters_editing::points;

use super::{Held, Shared, props, window};
use crate::turn::{self, Converse, Event, Kind, Leg, Record};

/// The vocabulary the editor's structure is registered under.
pub const DOMAIN: &str = POINTS;

/// **What one turn came to.**
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    /// What kind of turn the message was.
    pub turn: Kind,
    /// What to send the host.
    pub answer: Option<Answer>,
    /// The stamp a [`Kind::Step`] is answered with.
    pub seq: i64,
    /// Whether a [`Kind::Step`] walks forward.
    pub redo: bool,
    /// The entry to record, when the turn edited the curve.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record: Option<Record>,
    /// Whether the curve changed.
    pub changed: bool,
    /// The version after the turn.
    pub version: i64,
    /// **The curve's points after an edit**, as the flat `t v shape curve`
    /// quads: what a caller whose curve is an object of its own writes back.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub points: Option<Vec<f64>>,
    /// Where the position cursor was placed, in the curve's seconds -- a click
    /// on its ruler. Not an edit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locate: Option<f64>,
    /// **The time range a sweep left**, when the hand moved it: `[start, end]`
    /// in the curve's seconds, or `null` once it was cleared. Not an edit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span: Option<Value>,
}

turn::turned!(Outcome);

/// **A points editor**: a shared curve, the axes its window holds, the time
/// range and the value band a sweep left, and its end of the conversation.
#[derive(Clone, Debug)]
pub struct PointsEditor {
    curve: Shared,
    rate: f64,
    conversation: Conversation,
    window: Option<i32>,
    widget: Option<i32>,
    title: String,
    size: (i64, i64),
    held: Held,
    /// The time range a sweep left, `[start, end]` in the curve's seconds.
    span: Option<(f64, f64)>,
    /// The value band the same sweep covered, when it had height.
    band: Option<(f64, f64)>,
}

/// What a points editor is opened with, as the context's door reads it.
#[derive(Deserialize)]
#[serde(default)]
struct Opened {
    rate: f64,
    title: String,
    w: i64,
    h: i64,
    /// The value axis the caller declared, both ends or neither.
    min: Option<f64>,
    max: Option<f64>,
    version: i64,
}

impl Default for Opened {
    fn default() -> Self {
        Self {
            rate: 48_000.0,
            title: "Curve".into(),
            w: 1000,
            h: 520,
            min: None,
            max: None,
            version: 1,
        }
    }
}

/// `payload` applied to the curve's points: the `points` vocabulary over the
/// one field of an automation it is about.
fn edit(curve: &mut Automation, payload: &Opaque) -> Applied {
    let mut held = Points(std::mem::take(&mut curve.points));
    let applied = held.apply(payload);
    curve.points = held.0;
    applied
}

impl PointsEditor {
    /// An editor over `curve`, its time axis counted at `rate` samples a
    /// second.
    pub fn new(curve: Shared, rate: f64, version: i64) -> Self {
        Self {
            curve,
            rate,
            conversation: Conversation::new(version),
            window: None,
            widget: None,
            title: "Curve".into(),
            size: (1000, 520),
            held: Held::default(),
            span: None,
            band: None,
        }
    }

    /// The curve it edits.
    pub fn curve(&self) -> &Shared {
        &self.curve
    }

    fn held(&self) -> std::sync::MutexGuard<'_, Automation> {
        // A poisoned lock is a panic elsewhere while the curve was held; the
        // data is still the curve, and refusing it here would lose it.
        self.curve.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// **The window**, with the curve under `widget` -- which is then the one
    /// the editor answers for.
    pub fn window(&mut self, widget: i32) -> Value {
        self.widget = Some(widget);
        let curve = self.held().clone();
        window(&curve, &mut self.held, widget, &self.title, self.size)
    }

    /// A position in seconds as the timeline samples the widget counts.
    fn units(&self, secs: f64) -> f64 {
        secs_to_samples(secs, self.rate) as f64
    }

    /// Timeline samples as the curve's seconds.
    fn secs(&self, units: f64) -> f64 {
        samples_to_secs(units.max(0.0).round() as i64, self.rate)
    }

    /// What `widget` is corrected with, or nothing for one that is not the
    /// curve.
    fn resync_widget(&mut self, widget: i64) -> Vec<Correction> {
        if self.widget.map(i64::from) != Some(widget) {
            return Vec::new();
        }
        let curve = self.held().clone();
        let mut props = props(&curve, &mut self.held);
        // The time range is drawn where the hand sweeps one, so a span set
        // from the client shows as the band a sweep leaves.
        let (start, len) = self.span.map_or((0.0, 0.0), |(a, b)| {
            let start = self.units(a);
            (start, self.units(b) - start)
        });
        let (min, max) = self.band.unwrap_or((0.0, 0.0));
        props.insert("sel_start".into(), json!(start));
        props.insert("sel_len".into(), json!(len));
        props.insert("sel_min".into(), json!(min));
        props.insert("sel_max".into(), json!(max));
        vec![Correction {
            widget,
            props: Value::Object(props),
        }]
    }

    /// **Every widget of the window, corrected** -- what a history step leaves
    /// behind.
    pub fn resync_all(&mut self, version: i64) -> Answer {
        let corrections = self
            .widget
            .map_or_else(Vec::new, |w| self.resync_widget(i64::from(w)));
        conversation::answer(0, version, None, corrections)
    }

    /// Answers the stamp a [`Kind::Step`] carried, once the caller has walked.
    pub fn acknowledge(&self, seq: i64, version: i64, reason: Option<String>) -> Answer {
        conversation::answer(seq, version, reason, Vec::new())
    }

    /// **One message from the host**, read and answered ([`turn::turn`]).
    pub fn event(&mut self, event: &Event, version: i64) -> Outcome {
        turn::turn(self, event, version)
    }

    /// **One payload of a history step**, applied to the curve. Answers the
    /// points it leaves, as quads, when it changed them.
    pub fn apply(&mut self, payload: &Value) -> Option<Vec<f64>> {
        let mut curve = self.held();
        edit(&mut curve, &Opaque(payload.clone()))
            .applied
            .then(|| points::quads(&curve.points))
    }

    /// **The points a sweep covers**, by index: those inside its time range
    /// and, where it had height, inside its value band. None without a range.
    pub fn selected(&self) -> Vec<usize> {
        let Some((start, end)) = self.span else {
            return Vec::new();
        };
        let inside = |v: f64, (lo, hi): (f64, f64)| v >= lo && v <= hi;
        self.held()
            .points
            .iter()
            .enumerate()
            .filter(|(_, p)| {
                inside(p.at, (start, end)) && self.band.is_none_or(|band| inside(p.value, band))
            })
            .map(|(i, _)| i)
            .collect()
    }

    fn gesture(
        &mut self,
        widget: i64,
        tag: &str,
        values: &[Value],
        out: &mut Outcome,
    ) -> (Option<String>, Vec<Correction>) {
        let at = |i: usize| values.get(i).and_then(Value::as_f64).unwrap_or(0.0);
        match tag {
            "selection" => {
                // A sweep's time range, on the axis's samples, kept in the
                // curve's seconds; a sweep with height covered a value band
                // too, in the curve's own values. A range of no length is none.
                let (start, len) = (at(0).max(0.0), at(1));
                self.span = (len > 0.0).then(|| (self.secs(start), self.secs(start + len)));
                self.band = (self.span.is_some() && values.len() >= 4)
                    .then(|| (at(2).min(at(3)), at(2).max(at(3))));
                out.span = Some(self.span.map_or(Value::Null, |(a, b)| json!([a, b])));
                (None, Vec::new())
            }
            "locate" => {
                // A click on the ruler: the reader put the position cursor
                // there. It is not a seek -- a curve has no transport.
                if !values.is_empty() {
                    out.locate = Some(self.secs(at(0)));
                }
                (None, Vec::new())
            }
            "points" => {
                let flat: Vec<f64> = values.iter().filter_map(Value::as_f64).collect();
                let intake = points::intake(tag, &flat);
                let Some(payload) = intake.payloads.into_iter().next() else {
                    return (None, Vec::new());
                };
                let recorded = {
                    let mut curve = self.held();
                    let backward = vocabulary::payload(&Points(curve.points.clone()).state());
                    let applied = edit(&mut curve, &Opaque(payload.clone()));
                    if let Some(why) = applied.reason {
                        drop(curve);
                        return (Some(why), self.resync_widget(widget));
                    }
                    applied.applied.then(|| {
                        out.points = Some(points::quads(&curve.points));
                        Record {
                            label: intake.label,
                            legs: vec![Leg {
                                forward: json!({ "edit": payload }),
                                backward: backward.0,
                                key: POINTS.into(),
                                ..Leg::default()
                            }],
                        }
                    })
                };
                if let Some(record) = recorded {
                    out.record = Some(record);
                    out.changed = true;
                    out.version += 1;
                }
                // The axis may have grown under the point the hand dragged out.
                (None, self.resync_widget(widget))
            }
            _ => (None, Vec::new()),
        }
    }
}

impl Converse for PointsEditor {
    type Outcome = Outcome;

    fn conversation(&mut self) -> &mut Conversation {
        &mut self.conversation
    }

    fn window_id(&self) -> Option<i32> {
        self.window
    }

    fn closed(&mut self) {
        self.window = None;
    }

    fn owns(&self, widget: i64, _tag: &str) -> bool {
        self.widget.map(i64::from) == Some(widget)
    }

    fn resync(&mut self, widget: i64) -> Vec<Correction> {
        self.resync_widget(widget)
    }

    fn route(
        &mut self,
        widget: i64,
        tag: &str,
        values: &[Value],
        out: &mut Outcome,
    ) -> (Option<String>, Vec<Correction>) {
        self.gesture(widget, tag, values, out)
    }
}

/// **A points editor from JSON**: `{"rate", "title", "w", "h", "min", "max",
/// "version"}` over `curve`. `min` and `max` declare the value axis the curve
/// is first drawn against; it still grows to hold a point dragged outside it.
pub fn new_json(curve: Shared, request: &str) -> PointsEditor {
    let opened: Opened = serde_json::from_str(request).unwrap_or_default();
    let mut editor = PointsEditor::new(curve, opened.rate, opened.version);
    editor.title = opened.title;
    editor.size = (opened.w, opened.h);
    if let (Some(min), Some(max)) = (opened.min, opened.max) {
        editor.held.axis = Some((min.min(max), min.max(max)));
    }
    editor
}

/// **One verb of the editor's own door**, over JSON:
///
/// - `window` -- `widget`: the GuiDef, the curve under that id.
/// - `props` -- `widget`: what it is corrected with (`{}` for another widget).
/// - `sync` -- `window` (the id it is open in, or `null`), `points` (the curve
///   as its holder has it now, flat quads), `name`, `rate`, `title`, `w`, `h`:
///   `{}`.
/// - `state` -- `{"points"}`: the curve as flat quads.
/// - `span` -- `span`, when given: `[start, end]` in the curve's seconds, or
///   `null` -- the time range drawn as the band a sweep leaves, with no value
///   band. Answers `{"span"}`, as it now is.
/// - `selected` -- `{"points"}`: the indices of the points the sweep covers.
///
/// An unknown verb answers `{}`.
pub fn call_json(editor: &mut PointsEditor, request: &str) -> String {
    let Ok(request) = serde_json::from_str::<Value>(request) else {
        return "{}".into();
    };
    let widget = request.get("widget").and_then(Value::as_i64).unwrap_or(0) as i32;
    match request
        .get("verb")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "window" => editor.window(widget).to_string(),
        "props" => match editor.resync_widget(i64::from(widget)).into_iter().next() {
            Some(c) => c.props.to_string(),
            None => "{}".into(),
        },
        "sync" => {
            if let Some(window) = request.get("window") {
                editor.window = window.as_i64().map(|w| w as i32);
            }
            if let Some(flat) = request.get("points").and_then(Value::as_array) {
                // The holder's curve as it stands -- a script may have changed
                // it since the last turn -- unless it is what is held already,
                // as a widget's `f32` round trip hands it back.
                let flat: Vec<f64> = flat.iter().filter_map(Value::as_f64).collect();
                let fresh = points::state(&flat);
                let mut curve = editor.held();
                if !points::same(&curve.points, &fresh) {
                    curve.points = fresh;
                }
            }
            if let Some(name) = request.get("name").and_then(Value::as_str) {
                editor.held().name = (!name.is_empty()).then(|| name.to_string());
            }
            if let Some(rate) = request.get("rate").and_then(Value::as_f64) {
                editor.rate = rate;
            }
            if let Some(title) = request.get("title").and_then(Value::as_str) {
                editor.title = title.into();
            }
            if let (Some(w), Some(h)) = (
                request.get("w").and_then(Value::as_i64),
                request.get("h").and_then(Value::as_i64),
            ) {
                editor.size = (w, h);
            }
            "{}".into()
        }
        "state" => json!({ "points": points::quads(&editor.held().points) }).to_string(),
        "span" => {
            if let Some(span) = request.get("span") {
                editor.span = span
                    .as_array()
                    .and_then(|r| Some((r.first()?.as_f64()?, r.get(1)?.as_f64()?)))
                    .filter(|(a, b)| b > a);
                editor.band = None;
            }
            json!({ "span": editor.span.map(|(a, b)| json!([a, b])) }).to_string()
        }
        "selected" => json!({ "points": editor.selected() }).to_string(),
        _ => "{}".into(),
    }
}

#[cfg(test)]
mod tests;
