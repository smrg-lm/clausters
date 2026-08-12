//! **The host's voices**: the node-id window they allocate from, and the two
//! messages a held key sends.
//!
//! A voice is the host's, not a widget's. An element only *declares* one
//! ([`VoiceSpec`](super::widget::element::VoiceSpec), through
//! [`Element::voice`](super::widget::element::Element::voice)) — which def to
//! play and what to pass it — and the [`Host`](super::Host) does the rest:
//! allocate a node, send it, remember it under the widget, and gate it off on
//! release or when the widget goes away. The keyboard is only the first element
//! to declare one, and nothing here knows a key from any other press.
//!
//! These lived beside the keyboard's geometry, which put OSC inside a model —
//! the one thing a model may not name. They are the host's business and this is
//! where it keeps it.

use clausters_core::osc::{OscMessage, OscType};
use clausters_core::scale;

/// The base of the node-id window the host's voices allocate from — far above
/// the Python client's ids (1000..) and the server's own auto range, so an
/// explicit voice id can never collide (see `docs/decisions.md`).
pub(super) const ID_BASE: i32 = 0x1000_0000;
/// The wrapping window of voice ids over the base.
pub(super) const ID_SPAN: i32 = 1 << 16;

/// The `/synth_new` a host-managed voice press sends: the voice def by name, an
/// explicit node id (so the release can gate it), head of the default group,
/// with the conventional controls — `freq` from the equal-tempered MIDI map,
/// `amp` from the velocity, `gate` open — followed by the widget's extra
/// `voice_args` pairs.
pub fn on_msg(
    name: &str,
    node: i32,
    pitch: i32,
    velocity: i32,
    extra: &[(String, f32)],
) -> OscMessage {
    let mut args = vec![
        OscType::String(name.to_string()),
        OscType::Int(node),
        OscType::Int(0), // add to head…
        OscType::Int(0), // …of the root group
        OscType::String("freq".into()),
        OscType::Float(scale::midi_to_hz(pitch as f64) as f32),
        OscType::String("amp".into()),
        OscType::Float((velocity as f32 / 127.0).clamp(0.0, 1.0)),
        OscType::String("gate".into()),
        OscType::Float(1.0),
    ];
    for (k, v) in extra {
        args.push(OscType::String(k.clone()));
        args.push(OscType::Float(*v));
    }
    OscMessage {
        addr: "/synth_new".into(),
        args,
    }
}

/// The `/node_set <node> gate 0` a voice release sends — the envelope closes and
/// the node frees itself (`FREE_SELF` done action in the voice def).
pub fn off_msg(node: i32) -> OscMessage {
    OscMessage {
        addr: "/node_set".into(),
        args: vec![
            OscType::Int(node),
            OscType::String("gate".into()),
            OscType::Float(0.0),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_messages_have_the_conventional_shape() {
        let extra = vec![("pan".to_string(), 0.5f32)];
        let on = on_msg("piano_voice", 0x1000_0000, 69, 127, &extra);
        assert_eq!(on.addr, "/synth_new");
        assert_eq!(
            &on.args[..4],
            &[
                OscType::String("piano_voice".into()),
                OscType::Int(0x1000_0000),
                OscType::Int(0),
                OscType::Int(0),
            ]
        );
        // freq from the equal-tempered map (A4 = 440), amp from the velocity.
        assert_eq!(on.args[4], OscType::String("freq".into()));
        assert_eq!(on.args[5], OscType::Float(440.0));
        assert_eq!(on.args[6], OscType::String("amp".into()));
        assert_eq!(on.args[7], OscType::Float(1.0));
        assert_eq!(on.args[8], OscType::String("gate".into()));
        assert_eq!(on.args[9], OscType::Float(1.0));
        assert_eq!(on.args[10], OscType::String("pan".into()));
        assert_eq!(on.args[11], OscType::Float(0.5));
        let off = off_msg(42);
        assert_eq!(off.addr, "/node_set");
        assert_eq!(
            off.args,
            vec![
                OscType::Int(42),
                OscType::String("gate".into()),
                OscType::Float(0.0),
            ]
        );
    }
}
