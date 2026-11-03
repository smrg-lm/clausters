//! **A window's own undo order**: what the `history` prop gives a GuiDef
//! instance that is not an application -- a panel of a synth's controls.
//!
//! A control bound to the server (`/gui_bind`) sends its value with the
//! script out of the loop, so the script cannot keep what the hand did; the
//! host does, here, in the document crate's [`History`] -- one per window,
//! which is what keeps two instances independent. The rules are
//! `crates/clausters-apps/PLAN.md`'s `X26` and `docs/decisions.md`, "A live
//! parameter's undo writes the value it had":
//!
//! - **The target is the parameter, never the widget.** A server binding's
//!   address and prefix name it (`/node_set 1001 freq`), so two widgets bound
//!   to one control are one target, and a widget and its server control are
//!   never two. Any other control is its own target.
//! - **An entry holds the value the target showed when the gesture began and
//!   the one it left.** [`Panel::shown`] is what each control of the window
//!   shows as of the last settled change -- the def, a set, a read-back, an
//!   entry -- since by the time a value is delivered the element has already
//!   moved.
//! - **A gesture is one entry.** The run of values a hold, or a run of wheel
//!   or key steps on one control, delivers merges into one entry until the
//!   run closes: at a release, at the next press, at a walk and as the focus
//!   leaves a widget. A field's typing is one run from focus to blur, a press
//!   that moves its caret included. A command (a button) is never an entry.
//! - **A walk writes the stored value, absolute**, through the widget's
//!   ordinary delivery, whatever the parameter holds by then.
//! - **A lost target leaves the order**: a node's `/node_end`, or no widget of
//!   the window writing the target any more. Its entries are forgotten and
//!   walked past ([`History::forget`]).
//!
//! What another writer does -- a read-back, a script's `/gui_set` -- is no
//! entry: it only moves what [`Panel::shown`] says.

use std::collections::HashMap;

use clausters_core::osc::OscType;
use clausters_document::Opaque;
use clausters_document::history::{Entry, History, Step, StructureId};
use serde_json::{Value, json};

use super::bind::Binding;
use super::widget::WidgetKind;
use super::{Host, status};

/// The undo order of one window that asked for one.
#[derive(Debug, Default)]
pub(crate) struct Panel {
    history: History,
    /// The targets the order has entries for, by their key.
    targets: HashMap<String, Target>,
    /// What each control of the window shows, as of the last settled change.
    shown: HashMap<i32, OscType>,
    /// The key of the target whose run is still open, merging what it
    /// delivers into its entry.
    open: Option<String>,
}

#[derive(Debug)]
struct Target {
    structure: StructureId,
    /// The node a server binding writes, whose `/node_end` loses the target.
    node: Option<i32>,
}

/// What a walk writes: the value, and the widgets that write its target --
/// the first one delivers it, every one shows it.
pub(crate) struct Write {
    pub writers: Vec<i32>,
    pub value: OscType,
}

impl Panel {
    /// The structure that stands for target `key` in the order, minted the
    /// first time the target is edited.
    fn structure(&mut self, key: &str, node: Option<i32>) -> StructureId {
        if let Some(target) = self.targets.get(key) {
            return target.structure;
        }
        let structure = self.history.register("panel");
        self.targets
            .insert(key.to_string(), Target { structure, node });
        structure
    }

    /// Forgets target `key`: its entries are walked past from now on, and a
    /// later edit of the same key is a new target.
    fn lose(&mut self, key: &str) {
        if let Some(target) = self.targets.remove(key) {
            self.history.forget(target.structure);
        }
        if self.open.as_deref() == Some(key) {
            self.open = None;
        }
    }
}

/// A value as the order stores it, keeping its OSC type: a binding to
/// `/node_run` takes an int and refuses a float.
fn encode(value: &OscType) -> Opaque {
    Opaque(match value {
        OscType::Int(n) => json!({ "i": n }),
        OscType::Long(n) => json!({ "i": n }),
        OscType::Float(x) => json!({ "f": x }),
        OscType::Double(x) => json!({ "d": x }),
        OscType::String(s) => json!({ "s": s }),
        OscType::Bool(b) => json!({ "i": i32::from(*b) }),
        _ => Value::Null,
    })
}

