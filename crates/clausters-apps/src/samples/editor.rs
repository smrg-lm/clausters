//! **The samples editor's turns**: the window over a take, and what one message
//! from the host is, what it does, and what the host is answered with.
//!
//! A host reports a stroke or a dragged sample and waits to be told what
//! happened. The editor reads the message ([`Conversation`]), reads the gesture
//! in the `samples` vocabulary (`clausters_editing::samples::intake`) — which
//! carries the run it wrote **and** the run it replaced, so the inverse is taken
//! from that one reading and nothing is kept between two calls — and answers:
//! an acknowledgement, and the reload an overtaken gesture needs.
//!
//! # What it hands back rather than does
//!
//! The samples are in a **server buffer**, not here, so an applied gesture comes
//! back as the write to carry out ([`Outcome::edit`]) and the caller writes it.
//! The undo order is the caller's too, as it is for the multitrack editor: a turn
//! answers the entry to record ([`Record`]), and the version the host names back
//! is that history's counter, in with every turn and out moved.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use clausters_core::tempoclock::samples_to_secs;
use clausters_document::samples::SAMPLES;
use clausters_document::view::NOT_AN_EDIT;
use clausters_document::{Opaque, domain};
use clausters_editing::apply::steps_json;
use clausters_editing::conversation::{self, Answer, Conversation, Correction, Message, Turn};
use clausters_editing::samples;

use super::{MEASURES, Window, measures, props, window};
use crate::turn::{Event, Kind, Leg, Record, int, number, text};

/// **What one turn came to.**
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    /// What kind of turn the message was.
    pub turn: Kind,
    /// What to send the host, `silent` for nothing.
    pub answer: Option<Answer>,
    /// The stamp a [`Kind::Step`] is answered with.
    pub seq: i64,
    /// Whether a [`Kind::Step`] walks forward.
    pub redo: bool,
    /// The entry to record, when the gesture carried its own inverse.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record: Option<Record>,
    /// Whether the take changed.
    pub changed: bool,
    /// The version after the turn.
    pub version: i64,
    /// **The write to carry out** on the take's buffer, when the turn made one:
    /// `{"intent": "write", "channel", "start", "values"}`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edit: Option<Value>,
    /// Where the position cursor was placed, in seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locate: Option<f64>,
    /// The selection a sweep left, in seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selection: Option<Value>,
}

/// **A samples editor**: one take, the measures its picture stacks, the window
/// it is drawn in, and one view's end of the conversation with the host.
#[derive(Clone, Debug)]
pub struct SamplesEditor {
    buffer: i64,
    channels: u32,
    name: Option<String>,
    layers: Vec<String>,
    rate: f64,
    title: String,
    size: (i64, i64),
    window: Option<i32>,
    widget: Option<i32>,
    conversation: Conversation,
}

impl SamplesEditor {
    /// An editor over the take in `buffer`, measured by `layers`, whose history
    /// is at `version` — or the reason the stack is refused ([`measures`]).
    pub fn new(
        buffer: i64,
        channels: u32,
        layers: &[String],
        version: i64,
    ) -> Result<Self, String> {
        Ok(Self {
            buffer,
            channels: channels.max(1),
            name: None,
            layers: measures(layers)?,
            rate: 48_000.0,
            title: "Samples".into(),
            size: (1000, 520),
            window: None,
            widget: None,
            conversation: Conversation::new(version),
        })
    }

    /// What the picture measures, innermost last.
    pub fn layers(&self) -> &[String] {
        &self.layers
    }

    /// Measure the picture by `stack` instead, or say why not and keep the one
    /// it had.
    pub fn set_layers(&mut self, stack: &[String]) -> Result<(), String> {
        self.layers = measures(stack)?;
        Ok(())
    }

    /// **The window**, numbered with the take widget's id, which is the one a
    /// hand's gestures come back on.
    pub fn window(&mut self, widget: i32) -> Value {
        self.widget = Some(widget);
        window(&Window {
            buffer: self.buffer,
            channels: self.channels,
            name: self.name.as_deref(),
            layers: &self.layers,
            rate: self.rate,
            widget,
            title: &self.title,
            size: self.size,
        })
    }

