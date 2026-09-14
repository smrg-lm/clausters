//! **The samples editor's state and its door**: what a window over a take is
//! composed from, held between messages, and the verbs a client reaches it by.
//!
//! What it holds today is the window's facts — the take, the measures, the axis
//! and the chrome — so a client composes and corrects the window through it
//! rather than for itself. The conversation joins it later, as the multitrack
//! editor's did.

use serde::Deserialize;
use serde_json::{Value, json};

use super::{MEASURES, Window, measures, props, window};

/// **A samples editor**: one take, the measures its picture stacks, and the
/// window it is drawn in.
#[derive(Clone, Debug, PartialEq)]
pub struct SamplesEditor {
    buffer: i64,
    channels: u32,
    name: Option<String>,
    layers: Vec<String>,
    rate: f64,
    tempo: f64,
    title: String,
    size: (i64, i64),
}

impl SamplesEditor {
    /// An editor over the take in `buffer`, measured by `layers`, or the reason
    /// the stack is refused ([`measures`]).
    pub fn new(buffer: i64, channels: u32, layers: &[String]) -> Result<Self, String> {
        Ok(Self {
            buffer,
            channels: channels.max(1),
            name: None,
            layers: measures(layers)?,
            rate: 48_000.0,
            tempo: 1.0,
            title: "Samples".into(),
            size: (1000, 520),
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

    /// The window, numbered with the take widget's id.
    pub fn window(&self, widget: i32) -> Value {
        window(&Window {
            buffer: self.buffer,
            channels: self.channels,
            name: self.name.as_deref(),
            layers: &self.layers,
            rate: self.rate,
            tempo: self.tempo,
            widget,
            title: &self.title,
            size: self.size,
        })
    }
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
    tempo: Option<f64>,
    title: Option<String>,
    w: Option<i64>,
    h: Option<i64>,
}

/// A key that is present, whatever its value — `null` included.
fn present<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(d).map(Some)
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
        if let Some(tempo) = facts.tempo {
            self.tempo = tempo;
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
    }
}

/// **An editor built from a JSON request** — `buffer`, `channels`, `name`,
/// `layers` (the measure stack, [`MEASURES`] when absent), `rate`, `tempo`,
/// `title`, `w` and `h` — or the reason it cannot be: a request that is not
/// JSON, or a measure stack that is refused.
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
/// - `sync` — any of `buffer`, `channels`, `name`, `rate`, `tempo`, `title`,
///   `w`, `h`: the facts a caller holds, handed over before the verbs that read
///   them. Answers `{}`.
/// - `layers` — `stack`, optional: measures the picture by it when given.
///   Answers `{"layers", "measure"}` — the stack and the prop it is drawn with —
///   or `{"error"}` naming what was refused, keeping the stack it had.
/// - `window` — `widget`: the window, as a GuiDef.
/// - `props` — `widget`: the correction.
///
/// An unknown verb answers `{}`.
pub fn call_json(editor: &mut SamplesEditor, request: &str) -> String {
    let Ok(request) = serde_json::from_str::<Value>(request) else {
        return "{}".into();
    };
    let widget = request
        .get("widget")
        .and_then(|w| w.as_i64().or_else(|| w.as_f64().map(|f| f as i64)))
        .unwrap_or(0) as i32;
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
            r#"{"buffer": 4, "channels": 1, "name": "saw", "rate": 44100, "tempo": 2,
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
}