fn decode(stored: &Opaque) -> Option<OscType> {
    let object = stored.0.as_object()?;
    let (tag, value) = object.iter().next()?;
    Some(match tag.as_str() {
        "i" => OscType::Int(value.as_i64()? as i32),
        "f" => OscType::Float(value.as_f64()? as f32),
        "d" => OscType::Double(value.as_f64()?),
        "s" => OscType::String(value.as_str()?.to_string()),
        _ => return None,
    })
}

fn same(a: &OscType, b: &OscType) -> bool {
    encode(a) == encode(b)
}

impl Host {
    /// Whether window `id` asked for an undo order of its own (its `history`
    /// prop).
    pub fn window_history(&self, id: i32) -> bool {
        matches!(
            self.window_def(id).map(|w| &w.kind),
            Some(WidgetKind::Window { history: true, .. })
        )
    }

    /// **Window `root` was defined, or a subtree of it**: an order is opened
    /// for it when it asks for one and kept across a redefinition, and what
    /// its controls show is read again; a window that does not ask has none.
    pub(super) fn panel_defined(&mut self, root: i32) {
        if !self.window_history(root) {
            self.panels.remove(&root);
            return;
        }
        let shown: HashMap<i32, OscType> = self
            .window_def(root)
            .map(|tree| {
                tree.descendants()
                    .filter_map(|w| Some((w.id?, w.kind.event_value()?)))
                    .collect()
            })
            .unwrap_or_default();
        let panel = self.panels.entry(root).or_default();
        panel.shown = shown;
        panel.open = None;
    }

    /// **Widget `id` was set from outside** (a `/gui_set`, a read-back): what
    /// it shows is the baseline its next gesture starts from, and nothing is
    /// recorded.
    pub(super) fn panel_saw(&mut self, id: i32) {
        let Some(root) = self.registry.root_of(id) else {
            return;
        };
        if !self.panels.contains_key(&root) {
            return;
        }
        let value = self
            .window_def(root)
            .and_then(|tree| tree.find(id))
            .and_then(|w| w.kind.event_value());
        if let (Some(panel), Some(value)) = (self.panels.get_mut(&root), value) {
            panel.shown.insert(id, value);
        }
    }

    /// Windows that are gone take their orders with them.
    pub(super) fn panels_forget_gone(&mut self) {
        let windows = &self.window_defs;
        self.panels.retain(|id, _| windows.contains_key(id));
    }

    /// **Node `node` ended**: every entry that writes one of its controls is
    /// walked past from now on, in every window.
    pub(super) fn panels_node_ended(&mut self, node: i32) {
        for panel in self.panels.values_mut() {
            let lost: Vec<String> = panel
                .targets
                .iter()
                .filter(|(_, t)| t.node == Some(node))
                .map(|(key, _)| key.clone())
                .collect();
            for key in lost {
                panel.lose(&key);
            }
        }
    }

    /// The target widget `id` writes: its key, and the node it names.
    fn panel_target(&self, id: i32) -> (String, Option<i32>) {
        match self.bindings.get(&id) {
            Some(Binding::Server { addr, prefix }) => {
                let node = match prefix.first() {
                    Some(OscType::Int(node)) if addr.starts_with("/node_") => Some(*node),
                    _ => None,
                };
                (format!("server {addr} {prefix:?}"), node)
            }
            _ => (format!("widget {id}"), None),
        }
    }

    /// The widgets of window `root` that write target `key`, in widget order.
    fn panel_writers(&self, root: i32, key: &str) -> Vec<i32> {
        let Some(tree) = self.window_def(root) else {
            return Vec::new();
        };
        let mut ids: Vec<i32> = tree
            .descendants()
            .filter_map(|w| w.id)
            .filter(|id| self.panel_target(*id).0 == key)
            .collect();
        ids.sort_unstable();
        ids
    }

