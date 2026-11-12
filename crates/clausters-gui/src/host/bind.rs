//! Widget bindings: a widget's value going somewhere without the script.
//!
//! `/gui_bind <id> "server" <addr> <prefix...>` makes a widget's value flow
//! **straight to the audio server** as the OSC message `addr prefix... value`,
//! with no round-trip through the script -- the same idea as a MIDI binding in
//! the server, where a control source is wired to a server-side destination
//! instead of being polled. A bound knob sends an `/node_set` (or any "friend":
//! `/bus_set`, `/node_setRange`, ...) to the audio server on every change; an unbound one
//! keeps emitting `/gui_event` back to the script. `/gui_bind <id>` with no
//! target removes the binding, restoring the event path.
//!
//! `/gui_bind <id> "widget" <target_id> <prop>` is the same idea aimed **at
//! another widget**: the value applies as a `/gui_set target_id prop <value>`,
//! so a toggle drives a `stack`'s `index`, a slider drives a plot's `max`, a
//! curve drives another curve's `points`. That is what makes a persisted
//! GuiDef an autonomous application: the two widgets talk to each other with
//! nothing attached.
//!
//! **A binding fires an apply, never another binding.** The set a widget
//! binding performs is the mutation `/gui_set` performs and nothing else: it
//! does not re-enter the delivery path, so a widget bound to a widget bound
//! back to it settles instead of cascading. The rule is stated here (and in
//! `docs/gui-protocol.md`) rather than detected -- there is no cycle check,
//! because there is no cycle to check: the chain is one hop by construction.
//!
//! The destination is named with a leading keyword so the message shape can
//! keep growing (binding to the script with a transform, say) without changing
//! the protocol.

use super::wire::int_arg;
use super::{ClientId, GUI_BIND, HostEffect, diag};
use clausters_core::osc::{OscMessage, OscType};
use serde_json::Value;

/// The destination keyword selecting the audio server (see the module docs for
/// why it is spelled out in the wire form).
pub const DEST_SERVER: &str = "server";

/// The destination keyword selecting another widget on this host.
pub const DEST_WIDGET: &str = "widget";

/// Where a bound widget's value goes.
#[derive(Debug, Clone, PartialEq)]
pub enum Binding {
    /// The audio server: an OSC `addr` and the fixed `prefix` arguments that
    /// precede the value (e.g. `/node_set` with prefix `[node, "cutoff"]`). On a
    /// change the host sends `addr prefix... value`.
    Server { addr: String, prefix: Vec<OscType> },
    /// Another widget: the value applies to `prop` of widget `id`, exactly as a
    /// `/gui_set id prop value` would.
    Widget { id: i32, prop: String },
}

impl Binding {
    /// Parses a `/gui_bind` target tail: `"server" <addr> <prefix...>` or
    /// `"widget" <target_id> <prop>`. The leading keyword names the
    /// destination; for a server binding the address must be an OSC path
    /// (starting with `/`) and the remaining arguments are the fixed prefix
    /// (any OSC primitives, kept verbatim so their int/float type survives).
    pub fn parse(target: &[OscType]) -> Result<Binding, String> {
        let dest = match target.first() {
            Some(OscType::String(s)) => s.as_str(),
            _ => {
                return Err(format!(
                    "binding target must start with a destination keyword \
                     (\"{DEST_SERVER}\" or \"{DEST_WIDGET}\")"
                ));
            }
        };
        match dest {
            DEST_SERVER => {
                let addr = match target.get(1) {
                    Some(OscType::String(s)) if s.starts_with('/') => s.clone(),
                    _ => return Err("binding needs an OSC address (e.g. \"/node_set\")".into()),
                };
                Ok(Binding::Server {
                    addr,
                    prefix: target[2..].to_vec(),
                })
            }
            DEST_WIDGET => {
                let id = match target.get(1) {
                    Some(OscType::Int(n)) => *n,
                    _ => return Err("a widget binding needs the target widget's integer id".into()),
                };
                let prop = match target.get(2) {
                    Some(OscType::String(s)) if !s.is_empty() => s.clone(),
                    _ => return Err("a widget binding needs the property to set".into()),
                };
                Ok(Binding::Widget { id, prop })
            }
            other => Err(format!(
                "unknown binding destination {other:?} \
                 (\"{DEST_SERVER}\" or \"{DEST_WIDGET}\")"
            )),
        }
    }

