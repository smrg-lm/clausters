//! **Sounding a take**: the def that plays a buffer, the group the transport
//! governs, and the seek that is a transport command rather than a def's.
//!
//! A buffer is data, and data does not sound: what sounds is an instrument
//! reading it (`docs/decisions.md`). A host that draws a take therefore needs a
//! def of its own to hear one, and it is deliberately the smallest one that
//! could be -- read one channel of the buffer at the transport's position, scale
//! it, out to one bus. Nothing here is a synthesis surface: a multitrack's
//! instruments are the client's, and this is the editor's monitor.
//!
//! **The reader follows the transport; it does not carry a position.** Its
//! phase is `TransportPos`, so playing from the cursor is `/transport_locate`,
//! looping a selection is `/transport_loop`, and pausing is `/transport_stop`
//! over the group bound with `/transport_group` -- which freezes the readers
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
//! two-channel def would be wrong in both directions -- silent on the right for a
//! mono take, and deaf to the third channel of anything wider.
//!
//! **The monitor has a group of its own**, and it is that group the transport
//! governs. Binding the root would freeze every sound the host has, which in a
//! session is all of them.
//!
//! **One take at a time, and the host holds its nodes.** Playing another take
//! replaces what is playing, because two takes over each other is noise and not
//! a preview. The nodes are freed rather than gated: the def has no envelope,
//! since a monitor that fades is a monitor lying about the contents.

use crate::host::diag;
use clausters_core::osc::{OscMessage, OscType};
use serde_json::json;

use super::{HeadClock, Host};

/// What the monitor is loaded with: whose contents, over how many channels,
/// and whether the transport is rolling it.
///
/// The channel count is here because stopping has to free every reader it
/// started; `rolling` is here because **pausing is not stopping** -- a paused
/// monitor keeps its readers, frozen with the governed group, so resuming
/// continues the sound instead of starting a second copy of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Monitor {
    pub widget: i32,
    /// The first of the readers' node ids, one per channel.
    pub first: i32,
    pub channels: usize,
    pub rolling: bool,
}

/// **How a pass over a take ends**: it loops over a span, or it runs until an
/// end and goes back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pass {
    /// Over and over, inside the half-open span.
    Loop(u64, u64),
    /// Until `end`, where the transport stops and is located to `back`.
    Until { end: u64, back: u64 },
}

/// The monitor's loop switch, and what the host has heard of the transport.
///
/// **An end the engine reached is told from a stop this host sent by
/// counting.** Every `/transport_*` command is answered with a broadcast of
/// the whole state, so the stream a host hears is full of "stopped" -- the
/// loop, the locate and the end a play sends first all say it. What marks a
/// stop is a **transition** from rolling to stopped, and each `/transport_stop`
/// this host sends cancels one; one left over is the engine stopping on its
/// end mark, or somebody else stopping the transport, and either way the
/// monitor's pass is over.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Follow {
    /// Whether the monitor loops (`L`): a selection, or the whole take.
    pub looping: bool,
    /// Whether the last state heard was rolling.
    seen_rolling: bool,
    /// Stops this host has sent and not yet heard the transition of.
    stops_in_flight: u32,
}

/// The def name the host plays a take through. Namespaced, because it is loaded
/// into the same server a multitrack's own defs live in.
pub const TAKE_DEF: &str = "clausters-gui-take";

/// The span a reader with no stated end lasts, in frames: past any multitrack
/// anybody edits (about 260 days at 48 kHz), and a number rather than a branch
/// so the gate is one comparison whoever is playing.
const NO_END: f64 = 1.0e12;

/// How long each edge of a region's gate takes, in frames -- about five
/// milliseconds at 48 kHz.
///
/// Short enough that nobody hears it as a fade and long enough that nobody
/// hears the edge as a click, which is the whole of what a declick is. It is
/// **not** the fades a region carries: those are the multitrack's and are authored,
/// and this is the one every edge needs whether or not anybody asked.
const RAMP: f64 = 240.0;

/// The most channels the monitor will play at once -- a bound rather than a
/// judgement about contents: it is what keeps a malformed channel count from
/// filling the node tree, and it is well past any take a person mixes by hand.
const MAX_CHANNELS: usize = 32;

