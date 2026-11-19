//! **What the application says to the host, and reads back**: the protocol a
//! script speaks (`docs/gui-protocol.md`), over the link the host gives an
//! application in its process. Nothing here is the editor's: a def, a set, a
//! free, a call to the dock, and the events and replies taken apart.

use std::cell::Cell;

use clausters_core::osc::{OscMessage, OscPacket, OscType};
use clausters_gui::host::gui::LocalLink;
use serde_json::Value;

/// The application's end of the link, with the stamps of its dock calls.
pub struct Link {
    local: LocalLink,
    seq: Cell<i32>,
}

impl Link {
    pub fn new(local: LocalLink) -> Self {
        Self {
            local,
            seq: Cell::new(0),
        }
    }

    /// One message to the host; `false` once the host has gone.
    pub fn send(&self, addr: &str, args: Vec<OscType>) -> bool {
        self.local.send(OscMessage {
            addr: addr.into(),
            args,
        })
    }

    /// `/gui_def id json`: a whole tree, or a subtree redefined in place.
    pub fn def(&self, id: i32, tree: &Value) {
        self.send(
            "/gui_def",
            vec![OscType::Int(id), OscType::String(tree.to_string())],
        );
    }

    /// `/gui_set id key value ...`: a value that is a list or an object rides
    /// as its JSON, as every client sends it.
    pub fn set(&self, id: i32, props: &[(&str, Value)]) {
        let mut args = vec![OscType::Int(id)];
        for (key, value) in props {
            args.push(OscType::String((*key).into()));
            args.push(scalar(value));
        }
        self.send("/gui_set", args);
    }

    /// Ends the host's loop: the program ends once the windows are down.
    pub fn quit(&self) {
        self.local.quit();
    }

    /// `/gui_free id`: a window freed is closed.
    pub fn free(&self, id: i32) {
        self.send("/gui_free", vec![OscType::Int(id)]);
    }

    /// `/gui_query id`: what a widget is now, answered by `/gui_info`.
    pub fn query(&self, id: i32) {
        self.send("/gui_query", vec![OscType::Int(id)]);
    }

    /// `/gui_verb window verb`: a verb performed on a window, as its key would.
    pub fn verb(&self, window: i32, verb: &str) {
        self.send(
            "/gui_verb",
            vec![OscType::Int(window), OscType::String(verb.into())],
        );
    }

    /// `/gui_theme json`: the colors the host draws from.
    pub fn theme(&self, table: &Value) {
        self.send("/gui_theme", vec![OscType::String(table.to_string())]);
    }

    /// `/gui_dock seq json`: one call to the dock, answered by
    /// `/gui_dock_reply seq`. Returns the stamp.
    pub fn dock(&self, call: &Value) -> i32 {
        let seq = self.seq.get() + 1;
        self.seq.set(seq);
        self.send(
            "/gui_dock",
            vec![OscType::Int(seq), OscType::String(call.to_string())],
        );
        seq
    }
}

/// A JSON value as the one OSC argument a set carries.
fn scalar(value: &Value) -> OscType {
    match value {
        Value::Bool(b) => OscType::Int(i32::from(*b)),
        Value::Number(n) => match n.as_i64() {
            Some(i) => match i32::try_from(i) {
                Ok(i) => OscType::Int(i),
                Err(_) => OscType::Long(i),
            },
            None => OscType::Double(n.as_f64().unwrap_or(0.0)),
        },
        Value::String(s) => OscType::String(s.clone()),
        Value::Null => OscType::String(String::new()),
        Value::Array(_) | Value::Object(_) => OscType::String(value.to_string()),
    }
}

/// **What the host said**, taken apart.
#[derive(Debug, Clone)]
pub enum Said {
    /// `/gui_event id seq version payload...`: a widget's value or edit, an
    /// interface event, a verb or a menu pick addressed to a window. `tag` is
    /// the first argument after the stamps when it is a string.
    Event {
        id: i32,
        tag: String,
        args: Vec<OscType>,
    },
    /// `/gui_closed id`: the window is gone.
    Closed(i32),
    /// `/gui_dock_reply seq json`.
    Dock { seq: i32, reply: Value },
    /// `/gui_app_reply seq json`: an application opened, or why not.
    App { reply: Value },
    /// `/gui_info id type key value ...`: what a widget is now.
    Info {
        id: i32,
        props: Vec<(String, OscType)>,
    },
    /// Anything else, unread.
    Other,
}

