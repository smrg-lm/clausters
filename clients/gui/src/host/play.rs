//! **Sounding a take**: the host's monitor, which is the audio editor's
//! playback held by the host.
//!
//! A buffer is data, and data does not sound: what sounds is an instrument
//! reading it (`docs/decisions.md`). A host that draws a take therefore needs
//! nodes of its own to hear one, and they are the audio editor's, written once
//! ([`clausters_core::audio_editor`]) and made by the playback every endpoint
//! holds ([`AudioEditorPlayback`]): a reader per channel following the
//! transport, gated to the take, into a pass and an output that meters it and
//! declicks it on the way to the hardware. A window whose owner plays its own
//! take (the `plays` prop, which the audio editor's window says) never reaches
//! this: the space bar is then the owner's verb, and the owner holds the same
//! playback. What is left here is every other window that draws samples --
//! a script's waveform, a standalone session's takes.
//!
//! **The readers follow the transport; they do not carry a position.** Their
//! phase is the transport's position, so playing from the cursor is a locate,
//! looping a selection is a loop and pausing is a stop over the governed group
//! -- which freezes the readers with their state intact, so playing again
//! *continues*. It is also the reason this host computes no playback time at
//! all: the server owns it, and the window reads it (`docs/decisions.md`, "A
//! clock is not a position").
//!
//! **The monitor has a transport of its own**, [`MONITOR_TRANSPORT`]: playing a
//! take in a session never moves the multitrack's position, and the audio
//! editor's own transport is its application's.
//!
//! **Every take the monitor has played stays made, paused**, and the one
//! played last is the focus: another take plays by switching which one runs,
//! and the same take again by locating it. A stop rolls out the transport's
//! ramp, so the output's declick falls to zero and nothing clicks.

use clausters_core::osc::OscType;
use clausters_editing::apply::Step;
#[cfg(doc)]
use clausters_editing::audio_playback::AudioEditorPlayback;
pub use clausters_editing::audio_playback::Pass;

use crate::host::diag;

use super::{HeadClock, Host};

/// What the monitor plays: whose contents, and whether the transport is
/// rolling it.
///
/// `rolling` is here because **pausing is not stopping** -- a paused monitor
/// keeps its readers, frozen with the governed group, so resuming continues
/// the sound instead of starting it again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Monitor {
    pub widget: i32,
    pub rolling: bool,
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

/// The transport the monitor plays on: neither the multitrack's (0) nor the
/// audio editor application's
/// ([`clausters_editing::audio_playback::AUDIO_EDITOR_TRANSPORT`]).
pub const MONITOR_TRANSPORT: i32 = 2;

impl Host {
    /// **Plays the contents a widget draws**, from `start` (a frame of the
    /// take), the pass ending as `pass` says. Returns whether anything sounds.
    ///
    /// The widget is named rather than the buffer because that is what the hand
    /// pointed at: the same lookup an edit takes, so what plays is what would
    /// be written. The take is synced first -- the buffer a join is now and
    /// its length -- so an edit made since the last play is what plays.
    pub fn play_buffer(&mut self, def_id: i32, widget_id: i32, start: u64, pass: Pass) -> bool {
        // The take is looked up once, and everything the readers need is read
        // off it: the buffer, its shape and the rate it was recorded at.
        let Some(samples) = self.samples_of(def_id, widget_id) else {
            return false;
        };
        let Some(bufnum) = samples.source_buffer() else {
            return false;
        };
        let (channels, frames) = samples.sample_shape().unwrap_or((1, 0));
        let recorded = samples.samples_rate();
        if self.player().is_none() {
            diag::warn!("nothing to play this take through: no audio server");
            return false;
        }
        let engine = if self.server_rate > 0.0 {
            self.server_rate
        } else {
            48_000.0
        };
        let take = recorded.unwrap_or(engine);
        let synced = self.monitor.sync(
            widget_id as u64,
            bufnum,
            channels,
            frames,
            take,
            engine,
            &mut self.ids,
        );
        let mut steps = match synced {
            Ok(steps) => steps,
            Err(e) => {
                diag::warn!("cannot make the monitor's nodes: {e}");
                return false;
            }
        };
        steps.extend(self.monitor.play(widget_id as u64, start, pass));
        self.run_monitor(steps);
        // **The play cursor is the transport's position**, drawn by the host
        // every frame from the engine's own counter -- an anchor of 0 on that
        // clock is the take's own frame, since the readers play it from the
        // transport's zero. It wraps where a loop wraps and holds where a pause
        // holds, with no message per frame.
        self.set_head_clock_of(widget_id, HeadClock::Transport(MONITOR_TRANSPORT as usize));
        self.set_timeline_playhead(widget_id, 0.0);
        self.playing = Some(Monitor {
            widget: widget_id,
            rolling: true,
        });
        true
    }