    /// **One message from the host**, read and answered.
    pub fn event(&mut self, event: &Event, version: i64) -> Outcome {
        let args = &event.args;
        let widget = args.first().map_or(0, int);
        let message = Message {
            addr: event.addr.clone(),
            argc: args.len(),
            widget,
            seq: args.get(1).map_or(0, int),
            against: args.get(2).map_or(0, int),
            owns: self.widget.is_some_and(|w| i64::from(w) == widget),
            tag: args.get(3).map(text).unwrap_or_default(),
            version,
            is_window: self.window.is_some()
                && (args.is_empty() || i64::from(self.window.unwrap_or_default()) == widget),
        };
        let mut out = Outcome {
            version,
            ..Outcome::default()
        };
        match self.conversation.read(&message) {
            Turn::Nothing => {}
            Turn::Closed => {
                out.turn = Kind::Closed;
                self.window = None;
            }
            Turn::Step { seq, redo } => {
                out.turn = Kind::Step;
                out.seq = seq;
                out.redo = redo;
            }
            Turn::Stale {
                widget,
                seq,
                reason,
            } => {
                out.turn = Kind::Stale;
                out.answer = Some(conversation::answer(
                    seq,
                    version,
                    Some(reason),
                    resync(widget),
                ));
            }
            Turn::Route { widget, seq } => {
                out.turn = Kind::Route;
                let values = args.get(4..).unwrap_or_default();
                let (reason, corrections) = self.route(widget, &message.tag, values, &mut out);
                self.conversation.applied(out.version);
                out.answer = Some(conversation::answer(seq, out.version, reason, corrections));
            }
        }
        out
    }

    /// **What a write does to the take's buffer**, as the steps a runner
    /// carries out (`clausters_editing::samples::write_steps`) in the JSON a
    /// client walks: a turn's `edit`, or a payload of a history step.
    pub fn write(&self, payload: &Value, chunk: usize) -> Value {
        steps_json(&samples::write_steps(
            self.buffer as i32,
            self.channels,
            payload,
            chunk,
        ))
    }

    /// **Every widget of the window, corrected**, with nothing to retire — what
    /// a history step leaves behind, and what a second window over the take is
    /// told when another one wrote it.
    pub fn resync_all(&self, version: i64) -> Answer {
        let corrections = self.widget.map_or_else(Vec::new, |w| resync(i64::from(w)));
        conversation::answer(0, version, None, corrections)
    }

    /// Answers the stamp a [`Kind::Step`] carried, once the caller has walked.
    pub fn acknowledge(&self, seq: i64, version: i64, reason: Option<String>) -> Answer {
        conversation::answer(seq, version, reason, Vec::new())
    }

    /// The second a position on the axis falls on: the frame rounded the way
    /// every client rounds it. A take holds no tempo, so nothing else applies.
    fn secs_at(&self, units: f64) -> f64 {
        samples_to_secs(units.round() as i64, self.rate)
    }

    /// One gesture onto the take: screen state, or a write. Answers the reason
    /// and the corrections the acknowledgement carries.
    fn route(
        &mut self,
        widget: i64,
        tag: &str,
        values: &[Value],
        out: &mut Outcome,
    ) -> (Option<String>, Vec<Correction>) {
        if NOT_AN_EDIT.contains(&tag) {
            self.observe(tag, values, out);
            return (None, Vec::new());
        }
        let taken = samples::intake(tag, values);
        let Some(payload) = taken.payloads.first() else {
            return match taken.refusal {
                Some(why) => (Some(why), resync(widget)),
                None => (None, Vec::new()),
            };
        };
        // **The inverse is the reading's**: the run the stroke replaced arrived
        // in the same message, and a run that did not cover the write is no
        // inverse at all — an entry the pile cannot invert is better not
        // recorded than pretended.
        if let Some(backward) = taken.inverse {
            out.record = Some(Record {
                label: taken.label.clone(),
                legs: vec![Leg {
                    forward: json!({ "edit": payload }),
                    backward,
                    key: domain::coalesce_key(SAMPLES, &Opaque(payload.clone()))
                        .unwrap_or_default(),
                }],
            });
        }
        out.edit = Some(payload.clone());
        out.changed = true;
        out.version += 1;
        (None, Vec::new())
    }

