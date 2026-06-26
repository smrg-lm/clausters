//! Widget -> audio-server bindings: the low-latency path that bypasses the script.
//!
//! `/gui_bind <id> "server" <addr> <prefix…>` makes a widget's value flow
//! **straight to the audio server** as the OSC message `addr prefix… value`,
//! with no round-trip through the script — the same idea as a MIDI binding in
//! the server, where a control source is wired to a server-side destination
//! instead of being polled. A bound knob sends an `/n_set` (or any "friend":
//! `/c_set`, `/n_setn`, …) to the audio server on every change; an unbound one
//! keeps emitting `/gui_event` back to the script. `/gui_bind <id>` with no
//! target removes the binding, restoring the event path.
//!
//! The destination is named with a leading keyword so the message shape can
//! grow later (binding to another widget, or to the script with a transform)
//! without changing the protocol; only `"server"` is meaningful at this
//! milestone.

use clausters_core::osc::{OscMessage, OscType};

/// The destination keyword selecting where a bound value goes — only the audio
/// server for now (see the module docs for why it is spelled out in the wire
/// form).
pub const DEST_SERVER: &str = "server";

/// A widget value forwarded to the audio server: an OSC `addr` and the fixed
/// `prefix` arguments that precede the value (e.g. `/n_set` with prefix
/// `[node, "cutoff"]`). On a change the host sends `addr prefix… value`.
#[derive(Debug, Clone, PartialEq)]
pub struct Binding {
    pub addr: String,
    pub prefix: Vec<OscType>,
}

impl Binding {
    /// Parses a `/gui_bind` target tail: `"server" <addr> <prefix…>`. The leading
    /// destination keyword must be [`DEST_SERVER`], the address an OSC path
    /// string (starting with `/`); the remaining arguments are the fixed prefix
    /// (any OSC primitives, kept verbatim so their int/float type survives).
    pub fn parse(target: &[OscType]) -> Result<Binding, String> {
        let dest = match target.first() {
            Some(OscType::String(s)) => s.as_str(),
            _ => {
                return Err(format!(
                    "binding target must start with a destination keyword (\"{DEST_SERVER}\")"
                ));
            }
        };
        if dest != DEST_SERVER {
            return Err(format!(
                "unknown binding destination {dest:?} (only \"{DEST_SERVER}\")"
            ));
        }
        let addr = match target.get(1) {
            Some(OscType::String(s)) if s.starts_with('/') => s.clone(),
            _ => return Err("binding needs an OSC address (e.g. \"/n_set\")".into()),
        };
        Ok(Binding {
            addr,
            prefix: target[2..].to_vec(),
        })
    }

    /// The OSC message that forwards `value`: `addr prefix… value`.
    pub fn message(&self, value: OscType) -> OscMessage {
        let mut args = self.prefix.clone();
        args.push(value);
        OscMessage {
            addr: self.addr.clone(),
            args,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_server_binding_and_builds_the_message() {
        let target = vec![
            OscType::String("server".into()),
            OscType::String("/n_set".into()),
            OscType::Int(1000),
            OscType::String("cutoff".into()),
        ];
        let b = Binding::parse(&target).unwrap();
        assert_eq!(b.addr, "/n_set");
        assert_eq!(
            b.prefix,
            vec![OscType::Int(1000), OscType::String("cutoff".into())]
        );
        // The widget's value is appended after the fixed prefix.
        let msg = b.message(OscType::Float(800.0));
        assert_eq!(msg.addr, "/n_set");
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
            OscType::String("/n_set".into()),
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
    fn a_prefix_is_optional() {
        // e.g. /c_set wired with the bus in the prefix would carry it, but a bare
        // address with no prefix is valid too: the value is the only argument.
        let target = vec![
            OscType::String("server".into()),
            OscType::String("/c_set".into()),
            OscType::Int(3),
        ];
        let b = Binding::parse(&target).unwrap();
        assert_eq!(b.prefix, vec![OscType::Int(3)]);
        assert_eq!(
            b.message(OscType::Float(0.5)).args,
            vec![OscType::Int(3), OscType::Float(0.5)]
        );
    }
}