    /// Builds a binding from a GuiDef inline `bind` array -- the declarative
    /// equivalent of a `/gui_bind`, so a **saved GuiDef carries its own
    /// bindings** (the standalone path) and a live script can bind without a
    /// separate `/gui_bind`.
    ///
    /// Three forms, the same destinations the wire names: `["widget", 42,
    /// "index"]`, `["server", "/node_set", 1000, "freq"]`, and the bare
    /// address-first `["/node_set", 1000, "freq"]`, which is a server binding
    /// with the keyword left out (the form the prop shipped with). The
    /// int/float distinction of the prefix is kept.
    pub fn from_json(items: &[Value]) -> Result<Binding, String> {
        match items.first().and_then(Value::as_str) {
            Some(DEST_WIDGET) => {
                let id = match items.get(1).and_then(Value::as_i64) {
                    Some(n) => n as i32,
                    None => {
                        return Err("`bind` to a widget needs the target's integer id".into());
                    }
                };
                let prop = match items.get(2).and_then(Value::as_str) {
                    Some(s) if !s.is_empty() => s.to_string(),
                    _ => return Err("`bind` to a widget needs the property to set".into()),
                };
                Ok(Binding::Widget { id, prop })
            }
            // The keyword is optional for a server binding: the address itself
            // says which destination this is.
            Some(DEST_SERVER) => Self::server_from_json(&items[1..]),
            _ => Self::server_from_json(items),
        }
    }

    /// A server binding from `[addr, prefix...]`.
    fn server_from_json(items: &[Value]) -> Result<Binding, String> {
        let addr = match items.first() {
            Some(Value::String(s)) if s.starts_with('/') => s.clone(),
            _ => return Err("`bind` needs an OSC address first (e.g. \"/node_set\")".into()),
        };
        let prefix = items[1..].iter().filter_map(json_to_osc).collect();
        Ok(Binding::Server { addr, prefix })
    }

    /// The OSC message a **server** binding forwards `value` as:
    /// `addr prefix... value`. `None` for a widget binding, which sends nothing
    /// over the wire (see [`prop`](Self::prop)).
    pub fn message(&self, value: OscType) -> Option<OscMessage> {
        self.message_args(vec![value])
    }

    /// [`message`](Self::message) for a **flat list** of values (an editor's
    /// edit-back payload, e.g. a breakpoint list or a `/buffer_getRange.reply`
    /// region): `addr prefix... values...` -- the widget-value forward generalized
    /// to more than one argument.
    pub fn message_args(&self, mut values: Vec<OscType>) -> Option<OscMessage> {
        let Binding::Server { addr, prefix } = self else {
            return None;
        };
        let mut args = prefix.clone();
        args.append(&mut values);
        Some(OscMessage {
            addr: addr.clone(),
            args,
        })
    }

    /// The `(target id, key, value)` a **widget** binding applies for `values`
    /// -- what a `/gui_set` would carry. A single value rides as the scalar it
    /// is (an int stays an int, so a toggle drives an `index`); a longer
    /// payload rides as its **JSON string**, the same scalar carrier the wire
    /// already uses for an array-valued prop (`points`, `notes`). `None` for a
    /// server binding, and for an empty payload.
    pub fn prop(&self, values: &[OscType]) -> Option<(i32, String, Value)> {
        let Binding::Widget { id, prop } = self else {
            return None;
        };
        let value = match values {
            [] => return None,
            [one] => osc_to_json(one)?,
            many => Value::String(
                serde_json::to_string(&many.iter().filter_map(osc_to_json).collect::<Vec<_>>())
                    .ok()?,
            ),
        };
        Some((*id, prop.clone(), value))
    }
}

