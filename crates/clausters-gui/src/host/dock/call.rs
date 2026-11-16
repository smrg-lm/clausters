//! **The dock's door on the wire** (`/gui_dock seq json`): its layout read
//! and put back, and every move a hand makes on it, by name -- the verbs a
//! client's handler and a menu call, as an application's are.
//!
//! The call is one JSON object naming its `verb`; the answer,
//! `/gui_dock_reply seq json`, is `{"ok": state}` -- the dock as it stands
//! after the call ([`Dock::state`]) -- or `{"error": {"type", "message"}}`,
//! the application door's shape.

use serde_json::{Value, json};

use super::{Dock, Landing, Side};
use crate::host::PointerKind;

/// One call of the dock's door.
#[derive(Clone, Debug, PartialEq)]
pub enum Call {
    /// `{"verb": "layout"}`: the dock as it stands.
    Layout,
    /// `{"verb": "restore", "layout": ...}`: a layout put in place of this
    /// one, as `layout` answered it.
    Restore(Value),
    /// `{"verb": "select", "pane": id}`: show a pane in its group.
    Select(i32),
    /// `{"verb": "join", "pane": id, "of": id, "at": index?}`: put a pane
    /// among the tabs of another's group.
    Join {
        pane: i32,
        of: i32,
        at: Option<usize>,
    },
    /// `{"verb": "split", "pane": id, "side": "left" | "right" | "top" |
    /// "bottom", "of": id}`: put a pane on a side of another's group.
    Split { pane: i32, side: Side, of: i32 },
    /// `{"verb": "pointer", "kind": "press" | "move" | "release", "x", "y"}`:
    /// a hand on the dock's surface, in its device pixels -- on its chrome
    /// or, through it, on a pane, as the platform's pointer is.
    Pointer { kind: PointerKind, x: f64, y: f64 },
    /// `{"verb": "capture", "path": path}`: the dock's picture as a PNG,
    /// answered once the file is whole.
    Capture(String),
    /// `{"verb": "undock", "pane": id}`: take a pane out into a window of its
    /// own -- an OS window natively, a canvas of its own in a page.
    Undock(i32),
    /// `{"verb": "dock", "pane": id}`: put a window taken out back in, as a
    /// column of its own.
    Dock(i32),
}

/// A request the door could not read.
fn bad(message: impl Into<String>) -> (String, String) {
    ("value".into(), message.into())
}

fn id(call: &Value, key: &str) -> Result<i32, (String, String)> {
    call.get(key)
        .and_then(Value::as_i64)
        .and_then(|v| i32::try_from(v).ok())
        .ok_or_else(|| bad(format!("`{key}` is a pane's id")))
}

impl Call {
    /// **Reads a call**; what it cannot read is an error of kind `value`.
    pub fn read(json: &str) -> Result<Self, (String, String)> {
        let call: Value = serde_json::from_str(json).map_err(|e| bad(format!("not JSON: {e}")))?;
        let verb = call
            .get("verb")
            .and_then(Value::as_str)
            .ok_or_else(|| bad("a call names its `verb`"))?;
        Ok(match verb {
            "layout" => Self::Layout,
            "restore" => Self::Restore(
                call.get("layout")
                    .cloned()
                    .ok_or_else(|| bad("`restore` takes a `layout`"))?,
            ),
            "select" => Self::Select(id(&call, "pane")?),
            "join" => Self::Join {
                pane: id(&call, "pane")?,
                of: id(&call, "of")?,
                at: call.get("at").and_then(Value::as_u64).map(|v| v as usize),
            },
            "split" => Self::Split {
                pane: id(&call, "pane")?,
                of: id(&call, "of")?,
                side: call
                    .get("side")
                    .and_then(Value::as_str)
                    .and_then(Side::named)
                    .ok_or_else(|| bad("`side` is left, right, top or bottom"))?,
            },
            "pointer" => {
                let kind = call
                    .get("kind")
                    .and_then(Value::as_str)
                    .and_then(PointerKind::named)
                    .ok_or_else(|| bad("`kind` is press, move or release"))?;
                let at = |key: &str| {
                    call.get(key)
                        .and_then(Value::as_f64)
                        .ok_or_else(|| bad(format!("`{key}` is a number")))
                };
                Self::Pointer {
                    kind,
                    x: at("x")?,
                    y: at("y")?,
                }
            }
            "undock" => Self::Undock(id(&call, "pane")?),
            "dock" => Self::Dock(id(&call, "pane")?),
            "capture" => Self::Capture(
                call.get("path")
                    .and_then(Value::as_str)
                    .ok_or_else(|| bad("`capture` takes a `path`"))?
                    .to_string(),
            ),
            other => return Err(bad(format!("the dock has no verb `{other}`"))),
        })
    }
}

