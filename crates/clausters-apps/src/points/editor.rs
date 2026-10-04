//! **The points editor, one turn at a time**: what a gesture on the curve does
//! to it, the entry it leaves, and what the window is corrected with.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use clausters_core::tempoclock::samples_to_secs;
use clausters_document::history::Editable;
use clausters_document::multitrack::Automation;
use clausters_document::points::{self as vocabulary, POINTS, Points};
use clausters_document::{Applied, Opaque};
use clausters_editing::conversation::{self, Answer, Conversation, Correction};
use clausters_editing::points;

use super::{Held, Ids, Rules, Shared, props, window};
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
}

turn::turned!(Outcome);

/// **A points editor**: a shared curve, the axes its window holds, and its
/// end of the conversation.
#[derive(Clone, Debug)]
pub struct PointsEditor {
    curve: Shared,
    rate: f64,
    conversation: Conversation,
    window: Option<i32>,
    widget: Option<i32>,
    /// The shape menu in the column beside the curve.
    shape: Option<i32>,
    /// **The selected segment**, by the point that starts it: what the shape
    /// menu sets. View state, the window's.
    segment: Option<usize>,
    title: String,
    size: (i64, i64),
    held: Held,
    /// The ranges the caller declared; the curve's own parameter fills in the
    /// value range where it declared none ([`Rules::of`]).
    declared: Rules,
}