    /// **A hand's change of widget `id` in window `def_id`**: recorded in the
    /// window's order when it has one, merged into the open run when it is
    /// the same target's.
    pub(crate) fn panel_record(&mut self, def_id: i32, id: i32, value: &OscType) {
        if !self.panels.contains_key(&def_id) {
            return;
        }
        let Some(record) = self.registry.get(id) else {
            return;
        };
        // A command is never an entry: a button's gate opens and closes
        // within its gesture.
        if record.kind == "button" {
            return;
        }
        let label = ["label", "title", "name"]
            .iter()
            .find_map(|key| record.props.get(*key).and_then(Value::as_str))
            .unwrap_or(record.kind.as_str())
            .to_string();
        let (key, node) = self.panel_target(id);
        let Some(panel) = self.panels.get_mut(&def_id) else {
            return;
        };
        let Some(before) = panel.shown.insert(id, value.clone()) else {
            // Nothing known to go back to: what it shows now is the baseline.
            return;
        };
        let continuing = panel.open.as_deref() == Some(key.as_str());
        if !continuing && same(&before, value) {
            return;
        }
        let structure = panel.structure(&key, node);
        let mut entry = Entry::new(
            format!("set {label}"),
            structure,
            Step::Edit(encode(value)),
            encode(&before),
        )
        .keyed(key.clone());
        if continuing {
            entry = entry.continuing();
        }
        panel.history.record(entry);
        panel.open = Some(key);
    }

    /// **The run in window `def_id` closes**: what the next change delivers
    /// is an entry of its own. At a walk and as the focus leaves a widget;
    /// a press and a release close it through [`Host::panel_gesture_over`].
    pub(crate) fn panel_close(&mut self, def_id: i32) {
        if let Some(panel) = self.panels.get_mut(&def_id) {
            panel.open = None;
        }
    }

    /// **A press on widget `id`, or its release**: a gesture of its own, so
    /// the open run closes -- but for the field the keys are typing in,
    /// where a press moves the caret and its run lasts from focus to blur.
    pub(crate) fn panel_gesture_over(&mut self, def_id: i32, id: i32) {
        let typing_here = self.focused() == Some((def_id, id))
            && self
                .widget_kind(def_id, id)
                .is_some_and(|kind| kind.takes_text());
        if !typing_here {
            self.panel_close(def_id);
        }
    }

    /// **Walks window `def_id`'s order one step**, back for an undo: what to
    /// write, or `None` when the window has no order of its own (the verb is
    /// then its owner's, as it always was). An order with nothing to walk
    /// answers no writes, and says so on the status line.
    pub(crate) fn panel_walk(&mut self, def_id: i32, undo: bool) -> Option<Vec<Write>> {
        let keys: Vec<String> = self.panels.get(&def_id)?.targets.keys().cloned().collect();
        // A target nothing in the window writes any more is lost first, so the
        // walk passes over it rather than stopping on a value it cannot write.
        let writers: HashMap<String, Vec<i32>> = keys
            .into_iter()
            .map(|key| {
                let ids = self.panel_writers(def_id, &key);
                (key, ids)
            })
            .collect();
        let panel = self.panels.get_mut(&def_id)?;
        for (key, ids) in &writers {
            if ids.is_empty() {
                panel.lose(key);
            }
        }
        panel.open = None;
        let (label, legs) = if undo {
            match panel.history.undo() {
                Some(undone) => (undone.label, undone.legs),
                None => (String::new(), Vec::new()),
            }
        } else {
            match panel.history.redo() {
                Some(redone) => (redone.label, redone.edits),
                None => (String::new(), Vec::new()),
            }
        };
        let verb = if undo { "undo" } else { "redo" };
        let mut writes = Vec::new();
        for (structure, stored) in legs {
            let Some(key) = panel
                .targets
                .iter()
                .find(|(_, t)| t.structure == structure)
                .map(|(key, _)| key.clone())
            else {
                continue;
            };
            let (Some(ids), Some(value)) = (writers.get(&key), decode(&stored)) else {
                continue;
            };
            for id in ids {
                panel.shown.insert(*id, value.clone());
            }
            writes.push(Write {
                writers: ids.clone(),
                value,
            });
        }
        let line = if writes.is_empty() {
            status::Line::of_reason(None, &format!("{verb}: nothing to {verb}"))
        } else {
            status::Line {
                kind: status::Kind::Did,
                widget: None,
                verb: verb.into(),
                text: format!("{verb} {label}"),
            }
        };
        self.say(def_id, line);
        Some(writes)
    }
}
