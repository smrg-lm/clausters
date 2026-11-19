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
    /// `{"verb": "pointer", "kind": "press" | "move" | "release", "x", "y",
    /// "panel": id?}`: a hand on the dock's surface, in its device pixels --
    /// on its chrome or, through it, on a pane, as the platform's pointer is.
    /// With `panel`, on the floating panel of that window taken out.
    Pointer {
        kind: PointerKind,
        x: f64,
        y: f64,
        panel: Option<i32>,
    },
    /// `{"verb": "capture", "path": path, "panel": id?}`: the dock's
    /// picture as a PNG, answered once the file is whole -- or, with
    /// `panel`, the floating panel's of that window.
    Capture { path: String, panel: Option<i32> },
    /// `{"verb": "undock", "pane": id}`: take a pane out into a window of its
    /// own -- an OS window natively, a canvas of its own in a page.
    Undock(i32),
    /// `{"verb": "dock", "pane": id}`: put a window taken out back in, as a
    /// column of its own.
    Dock(i32),
    /// `{"verb": "page", "name": name}`: show a page, made with nothing on
    /// it if there is none of that name.
    Page(String),
    /// `{"verb": "page_rename", "name": name, "to": name}`.
    PageRename { name: String, to: String },
    /// `{"verb": "page_close", "name": name}`: its arrangement goes, its
    /// panes stay.
    PageClose(String),
    /// `{"verb": "place", "pane": id}`: put a pane on the page shown.
    Place(i32),
    /// `{"verb": "leave", "pane": id}`: take a pane off the page shown; it
    /// stays a pane.
    Leave(i32),
}

/// A request the door could not read.
fn bad(message: impl Into<String>) -> (String, String) {
    ("value".into(), message.into())
}

fn name(call: &Value, key: &str) -> Result<String, (String, String)> {
    call.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| bad(format!("`{key}` is a page's name")))
}

fn id(call: &Value, key: &str) -> Result<i32, (String, String)> {
    call.get(key)
        .and_then(Value::as_i64)
        .and_then(|v| i32::try_from(v).ok())
        .ok_or_else(|| bad(format!("`{key}` is a pane's id")))
}

/// The floating panel a call names, if it names one.
fn panel(call: &Value) -> Result<Option<i32>, (String, String)> {
    match call.get("panel") {
        None | Some(Value::Null) => Ok(None),
        Some(_) => id(call, "panel").map(Some),
    }
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
                    panel: panel(&call)?,
                }
            }
            "undock" => Self::Undock(id(&call, "pane")?),
            "page" => Self::Page(name(&call, "name")?),
            "page_rename" => Self::PageRename {
                name: name(&call, "name")?,
                to: name(&call, "to")?,
            },
            "page_close" => Self::PageClose(name(&call, "name")?),
            "place" => Self::Place(id(&call, "pane")?),
            "leave" => Self::Leave(id(&call, "pane")?),
            "dock" => Self::Dock(id(&call, "pane")?),
            "capture" => Self::Capture {
                path: call
                    .get("path")
                    .and_then(Value::as_str)
                    .ok_or_else(|| bad("`capture` takes a `path`"))?
                    .to_string(),
                panel: panel(&call)?,
            },
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
            Call::Page(name) => {
                self.show_page(name);
            }
            Call::PageRename { name, to } => self.rename_page(name, to).map_err(refused)?,
            Call::PageClose(name) => self.close_page(name).map_err(refused)?,
            Call::Place(pane) => self.place(*pane).map_err(refused)?,
            Call::Leave(pane) => self.leave(*pane).map_err(refused)?,
            Call::Pointer { .. } | Call::Capture { .. } | Call::Undock(_) | Call::Dock(_) => {}
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
        let mut bars = Vec::new();
        for group in self.groups() {
            let parts = self.bar_parts(group);
            let part = |want: super::BarPart| {
                parts
                    .iter()
                    .find(|(p, _)| *p == want)
                    .map_or(Value::Null, |(_, r)| rect(*r))
            };
            for (i, &pane) in group.panes.iter().enumerate() {
                // what a hand drags the pane by: its tab, or the title of a
                // panel showing no tabs
                let tab = if self.floating || !self.shows_tabs(&group.panes) {
                    part(super::BarPart::Title)
                } else {
                    part(super::BarPart::Tab(i))
                };
                panes.push(json!({
                    "id": pane,
                    "title": self.title(pane),
                    "shown": i == group.active,
                    "rect": rect(group.body.rect()),
                    "tab": tab,
                    "tab_close": part(super::BarPart::TabClose(i)),
                }));
            }
            bars.push(json!({
                "panes": group.panes,
                "bar": rect(group.strip),
                "grip": part(super::BarPart::Grip),
                "menu": part(super::BarPart::Menu),
                "close": part(super::BarPart::Close),
            }));
        }
        // the open menu's rows, list by list, where a hand picks them
        let menu: Vec<Value> = match self.menu() {
            Some(open) => open
                .stack
                .levels
                .iter()
                .zip(self.menu_placed())
                .map(|(level, placed)| {
                    let rows: Vec<Value> = level
                        .entries
                        .iter()
                        .zip(&placed.rows)
                        .filter(|(e, _)| !e.is_separator())
                        .map(|(e, r)| json!({"label": e.label, "enabled": e.enabled, "rect": rect(*r)}))
                        .collect();
                    json!(rows)
                })
                .collect(),
            None => Vec::new(),
        };
        let dividers: Vec<Value> = self.dividers().iter().map(|d| rect(d.rect)).collect();
        let edges: Vec<Value> = self
            .edge_placed()
            .map(|(id, edge, r)| json!({"id": id, "edge": edge.name(), "rect": rect(r.rect())}))
            .collect();
        // the page strip's tabs, one per page
        let page_tabs: Vec<Value> = (0..self.page_names().len())
            .map(|i| self.page_tab(i).map_or(Value::Null, rect))
            .collect();
        json!({
            "layout": self.tree().to_json(),
            "pages": self.pages_json(),
            "page": self.page_name(),
            "page_tabs": page_tabs,
            "size": [self.size().0, self.size().1],
            "scale": self.metrics().ui_scale,
            "panes": panes,
            "bars": bars,
            "menu": menu,
            "dividers": dividers,
            "edges": edges,
            "focus": self.focus(),
        })
    }
}

/// **The state of a dock that is not open** -- a host asked for one whose
/// panes have all left it, or not arrived yet: no pane, no surface.
pub fn closed_state() -> Value {
    json!({
        "layout": super::Node::default().to_json(),
        "pages": [],
        "page": null,
        "page_tabs": [],
        "size": [0, 0],
        "scale": 1.0,
        "panes": [],
        "bars": [],
        "menu": [],
        "dividers": [],
        "edges": [],
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
