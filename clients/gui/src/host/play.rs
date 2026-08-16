//! **Sounding a take**: the def that plays a buffer, the group the transport
//! governs, and the seek that is a transport command rather than a def's.
//!
//! A buffer is data, and data does not sound: what sounds is an instrument
//! reading it (`docs/decisions.md`). A host that draws a take therefore needs a
//! def of its own to hear one, and it is deliberately the smallest one that
//! could be — read one channel of the buffer at the transport's position, scale
//! it, out to one bus. Nothing here is a synthesis surface: a composition's
//! instruments are the client's, and this is the editor's monitor.
//!
//! **The reader follows the transport; it does not carry a position.** Its
//! phase is `TransportPos`, so playing from the cursor is `/transport_locate`,
//! looping a selection is `/transport_loop`, and pausing is `/transport_stop`
//! over the group bound with `/transport_group` — which freezes the readers
//! with their state intact, so playing again *continues*. None of those are
//! things this def has to know, and none of them cost a message per pass. It is
//! also what a multitrack needs, where the same time drives many readers, and
//! the reason this host computes no playback time at all: the server owns it,
//! and the window reads it (`docs/decisions.md`, "A clock is not a position").
//!
//! **One node per channel**, which is the server's own convention rather than a
//! shape chosen here: the buffer readers are mono (`BufRd`'s `chan` input picks
//! the channel, and two readers on one phase stay sample-locked), so a stereo
//! take is two nodes exactly as a stereo file is two readers. A fixed
//! two-channel def would be wrong in both directions — silent on the right for a
//! mono take, and deaf to the third channel of anything wider.
//!
//! **The monitor has a group of its own**, and it is that group the transport
//! governs. Binding the root would freeze every sound the host has, which in a
//! session is all of them.
//!
//! **One take at a time, and the host holds its nodes.** Playing another take
//! replaces what is playing, because two takes over each other is noise and not
//! a preview. The nodes are freed rather than gated: the def has no envelope,
//! since a monitor that fades is a monitor lying about the material.

use clausters_core::osc::{OscMessage, OscType};
use serde_json::json;

use super::Host;

/// What the monitor is loaded with: whose material, over how many channels,
/// and whether the transport is rolling it.
///
/// The channel count is here because stopping has to free every reader it
/// started; `rolling` is here because **pausing is not stopping** — a paused
/// monitor keeps its readers, frozen with the governed group, so resuming
/// continues the sound instead of starting a second copy of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Monitor {
    pub widget: i32,
    pub channels: usize,
    pub rolling: bool,
}

/// The def name the host plays a take through. Namespaced, because it is loaded
/// into the same server a composition's own defs live in.
pub const TAKE_DEF: &str = "clausters-gui-take";

/// The first node id the take monitor plays on — a **fixed** one, over the
/// voice window's base and outside its wrapping span, because there is only
/// ever one monitor and a fixed id is what makes a stop that arrives after a
/// lost reply still stop the right node. Channel `n` plays on `TAKE_NODE + n`.
const TAKE_NODE: i32 = super::voices::ID_BASE + super::voices::ID_SPAN;

/// The group the monitor's readers live in, and the one the transport governs.
/// Fixed for the same reason the node ids are, and one past them so a stop can
/// name either without arithmetic.
const TAKE_GROUP: i32 = TAKE_NODE - 1;

/// The most channels the monitor will play at once — a bound rather than a
/// judgement about material: it is what keeps a malformed channel count from
/// filling the node tree, and it is well past any take a person mixes by hand.
const MAX_CHANNELS: usize = 32;

/// The `/def_send synth` that loads the take monitor.
///
/// Sent once, by whoever gives a session its server: a def is asynchronous, so
/// loading it at the first press would race the `/synth_new` that wanted it.
pub fn take_def_message() -> OscMessage {
    // `TransportPos` is the whole of the seek: the reader plays wherever the
    // piece is, so this def has no start frame, no trigger and no loop of its
    // own. `offset` is where this take sits in the piece — 0 while a take *is*
    // the piece, and the door a multitrack clip goes through later.
    let spec = json!({
        "name": TAKE_DEF,
        "controls": [
            {"name": "bufnum", "default": 0.0},
            {"name": "chan", "default": 0.0},
            {"name": "amp", "default": 1.0},
            {"name": "out", "default": 0.0},
            {"name": "offset", "default": 0.0},
        ],
        "ugens": [
            {"kind": "TransportPos", "inputs": [{"control": 4}]},
            {"kind": "BufRd", "inputs": [
                {"control": 0}, {"control": 1}, {"ugen": 0}, {"const": 0.0}]},
            {"kind": "Mul", "inputs": [{"ugen": 1}, {"control": 2}]},
            {"kind": "Out", "inputs": [{"control": 3}, {"ugen": 2}]},
        ],
    });
    OscMessage {
        addr: "/def_send".into(),
        args: vec![
            OscType::String("synth".into()),
            OscType::Blob(spec.to_string().into_bytes()),
        ],
    }
}