/// One JSON value as an OSC primitive for a `bind` prefix, keeping integers and
/// floats apart.
fn json_to_osc(v: &Value) -> Option<OscType> {
    match v {
        Value::Number(n) if n.is_i64() || n.is_u64() => Some(OscType::Int(n.as_i64()? as i32)),
        Value::Number(n) => Some(OscType::Float(n.as_f64()? as f32)),
        Value::String(s) => Some(OscType::String(s.clone())),
        Value::Bool(b) => Some(OscType::Int(*b as i32)),
        _ => None,
    }
}

/// One OSC primitive as the JSON value a prop takes, keeping integers and
/// floats apart the way `/gui_set` does.
fn osc_to_json(v: &OscType) -> Option<Value> {
    match v {
        OscType::Int(n) => Some(Value::from(*n)),
        OscType::Long(n) => Some(Value::from(*n)),
        OscType::Float(x) => Some(Value::from(*x)),
        OscType::Double(x) => Some(Value::from(*x)),
        OscType::String(s) => Some(Value::from(s.clone())),
        _ => None,
    }
}

impl super::Host {
    /// `/gui_bind <id> "server" <addr> <prefix...>` -- forward this widget's value
    /// straight to the audio server on every change, bypassing the script (the
    /// low-latency interactive path). With no target (`/gui_bind <id>`) the
    /// binding is removed and the `/gui_event` path restored.
    pub(super) fn on_bind(&mut self, args: &[OscType], from: ClientId) {
        let Some(id) = int_arg(args, 0) else {
            return diag::warn!("{from}: {GUI_BIND} needs an integer id");
        };
        if args.len() <= 1 {
            if self.bindings.remove(&id).is_some() {
                diag::info!("{from}: {GUI_BIND} {id}: unbound (events restored)");
            } else {
                diag::warn!("{from}: {GUI_BIND} {id}: no binding to remove");
            }
            return;
        }
        let binding = match Binding::parse(&args[1..]) {
            Ok(b) => b,
            Err(e) => return diag::warn!("{from}: {GUI_BIND} {id}: {e}"),
        };
        match &binding {
            Binding::Server { addr, prefix } => {
                if self.server.is_none() {
                    diag::warn!(
                        "{from}: {GUI_BIND} {id}: no audio server attached (--server); the \
                         binding will swallow the value but cannot forward it"
                    );
                }
                diag::info!("{from}: {GUI_BIND} {id} -> audio server {addr} {prefix:?}");
            }
            Binding::Widget {
                id: target,
                prop: key,
            } => diag::info!("{from}: {GUI_BIND} {id} -> widget {target} {key}"),
        }
        self.bindings.insert(id, binding);
    }

    /// Forwards `widget_id`'s `value` to wherever it is bound, returning whether
    /// the binding handled it. When it returns `true` the caller must **not**
    /// also emit a `/gui_event` -- bypassing the script is the whole point. A
    /// widget bound to an audio server that is not attached still returns
    /// `true` (the value is swallowed, not sent to the script); the missing
    /// `--server` was already warned about at bind time.
    pub fn forward(
        &mut self,
        widget_id: i32,
        value: OscType,
        effects: &mut Vec<HostEffect>,
    ) -> bool {
        self.forward_args(widget_id, vec![value], effects)
    }

    /// [`forward`](Self::forward) for a **flat list** of values -- the edit-back
    /// payload of an editor widget (a `bpf`'s breakpoint list today, a drawn
    /// buffer region later): a bound editor sends `addr prefix... values...` to
    /// the audio server, or the payload's JSON carrier to another widget's
    /// prop, bypassing the script exactly as a bound knob does.
    pub fn forward_args(
        &mut self,
        widget_id: i32,
        values: Vec<OscType>,
        effects: &mut Vec<HostEffect>,
    ) -> bool {
        let Some(binding) = self.bindings.get(&widget_id) else {
            return false;
        };
        // Bound to another widget: the value lands on that widget's prop, as
        // the one apply a `/gui_set` would perform. It never re-enters this
        // path, so a binding fires an apply and never another binding.
        if let Binding::Widget { .. } = binding {
            if let Some((target, key, value)) = binding.prop(&values)
                && !self.set_props(target, vec![(key.clone(), value)], effects)
            {
                diag::warn!("{GUI_BIND} {widget_id}: no widget {target} to set {key:?} on");
            }
            return true;
        }
        if let Some(msg) = binding.message_args(values)
            && let Some(server) = self.server.as_ref()
            && let Err(e) = server.send(msg)
        {
            diag::warn!("{GUI_BIND} {widget_id}: failed to forward to the audio server: {e}");
        }
        true
    }

