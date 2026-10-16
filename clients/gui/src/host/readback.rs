//! **A binding read back**: a widget bound to a node's control moves when the
//! control changes from anywhere else.
//!
//! A `/gui_bind` to the server runs one way -- the widget writes, the script
//! is not in the loop. What comes back runs the same way without one: the host
//! asks the server for what each binding writes, with the binding's **read
//! form**, and the server answers with the very message the binding would
//! send. `/synth_get node control` answers `/node_set node control value`;
//! `/synth_getText node control` answers `/node_setText node control text`. So
//! a reply is matched to its widgets by the one rule a binding already is:
//! the address and the prefix, and the value after them.
//!
//! The host asks; it is not told. The server's answer is its mirror of the
//! node, which a `/node_set` from any client, an edit in a plugin's own window
//! and a loaded state all update -- so one question covers every way a value
//! changes. A control mapped to a bus reads what was last set on it rather
//! than what the bus carries, which is what the mirror knows.
//!
//! A node is read only once the server has said it is there -- a
//! `/node_query`, which answers a node that is not with a state rather than a
//! failure -- and no longer after its `/node_end`, so a window bound to a node
//! not made yet, or freed, asks nothing that fails.
//!
//! Two widgets are left alone: the one a hand is holding, so a drag is never
//! pulled back under it by an answer to a question asked before it moved, and
//! the one the keyboard is in, so text being typed is not replaced. A value
//! that is already what the widget shows changes nothing and draws nothing.
//!
//! Both fronts ask at the same period ([`READ_BACK_MS`]) and hand the replies
//! here; the rule is written once.

use super::*;
use std::collections::BTreeMap;

/// How often the bound widgets of the open windows are read back, in
/// milliseconds: often enough that a value moved elsewhere follows within a
/// glance, seldom enough that a window of a hundred controls is a few small
/// messages a second.
pub const READ_BACK_MS: u32 = 200;

/// The most controls one read asks for, so a page of a large plugin is a
/// few datagrams and never one that does not fit.
const READ_BATCH: usize = 32;

/// **The read form of a binding**: the address that asks the server for
/// what a binding to `addr` writes, answered with a message to `addr` itself.
/// `None` for an address that has none -- a binding to a bus, a range, a
/// command -- which is not read back.
pub fn read_form(addr: &str) -> Option<&'static str> {
    match addr {
        "/node_set" => Some("/synth_get"),
        "/node_setText" => Some("/synth_getText"),
        _ => None,
    }
}

impl Host {
    /// Whether some open widget has a binding that is read back -- what makes
    /// a front keep asking.
    pub fn reads_back(&self) -> bool {
        self.read_backs().next().is_some()
    }

    /// The bindings that are read back, as `(widget, read form, node,
    /// control)`, for the widgets that exist.
    fn read_backs(&self) -> impl Iterator<Item = (i32, &'static str, i32, &OscType)> {
        self.bindings.iter().filter_map(|(id, binding)| {
            let Binding::Server { addr, prefix } = binding else {
                return None;
            };
            let [OscType::Int(node), control] = prefix.as_slice() else {
                return None;
            };
            let read = read_form(addr)?;
            self.registry
                .contains(*id)
                .then_some((*id, read, *node, control))
        })
    }