/// The messages that put the monitor's group under the transport, sent once
/// beside the def.
///
/// The group is created **stopped**: the transport rolls only when a hand asks
/// it to, and a node added to a frozen group is added frozen, so a take that is
/// prepared before the first press does not start sounding on its own.
pub fn take_group_messages() -> Vec<OscMessage> {
    vec![
        OscMessage {
            addr: "/group_new".into(),
            args: vec![
                OscType::Int(TAKE_GROUP),
                OscType::Int(1), // add to the tail…
                OscType::Int(0), // …of the root group
            ],
        },
        OscMessage {
            addr: "/transport_group".into(),
            args: vec![OscType::Int(TAKE_GROUP)],
        },
    ]
}

impl Host {
    /// **Plays the material a widget draws**, from `start` (a frame of the
    /// take), stopping whatever the monitor was playing. Returns whether
    /// anything sounds.
    ///
    /// The widget is named rather than the buffer because that is what the hand
    /// pointed at: the same lookup an edit takes, so what plays is what would
    /// be written.
    ///
    /// `looping` names the span to repeat, in frames; `None` plays on past the
    /// end, where the reader clamps and goes quiet — there is no "one shot" to
    /// arrange, because the transport simply keeps rolling and the head keeps
    /// moving, which is what a DAW does.
    pub fn play_material(
        &mut self,
        def_id: i32,
        widget_id: i32,
        start: u64,
        looping: Option<(u64, u64)>,
    ) -> bool {
        let Some(bufnum) = self.buffer_of(def_id, widget_id) else {
            return false;
        };
        if self.server.is_none() {
            tracing::warn!("nothing to play this take through: no audio server");
            return false;
        }
        // As many readers as the material has channels, each to the bus of the
        // same number: channel 0 is the left output, and a mono take is one
        // reader on it. What the device does with a bus past its own outputs is
        // the device's business, and it is the same answer any wide graph gets.
        let channels = self
            .material_channels(def_id, widget_id)
            .unwrap_or(1)
            .clamp(1, MAX_CHANNELS);
        // Stop before rebuilding: the readers are created into the frozen
        // group, so they stand at the new position rather than racing from
        // wherever the last take left the piece.
        self.stop_material();
        self.set_loop(looping);
        self.locate(start);
        for ch in 0..channels {
            self.send_to_server(OscMessage {
                addr: "/synth_new".into(),
                args: vec![
                    OscType::String(TAKE_DEF.into()),
                    OscType::Int(TAKE_NODE + ch as i32),
                    OscType::Int(0),          // add to head…
                    OscType::Int(TAKE_GROUP), // …of the monitor's own group
                    OscType::String("bufnum".into()),
                    OscType::Float(bufnum as f32),
                    OscType::String("chan".into()),
                    OscType::Float(ch as f32),
                    OscType::String("out".into()),
                    OscType::Float(ch as f32),
                ],
            });
        }
        self.send_to_server(OscMessage {
            addr: "/transport_play".into(),
            args: vec![],
        });
        self.playing = Some(Monitor {
            widget: widget_id,
            channels,
            rolling: true,
        });
        true
    }

    /// Stops the take monitor, if it is playing, and frees its readers.
    /// Returns whether it was playing.
    ///
    /// This is the **end** of a preview, not a pause: [`Self::pause_material`]
    /// is the one that leaves the readers standing where they are.
    pub fn stop_material(&mut self) -> bool {
        let Some(monitor) = self.playing.take() else {
            return false;
        };
        self.send_to_server(OscMessage {
            addr: "/transport_stop".into(),
            args: vec![],
        });
        // One `/node_free` naming every reader: the ids are contiguous from
        // `TAKE_NODE`, and freeing them together is what keeps a stereo take
        // from half-stopping.
        self.send_to_server(OscMessage {
            addr: "/node_free".into(),
            args: (0..monitor.channels)
                .map(|ch| OscType::Int(TAKE_NODE + ch as i32))
                .collect(),
        });
        true
    }