    /// Whether widget `id` currently has a binding (its value goes to the audio
    /// server, not the script).
    pub fn is_bound(&self, id: i32) -> bool {
        self.bindings.contains_key(&id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The server binding's address and prefix, for the tests that assert on
    /// them.
    fn server(b: &Binding) -> (&str, &[OscType]) {
        match b {
            Binding::Server { addr, prefix } => (addr.as_str(), prefix.as_slice()),
            other => panic!("not a server binding: {other:?}"),
        }
    }

    #[test]
    fn parses_a_server_binding_and_builds_the_message() {
        let target = vec![
            OscType::String("server".into()),
            OscType::String("/node_set".into()),
            OscType::Int(1000),
            OscType::String("cutoff".into()),
        ];
        let b = Binding::parse(&target).unwrap();
        let (addr, prefix) = server(&b);
        assert_eq!(addr, "/node_set");
        assert_eq!(
            prefix,
            [OscType::Int(1000), OscType::String("cutoff".into())]
        );
        // The widget's value is appended after the fixed prefix.
        let msg = b.message(OscType::Float(800.0)).unwrap();
        assert_eq!(msg.addr, "/node_set");
        assert_eq!(
            msg.args,
            vec![
                OscType::Int(1000),
                OscType::String("cutoff".into()),
                OscType::Float(800.0)
            ]
        );
    }

    #[test]
    fn rejects_unknown_destination_and_a_non_address() {
        let bad_dest = vec![
            OscType::String("router".into()),
            OscType::String("/node_set".into()),
        ];
        assert!(Binding::parse(&bad_dest).is_err());
        // The address must be present and look like an OSC path.
        let missing = vec![OscType::String("server".into()), OscType::Int(5)];
        assert!(Binding::parse(&missing).is_err());
        let not_path = vec![
            OscType::String("server".into()),
            OscType::String("n_set".into()),
        ];
        assert!(Binding::parse(&not_path).is_err());
        // An empty target (the unbind case) is handled by the host, not here.
        assert!(Binding::parse(&[]).is_err());
    }

    #[test]
    fn parses_a_widget_binding_and_builds_the_prop() {
        let target = vec![
            OscType::String("widget".into()),
            OscType::Int(20),
            OscType::String("index".into()),
        ];
        let b = Binding::parse(&target).unwrap();
        assert_eq!(
            b,
            Binding::Widget {
                id: 20,
                prop: "index".into()
            }
        );
        // A toggle's int value stays an int: a `stack` index is not a float.
        assert_eq!(
            b.prop(&[OscType::Int(1)]),
            Some((20, "index".into(), Value::from(1)))
        );
        // A widget binding sends nothing to the audio server.
        assert!(b.message(OscType::Int(1)).is_none());
        // And a server binding names no prop.
        let s = Binding::from_json(&[Value::from("/node_set")]).unwrap();
        assert!(s.prop(&[OscType::Int(1)]).is_none());
    }

    #[test]
    fn a_widget_binding_needs_an_id_and_a_prop() {
        let no_id = vec![
            OscType::String("widget".into()),
            OscType::String("20".into()),
            OscType::String("index".into()),
        ];
        assert!(Binding::parse(&no_id).is_err());
        let no_prop = vec![OscType::String("widget".into()), OscType::Int(20)];
        assert!(Binding::parse(&no_prop).is_err());
        assert!(Binding::from_json(&[Value::from("widget"), Value::from(20)]).is_err());
    }

    #[test]
    fn a_multi_value_payload_rides_to_a_widget_as_its_json_string() {
        // The edit-back forward aimed at a widget: one curve drives another's
        // `points`, through the scalar carrier the prop already accepts.
        let b = Binding::from_json(&[
            Value::from("widget"),
            Value::from(30),
            Value::from("points"),
        ])
        .unwrap();
        let (id, key, value) = b
            .prop(&[
                OscType::Float(0.0),
                OscType::Float(1.0),
                OscType::Int(5),
                OscType::Float(-4.0),
            ])
            .unwrap();
        assert_eq!((id, key.as_str()), (30, "points"));
        assert_eq!(value, Value::from("[0.0,1.0,5,-4.0]"));
        // An empty payload sets nothing.
        assert!(b.prop(&[]).is_none());
    }

    #[test]
    fn from_json_builds_an_inline_binding_keeping_int_float() {
        let items = vec![
            Value::from("/node_set"),
            Value::from(1000),
            Value::from("freq"),
        ];
        let b = Binding::from_json(&items).unwrap();
        let (addr, prefix) = server(&b);
        assert_eq!(addr, "/node_set");
        assert_eq!(prefix, [OscType::Int(1000), OscType::String("freq".into())]);
        // A value rides as a float; the node id stayed an int.
        assert_eq!(
            b.message(OscType::Float(440.0)).unwrap().args,
            vec![
                OscType::Int(1000),
                OscType::String("freq".into()),
                OscType::Float(440.0)
            ]
        );
        // The address must look like an OSC path.
        assert!(Binding::from_json(&[Value::from("n_set")]).is_err());
        // The keyword form means the same thing as the bare address.
        let spelled = Binding::from_json(&[
            Value::from("server"),
            Value::from("/node_set"),
            Value::from(1000),
            Value::from("freq"),
        ])
        .unwrap();
        assert_eq!(spelled, b);
    }

    #[test]
    fn message_args_forwards_a_flat_list_after_the_prefix() {
        // The edit-back forward: a bound envelope editor sends its whole flat
        // breakpoint list after the fixed prefix, ints kept int.
        let target = vec![
            OscType::String("server".into()),
            OscType::String("/node_setRange".into()),
            OscType::Int(1000),
            OscType::String("env".into()),
        ];
        let b = Binding::parse(&target).unwrap();
        let msg = b
            .message_args(vec![
                OscType::Float(0.0),
                OscType::Float(1.0),
                OscType::Int(5),
                OscType::Float(-4.0),
            ])
            .unwrap();
        assert_eq!(msg.addr, "/node_setRange");
        assert_eq!(
            msg.args,
            vec![
                OscType::Int(1000),
                OscType::String("env".into()),
                OscType::Float(0.0),
                OscType::Float(1.0),
                OscType::Int(5),
                OscType::Float(-4.0),
            ]
        );
    }

    #[test]
    fn a_prefix_is_optional() {
        // e.g. /bus_set wired with the bus in the prefix would carry it, but a bare
        // address with no prefix is valid too: the value is the only argument.
        let target = vec![
            OscType::String("server".into()),
            OscType::String("/bus_set".into()),
            OscType::Int(3),
        ];
        let b = Binding::parse(&target).unwrap();
        assert_eq!(server(&b).1, [OscType::Int(3)]);
        assert_eq!(
            b.message(OscType::Float(0.5)).unwrap().args,
            vec![OscType::Int(3), OscType::Float(0.5)]
        );
    }

    /// A `toggle` bound to `/node_run` is a play/stop switch -- the shape a bundle
    /// with several instruments on one page needs. It only works because the
    /// forwarded value keeps its **type**: `/node_run` takes `(nodeID, flag)` as
    /// ints and refuses a float, and a toggle's value is an int
    /// ([`WidgetKind::event_value`](super::widget::WidgetKind::event_value)).
    #[test]
    fn a_toggle_bound_to_n_run_forwards_ints() {
        let b = Binding::from_json(&[Value::from("/node_run"), Value::from(1000)]).unwrap();
        assert_eq!(
            b.message(OscType::Int(0)).unwrap().args,
            vec![OscType::Int(1000), OscType::Int(0)],
            "the node id and the flag both stay ints"
        );
        assert_eq!(
            b.message(OscType::Int(1)).unwrap().args,
            vec![OscType::Int(1000), OscType::Int(1)]
        );
    }
}