    /// **The questions one read-back pass asks**: per read form and node, the
    /// controls its bindings write, each once, in batches.
    pub fn read_back_asks(&self) -> Vec<OscMessage> {
        let mut asked: BTreeMap<(&'static str, i32), Vec<OscType>> = BTreeMap::new();
        // In widget order, so a pass asks the same questions in the same
        // order whatever order the bindings were made in.
        let mut bound: Vec<_> = self.read_backs().collect();
        bound.sort_by_key(|(id, ..)| *id);
        for (_, read, node, control) in bound {
            let controls = asked.entry((read, node)).or_default();
            if !controls.contains(control) {
                controls.push(control.clone());
            }
        }
        let mut out = Vec::new();
        let mut confirming = Vec::new();
        for ((read, node), controls) in asked {
            // A node not yet known to be there is asked about first, with the
            // query that answers its absence as a state: a binding made
            // before its node (a bundle's window, defined before its nodes
            // are sent) is not read into a failure.
            if !self.read_live.contains(&node) {
                if !confirming.contains(&node) {
                    confirming.push(node);
                    out.push(OscMessage {
                        addr: "/node_query".into(),
                        args: vec![OscType::Int(node)],
                    });
                }
                continue;
            }
            for batch in controls.chunks(READ_BATCH) {
                let mut args = vec![OscType::Int(node)];
                args.extend_from_slice(batch);
                out.push(OscMessage {
                    addr: read.into(),
                    args,
                });
            }
        }
        out
    }

    /// **Applies one reply of the server to the widgets bound to what it
    /// names**, but for the widgets in `held` (a hand is on them) and the one
    /// the keyboard is in. Answers the windows to repaint, or `None` when the
    /// message is not the answer to a read-back.
    pub fn read_back(&mut self, msg: &OscMessage, held: &[i32]) -> Option<Vec<i32>> {
        self.read_presence(msg);
        read_form(&msg.addr)?;
        let [OscType::Int(node), pairs @ ..] = msg.args.as_slice() else {
            return Some(Vec::new());
        };
        let mut effects = Vec::new();
        for [control, value] in pairs.as_chunks::<2>().0 {
            let bound: Vec<i32> = self
                .bindings
                .iter()
                .filter(|(_, b)| {
                    matches!(b, Binding::Server { addr, prefix }
                        if *addr == msg.addr
                            && prefix.len() == 2
                            && prefix[0] == OscType::Int(*node)
                            && prefix[1] == *control)
                })
                .map(|(id, _)| *id)
                .collect();
            for id in bound {
                let typing = self.focused.is_some_and(|(_, focused)| focused == id);
                if held.contains(&id) || typing || self.shows(id, value) {
                    continue;
                }
                if let Some(prop) = self.read_back_prop(id, value) {
                    self.set_props(id, vec![prop], &mut effects);
                }
            }
        }
        Some(
            effects
                .into_iter()
                .filter_map(|e| match e {
                    HostEffect::Redraw(root) | HostEffect::RedrawLive(root) => Some(root),
                    _ => None,
                })
                .collect(),
        )
    }

    /// **Whether a node is there to be read**, from what the server says of
    /// it: a `/node_query.reply` (absent is `isGroup` -1) or its `/node_end`.
    /// Only nodes some binding reads are kept, and the message goes on to
    /// whoever else reads it.
    fn read_presence(&mut self, msg: &OscMessage) {
        let node = match msg.args.first() {
            Some(OscType::Int(node)) => *node,
            _ => return,
        };
        match msg.addr.as_str() {
            "/node_query.reply" => {
                let there = !matches!(msg.args.get(4), Some(OscType::Int(-1)));
                let read = self.read_backs().any(|(_, _, bound, _)| bound == node);
                if there && read {
                    self.read_live.insert(node);
                } else {
                    self.read_live.remove(&node);
                }
            }
            "/node_end" => {
                self.read_live.remove(&node);
            }
            _ => {}
        }
    }

    /// Whether widget `id` already shows `value`, so reading it back would
    /// change nothing.
    fn shows(&self, id: i32, value: &OscType) -> bool {
        let shown = self
            .registry
            .root_of(id)
            .and_then(|root| self.window_def(root))
            .and_then(|tree| tree.find(id))
            .and_then(|w| w.kind.event_value());
        match (shown, value) {
            (Some(OscType::String(a)), OscType::String(b)) => a == *b,
            (Some(shown), value) => match (number(&shown), number(value)) {
                (Some(a), Some(b)) => (a - b).abs() <= 1e-6 * a.abs().max(b.abs()).max(1.0),
                _ => false,
            },
            // A widget that reports no value (a label) shows what was last set
            // on it.
            (None, value) => self.read_back_prop(id, value).is_some_and(|(key, v)| {
                self.registry.get(id).and_then(|r| r.props.get(&key)) == Some(&v)
            }),
        }
    }

    /// The prop a read-back value sets on widget `id`: its `value`, which a
    /// control, a toggle (by its `on` and `off`), a text field and a choice
    /// over numbers all read -- or the `index` of a choice over its rows, or
    /// the `text` of a label, which reports nothing and so is bound only to be
    /// read back: a value shown as text.
    fn read_back_prop(&self, id: i32, value: &OscType) -> Option<(String, Value)> {
        let record = self.registry.get(id)?;
        let value = match value {
            OscType::String(text) => Value::from(text.clone()),
            other => Value::from(number(other)?),
        };
        let by_row = record.kind == "choice" && !record.props.contains_key("values");
        Some(if by_row {
            (
                "index".into(),
                Value::from(value.as_f64()?.round().max(0.0) as u64),
            )
        } else if record.kind == "label" {
            ("text".into(), value)
        } else {
            ("value".into(), value)
        })
    }
}

/// An OSC number as `f64`.
fn number(v: &OscType) -> Option<f64> {
    match v {
        OscType::Int(n) => Some(f64::from(*n)),
        OscType::Long(n) => Some(*n as f64),
        OscType::Float(x) => Some(f64::from(*x)),
        OscType::Double(x) => Some(*x),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A window of a slider bound to a node's `freq`, a text field and a
    /// label bound to its text, and a choice of rows bound to its `mode`.
    fn host() -> Host {
        let mut host = Host::new();
        let tree = json!({"type": "window", "children": [
            {"id": 11, "type": "slider", "min": 20.0, "max": 2000.0, "value": 440.0,
             "bind": ["/node_set", 1000, "freq"]},
            {"id": 12, "type": "text", "value": "440 Hz",
             "bind": ["/node_setText", 1000, "freq"]},
            {"id": 13, "type": "choice", "options": ["a", "b", "c"],
             "bind": ["/node_set", 1000, "mode"]},
            {"id": 14, "type": "slider", "bind": ["/bus_set", 3]},
            {"id": 15, "type": "label", "text": "440 Hz",
             "bind": ["/node_setText", 1000, "freq"]},
        ]});
        host.define(1, serde_json::from_value::<GuiNode>(tree).unwrap());
        host
    }

    fn msg(addr: &str, args: Vec<OscType>) -> OscMessage {
        OscMessage {
            addr: addr.into(),
            args,
        }
    }

    fn prop(host: &Host, id: i32, key: &str) -> Value {
        host.registry.get(id).unwrap().props[key].clone()
    }

    /// **One pass asks each node once per read form**, for every control
    /// bound to it, and nothing for a binding with no read form.
    #[test]
    fn a_pass_asks_for_what_the_bindings_write() {
        let mut host = host();
        let s = |t: &str| OscType::String(t.into());
        // Not known to be there: the node is asked about, not read.
        let query = msg("/node_query", vec![OscType::Int(1000)]);
        assert_eq!(host.read_back_asks(), std::slice::from_ref(&query));
        let absent = (0..5)
            .map(|i| OscType::Int(if i == 0 { 1000 } else { -1 }))
            .collect();
        assert_eq!(host.read_back(&msg("/node_query.reply", absent), &[]), None);
        assert_eq!(host.read_back_asks(), [query], "absent: asked about again");
        let there = vec![
            OscType::Int(1000),
            OscType::Int(0),
            OscType::Int(-1),
            OscType::Int(-1),
            OscType::Int(0),
        ];
        host.read_back(&msg("/node_query.reply", there), &[]);
        let asks = host.read_back_asks();
        assert_eq!(
            asks,
            [
                msg("/synth_get", vec![OscType::Int(1000), s("freq"), s("mode")]),
                msg("/synth_getText", vec![OscType::Int(1000), s("freq")]),
            ]
        );
        assert!(host.reads_back());
        host.read_back(&msg("/node_end", vec![OscType::Int(1000)]), &[]);
        assert_eq!(
            host.read_back_asks().len(),
            1,
            "ended: asked about, not read"
        );
    }

    /// **The answer is the binding's own message**, and it moves the widgets
    /// bound to it -- but not the one a hand holds, nor the one being typed in.
    #[test]
    fn an_answer_moves_its_widgets_but_not_a_held_one() {
        let mut host = host();
        let s = |t: &str| OscType::String(t.into());
        let set = msg(
            "/node_set",
            vec![
                OscType::Int(1000),
                s("freq"),
                OscType::Float(880.0),
                s("mode"),
                OscType::Float(2.0),
            ],
        );
        assert_eq!(host.read_back(&set, &[11]), Some(vec![1]));
        assert_eq!(prop(&host, 11, "value"), json!(440.0), "held: untouched");
        assert_eq!(
            prop(&host, 13, "index"),
            json!(2),
            "a choice of rows: its index"
        );
        assert_eq!(host.read_back(&set, &[]), Some(vec![1]));
        assert_eq!(prop(&host, 11, "value"), json!(880.0));
        assert_eq!(
            host.read_back(&set, &[]),
            Some(vec![]),
            "nothing changed, nothing drawn"
        );

        let text = msg(
            "/node_setText",
            vec![OscType::Int(1000), s("freq"), s("880 Hz")],
        );
        host.focused = Some((1, 12));
        host.read_back(&text, &[]);
        assert_eq!(prop(&host, 12, "value"), json!("440 Hz"), "being typed in");
        assert_eq!(
            prop(&host, 15, "text"),
            json!("880 Hz"),
            "a label: its text"
        );
        assert!(host.shows(15, &s("880 Hz")), "and it is not set again");
        host.focused = None;
        host.read_back(&text, &[]);
        assert_eq!(prop(&host, 12, "value"), json!("880 Hz"));

        assert_eq!(host.read_back(&msg("/bus_set", vec![]), &[]), None);
    }
}