    /// **Pauses or resumes** the monitor, leaving its readers exactly where
    /// they are. Returns whether the transport is now rolling, or `None` when
    /// nothing is loaded to pause.
    ///
    /// The freeze is the server's: the governed group stops processing with its
    /// state intact, so a resume continues the sound instead of restarting it —
    /// and the position, and therefore the drawn head, holds with it. Nothing
    /// here has to remember where the piece was, which is the whole reason a
    /// pause is a transport command and not a re-`/synth_new`.
    pub fn pause_material(&mut self) -> Option<bool> {
        let mut monitor = self.playing?;
        monitor.rolling = !monitor.rolling;
        self.playing = Some(monitor);
        self.send_to_server(OscMessage {
            addr: if monitor.rolling {
                "/transport_play".into()
            } else {
                "/transport_stop".into()
            },
            args: vec![],
        });
        Some(monitor.rolling)
    }

    /// Moves the piece to `frame` — the seek, which is the transport's and not
    /// the reader's. Safe to call while stopped, which is what a click on the
    /// ruler does.
    pub fn locate(&mut self, frame: u64) {
        self.send_to_server(OscMessage {
            addr: "/transport_locateSample".into(),
            args: vec![OscType::Long(frame as i64)],
        });
    }

    /// Sets the span the transport loops inside, or clears it with `None`. The
    /// span is half-open, so a selection plays every frame it covers exactly
    /// once per pass.
    pub fn set_loop(&mut self, span: Option<(u64, u64)>) {
        self.send_to_server(OscMessage {
            addr: "/transport_loop".into(),
            args: match span {
                Some((start, end)) => vec![OscType::Long(start as i64), OscType::Long(end as i64)],
                None => vec![],
            },
        });
    }

    /// The widget whose material the monitor is loaded with, if any — whether
    /// or not the transport is rolling it.
    pub fn playing_material(&self) -> Option<i32> {
        self.playing.map(|m| m.widget)
    }

    /// The monitor's whole state, for a caller that has to tell a paused take
    /// from an unloaded one.
    pub fn monitor(&self) -> Option<Monitor> {
        self.playing
    }

    /// Declares that this host drives the server's transport — that it is the
    /// one that bound the governed group. See [`Host::owns_transport`].
    pub fn set_owns_transport(&mut self, owns: bool) {
        self.owns_transport = owns;
    }

    /// Whether this host drives the server's transport. A host that does not
    /// sends no `/transport_*` at all: the transport is somebody else's.
    pub fn owns_transport(&self) -> bool {
        self.owns_transport
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The def is the wire's own shape, checked here because nothing else
    /// reads it until a server refuses it out loud on a machine with sound.
    #[test]
    fn the_take_def_reads_a_buffer_at_the_transports_position() {
        let msg = take_def_message();
        assert_eq!(msg.addr, "/def_send");
        assert_eq!(msg.args[0], OscType::String("synth".into()));
        let OscType::Blob(spec) = &msg.args[1] else {
            panic!("the spec rides as a blob")
        };
        let spec: serde_json::Value = serde_json::from_slice(spec).expect("json");
        assert_eq!(spec["name"], TAKE_DEF);
        let kinds: Vec<&str> = spec["ugens"]
            .as_array()
            .expect("ugens")
            .iter()
            .map(|u| u["kind"].as_str().expect("a kind"))
            .collect();
        assert_eq!(kinds, vec!["TransportPos", "BufRd", "Mul", "Out"]);
        assert_eq!(
            spec["ugens"][1]["inputs"][2]["ugen"], 0,
            "the phase is the transport's position, so a seek is the transport's"
        );
        assert_eq!(
            spec["ugens"][1]["inputs"][3]["const"], 0.0,
            "the reader never wraps: the loop is the transport's too"
        );
    }

    /// The monitor is governed, and it is governed through a group of its own —
    /// binding the root would freeze every sound in the session.
    #[test]
    fn the_monitor_binds_its_own_group_to_the_transport() {
        let msgs = take_group_messages();
        assert_eq!(msgs[0].addr, "/group_new");
        assert_eq!(msgs[0].args[0], OscType::Int(TAKE_GROUP));
        assert_eq!(msgs[1].addr, "/transport_group");
        assert_eq!(msgs[1].args[0], OscType::Int(TAKE_GROUP));
        assert_ne!(TAKE_GROUP, 0, "not the root group");
    }
}