impl Said {
    /// Reads one packet the host sent; a bundle's messages in order.
    pub fn read(packet: OscPacket) -> Vec<Said> {
        match packet {
            OscPacket::Message(m) => vec![Self::message(m)],
            OscPacket::Bundle(b) => b.content.into_iter().flat_map(Self::read).collect(),
        }
    }

    fn message(m: OscMessage) -> Said {
        let int = |i: usize| match m.args.get(i) {
            Some(OscType::Int(v)) => Some(*v),
            Some(OscType::Long(v)) => i32::try_from(*v).ok(),
            _ => None,
        };
        match m.addr.as_str() {
            "/gui_event" => {
                let Some(id) = int(0) else {
                    return Said::Other;
                };
                let mut rest: Vec<OscType> = m.args.into_iter().skip(3).collect();
                let tag = match rest.first() {
                    Some(OscType::String(s)) => {
                        let tag = s.clone();
                        rest.remove(0);
                        tag
                    }
                    _ => String::new(),
                };
                Said::Event {
                    id,
                    tag,
                    args: rest,
                }
            }
            "/gui_closed" => match int(0) {
                Some(id) => Said::Closed(id),
                None => Said::Other,
            },
            "/gui_dock_reply" => {
                let seq = int(0).unwrap_or(0);
                let reply = match m.args.get(1) {
                    Some(OscType::String(s)) => serde_json::from_str(s).unwrap_or(Value::Null),
                    _ => Value::Null,
                };
                Said::Dock { seq, reply }
            }
            "/gui_app_reply" => {
                let reply = match m.args.get(1) {
                    Some(OscType::String(s)) => serde_json::from_str(s).unwrap_or(Value::Null),
                    _ => Value::Null,
                };
                Said::App { reply }
            }
            "/gui_info" => {
                let Some(id) = int(0) else {
                    return Said::Other;
                };
                let mut props = Vec::new();
                let mut rest = m.args.into_iter().skip(2);
                while let (Some(OscType::String(key)), Some(value)) = (rest.next(), rest.next()) {
                    props.push((key, value));
                }
                Said::Info { id, props }
            }
            _ => Said::Other,
        }
    }
}

/// An event's argument as a string.
pub fn text(args: &[OscType], i: usize) -> String {
    match args.get(i) {
        Some(OscType::String(s)) => s.clone(),
        Some(OscType::Int(v)) => v.to_string(),
        Some(OscType::Long(v)) => v.to_string(),
        Some(OscType::Float(v)) => v.to_string(),
        Some(OscType::Double(v)) => v.to_string(),
        _ => String::new(),
    }
}

/// An event's argument as an integer.
pub fn int(args: &[OscType], i: usize) -> i64 {
    match args.get(i) {
        Some(OscType::Int(v)) => i64::from(*v),
        Some(OscType::Long(v)) => *v,
        Some(OscType::Float(v)) => *v as i64,
        Some(OscType::Double(v)) => *v as i64,
        Some(OscType::String(s)) => s.parse().unwrap_or(0),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An event reads as its widget, its tag and what follows the tag; a
    /// control's bare value has no tag.
    #[test]
    fn an_event_reads_as_its_tag_and_payload() {
        let m = OscMessage {
            addr: "/gui_event".into(),
            args: vec![
                OscType::Int(12),
                OscType::Int(3),
                OscType::Int(0),
                OscType::String("evaluate".into()),
                OscType::String("x = 1".into()),
                OscType::Int(4),
            ],
        };
        let [Said::Event { id, tag, args }] = &Said::read(OscPacket::Message(m))[..] else {
            panic!("not an event");
        };
        assert_eq!((*id, tag.as_str()), (12, "evaluate"));
        assert_eq!((text(args, 0).as_str(), int(args, 1)), ("x = 1", 4));
        let bare = OscMessage {
            addr: "/gui_event".into(),
            args: vec![
                OscType::Int(5),
                OscType::Int(1),
                OscType::Int(0),
                OscType::Int(2),
            ],
        };
        let [Said::Event { tag, args, .. }] = &Said::read(OscPacket::Message(bare))[..] else {
            panic!("not an event");
        };
        assert!(tag.is_empty());
        assert_eq!(int(args, 0), 2);
    }

    /// A list or an object rides as its JSON; a whole number as an int.
    #[test]
    fn a_set_carries_scalars() {
        assert_eq!(scalar(&serde_json::json!(3)), OscType::Int(3));
        assert_eq!(scalar(&serde_json::json!(1.5)), OscType::Double(1.5));
        assert_eq!(scalar(&serde_json::json!(true)), OscType::Int(1));
        assert_eq!(
            scalar(&serde_json::json!([1, 2])),
            OscType::String("[1,2]".into())
        );
    }
}