    /// A tag that says what the view is looking at rather than what changed.
    fn observe(&mut self, tag: &str, values: &[Value], out: &mut Outcome) {
        match tag {
            "locate" if !values.is_empty() => {
                out.locate = Some(self.secs_at(number(&values[0])));
            }
            "selection" => {
                let at = |i: usize| values.get(i).map_or(0.0, |v| self.secs_at(number(v)));
                let mut selection = json!({ "start": at(0), "len": at(1) });
                if values.len() >= 4 {
                    // The sweep restricted the value axis too, carried as it
                    // came: no unit of this editor's applies to it.
                    selection["value"] =
                        json!({ "min": number(&values[2]), "max": number(&values[3]) });
                }
                out.selection = Some(selection);
            }
            _ => {}
        }
    }
}

/// What `widget` should be drawing, as the one correction it needs: read the
/// take again.
fn resync(widget: i64) -> Vec<Correction> {
    let Ok(id) = i32::try_from(widget) else {
        return Vec::new();
    };
    vec![Correction {
        widget,
        props: Value::Object(props(id)),
    }]
}

/// The facts a caller hands over, in [`new_json`] and in `sync`: each one
/// optional in `sync`, where an absent key leaves what the editor had.
#[derive(Deserialize, Default)]
#[serde(default)]
struct Facts {
    buffer: Option<i64>,
    channels: Option<u32>,
    /// Absent leaves the name; `null` takes it away, which a plain `Option`
    /// would read as absent.
    #[serde(deserialize_with = "present")]
    name: Option<Option<String>>,
    layers: Option<Vec<String>>,
    rate: Option<f64>,
    title: Option<String>,
    w: Option<i64>,
    h: Option<i64>,
    /// The window the editor is open in; `null` once it is not.
    #[serde(deserialize_with = "present")]
    window: Option<Option<i32>>,
    version: Option<i64>,
}

/// A key that is present, whatever its value — `null` included.
fn present<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

impl SamplesEditor {
    fn adopt(&mut self, facts: Facts) {
        if let Some(buffer) = facts.buffer {
            self.buffer = buffer;
        }
        if let Some(channels) = facts.channels {
            self.channels = channels.max(1);
        }
        if let Some(name) = facts.name {
            self.name = name;
        }
        if let Some(rate) = facts.rate.filter(|r| *r > 0.0) {
            self.rate = rate;
        }
        if let Some(title) = facts.title {
            self.title = title;
        }
        if let Some(w) = facts.w {
            self.size.0 = w;
        }
        if let Some(h) = facts.h {
            self.size.1 = h;
        }
        if let Some(window) = facts.window {
            self.window = window;
        }
    }
}

/// **An editor built from a JSON request** — `buffer`, `channels`, `name`,
/// `layers` (the measure stack, [`MEASURES`] when absent), `rate`,
/// `title`, `w`, `h` and `version` (the history's counter) — or the reason it
/// cannot be: a request that is not JSON, or a measure stack that is refused.
pub fn new_json(request: &str) -> Result<SamplesEditor, String> {
    let facts: Facts =
        serde_json::from_str(request).map_err(|e| format!("not a samples editor request: {e}"))?;
    let layers = facts
        .layers
        .clone()
        .unwrap_or_else(|| MEASURES.iter().map(|m| m.to_string()).collect());
    let mut editor = SamplesEditor::new(
        facts.buffer.unwrap_or(0),
        facts.channels.unwrap_or(1),
        &layers,
        facts.version.unwrap_or(0),
    )?;
    editor.adopt(Facts {
        layers: None,
        ..facts
    });
    Ok(editor)
}