/// The `/def_send synth` that loads the take monitor.
///
/// Sent once, by whoever gives a session its server: a def is asynchronous, so
/// loading it at the first press would race the `/synth_new` that wanted it.
pub fn take_def_message() -> OscMessage {
    // `TransportPos` is the whole of the seek: the reader plays wherever the
    // multitrack is, so this def has no start frame, no trigger and no loop of its
    // own. `offset` is where this take sits in the multitrack -- 0 while a take *is*
    // the multitrack, and the door a multitrack clip goes through later.
    let spec = json!({
        "name": TAKE_DEF,
        "controls": [
            {"name": "bufnum", "default": 0.0},
            {"name": "chan", "default": 0.0},
            {"name": "amp", "default": 1.0},
            {"name": "out", "default": 0.0},
            {"name": "offset", "default": 0.0},
            // Where in the **source** this reader's own zero is: what a left
            // trim moves, and 0 for a take played whole.
            {"name": "start", "default": 0.0},
            // How long it lasts, in frames. The default is **no end** rather
            // than none: a control left unstated has to be inert, and a span of
            // zero would be a silent node.
            {"name": "span", "default": NO_END},
        ],
        "ugens": [
            {"kind": "TransportPos", "inputs": [{"control": 4}]},
            // The frame of the source: the multitrack's position, shifted by the
            // window this reader opens at.
            {"kind": "Add", "inputs": [{"ugen": 0}, {"control": 5}]},
            {"kind": "BufRd", "inputs": [
                {"control": 0}, {"control": 1}, {"ugen": 1}, {"const": 0.0}]},
            // **The gate is the region's span, with a ramp at each edge.**
            // What it has to say is three things at once: a reader that has
            // not started is silent, one that is over is silent, and neither
            // edge is a step. Without the gate `BufRd` clamps past the end and
            // holds the last sample -- a tone where the multitrack has silence --
            // and without the ramp both edges click, which is what a hard cut
            // at a non-zero sample is.
            //
            // **Where it ends is the nearer of the span and the buffer's own
            // end**, so the gate closes whether or not anybody stated a span:
            // a take played whole leaves `span` at no end, and it is the
            // buffer that says where it stops. Asked of the buffer every
            // block, so an edit that makes the take shorter or longer while
            // it sounds moves the end with it.
            //
            // It is one expression rather than two comparisons and an
            // envelope: the distance to the nearer edge, over the ramp,
            // clamped to `[0, 1]`. Outside the span that distance is negative,
            // so the clamp *is* the gate; inside, it is 1 everywhere but the
            // ramp. A region shorter than two ramps gets a triangle, which is
            // the right answer rather than a special case.
            {"kind": "BufFrames", "inputs": [{"control": 0}]},
            {"kind": "Sub", "inputs": [{"ugen": 3}, {"control": 5}]},
            {"kind": "BinaryOpUGen", "op": "min", "inputs": [
                {"control": 6}, {"ugen": 4}]},
            {"kind": "Sub", "inputs": [{"ugen": 5}, {"ugen": 0}]},
            {"kind": "BinaryOpUGen", "op": "min", "inputs": [
                {"ugen": 0}, {"ugen": 6}]},
            {"kind": "Mul", "inputs": [{"ugen": 7}, {"const": 1.0 / RAMP}]},
            {"kind": "BinaryOpUGen", "op": "max", "inputs": [
                {"ugen": 8}, {"const": 0.0}]},
            {"kind": "BinaryOpUGen", "op": "min", "inputs": [
                {"ugen": 9}, {"const": 1.0}]},
            {"kind": "Mul", "inputs": [{"ugen": 2}, {"ugen": 10}]},
            {"kind": "Mul", "inputs": [{"ugen": 11}, {"control": 2}]},
            {"kind": "Out", "inputs": [{"control": 3}, {"ugen": 12}]},
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
pub fn take_group_messages(group: i32) -> Vec<OscMessage> {
    vec![
        OscMessage {
            addr: "/group_new".into(),
            args: vec![
                OscType::Int(group),
                OscType::Int(1), // add to the tail...
                OscType::Int(0), // ...of the root group
            ],
        },
        OscMessage {
            addr: "/transport_group".into(),
            args: vec![OscType::Int(group)],
        },
    ]
}

impl Host {
    /// **Plays the contents a widget draws**, from `start` (a frame of the
    /// take), stopping whatever the monitor was playing. Returns whether
    /// anything sounds.
    ///
    /// The widget is named rather than the buffer because that is what the hand
    /// pointed at: the same lookup an edit takes, so what plays is what would
    /// be written.
    ///
    /// `pass` says how it ends: a [`Pass::Loop`] repeats its span, in frames;
    /// a [`Pass::Until`] sets the transport's end mark, so the engine stops on
    /// that frame and locates back -- the take's end or a selection's, and the
    /// position cursor, which is where the play cursor then stands.
    pub fn play_buffer(&mut self, def_id: i32, widget_id: i32, start: u64, pass: Pass) -> bool {
        let Some(bufnum) = self.buffer_of(def_id, widget_id) else {
            return false;
        };
        if self.player().is_none() {
            diag::warn!("nothing to play this take through: no audio server");
            return false;
        }
        // As many readers as the contents has channels, each to the bus of the
        // same number: channel 0 is the left output, and a mono take is one
        // reader on it. What the device does with a bus past its own outputs is
        // the device's business, and it is the same answer any wide graph gets.
        let channels = self
            .buffer_channels(def_id, widget_id)
            .unwrap_or(1)
            .clamp(1, MAX_CHANNELS);
        // Stop before rebuilding: the readers are created into the frozen
        // group, so they stand at the new position rather than racing from
        // wherever the last take left the multitrack.
        self.stop_playback();
        // The readers' ids are the host's like any node's, and come back on
        // their `/node_end` once the stop frees them.
        let Some(first) = self.alloc_nodes(channels) else {
            return false;
        };
        let group = self.monitor_group().unwrap_or(0);
        match pass {
            Pass::Loop(from, to) => {
                self.set_loop(Some((from, to)));
                self.set_end(None);
            }
            Pass::Until { end, back } => {
                self.set_loop(None);
                self.set_end(Some((end, back)));
            }
        }
        self.locate(start);
        for ch in 0..channels {
            self.send_sound(OscMessage {
                addr: "/synth_new".into(),
                args: vec![
                    OscType::String(TAKE_DEF.into()),
                    OscType::Int(first + ch as i32),
                    OscType::Int(0),     // add to head...
                    OscType::Int(group), // ...of the governed group
                    OscType::String("bufnum".into()),
                    OscType::Float(bufnum as f32),
                    OscType::String("chan".into()),
                    OscType::Float(ch as f32),
                    OscType::String("out".into()),
                    OscType::Float(ch as f32),
                ],
            });
        }
        self.send_sound(OscMessage {
            addr: "/transport_play".into(),
            args: vec![],
        });
        // **The play cursor is the transport's position**, drawn by the host
        // every frame from the engine's own counter -- an anchor of 0 on that
        // clock is the take's own frame, since the readers play it from the
        // transport's zero. It wraps where a loop wraps and holds where a pause
        // holds, with no message per frame.
        self.set_head_clock(HeadClock::Transport);
        self.set_timeline_playhead(widget_id, 0.0);
        self.playing = Some(Monitor {
            widget: widget_id,
            first,
            channels,
            rolling: true,
        });
        true
    }

    /// Stops the take monitor, if it is playing, and frees its readers.
    /// Returns whether it was playing.
    ///
    /// This is the **end** of a preview, not a pause: [`Self::pause_playback`]
    /// is the one that leaves the readers standing where they are.
    pub fn stop_playback(&mut self) -> bool {
        let Some(monitor) = self.playing.take() else {
            return false;
        };
        self.send_stop();
        // One `/node_free` naming every reader: the ids are one contiguous run,
        // and freeing them together is what keeps a stereo take from
        // half-stopping.
        self.send_sound(OscMessage {
            addr: "/node_free".into(),
            args: (0..monitor.channels)
                .map(|ch| OscType::Int(monitor.first + ch as i32))
                .collect(),
        });
        true
    }

    /// **Pauses or resumes** the monitor, leaving its readers exactly where
    /// they are. Returns whether the transport is now rolling, or `None` when
    /// nothing is loaded to pause.
    ///
    /// The freeze is the server's: the governed group stops processing with its
    /// state intact, so a resume continues the sound instead of restarting it --
    /// and the position, and therefore the drawn head, holds with it. Nothing
    /// here has to remember where the multitrack was, which is the whole reason a
    /// pause is a transport command and not a re-`/synth_new`.
    pub fn pause_playback(&mut self) -> Option<bool> {
        let mut monitor = self.playing?;
        monitor.rolling = !monitor.rolling;
        self.playing = Some(monitor);
        if monitor.rolling {
            self.send_sound(OscMessage {
                addr: "/transport_play".into(),
                args: vec![],
            });
        } else {
            self.send_stop();
        }
        Some(monitor.rolling)
    }

    /// Moves the multitrack to `frame` -- the seek, which is the transport's and not
    /// the reader's. Safe to call while stopped, which is what a click on the
    /// ruler does.
    pub fn locate(&mut self, frame: u64) {
        self.send_sound(OscMessage {
            addr: "/transport_locateSample".into(),
            args: vec![OscType::Long(frame as i64)],
        });
    }

    /// Sends a `/transport_stop`, counted, so the stop it causes is not taken
    /// for the engine ending a pass ([`Follow`]).
    fn send_stop(&mut self) {
        self.follow.stops_in_flight += 1;
        self.send_sound(OscMessage {
            addr: "/transport_stop".into(),
            args: vec![],
        });
    }

    /// Sets the transport's end mark -- where a rolling pass stops, and where
    /// it goes back to -- or clears it with `None`.
    pub fn set_end(&mut self, mark: Option<(u64, u64)>) {
        self.send_sound(OscMessage {
            addr: "/transport_end".into(),
            args: match mark {
                Some((end, back)) => vec![OscType::Long(end as i64), OscType::Long(back as i64)],
                None => vec![],
            },
        });
    }

    /// Whether the monitor loops.
    pub fn monitor_loops(&self) -> bool {
        self.follow.looping
    }

    /// Switches the monitor's loop (`L`), and answers the new state. It takes
    /// effect on the next play: a pass already running keeps the end it began
    /// with.
    pub fn toggle_monitor_loop(&mut self) -> bool {
        self.follow.looping = !self.follow.looping;
        self.follow.looping
    }

    /// **A `/transport_query.reply` heard**: a transition from rolling to
    /// stopped that no stop of this host's accounts for is a pass that ended
    /// without it -- the engine on its end mark, or another client -- and the
    /// monitor's readers are freed, so the next press of the space bar plays.
    /// The engine has already located the transport back, so nothing is sent
    /// but the free.
    pub(crate) fn on_transport_state(&mut self, args: &[OscType]) {
        let Some(OscType::Int(playing)) = args.get(3) else {
            return;
        };
        let rolling = *playing != 0;
        let stopped = self.follow.seen_rolling && !rolling;
        self.follow.seen_rolling = rolling;
        if !stopped {
            return;
        }
        if self.follow.stops_in_flight > 0 {
            self.follow.stops_in_flight -= 1;
            return;
        }
        if let Some(monitor) = self.playing.take() {
            self.send_sound(OscMessage {
                addr: "/node_free".into(),
                args: (0..monitor.channels)
                    .map(|ch| OscType::Int(monitor.first + ch as i32))
                    .collect(),
            });
        }
    }

    /// Sets the span the transport loops inside, or clears it with `None`. The
    /// span is half-open, so a selection plays every frame it covers exactly
    /// once per pass.
    pub fn set_loop(&mut self, span: Option<(u64, u64)>) {
        self.send_sound(OscMessage {
            addr: "/transport_loop".into(),
            args: match span {
                Some((start, end)) => vec![OscType::Long(start as i64), OscType::Long(end as i64)],
                None => vec![],
            },
        });
    }

    /// The widget whose contents the monitor is loaded with, if any -- whether
    /// or not the transport is rolling it.
    pub fn playing_widget(&self) -> Option<i32> {
        self.playing.map(|m| m.widget)
    }

    /// The monitor's whole state, for a caller that has to tell a paused take
    /// from an unloaded one.
    pub fn monitor(&self) -> Option<Monitor> {
        self.playing
    }

    /// Declares that this host drives the server's transport -- that it is the
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
        assert_eq!(
            kinds,
            vec![
                "TransportPos",
                "Add",
                "BufRd",
                "BufFrames",
                "Sub",
                "BinaryOpUGen",
                "Sub",
                "BinaryOpUGen",
                "Mul",
                "BinaryOpUGen",
                "BinaryOpUGen",
                "Mul",
                "Mul",
                "Out"
            ]
        );
        assert_eq!(
            spec["ugens"][1]["inputs"][0]["ugen"], 0,
            "the frame read is the transport's position, shifted by the window"
        );
        assert_eq!(
            spec["ugens"][2]["inputs"][2]["ugen"], 1,
            "the phase is that frame, so a seek is the transport's"
        );
        assert_eq!(
            spec["ugens"][2]["inputs"][3]["const"], 0.0,
            "the reader never wraps: the loop is the transport's too"
        );
        // **The gate is the region's span**, and it is what a multitrack of many
        // regions needs: a reader before its own start and one past its end are
        // both silent, so the boxes on a lane do not bleed into each other.
        // The gate: the distance to the nearer edge, over the ramp, clamped
        // to `[0, 1]` -- so outside the span the clamp is the gate and inside
        // it is 1 everywhere but the ramp, which is what keeps both edges from
        // clicking.
        //
        // The end is the nearer of the span and the buffer's own end, so a
        // take played with no span still stops where its samples do.
        assert_eq!(spec["ugens"][3]["inputs"][0]["control"], 0, "this buffer");
        assert_eq!(spec["ugens"][5]["op"], "min", "span or buffer, the nearer");
        assert_eq!(spec["ugens"][7]["op"], "min", "the nearer edge");
        assert_eq!(spec["ugens"][9]["op"], "max", "clamped below");
        assert_eq!(spec["ugens"][10]["op"], "min", "and above");
        assert_eq!(spec["ugens"][8]["inputs"][1]["const"], 1.0 / RAMP);
        let span = spec["controls"]
            .as_array()
            .expect("controls")
            .iter()
            .find(|c| c["name"] == "span")
            .expect("a span");
        assert_eq!(
            span["default"], NO_END,
            "unstated is no end, because a control left alone has to be inert"
        );
    }

    /// **A take played past its end is silent**, heard on a server rather than
    /// read off the def. The transport keeps rolling after the last frame and
    /// `BufRd` clamps there, so what the gate is for is exactly this: with no
    /// span stated, the take's last sample stayed on the output as a constant
    /// for as long as the transport rolled.
    #[cfg(feature = "standalone")]
    #[test]
    fn a_take_played_past_its_end_is_silent() {
        use clausters::server::nrtsession::{NrtSession, SessionConfig};

        const BLOCK: usize = 64;
        let mut s = NrtSession::open(&SessionConfig {
            sample_rate: 48_000.0,
            channels: 2,
            ..Default::default()
        })
        .expect("open");
        let send = |s: &mut NrtSession, msg: OscMessage| {
            assert!(
                s.send_msg(&msg.addr, msg.args).expect("encode"),
                "ring full"
            );
        };
        send(&mut s, take_def_message());
        for msg in take_group_messages(1001) {
            send(&mut s, msg);
        }
        let frames = 16 * BLOCK as i32;
        send(
            &mut s,
            OscMessage {
                addr: "/buffer_alloc".into(),
                args: vec![OscType::Int(0), OscType::Int(frames), OscType::Int(1)],
            },
        );
        s.settle_for(8);
        send(
            &mut s,
            OscMessage {
                addr: "/buffer_fill".into(),
                args: vec![
                    OscType::Int(0),
                    OscType::Int(0),
                    OscType::Int(frames),
                    OscType::Float(0.5),
                ],
            },
        );
        send(
            &mut s,
            OscMessage {
                addr: "/synth_new".into(),
                args: vec![
                    OscType::String(TAKE_DEF.into()),
                    OscType::Int(2000),
                    OscType::Int(0),
                    OscType::Int(1001),
                    OscType::String("bufnum".into()),
                    OscType::Float(0.0),
                ],
            },
        );
        send(
            &mut s,
            OscMessage {
                addr: "/transport_play".into(),
                args: vec![],
            },
        );
        s.settle_for(4);

        let out = s.run_to_vec((64 * BLOCK) as u64).expect("the render ran");
        let left: Vec<f32> = out.as_chunks::<2>().0.iter().map(|f| f[0]).collect();
        assert!(
            left.iter().any(|x| *x > 0.4),
            "the take is heard while the transport is inside it"
        );
        let tail = &left[32 * BLOCK..];
        assert!(
            tail.iter().all(|x| *x == 0.0),
            "and past its end the output is zero, not its last sample held: {:?}",
            tail.iter().find(|x| **x != 0.0)
        );
    }

    /// The monitor is governed, and it is governed through a group of its own --
    /// binding the root would freeze every sound in the session.
    #[test]
    fn the_monitor_binds_its_own_group_to_the_transport() {
        let msgs = take_group_messages(1001);
        assert_eq!(msgs[0].addr, "/group_new");
        assert_eq!(msgs[0].args[0], OscType::Int(1001));
        assert_eq!(msgs[1].addr, "/transport_group");
        assert_eq!(msgs[1].args[0], OscType::Int(1001));
    }
}