impl Dock {
    /// **Carries out a call that only moves the layout** -- every one but a
    /// hand, a capture and a pane taken out or put back, which the front
    /// carries out -- and answers the
    /// dock as it stands after it. A move it cannot make is an error of kind
    /// `command`.
    pub fn apply(&mut self, call: &Call) -> Result<Value, (String, String)> {
        let refused = |e: String| ("command".to_string(), e);
        match call {
            Call::Layout => {}
            Call::Restore(layout) => self.restore(layout).map_err(bad)?,
            Call::Select(pane) => self.select(*pane).map_err(refused)?,
            Call::Join { pane, of, at } => self
                .land(*pane, Landing::Join { of: *of, at: *at })
                .map_err(refused)?,
            Call::Split { pane, side, of } => self
                .land(
                    *pane,
                    Landing::Split {
                        of: *of,
                        side: *side,
                    },
                )
                .map_err(refused)?,
            Call::Pointer { .. } | Call::Capture(_) | Call::Undock(_) | Call::Dock(_) => {}
        }
        Ok(self.state())
    }

    /// **The dock as it stands**, as every call answers it: the layout as
    /// data, the surface's size, and where everything a hand can reach is --
    /// each pane's body and tab, each divider -- in the surface's device
    /// pixels, which are a capture's.
    pub fn state(&self) -> Value {
        let rect = |r: super::Rect| json!([r.x, r.y, r.w, r.h]);
        let mut panes = Vec::new();
        for group in self.groups() {
            let tabs = self.tab_parts(group);
            for (i, &pane) in group.panes.iter().enumerate() {
                let tab = tabs
                    .iter()
                    .find(|(part, _)| *part == super::Part::Option(i))
                    .map(|(_, r)| rect(*r));
                panes.push(json!({
                    "id": pane,
                    "title": self.title(pane),
                    "shown": i == group.active,
                    "rect": rect(group.body.rect()),
                    "tab": tab,
                }));
            }
        }
        let dividers: Vec<Value> = self.dividers().iter().map(|d| rect(d.rect)).collect();
        json!({
            "layout": self.tree().to_json(),
            "size": [self.size().0, self.size().1],
            "panes": panes,
            "dividers": dividers,
            "focus": self.focus(),
        })
    }
}

/// **The state of a dock that is not open** -- a host asked for one whose
/// panes have all left it, or not arrived yet: no pane, no surface.
pub fn closed_state() -> Value {
    json!({
        "layout": super::Node::default().to_json(),
        "size": [0, 0],
        "panes": [],
        "dividers": [],
        "focus": null,
    })
}

/// The state answered, with the windows taken out of the dock that a
/// `dock` call puts back.
pub fn with_out(mut state: Value, out: &[i32]) -> Value {
    let mut out = out.to_vec();
    out.sort_unstable();
    state["out"] = json!(out);
    state
}

/// **The dock's answer**, as `/gui_dock_reply` carries it.
pub fn reply(seq: i32, answer: Result<Value, (String, String)>) -> clausters_core::osc::OscMessage {
    use clausters_core::osc::{OscMessage, OscType};
    let doc = match answer {
        Ok(value) => json!({"ok": value}),
        Err((kind, message)) => json!({"error": {"type": kind, "message": message}}),
    };
    OscMessage {
        addr: crate::host::GUI_DOCK_REPLY.into(),
        args: vec![OscType::Int(seq), OscType::String(doc.to_string())],
    }
}