/// What a points editor is opened with, as the context's door reads it.
#[derive(Deserialize)]
#[serde(default)]
struct Opened {
    rate: f64,
    title: String,
    w: i64,
    h: i64,
    /// The value range the caller declared, both ends or neither.
    min: Option<f64>,
    max: Option<f64>,
    /// The time range the caller declared, both ends or neither.
    start: Option<f64>,
    end: Option<f64>,
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
            start: None,
            end: None,
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
            shape: None,
            segment: None,
            title: "Curve".into(),
            size: (1000, 520),
            held: Held::default(),
            declared: Rules::default(),
        }
    }

    /// The curve it edits.
    pub fn curve(&self) -> &Shared {
        &self.curve
    }

    /// **The rules the curve is edited under**, as it stands now: a target
    /// the curve was given since is read on the next turn.
    pub fn rules(&self) -> Rules {
        Rules::of(&self.held(), self.declared)
    }

    fn held(&self) -> std::sync::MutexGuard<'_, Automation> {
        // A poisoned lock is a panic elsewhere while the curve was held; the
        // data is still the curve, and refusing it here would lose it.
        self.curve.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// **The window**, with the curve and the shape menu under the ids given
    /// -- which are then the ones the editor answers for.
    pub fn window(&mut self, ids: Ids) -> Value {
        self.widget = Some(ids.curve);
        self.shape = ids.shape;
        let curve = self.held().clone();
        let rules = self.rules();
        window(&curve, &mut self.held, &rules, ids, &self.title, self.size)
    }

    /// The selected segment, while it is one: a point starts it and another
    /// ends it.
    fn segment(&self) -> Option<usize> {
        self.segment.filter(|i| i + 1 < self.held().points.len())
    }

    /// The selected segment's shape, as a menu index.
    fn shape_of(&self, segment: usize) -> Option<i64> {
        let curve = self.held();
        let point = curve.points.get(segment)?;
        Some(points::quad(point.at, point)[2] as i64)
    }

    /// Timeline samples as the curve's seconds.
    fn secs(&self, units: f64) -> f64 {
        samples_to_secs(units.max(0.0).round() as i64, self.rate)
    }

    /// What `widget` is corrected with, or nothing for one that is not the
    /// curve.
    fn resync_widget(&mut self, widget: i64) -> Vec<Correction> {
        if self.shape.map(i64::from) == Some(widget) {
            return self.menu_correction().into_iter().collect();
        }
        if self.widget.map(i64::from) != Some(widget) {
            return Vec::new();
        }
        let curve = self.held().clone();
        let rules = self.rules();
        let mut props = props(&curve, &mut self.held, &rules);
        // The selected segment, which a step that took points away may have
        // taken with it.
        self.segment = self.segment();
        props.insert(
            "segment".into(),
            json!(self.segment.map_or(-1, |i| i as i64)),
        );
        let mut out = vec![Correction {
            widget,
            props: Value::Object(props),
        }];
        out.extend(self.menu_correction());
        out
    }

    /// The shape menu, showing the selected segment's shape.
    fn menu_correction(&self) -> Option<Correction> {
        let (menu, segment) = (self.shape?, self.segment()?);
        Some(Correction {
            widget: i64::from(menu),
            props: json!({ "index": self.shape_of(segment)? }),
        })
    }

    /// **The menu chose shape `shape`** for the selected segment: an edit of
    /// the curve, recorded like a gesture's -- or a refusal, with no segment
    /// selected.
    fn set_shape(&mut self, shape: i32, out: &mut Outcome) -> (Option<String>, Vec<Correction>) {
        let Some(segment) = self.segment() else {
            return (
                Some("select a segment of the curve first".into()),
                Vec::new(),
            );
        };
        let mut flat = points::quads(&self.held().points);
        flat[segment * points::QUAD + 2] = f64::from(shape);
        self.edit(flat, "set the segment's shape".into(), out)
    }

    /// **Flat quads made the curve**, inside its rules: applied to the shared
    /// curve and recorded with the points as they were as its inverse, the
    /// points handed to the caller and the curve's widgets corrected.
    fn edit(
        &mut self,
        mut flat: Vec<f64>,
        label: String,
        out: &mut Outcome,
    ) -> (Option<String>, Vec<Correction>) {
        self.rules().keep(&mut flat);
        let payload = json!({ "intent": "setpoints", "points": points::state(&flat) });
        let recorded = {
            let mut curve = self.held();
            let backward = vocabulary::payload(&Points(curve.points.clone()).state());
            let applied = edit(&mut curve, &Opaque(payload.clone()));
            if let Some(why) = applied.reason {
                drop(curve);
                let widget = self.widget.map_or(0, i64::from);
                return (Some(why), self.resync_widget(widget));
            }
            applied.applied.then(|| {
                out.points = Some(points::quads(&curve.points));
                Record {
                    label,
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
        let widget = self.widget.map_or(0, i64::from);
        (None, self.resync_widget(widget))
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

    fn gesture(
        &mut self,
        widget: i64,
        tag: &str,
        values: &[Value],
        out: &mut Outcome,
    ) -> (Option<String>, Vec<Correction>) {
        let at = |i: usize| values.get(i).and_then(Value::as_f64).unwrap_or(0.0);
        if self.shape.map(i64::from) == Some(widget) {
            // The menu reports its index as its value, where a tag would be.
            return match tag.parse::<f64>() {
                Ok(index) => self.set_shape(index as i32, out),
                Err(_) => (None, Vec::new()),
            };
        }
        match tag {
            "segment" => {
                // A press on a segment chose it, or one on a point let it go:
                // the menu shows the shape of the one chosen.
                self.segment = usize::try_from(at(0) as i64).ok();
                (None, self.menu_correction().into_iter().collect())
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
                // **Inside the rules**, whatever the hand reported: the host
                // keeps a drag in the field it draws, and this is the rule
                // itself, for every caller that reports a curve.
                let flat: Vec<f64> = values.iter().filter_map(Value::as_f64).collect();
                let intake = points::intake(tag, &flat);
                if intake.payloads.is_empty() {
                    return (None, Vec::new());
                }
                self.edit(flat, intake.label, out)
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
        [self.widget, self.shape].contains(&i32::try_from(widget).ok())
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
/// "start", "end", "version"}` over `curve`. `min` and `max` declare the range
/// its values are kept in, over the range of the parameter it automates;
/// `start` and `end` the range its times are kept in.
pub fn new_json(curve: Shared, request: &str) -> PointsEditor {
    let opened: Opened = serde_json::from_str(request).unwrap_or_default();
    let mut editor = PointsEditor::new(curve, opened.rate, opened.version);
    editor.title = opened.title;
    editor.size = (opened.w, opened.h);
    let ordered = |a: f64, b: f64| (a.min(b), a.max(b));
    editor.declared = Rules {
        values: opened.min.zip(opened.max).map(|(a, b)| ordered(a, b)),
        time: opened.start.zip(opened.end).map(|(a, b)| ordered(a, b)),
    };
    editor
}

/// **One verb of the editor's own door**, over JSON:
///
/// - `window` -- `widget`, `shape`: the GuiDef, the curve under the one id and
///   the shape menu, in the column beside it, under the other.
/// - `props` -- `widget`: what it is corrected with (`{}` for another widget).
/// - `sync` -- `window` (the id it is open in, or `null`), `points` (the curve
///   as its holder has it now, flat quads), `name`, `target`, `rate`,
///   `title`, `w`, `h`: `{}`.
/// - `rules` -- `{"values", "time"}`: the ranges the curve is edited inside,
///   each `[low, high]` or `null`.
/// - `state` -- `{"points"}`: the curve as flat quads.
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
        "window" => editor
            .window(Ids {
                curve: widget,
                shape: request
                    .get("shape")
                    .and_then(Value::as_i64)
                    .map(|id| id as i32),
            })
            .to_string(),
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
            if let Some(target) = request.get("target") {
                editor.held().target = Opaque(target.clone());
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
        "rules" => {
            let rules = editor.rules();
            let pair = |r: Option<(f64, f64)>| r.map(|(a, b)| json!([a, b]));
            json!({ "values": pair(rules.values), "time": pair(rules.time) }).to_string()
        }
        _ => "{}".into(),
    }
}

#[cfg(test)]
mod tests;