/// **One verb of an editor, over JSON** — the door both clients bind.
///
/// `request` names the `verb` and carries its arguments:
///
/// - `sync` — any of `buffer`, `channels`, `name`, `rate`, `title`,
///   `w`, `h`, `window`: the facts a caller holds, handed over before the verbs
///   that read them. Answers `{}`.
/// - `layers` — `stack`, optional: measures the picture by it when given.
///   Answers `{"layers", "measure"}` — the stack and the prop it is drawn with —
///   or `{"error"}` naming what was refused, keeping the stack it had.
/// - `window` — `widget`: the window, as a GuiDef.
/// - `props` — `widget`: the correction.
/// - `event` — `addr`, `args` (a blob decoded to its numbers), `version`: an
///   [`Outcome`].
/// - `write` — `edit` (a `write` payload), `chunk` (the most values one
///   message carries, 8192 when absent): `{"steps"}`, what the write does to the
///   take's buffer.
/// - `acknowledge` — `seq`, `version`, `reason`: an [`Answer`].
///
/// An unknown verb answers `{}`.
pub fn call_json(editor: &mut SamplesEditor, request: &str) -> String {
    let Ok(request) = serde_json::from_str::<Value>(request) else {
        return "{}".into();
    };
    let get = |key: &str| request.get(key).cloned().unwrap_or(Value::Null);
    let widget = int(&get("widget")) as i32;
    let version = int(&get("version"));
    match request
        .get("verb")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "sync" => {
            if let Ok(facts) = serde_json::from_value::<Facts>(request.clone()) {
                editor.adopt(Facts {
                    layers: None,
                    ..facts
                });
            }
            "{}".into()
        }
        "layers" => {
            if let Some(stack) = request.get("stack") {
                let stack: Vec<String> = serde_json::from_value(stack.clone()).unwrap_or_default();
                if let Err(error) = editor.set_layers(&stack) {
                    return json!({ "error": error }).to_string();
                }
            }
            json!({ "layers": editor.layers(), "measure": editor.layers().join(" ") }).to_string()
        }
        "window" => editor.window(widget).to_string(),
        "props" => Value::Object(props(widget)).to_string(),
        "event" => {
            let event = serde_json::from_value::<Event>(request.clone()).unwrap_or_default();
            serde_json::to_string(&editor.event(&event, version)).unwrap_or_else(|_| "{}".into())
        }
        "write" => {
            let chunk = get("chunk").as_u64().map_or(8192, |c| c as usize);
            json!({ "steps": editor.write(&get("edit"), chunk) }).to_string()
        }
        "acknowledge" => serde_json::to_string(&editor.acknowledge(
            int(&get("seq")),
            version,
            get("reason").as_str().map(str::to_string),
        ))
        .unwrap_or_else(|_| "{}".into()),
        _ => "{}".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(editor: &mut SamplesEditor, request: Value) -> Value {
        serde_json::from_str(&call_json(editor, &request.to_string())).unwrap()
    }

    /// **The window is composed from what the editor was handed**, and a sync
    /// changes only what it names.
    #[test]
    fn the_window_follows_the_facts_the_caller_holds() {
        let mut editor = new_json(
            r#"{"buffer": 4, "channels": 1, "name": "saw", "rate": 44100,
                "title": "take", "w": 800, "h": 400}"#,
        )
        .unwrap();
        let def = call(&mut editor, json!({"verb": "window", "widget": 12}));
        let take = &def["children"][0];
        assert_eq!(
            (take["id"].clone(), take["buffer"].clone()),
            (json!(12), json!(4))
        );
        assert_eq!(
            take["measure"], "peak rms",
            "both measures when none are named"
        );
        assert_eq!(take["label"], "saw");
        assert_eq!(take["axes"]["x"]["sample_rate"], 44100.0);
        assert_eq!(def["title"], "take");

        call(
            &mut editor,
            json!({"verb": "sync", "channels": 2, "name": null}),
        );
        let take = &call(&mut editor, json!({"verb": "window", "widget": 12}))["children"][0];
        assert_eq!(take["channels"], 2);
        assert_eq!(take["label"], "buffer 4", "a name taken away");
        assert_eq!(
            take["axes"]["x"]["sample_rate"], 44100.0,
            "what a sync does not name stays"
        );
    }

    /// **A refused stack keeps the one the picture had**, and says why.
    #[test]
    fn a_refused_stack_keeps_the_measures_it_had() {
        let mut editor = new_json(r#"{"buffer": 1, "layers": ["peak"]}"#).unwrap();
        assert_eq!(
            call(
                &mut editor,
                json!({"verb": "layers", "stack": ["peak", "rms"]})
            ),
            json!({"layers": ["peak", "rms"], "measure": "peak rms"})
        );
        let refused = call(&mut editor, json!({"verb": "layers", "stack": ["loud"]}));
        assert!(refused["error"].as_str().unwrap().contains("'loud'"));
        assert_eq!(
            call(&mut editor, json!({"verb": "layers"}))["layers"],
            json!(["peak", "rms"])
        );
        assert!(new_json(r#"{"buffer": 1, "layers": []}"#).is_err());
    }

    /// A correction is the reload, whichever widget it names.
    #[test]
    fn a_correction_is_a_reload() {
        let mut editor = new_json("{}").unwrap();
        assert_eq!(
            call(&mut editor, json!({"verb": "props", "widget": 3})),
            json!({"reload": 1})
        );
    }

    /// An editor open in window 900 with its take drawn by widget 12.
    fn opened() -> SamplesEditor {
        let mut editor = new_json(r#"{"buffer": 7, "rate": 48000}"#).unwrap();
        call(&mut editor, json!({"verb": "window", "widget": 12}));
        call(&mut editor, json!({"verb": "sync", "window": 900}));
        editor
    }

    /// **A stroke is the write to carry out and the entry that undoes it**, the
    /// inverse taken from the same reading, and the version moves.
    #[test]
    fn a_stroke_answers_its_write_and_the_entry_that_undoes_it() {
        let mut editor = opened();
        let out = call(
            &mut editor,
            json!({"verb": "event", "addr": "/gui_event", "version": 0,
                   "args": [12, 1, 0, "draw", 0, 2, [0.5, -0.5], [0.0, 0.0]]}),
        );
        assert_eq!(out["turn"], "route");
        assert_eq!(
            out["edit"],
            json!({"intent": "write", "channel": 0, "start": 2, "values": [0.5, -0.5]})
        );
        assert_eq!(out["record"]["label"], "draw the samples");
        let leg = &out["record"]["legs"][0];
        assert_eq!(leg["backward"]["values"], json!([0.0, 0.0]));
        assert_eq!(leg["forward"]["edit"], out["edit"]);
        assert_eq!(
            (out["changed"].clone(), out["version"].clone()),
            (json!(true), json!(1))
        );
        assert_eq!(out["answer"]["seq"], 1);
        assert_eq!(out["answer"]["docVersion"], 1);
    }

    /// A run that does not cover the write is still written, and records
    /// nothing it cannot put back.
    #[test]
    fn a_ragged_inverse_writes_and_records_nothing() {
        let mut editor = opened();
        let out = call(
            &mut editor,
            json!({"verb": "event", "addr": "/gui_event", "version": 0,
                   "args": [12, 1, 0, "draw", 0, 0, [0.1, 0.2], [0.0]]}),
        );
        assert!(out["edit"].is_object());
        assert!(out.get("record").is_none());
    }

    /// **An edit made against a picture that is gone is refused**, and the
    /// picture is read again.
    #[test]
    fn an_overtaken_stroke_is_answered_with_a_reload() {
        let mut editor = opened();
        // The history moved by a route that was no event of this view.
        let out = call(
            &mut editor,
            json!({"verb": "event", "addr": "/gui_event", "version": 3,
                   "args": [12, 4, 1, "draw", 0, 0, [0.5], [0.0]]}),
        );
        assert_eq!(out["turn"], "stale");
        assert!(out.get("edit").is_none());
        assert_eq!(
            out["answer"]["corrections"],
            json!([{"widget": 12, "props": {"reload": 1}}])
        );
    }

    /// **The write a turn answered becomes steps over this editor's take**:
    /// one channel of a stereo take by its own command.
    #[test]
    fn a_write_is_steps_over_the_take() {
        let mut editor = opened();
        call(&mut editor, json!({"verb": "sync", "channels": 2}));
        let out = call(
            &mut editor,
            json!({"verb": "write", "chunk": 16,
                   "edit": {"intent": "write", "channel": 1, "start": 3, "values": [0.5]}}),
        );
        let steps = out["steps"].as_array().unwrap();
        assert_eq!(steps[0]["send"]["addr"], "/buffer_setRangeChannel");
        assert_eq!(
            steps[0]["send"]["args"].as_array().unwrap()[..3],
            [json!({"i": 7}), json!({"i": 1}), json!({"i": 3})]
        );
        assert_eq!(
            steps[1],
            json!({"await": {"command": "/buffer_setRangeChannel", "index": 7}})
        );
    }

    /// A click on the ruler is a cursor in seconds, and another view's widget
    /// is nothing of this editor's.
    #[test]
    fn a_locate_is_in_seconds_and_a_stranger_is_nothing() {
        let mut editor = opened();
        let out = call(
            &mut editor,
            json!({"verb": "event", "addr": "/gui_event", "version": 0,
                   "args": [12, 1, 0, "locate", 48000.0]}),
        );
        assert_eq!(out["locate"], 1.0, "one second");
        let out = call(
            &mut editor,
            json!({"verb": "event", "addr": "/gui_event", "version": 0,
                   "args": [55, 2, 0, "draw", 0, 0, [0.5], [0.0]]}),
        );
        assert_eq!(out["turn"], "nothing");
    }
}