    /// **Stops the monitor**, if it is playing: the stop rolls out its ramp
    /// and the readers freeze where they are. Returns whether it was playing.
    ///
    /// Where the transport goes back to is the caller's ([`Self::locate`]);
    /// [`Self::pause_playback`] is the one that leaves it to resume.
    pub fn stop_playback(&mut self) -> bool {
        if self.playing.take().is_none() {
            return false;
        }
        self.send_stop();
        true
    }

    /// **Pauses or resumes** the monitor, leaving its readers exactly where
    /// they are. Returns whether the transport is now rolling, or `None` when
    /// nothing is loaded to pause.
    pub fn pause_playback(&mut self) -> Option<bool> {
        let mut monitor = self.playing?;
        monitor.rolling = !monitor.rolling;
        self.playing = Some(monitor);
        if monitor.rolling {
            let steps = self.monitor.resume();
            self.run_monitor(steps);
        } else {
            self.send_stop();
        }
        Some(monitor.rolling)
    }

    /// Moves the monitor's transport to `frame` of the take in focus -- the
    /// seek, which is the transport's and not the reader's. Safe to call while
    /// stopped, which is what a click on the ruler does.
    pub fn locate(&mut self, frame: u64) {
        let steps = self.monitor.locate(frame);
        self.run_monitor(steps);
    }

    /// **The position cursor of take `widget` moved to `frame`**: while the
    /// monitor is not rolling that take, its transport is located there, so
    /// the play cursor stands on the position cursor. Rolling, the mark moves
    /// and the music does not.
    pub fn cue_monitor(&mut self, widget: i32, frame: u64) {
        if self.monitor.focus() != Some(widget as u64)
            || self.playing.is_some_and(|m| m.rolling)
            || self.player().is_none()
        {
            return;
        }
        self.locate(frame);
    }

    /// Sends the stop, counted, so the stop it causes is not taken for the
    /// engine ending a pass ([`Follow`]).
    fn send_stop(&mut self) {
        self.follow.stops_in_flight += 1;
        let steps = self.monitor.pause();
        self.run_monitor(steps);
    }

    /// Carries the monitor's steps out on the server that sounds, behind
    /// whatever is still waiting there.
    fn run_monitor(&mut self, steps: Vec<Step>) {
        self.send_sound_steps(steps);
    }

    /// Frees what the monitor made for takes no window draws any more, and
    /// forgets a take it was playing that is gone.
    pub(super) fn prune_monitor(&mut self) {
        if self
            .playing
            .is_some_and(|m| !self.registry.contains(m.widget))
        {
            self.playing = None;
        }
        let gone: Vec<u64> = self
            .monitor
            .files()
            .into_iter()
            .filter(|file| !self.registry.contains(*file as i32))
            .collect();
        for file in gone {
            if let Ok(steps) = self.monitor.close_file(file, &mut self.ids) {
                self.run_monitor(steps);
            }
        }
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
    /// without it -- the engine on its end mark, or another client -- so the
    /// monitor is no longer playing and the next press of the space bar plays.
    /// The engine has already located the transport back. A reply about
    /// another transport is not the monitor's.
    pub(crate) fn on_transport_state(&mut self, args: &[OscType]) {
        if args.get(12) != Some(&OscType::Int(MONITOR_TRANSPORT)) {
            return;
        }
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
        self.playing = None;
        self.monitor.set_rolling(false);
    }

    /// Sets the span the monitor's transport loops inside, or clears it with
    /// `None` -- a sweep over a take while it plays. The span is half-open, so
    /// a selection plays every frame it covers exactly once per pass, and in
    /// the take's frames: the playback converts it, as it does a locate.
    pub fn set_loop(&mut self, span: Option<(u64, u64)>) {
        let steps = self.monitor.set_loop(span);
        self.run_monitor(steps);
    }

    /// The widget whose contents the monitor plays, if any -- whether or not
    /// the transport is rolling it.
    pub fn playing_widget(&self) -> Option<i32> {
        self.playing.map(|m| m.widget)
    }

    /// The monitor's whole state, for a caller that has to tell a paused take
    /// from one that is not playing.
    pub fn monitor(&self) -> Option<Monitor> {
        self.playing
    }

    /// How many nodes the monitor holds.
    pub fn monitor_nodes(&self) -> usize {
        self.monitor.node_count()
    }

    /// Declares that this host drives the server's transport. See
    /// [`Host::owns_transport`].
    pub fn set_owns_transport(&mut self, owns: bool) {
        self.owns_transport = owns;
    }

    /// Whether this host drives the server's transport. A host that does not
    /// sends no `/transport_*` at all: the transport is somebody else's.
    pub fn owns_transport(&self) -> bool {
        self.owns_transport
    }
}
