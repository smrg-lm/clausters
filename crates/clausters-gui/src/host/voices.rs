//! **The host's voices**: the two messages a held key sends. Their node ids
//! are the host's like any node's (`ids.rs`), and come back on `/node_end`
//! once the def frees itself.
//!
//! A voice is the host's, not a widget's. An element only *declares* one
//! ([`VoiceSpec`](super::widget::element::VoiceSpec), through
//! [`Element::voice`](super::widget::element::Element::voice)) -- which def to
//! play and what to pass it -- and the [`Host`](super::Host) does the rest:
//! allocate a node, send it, remember it under the widget, and gate it off on
//! release or when the widget goes away. The keyboard is only the first element
//! to declare one, and nothing here knows a key from any other press.
//!
//! These lived beside the keyboard's geometry, which put OSC inside a model --
//! the one thing a model may not name. They are the host's business and this is
//! where it keeps it.

use clausters_core::osc::{OscMessage, OscType};
use clausters_core::scale;

/// The `/synth_new` a host-managed voice press sends: the voice def by name, an
/// explicit node id (so the release can gate it), head of the default group,
/// with the conventional controls -- `freq` from the equal-tempered MIDI map,
/// `amp` from the velocity, `gate` open -- followed by the widget's extra
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
        OscType::Int(0), // add to head...
        OscType::Int(0), // ...of the root group
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

/// The `/node_set <node> gate 0` a voice release sends -- the envelope closes and
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

impl super::Host {
    /// Delivers a live MIDI note to the element `widget_id`, returning the
    /// message arguments it reported (empty when it consumed the note
    /// silently, `None` when it is not an element or reads no MIDI).
    ///
    /// The one door the native front's input port goes through, so what a note
    /// does to a picture stays the element's.
    pub fn element_midi(
        &mut self,
        def_id: i32,
        widget_id: i32,
        note: super::widget::element::MidiNote,
        playhead: Option<f64>,
    ) -> Option<Vec<Vec<clausters_core::osc::OscType>>> {
        let super::widget::WidgetKind::Custom(el) = self.widget_kind_mut(def_id, widget_id)? else {
            return None;
        };
        Some(el.midi(note, playhead)?.into_messages())
    }

    /// Starts a host-managed voice for a widget that **declared one**
    /// ([`Element::voice`](super::widget::element::Element::voice)): allocates an
    /// explicit node id, sends the `/synth_new` and records the `(pitch, node)`
    /// pair so the release can gate it. A re-press of an already-sounding
    /// pitch releases the old voice first. Bookkeeping happens even with no
    /// server attached, so the logic is testable without a transport.
    ///
    /// It is the host's because only the host has a leg to the audio server;
    /// *when* to sound is the element's, and arrives as a
    /// [`Voice`](super::widget::element::Voice) beside what it reported.
    pub fn voice_on(&mut self, def_id: i32, widget_id: i32, pitch: i32, velocity: i32) {
        let Some(spec) = self
            .window_def(def_id)
            .and_then(|t| t.find(widget_id))
            .and_then(|w| match &w.kind {
                super::widget::WidgetKind::Custom(el) => el.voice(),
                _ => None,
            })
        else {
            return;
        };
        let (name, extra) = (spec.def, spec.args);
        self.voice_off(widget_id, pitch);
        let Some(node) = self.alloc_nodes(1) else {
            return;
        };
        self.send_to_player(on_msg(&name, node, pitch, velocity, &extra));
        self.voices
            .entry(widget_id)
            .or_default()
            .push((pitch, node));
    }

    /// Releases a host-managed voice (`gate 0`; the def frees the node
    /// itself). A no-op when no voice is sounding for the pitch -- including
    /// when `voice` was unset mid-hold, so a recorded voice always gets its
    /// release.
    pub fn voice_off(&mut self, widget_id: i32, pitch: i32) {
        let Some(list) = self.voices.get_mut(&widget_id) else {
            return;
        };
        let mut nodes = Vec::new();
        list.retain(|&(p, node)| {
            if p == pitch {
                nodes.push(node);
                false
            } else {
                true
            }
        });
        if list.is_empty() {
            self.voices.remove(&widget_id);
        }
        for node in nodes {
            self.send_to_player(off_msg(node));
        }
    }

    /// The live voice nodes of widget `widget_id` (for tests/introspection).
    pub fn voices_of(&self, widget_id: i32) -> &[(i32, i32)] {
        self.voices.get(&widget_id).map_or(&[], Vec::as_slice)
    }

    /// Releases every live voice of widgets that no longer exist -- the part
    /// of [`Self::forget_gone`] that has to say so to the server.
    pub(super) fn prune_voices(&mut self) {
        let stale: Vec<i32> = self
            .voices
            .keys()
            .filter(|id| !self.registry.contains(**id))
            .copied()
            .collect();
        for id in stale {
            if let Some(list) = self.voices.remove(&id) {
                for (_, node) in list {
                    self.send_to_player(off_msg(node));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_messages_have_the_conventional_shape() {
        let extra = vec![("pan".to_string(), 0.5f32)];
        let on = on_msg("piano_voice", 1000, 69, 127, &extra);
        assert_eq!(on.addr, "/synth_new");
        assert_eq!(
            &on.args[..4],
            &[
                OscType::String("piano_voice".into()),
                OscType::Int(1000),
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
