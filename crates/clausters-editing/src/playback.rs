//! **How a multitrack is played**: its instance, its applier and its transport, as
//! the steps that carry every verb out.
//!
//! [`crate::instance`] answers what has to change on a server for it to hold
//! what a multitrack says, and [`crate::apply`] what messages that is. What was left
//! over was *playing* it -- which sample a second of the multitrack is when the
//! transport is located, what play, pause, stop and cue send, and that a paused
//! meter is zeroed -- and it was written once in
//! each client's `Playback` and once more in the GUI host. A standalone host
//! and a script's editor are the same multitrack, so they are the same program:
//! this object, which every one of them holds.
//!
//! **It sends nothing.** Every verb answers [`Step`]s -- a message, a `/done` the
//! rest waits for, a barrier -- and the caller sends them and waits where they
//! say. That half is the only one a language, or a host's reply loop, owns.
//!
//! **The position is the engine's.** Every reader follows the transport, so a
//! locate is one `/transport_locateSample` and nothing is re-cued; what this
//! keeps is whether it last told the transport to roll, which is what a cue
//! asks, and a caller that learns otherwise (another client rolled it) says so
//! with [`MultitrackPlayback::set_rolling`].

use std::collections::{BTreeSet, HashMap};

use clausters_core::event::render::Type;
use clausters_core::ids::{IdError, IdSpaces};
use clausters_core::osc::OscType;
use clausters_core::tempoclock::{samples_to_secs, secs_to_samples};
use clausters_document::multitrack::Multitrack;
use clausters_document::multitrack::nodes::{self, SourceInfo};
use clausters_document::{NodeId, SourceId};
use serde_json::{Value, json};

use crate::apply::{Applier, Endpoint, MULTITRACK_TRANSPORT, Medium, Step, steps_json};
use crate::instance::{Instance, Op};
use crate::note_curves::{self, NoteCurves};
use crate::notes_playback::Placement;
use crate::pass::{self, Pass, Passes, samples};

/// **Where a pass ends**: the document's own type, since a session keeps it
/// ([`clausters_document::End`]). Both playbacks that end on the transport
/// take one -- the multitrack's and the notes editor's.
pub use clausters_document::End;

/// **What a pass is read off**, in samples of the multitrack.
pub use crate::pass::View;

/// What a multitrack was last synced from: what a plan is made of, kept so
/// the boxes that play notes can be planned when the notes say which they
/// are -- after the sync, and with no multitrack in hand.
#[derive(Debug, Clone)]
struct Synced {
    multitrack: Multitrack,
    sources: HashMap<SourceId, SourceInfo>,
    /// The tracks' takes, when the application reads them ([`MultitrackPlayback::sync_fed`]).
    feeds: Option<HashMap<NodeId, nodes::PlannedFeed>>,
    gain: f32,
}

impl Synced {
    /// The plan it makes at `rate`, with the boxes `voiced` names.
    fn plan(&self, rate: f64, voiced: &BTreeSet<NodeId>) -> nodes::Plan {
        match &self.feeds {
            Some(feeds) => nodes::plan_fed(&self.multitrack, rate, voiced, feeds),
            None => nodes::plan_voiced(&self.multitrack, rate, &self.sources, voiced),
        }
    }
}

/// The box an event is of, when it is a note of one: a box of a multitrack is
/// its events' scope, by its region.
fn box_of(event: &crate::notes_playback::Placed) -> Option<u64> {
    (Type::of(&event.keys) == Type::Note)
        .then(|| event.scope.parse().ok())
        .flatten()
}

/// **One multitrack, as it is playing**, its ops carried out in a
/// [`Medium`]: an endpoint's [`Applier`], whose steps a client walks, or an
/// engine the application holds, which answers none.
#[derive(Debug, Clone)]
pub struct MultitrackPlayback<M: Medium = Applier> {
    instance: Instance,
    applier: M,
    /// The rate the multitrack was last planned at.
    rate: f64,
    /// **Whether the transport rolls and its pass loops**: the rules every
    /// editor plays by ([`crate::pass`]), in samples at [`Self::rate`].
    passes: Passes,
    /// **What the pass in progress was read off**, the selection and the
    /// position cursor in samples: a pass over no selection ends where the
    /// multitrack does, so it is read again when that moves.
    view: Option<View>,
    /// The pass in progress as last asked of the transport, so it is asked
    /// again only when it moves.
    pass: Option<Pass>,
    /// Where a pass ends ([`Self::set_end`]).
    end: End,
    /// Where the contents end, in seconds of the multitrack: the last region's
    /// end on any track and any take lane, as of the last [`Self::sync`].
    content_end: f64,
    /// Whether a [`Self::sync`] has said where the contents end.
    measured: bool,
    /// The transport's ramp last sent, in samples.
    fade_sent: Option<i64>,
    /// **The event lane the boxes over sequences play from**, once made: its
    /// id, which is the tracks' group's, and the group its notes are made in
    /// -- the voices' group before the multitrack, which follows the
    /// transport and which a stop does not freeze.
    lane: Option<(i32, i32)>,
    /// **The boxes that play notes**, by region, as the last [`Self::notes`]
    /// said: each is a clip of its track, over a bus its voices write.
    voiced: BTreeSet<NodeId>,
    /// What the last [`Self::sync`] was of.
    synced: Option<Synced>,
    /// The graphs, readers and tables the notes' curves play through.
    curves: NoteCurves,
    /// Whether a pass started since the notes were last planned: a note's
    /// old tables are given back then (`NoteCurves::ops`).
    passed: bool,
}

impl MultitrackPlayback {
    /// A playback that has made nothing yet, carrying out its steps for
    /// `endpoint`.
    pub fn new(endpoint: Endpoint) -> MultitrackPlayback {
        MultitrackPlayback::in_medium(Applier::new(endpoint))
    }
}

impl<M: Medium> MultitrackPlayback<M> {
    /// A playback that has made nothing yet, carrying out its ops in
    /// `medium`.
    pub fn in_medium(medium: M) -> MultitrackPlayback<M> {
        MultitrackPlayback {
            instance: Instance::new(),
            applier: medium,
            rate: 48_000.0,
            passes: Passes::default(),
            view: None,
            pass: None,
            end: End::Open,
            measured: false,
            content_end: 0.0,
            fade_sent: None,
            lane: None,
            voiced: BTreeSet::new(),
            synced: None,
            curves: NoteCurves::new("mt/notes"),
            passed: false,
        }
    }

    /// **What the boxes over sequences play**, as the event lane's data on the
    /// multitrack's transport: `placed` is `crate::multitrack::placed_notes`,
    /// in seconds of the multitrack, and the server plays it by the position --
    /// a locate, the loop and a stop are the transport's. The lane is made the
    /// first time there is something to play, named by the tracks' group, and
    /// sent its data again on every call; a multitrack that never had notes
    /// makes nothing.
    ///
    /// **A box of notes is a source of sound inside its track**, as a box of
    /// samples is. Each box with notes is a clip of its track over a bus of
    /// its own (`nodes::plan_voiced`), and its notes are told that bus as
    /// their `out` -- the control an event's `out` key sets, and the track's
    /// to say here, so a key of that name on a note of a box is not read. A
    /// note's def, its velocity and the curves inside it make its sound; the
    /// box's strip and then the track's -- gain, curves, mute, meter -- are
    /// after it, as after a take. A def with no `out` control sounds where it
    /// was written to, outside the track.
    ///
    /// The voices are made in a group before the multitrack
    /// (`crate::instance::VOICES`), so a voice writes its bus in the block
    /// the clip reads it; and **the curves are heard through graphs**
    /// (`crate::note_curves`) made in the same group, allocating from `ids`.
    ///
    /// A MIDI message and a command are not voices of this: they go where
    /// they name, and no strip reaches them.
    pub fn notes(&mut self, placed: &Placement, ids: &mut IdSpaces) -> Result<Vec<Step>, IdError> {
        let Some(group) = self.applier.node(crate::instance::TRACKS) else {
            return Ok(Vec::new());
        };
        if placed.events.is_empty() && self.lane.is_none() {
            return Ok(Vec::new());
        }
        let mut steps = self.voice(placed, ids)?;
        // The voices' group, once a box of notes made it; before that -- a
        // lane of MIDI and commands alone -- the transport's group around
        // the multitrack. Either follows the transport and is not frozen by
        // a stop: a stop releases the notes, and their releases ring out.
        let parent = if self.applier.node(crate::instance::VOICES).is_some() {
            crate::instance::VOICES
        } else {
            crate::instance::TRANSPORT
        };
        let target = self.applier.node(parent).unwrap_or(group);
        if self.lane != Some((group, target)) {
            // A tracks' group made again is a new lane, and so is one whose
            // notes are made somewhere else.
            steps.extend(self.free_lane());
            steps.extend(self.plain(vec![Op::Lane {
                transport: MULTITRACK_TRANSPORT,
                lane: group,
                target,
            }]));
            self.lane = Some((group, target));
        }
        let routed = self.routed(placed);
        let plan = note_curves::plan(&routed, self.rate).scoped(group);
        let ops = self
            .curves
            .ops(&plan, parent, std::mem::take(&mut self.passed));
        steps.extend(self.applier.apply(ops, ids)?);
        let slots = self.curves.slots(&plan, &self.applier);
        let data = crate::notes_playback::data(&routed.events, self.rate, &slots).to_string();
        steps.extend(self.plain(vec![Op::LaneSet { lane: group, data }]));
        Ok(steps)
    }

    /// **Makes the boxes that play notes be clips of their tracks**, when
    /// which boxes those are changed: the multitrack as it was last synced,
    /// planned again with them, and the difference as steps. A sync after
    /// this plans them itself.
    fn voice(&mut self, placed: &Placement, ids: &mut IdSpaces) -> Result<Vec<Step>, IdError> {
        let voiced: BTreeSet<NodeId> = placed
            .events
            .iter()
            .filter_map(box_of)
            .map(NodeId)
            .collect();
        if voiced == self.voiced {
            return Ok(Vec::new());
        }
        self.voiced = voiced;
        let Some(synced) = &self.synced else {
            return Ok(Vec::new());
        };
        let plan = synced.plan(self.rate, &self.voiced);
        let ops = self.instance.reconcile(&plan, synced.gain);
        self.applier.apply(ops, ids)
    }

    /// **`placed` with each note sounding into its box**: its `out` is the
    /// bus the box's clip reads. A note of no box, or of a box that has no
    /// bus yet, is left as it is.
    fn routed(&self, placed: &Placement) -> Placement {
        let mut routed = placed.clone();
        for event in &mut routed.events {
            let bus = box_of(event)
                .and_then(|region| self.applier.bus(&crate::instance::voice_bus_handle(region)));
            if let Some((bus, _)) = bus {
                event.keys.insert("out".into(), json!(bus));
            }
        }
        routed
    }

    /// Frees the lane, when there is one: its notes are released.
    fn free_lane(&mut self) -> Vec<Step> {
        let Some((lane, _)) = self.lane.take() else {
            return Vec::new();
        };
        self.plain(vec![Op::LaneFree { lane }])
    }

    /// **Makes what sounds be what the multitrack says**: the plan at `rate`, the
    /// difference from what is made, as steps, allocating from `ids`. It runs
    /// on every change of the multitrack, whoever made it; a node that did not
    /// change costs nothing.
    pub fn sync(
        &mut self,
        multitrack: &Multitrack,
        rate: f64,
        sources: &HashMap<SourceId, SourceInfo>,
        gain: f32,
        ids: &mut IdSpaces,
    ) -> Result<Vec<Step>, IdError> {
        self.synced_from(
            Synced {
                multitrack: multitrack.clone(),
                sources: sources.clone(),
                feeds: None,
                gain,
            },
            rate,
            ids,
        )
    }

    /// **[`Self::sync`] over a multitrack whose tracks the application
    /// reads**: each track's regions of samples are its take, read into the
    /// buffers `feeds` names by track (`nodes::plan_fed`), so an edit of
    /// its regions makes and frees no node -- the take is handed to the
    /// engine again and the buffers stay.
    pub fn sync_fed(
        &mut self,
        multitrack: &Multitrack,
        rate: f64,
        feeds: &HashMap<NodeId, nodes::PlannedFeed>,
        gain: f32,
        ids: &mut IdSpaces,
    ) -> Result<Vec<Step>, IdError> {
        self.synced_from(
            Synced {
                multitrack: multitrack.clone(),
                sources: HashMap::new(),
                feeds: Some(feeds.clone()),
                gain,
            },
            rate,
            ids,
        )
    }

    fn synced_from(
        &mut self,
        synced: Synced,
        rate: f64,
        ids: &mut IdSpaces,
    ) -> Result<Vec<Step>, IdError> {
        self.rate = rate;
        let plan = synced.plan(rate, &self.voiced);
        let ops = self.instance.reconcile(&plan, synced.gain);
        let multitrack = synced.multitrack.clone();
        let multitrack = &multitrack;
        self.synced = Some(synced);
        let mut steps = self.applier.apply(ops, ids)?;
        // **The transport's ramp**, so a stop rolls the tracks out while the
        // master's way out fades them and a play fades them in: the master is
        // outside the governed group, and this is what it declicks across.
        let fade = secs_to_samples(crate::audio_playback::FADE_SECS, rate);
        if self.fade_sent != Some(fade) {
            self.fade_sent = Some(fade);
            steps.extend(self.command("/transport_fade", vec![OscType::Long(fade)]));
        }
        // An edit that moves the last region moves where a pass stops -- and
        // takes an end marker with it when the contents grow past one
        // (`End::carried`). Not on the first sync: a marker read with the
        // multitrack was put where it is against these same contents.
        let now = multitrack.end().0;
        if self.measured {
            self.end = self.end.carried(self.content_end, now);
        }
        self.measured = true;
        self.content_end = now;
        steps.extend(self.remark());
        Ok(steps)
    }

    /// **Where a pass ends** ([`End`]): open by default, or where the last
    /// region ends on any track and any take lane, or at an end marker in
    /// seconds -- going back to the position cursor. A pass in progress over
    /// no selection ends there from now on.
    pub fn set_end(&mut self, end: End) -> Vec<Step> {
        self.end = end;
        self.remark()
    }

    /// The transport this multitrack plays on -- what its transport commands
    /// name, and what a view drawing its play cursor reads.
    pub fn transport(&self) -> i32 {
        MULTITRACK_TRANSPORT
    }

    /// Where a pass ends.
    pub fn end(&self) -> End {
        self.end
    }

    /// **What the space bar plays over the multitrack**, read off the view
    /// in samples ([`pass::space`]). The end it hands the rule is its own:
    /// where [`End`] says a pass stops, and for a loop over no selection the
    /// contents, which it repeats whether or not a pass stops where they end.
    pub fn space(&self, looping: bool, view: View) -> (u64, Pass) {
        let end = if looping {
            (self.content_end > 0.0).then_some(self.content_end)
        } else {
            self.end.at(self.content_end)
        };
        let end = end.map(|secs| self.secs_to_samples(secs) as u64);
        pass::space(looping, view.selection, view.cursor, end)
    }

    /// **Plays what the view asks**: the pass [`Self::space`] reads off it,
    /// located, cued -- tracks the application reads stream from disk, and
    /// the play waits until they hold the frames it starts on -- and rolled.
    pub fn play(&mut self, looping: bool, view: View) -> Vec<Step> {
        let (start, pass) = self.space(looping, view);
        self.view = Some(view);
        self.pass = Some(pass);
        self.passed = true;
        let ops = self
            .passes
            .play(Some(MULTITRACK_TRANSPORT), start, pass, &samples);
        self.plain(ops)
    }

    /// **The loop switch changed while it plays**: the pass in progress ends
    /// as the view now says, from where the transport stands. Stopped,
    /// nothing: the next play reads the switch.
    pub fn set_pass(&mut self, looping: bool, view: View) -> Vec<Step> {
        if !self.passes.rolling() {
            return Vec::new();
        }
        let pass = self.space(looping, view).1;
        self.view = Some(view);
        if self.pass == Some(pass) {
            return Vec::new();
        }
        self.pass = Some(pass);
        let ops = self
            .passes
            .set_pass(Some(MULTITRACK_TRANSPORT), pass, &samples);
        self.plain(ops)
    }

    /// **The selection was redrawn while the pass loops**: the loop is now
    /// over the span the view's loop covers -- the selection, or everything
    /// with none -- and with `place` the transport goes to its start
    /// ([`Passes::follow`]).
    pub fn follow(&mut self, view: View, place: bool) -> Vec<Step> {
        let Pass::Loop { from, to } = self.space(true, view).1 else {
            return Vec::new();
        };
        let ops = self
            .passes
            .follow(Some(MULTITRACK_TRANSPORT), (from, to), place, &samples);
        if !ops.is_empty() {
            self.view = Some(view);
            self.pass = Some(Pass::Loop { from, to });
        }
        self.plain(ops)
    }

    /// **The position cursor was placed at `frame`**: rolling, the music goes
    /// on from it, its pass ending as the view says; stopped, the transport
    /// stands on it ([`Passes::place`]).
    pub fn place(&mut self, frame: u64, view: View) -> Vec<Step> {
        let pass = self.space(self.passes.looping(), view).1;
        if self.passes.rolling() {
            self.view = Some(view);
            self.pass = Some(pass);
        }
        let ops = self
            .passes
            .place(Some(MULTITRACK_TRANSPORT), frame, pass, &samples);
        self.plain(ops)
    }

    /// Rolls the transport again from where it stands: a paused pass goes on.
    pub fn resume(&mut self) -> Vec<Step> {
        let ops = self.passes.resume(Some(MULTITRACK_TRANSPORT));
        self.plain(ops)
    }

    /// **Freezes the multitrack where it stands**, every node's state intact -- and
    /// zeroes its meters, because a frozen meter gets no time to fall and would
    /// go on claiming the last level it wrote.
    pub fn pause(&mut self) -> Vec<Step> {
        let mut ops = self.passes.pause(Some(MULTITRACK_TRANSPORT));
        ops.extend(self.zeroed());
        self.plain(ops)
    }

    /// **Halts and goes back to `back`**, the position cursor in samples: the
    /// next play starts from it, not from where the pass got to.
    pub fn stop(&mut self, back: u64) -> Vec<Step> {
        let mut ops = self.passes.stop(Some(MULTITRACK_TRANSPORT), back, &samples);
        ops.extend(self.zeroed());
        self.plain(ops)
    }

    /// **Puts the transport at sample `frame`** of the multitrack. The readers
    /// seek in the engine, so what is sounding carries on from there.
    pub fn locate(&mut self, frame: u64) -> Vec<Step> {
        let ops = pass::locate(Some(MULTITRACK_TRANSPORT), frame, &samples);
        self.plain(ops)
    }

    /// **The position cursor moved to `frame` where nothing plays**: a
    /// stopped transport stands on it, and a rolling one is left alone
    /// ([`Passes::cue`]).
    pub fn cue(&mut self, frame: u64) -> Vec<Step> {
        let ops = self.passes.cue(Some(MULTITRACK_TRANSPORT), frame, &samples);
        self.plain(ops)
    }

    /// **A pass over no selection ends where the multitrack now does**: read
    /// again off the view it was played from, when the contents or the end
    /// moved while it plays.
    fn remark(&mut self) -> Vec<Step> {
        match self.view {
            Some(view) if view.selection.is_none() && self.passes.rolling() => {
                self.set_pass(self.passes.looping(), view)
            }
            _ => Vec::new(),
        }
    }

    /// The meters zeroed, as a pause leaves them.
    fn zeroed(&self) -> Vec<Op> {
        self.instance
            .meters()
            .into_iter()
            .map(|(_, handle, _)| Op::FillBus { handle, value: 0.0 })
            .collect()
    }

    /// Frees everything the multitrack made. The multitrack itself is untouched: what a
    /// playback holds is nodes, and nodes are not the document.
    pub fn close(&mut self, ids: &mut IdSpaces) -> Result<Vec<Step>, IdError> {
        self.passes = Passes::default();
        self.view = None;
        self.pass = None;
        self.fade_sent = None;
        let mut steps = self.free_lane();
        self.voiced.clear();
        self.synced = None;
        let mut ops = self.curves.teardown();
        ops.extend(self.instance.teardown());
        steps.extend(self.applier.apply(ops, ids)?);
        Ok(steps)
    }

    /// **A transport command on the multitrack's transport**, as the op
    /// that carries it out in the medium.
    fn command(&mut self, addr: &str, args: Vec<OscType>) -> Vec<Step> {
        match crate::apply::transport_op(MULTITRACK_TRANSPORT, addr, &args) {
            Some(op) => self.plain(vec![op]),
            None => Vec::new(),
        }
    }

    /// Ops that allocate nothing, carried out in the medium.
    fn plain(&mut self, ops: Vec<Op>) -> Vec<Step> {
        let mut none = IdSpaces::new(
            clausters_core::ids::ServerShape::DEFAULT,
            clausters_core::ids::IdShare::WHOLE,
        );
        self.applier.apply(ops, &mut none).unwrap_or_default()
    }

    /// The medium its ops are carried out in.
    pub fn medium(&self) -> &M {
        &self.applier
    }

    /// Says whether the transport is rolling, when the caller learned it from
    /// the engine rather than from this.
    pub fn set_rolling(&mut self, rolling: bool) {
        self.passes.set_rolling(rolling);
    }

    /// Whether the transport was last told to roll.
    pub fn rolling(&self) -> bool {
        self.passes.rolling()
    }

    /// Whether the pass last asked for loops.
    pub fn looping(&self) -> bool {
        self.passes.looping()
    }

    /// Whether the transport stands where a pause left it, so a play
    /// resumes ([`Passes::paused`]).
    pub fn paused(&self) -> bool {
        self.passes.paused()
    }

    /// Whether anything of the multitrack is made.
    pub fn is_sounding(&self) -> bool {
        self.instance.is_sounding()
    }

    /// **The transport's group**, once the multitrack has made it: the group
    /// that follows the transport, holding the multitrack's graph, whose
    /// tracks' group is the one the transport governs.
    pub fn group(&self) -> Option<i32> {
        self.applier.node(crate::instance::TRANSPORT)
    }

    /// How many nodes the multitrack holds.
    pub fn node_count(&self) -> usize {
        self.applier.node_count()
    }

    /// **A second of the multitrack as a sample**, at the rate it was last
    /// planned at and with the core's seconds -> samples rounding.
    pub fn secs_to_samples(&self, secs: f64) -> i64 {
        secs_to_samples(secs.max(0.0), self.rate)
    }

    /// A sample as a second of the multitrack: the same rate, read the other
    /// way.
    pub fn samples_to_secs(&self, samples: i64) -> f64 {
        samples_to_secs(samples, self.rate)
    }

    /// The meters the multitrack writes, as `(track id, first bus, channels)`: a run
    /// of `2 * channels`, the level first and the mark after it.
    pub fn meters(&self) -> Vec<(u64, i32, usize)> {
        self.instance
            .meters()
            .into_iter()
            .filter_map(|(track, handle, channels)| {
                let (bus, _) = self.applier.bus(&handle)?;
                Some((track, bus, channels))
            })
            .collect()
    }
}

/// **Steps as the JSON a client is handed**: `{"steps": [...]}`, or
/// `{"error": "..."}` when an id space was exhausted.
pub fn answer_json(steps: Result<Vec<Step>, IdError>) -> String {
    match steps {
        Ok(steps) => json!({ "steps": steps_json(&steps) }).to_string(),
        Err(e) => json!({ "error": e.to_string() }).to_string(),
    }
}

/// [`MultitrackPlayback::sync`] over JSON: the multitrack as the document's own JSON and
/// the source table as [`crate::instance::sources_table`] reads it.
pub fn sync_json(
    playback: &mut MultitrackPlayback,
    multitrack: &str,
    rate: f64,
    sources: &str,
    gain: f32,
    ids: &mut IdSpaces,
) -> String {
    let multitrack = match serde_json::from_str::<Multitrack>(multitrack) {
        Ok(multitrack) => multitrack,
        Err(e) => return json!({ "error": format!("not a multitrack: {e}") }).to_string(),
    };
    let table = crate::instance::sources_table(sources);
    answer_json(playback.sync(&multitrack, rate, &table, gain, ids))
}

/// [`MultitrackPlayback::notes`] over JSON: the placed notes as
/// `crate::multitrack::placed_notes` answers them, `[{"start", "end",
/// "keys"}]`.
pub fn notes_json(playback: &mut MultitrackPlayback, placed: &str, ids: &mut IdSpaces) -> String {
    match serde_json::from_str::<Placement>(placed) {
        Ok(placed) => answer_json(playback.notes(&placed, ids)),
        Err(e) => json!({ "error": format!("not placed notes: {e}") }).to_string(),
    }
}

/// [`MultitrackPlayback::meters`] as JSON: `[{"track", "bus", "channels"}]`.
pub fn meters_json(playback: &MultitrackPlayback) -> String {
    Value::Array(
        playback
            .meters()
            .into_iter()
            .map(|(track, bus, channels)| json!({"track": track, "bus": bus, "channels": channels}))
            .collect(),
    )
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apply::send;
    use clausters_core::ids::{IdShare, ServerShape};
    use clausters_document::multitrack::{Tempo, Track};
    use clausters_document::{Beat, NodeId};

    fn spaces() -> IdSpaces {
        IdSpaces::new(ServerShape::DEFAULT, IdShare::WHOLE)
    }

    fn addrs(steps: &[Step]) -> Vec<String> {
        steps
            .iter()
            .filter_map(|step| match step {
                Step::Send(m) => Some(m.addr.clone()),
                _ => None,
            })
            .collect()
    }

    fn multitrack() -> Multitrack {
        Multitrack {
            tracks: vec![Track::new(NodeId(10), NodeId(11))],
            ..Multitrack::default()
        }
    }

    /// A multitrack whose one region ends at `end` seconds.
    fn ending_at(end: f64) -> Multitrack {
        use clausters_document::multitrack::Window;
        use clausters_document::multitrack::{Content, Region};
        use clausters_document::{Lifetime, Second, SourceRef};

        let mut multitrack = multitrack();
        let window = Window {
            source: SourceRef {
                source: SourceId(1),
                lifetime: Lifetime::Session,
                generation: 0,
                range: None,
            },
            start: 0.0,
            duration: end,
        };
        multitrack.tracks[0].take_lanes[0].place(Region::new(
            NodeId(20),
            Second(0.0),
            Second(end),
            Content::window(window),
        ));
        multitrack
    }

    /// The `/transport_end` among `steps`, as its arguments.
    fn end_mark(steps: &[Step]) -> Option<Vec<OscType>> {
        steps.iter().find_map(|step| match step {
            Step::Send(m) if m.addr == "/transport_end" => Some(m.args[1..].to_vec()),
            _ => None,
        })
    }

    /// The arguments of the message to `addr` among `steps`, past the
    /// transport.
    fn args_of(steps: &[Step], addr: &str) -> Option<Vec<OscType>> {
        steps.iter().find_map(|step| match step {
            Step::Send(m) if m.addr == addr => Some(m.args[1..].to_vec()),
            _ => None,
        })
    }

    /// A playback synced to a multitrack whose contents end at `end` seconds.
    fn synced(end: f64) -> MultitrackPlayback {
        let mut playback = MultitrackPlayback::new(Endpoint::default());
        playback
            .sync(
                &ending_at(end),
                48_000.0,
                &HashMap::new(),
                1.0,
                &mut spaces(),
            )
            .unwrap();
        playback
    }

    /// The view with the position cursor at `cursor` seconds and no selection.
    fn at(cursor: f64) -> View {
        View {
            selection: None,
            cursor: Some((cursor * 48_000.0) as u64),
        }
    }

    /// **A pass is open by default**; at the contents it ends where the last
    /// region does and goes back to the position cursor, and a pass in
    /// progress over no selection follows an edit that moves them, or an end
    /// marker put in their place.
    #[test]
    fn a_pass_ends_where_it_is_asked_to() {
        let mut playback = synced(5.0);
        assert!(playback.set_end(End::Contents).is_empty(), "stopped");
        playback.set_end(End::Open);
        let open = playback.play(false, at(1.0));
        assert_eq!(end_mark(&open), Some(vec![]), "open: nothing is marked");
        assert_eq!(
            end_mark(&playback.set_end(End::Contents)),
            Some(vec![OscType::Long(240_000), OscType::Long(48_000)]),
            "the last region's end, back to the cursor"
        );
        let sync = |playback: &mut MultitrackPlayback, secs: f64| {
            playback
                .sync(
                    &ending_at(secs),
                    48_000.0,
                    &HashMap::new(),
                    1.0,
                    &mut spaces(),
                )
                .unwrap()
        };
        assert_eq!(
            end_mark(&sync(&mut playback, 7.0)),
            Some(vec![OscType::Long(336_000), OscType::Long(48_000)]),
            "an edit that moves the last region moves it"
        );
        assert_eq!(
            end_mark(&playback.set_end(End::At(2.5))),
            Some(vec![OscType::Long(120_000), OscType::Long(48_000)]),
            "an end marker is where it says"
        );
        // **A region placed past an end marker takes the marker with it**: one
        // at or past the contents' end is carried forward when they grow past
        // it, and one put inside them is an early stop that stays.
        sync(&mut playback, 9.0);
        assert_eq!(playback.end(), End::At(2.5), "inside: it stays");
        playback.set_end(End::At(10.0));
        sync(&mut playback, 9.5);
        assert_eq!(playback.end(), End::At(10.0), "not reached");
        assert_eq!(
            end_mark(&sync(&mut playback, 12.0)),
            Some(vec![OscType::Long(576_000), OscType::Long(48_000)]),
            "passed: the marker is where the contents end"
        );
        assert_eq!(playback.end(), End::At(12.0));
        sync(&mut playback, 6.0);
        assert_eq!(playback.end(), End::At(12.0), "shrinking leaves it");
        assert_eq!(
            end_mark(&playback.set_end(End::Open)),
            Some(vec![]),
            "open clears it"
        );
        assert_eq!(End::from_json(&json!("contents")), Some(End::Contents));
        assert_eq!(End::from_json(&json!(3.0)), Some(End::At(3.0)));
        assert_eq!(End::from_json(&Value::Null), Some(End::Open));
        assert_eq!(End::from_json(&json!("nope")), None);
    }

    /// **The space bar's pass over a selection** starts at it, ends at its
    /// end and goes back to the position cursor; the loop switch loops the
    /// selection, or with none the whole contents, whatever the end says; and
    /// a pass with neither puts back the end asked for.
    #[test]
    fn a_pass_over_a_range_ends_there_and_the_loop_switch_loops_it() {
        let mut playback = synced(5.0);
        let range = View {
            selection: Some((48_000, 96_000)),
            cursor: Some(24_000),
        };
        let steps = playback.play(false, range);
        assert_eq!(
            args_of(&steps, "/transport_end"),
            Some(vec![OscType::Long(96_000), OscType::Long(24_000)])
        );
        assert_eq!(
            args_of(&steps, "/transport_locateSample"),
            Some(vec![OscType::Long(48_000)])
        );
        assert_eq!(
            addrs(&steps).last().map(String::as_str),
            Some("/transport_play")
        );
        let looped = playback.play(true, at(0.5));
        assert_eq!(
            args_of(&looped, "/transport_loop"),
            Some(vec![OscType::Long(0), OscType::Long(240_000)]),
            "the whole contents, though a pass runs on"
        );
        let plain = playback.play(false, at(0.5));
        assert_eq!(args_of(&plain, "/transport_loop"), Some(vec![]));
        assert_eq!(
            args_of(&plain, "/transport_end"),
            Some(vec![]),
            "the selection's end was for its pass: open again"
        );
    }

    /// **The loop switch changes the pass in progress**: on, it loops the
    /// selection or the whole contents from where it stands; off, it ends at
    /// the selection's end and goes back to the cursor; stopped, nothing.
    #[test]
    fn the_loop_switch_changes_the_pass_in_progress() {
        let mut playback = synced(5.0);
        let range = View {
            selection: Some((48_000, 96_000)),
            cursor: Some(24_000),
        };
        assert!(playback.set_pass(true, range).is_empty(), "nothing playing");
        playback.play(false, range);
        let on = playback.set_pass(true, range);
        assert_eq!(
            args_of(&on, "/transport_loop"),
            Some(vec![OscType::Long(48_000), OscType::Long(96_000)])
        );
        assert!(
            args_of(&on, "/transport_play").is_none(),
            "nothing restarted"
        );
        let off = playback.set_pass(false, range);
        assert_eq!(args_of(&off, "/transport_loop"), Some(vec![]));
        assert_eq!(
            args_of(&off, "/transport_end"),
            Some(vec![OscType::Long(96_000), OscType::Long(24_000)])
        );
        let whole = playback.set_pass(true, at(0.5));
        assert_eq!(
            args_of(&whole, "/transport_loop"),
            Some(vec![OscType::Long(0), OscType::Long(240_000)])
        );
    }

    /// **The cursor moves the music where it plays**: placed while it rolls,
    /// the head goes there and the pass ends as the view says from there; a
    /// selection redrawn while it loops is the loop, and the head goes into
    /// it when the sweep is let go.
    #[test]
    fn a_cursor_placed_while_it_plays_moves_the_music() {
        let mut playback = synced(5.0);
        assert_eq!(
            addrs(&playback.place(48_000, at(1.0))),
            ["/transport_locateSample"],
            "stopped: it stands there"
        );
        playback.play(false, at(1.0));
        let placed = playback.place(96_000, at(2.0));
        assert_eq!(
            args_of(&placed, "/transport_locateSample"),
            Some(vec![OscType::Long(96_000)])
        );
        assert!(args_of(&placed, "/transport_play").is_none());
        assert!(
            playback.follow(at(2.0), true).is_empty(),
            "the pass does not loop"
        );
        playback.play(true, at(2.0));
        let swept = View {
            selection: Some((48_000, 72_000)),
            cursor: Some(96_000),
        };
        let drawn = playback.follow(swept, false);
        assert_eq!(
            args_of(&drawn, "/transport_loop"),
            Some(vec![OscType::Long(48_000), OscType::Long(72_000)])
        );
        assert!(args_of(&drawn, "/transport_locateSample").is_none());
        let let_go = playback.follow(swept, true);
        assert_eq!(
            args_of(&let_go, "/transport_locateSample"),
            Some(vec![OscType::Long(48_000)])
        );
    }

    /// **A locate is the second's sample**, and a tempo in the multitrack
    /// changes nothing about it: the tempo map only draws a ruler.
    #[test]
    fn a_locate_is_the_seconds_sample_whatever_the_tempo() {
        let mut playback = MultitrackPlayback::new(Endpoint::default());
        playback
            .sync(&multitrack(), 48_000.0, &HashMap::new(), 1.0, &mut spaces())
            .unwrap();
        assert_eq!(playback.secs_to_samples(2.0), 96_000);
        let steps = playback.locate(96_000);
        assert_eq!(
            steps[0],
            send(
                "/transport_locateSample",
                vec![OscType::Int(MULTITRACK_TRANSPORT), OscType::Long(96_000)]
            )
        );
        assert!(
            matches!(steps[1], Step::AwaitDone { .. }),
            "and waits for it"
        );

        let mut faster = multitrack();
        faster.tempo = vec![Tempo::at(Beat(0.0), 2.0)];
        playback
            .sync(&faster, 48_000.0, &HashMap::new(), 1.0, &mut spaces())
            .unwrap();
        assert_eq!(playback.secs_to_samples(2.0), 96_000, "no tempo moves it");
        assert_eq!(playback.samples_to_secs(48_000), 1.0);
    }

    /// **A cue moves a stopped transport and leaves a rolling one alone**, a
    /// stop goes back to the cursor, and a play after a pause resumes.
    #[test]
    fn a_cue_waits_for_a_stopped_transport_and_a_stop_returns_to_the_mark() {
        let mut playback = MultitrackPlayback::new(Endpoint::default());
        assert_eq!(addrs(&playback.cue(48_000)), ["/transport_locateSample"]);
        assert_eq!(
            addrs(&playback.play(false, View::default())),
            [
                "/transport_loop",
                "/transport_end",
                "/transport_locateSample",
                "/transport_play"
            ]
        );
        assert!(
            playback.cue(48_000).is_empty(),
            "rolling: the mark moves, not the music"
        );
        playback.pause();
        assert!(playback.paused());
        assert_eq!(addrs(&playback.resume()), ["/transport_play"]);
        assert_eq!(
            addrs(&playback.stop(144_000)),
            ["/transport_stop", "/transport_locateSample"]
        );
        assert!(!playback.rolling());
        assert!(!playback.paused(), "a stop is not a pause");
        playback.set_rolling(true);
        assert!(playback.cue(48_000).is_empty(), "another client rolled it");
    }

    /// **The boxes over sequences play from one lane in the tracks' group**,
    /// made the first time there are notes and sent its data after that; a
    /// close frees it.
    #[test]
    fn the_notes_are_a_lane_in_the_tracks_group() {
        let mut playback = MultitrackPlayback::new(Endpoint::default());
        let mut ids = spaces();
        playback
            .sync(&multitrack(), 48_000.0, &HashMap::new(), 1.0, &mut ids)
            .unwrap();
        assert!(
            playback
                .notes(&Placement::default(), &mut ids)
                .unwrap()
                .is_empty(),
            "no notes, no lane"
        );
        let note = crate::notes_playback::Placed {
            start: 1.0,
            end: 1.5,
            keys: serde_json::from_value(json!({"midinote": 60})).unwrap(),
            id: String::new(),
            scope: String::new(),
            curves: Vec::new(),
        };
        let placement = Placement {
            events: vec![note],
            curves: Vec::new(),
        };
        let steps = playback.notes(&placement, &mut ids).unwrap();
        assert_eq!(addrs(&steps), ["/lane_new", "/lane_set"]);
        let group = playback.applier.node(crate::instance::TRACKS).unwrap();
        let around = playback.applier.node(crate::instance::TRANSPORT).unwrap();
        let Step::Send(made) = &steps[0] else {
            panic!("a send");
        };
        assert_eq!(
            made.args,
            vec![
                OscType::Int(MULTITRACK_TRANSPORT),
                OscType::Int(group),
                OscType::Int(around)
            ],
            "named by the tracks' group, its notes made where a stop does not freeze them"
        );
        let again = playback.notes(&placement, &mut ids).unwrap();
        assert_eq!(addrs(&again), ["/lane_set"], "made once");
        let closed = addrs(&playback.close(&mut ids).unwrap());
        assert_eq!(closed[0], "/lane_free");
    }

    /// **A box of notes sounds into its track.** The notes say which box they
    /// are of; the box becomes a clip of its track over a bus of its own, the
    /// lane's notes are made in the voices' group before the multitrack, and
    /// each note's `out` is its box's bus -- whatever its own keys said. A
    /// note of no box is left as it was.
    #[test]
    fn a_box_s_notes_sound_into_its_track() {
        let mut playback = MultitrackPlayback::new(Endpoint::default());
        let mut ids = spaces();
        let multitrack = ending_at(4.0);
        playback
            .sync(&multitrack, 48_000.0, &HashMap::new(), 1.0, &mut ids)
            .unwrap();
        let note = |scope: &str, keys: Value| crate::notes_playback::Placed {
            start: 1.0,
            end: 1.5,
            keys: serde_json::from_value(keys).unwrap(),
            id: String::new(),
            scope: scope.into(),
            curves: Vec::new(),
        };
        let placement = Placement {
            events: vec![
                note("20", json!({"midinote": 60, "out": 0})),
                note("", json!({"midinote": 62})),
            ],
            curves: Vec::new(),
        };
        let steps = playback.notes(&placement, &mut ids).unwrap();
        let (bus, _) = playback
            .applier
            .bus(&crate::instance::voice_bus_handle(20))
            .expect("the box has a bus");
        let voices = playback
            .applier
            .node(crate::instance::VOICES)
            .expect("the voices' group");
        let slot = steps
            .iter()
            .find_map(|step| match step {
                Step::Send(m)
                    if m.addr == "/graph_addSlot"
                        && m.args.get(1)
                            == Some(&OscType::String(clausters_core::mixer::VOICE_SLOT.into())) =>
                {
                    Some(m.args.clone())
                }
                _ => None,
            })
            .expect("the box is a slot of its track");
        let track = playback.applier.node("track:10").unwrap();
        assert_eq!(slot[0], OscType::Int(track));
        assert!(
            slot.windows(2)
                .any(|pair| pair == [OscType::String("in0".into()), OscType::Float(bus as f32)]),
            "over its bus: {slot:?}"
        );
        let made = steps
            .iter()
            .find_map(|step| match step {
                Step::Send(m) if m.addr == "/lane_new" => Some(m.args.clone()),
                _ => None,
            })
            .expect("the lane");
        assert_eq!(
            made[2],
            OscType::Int(voices),
            "its notes are made before the multitrack"
        );
        let data: Value = steps
            .iter()
            .find_map(|step| match step {
                Step::Send(m) if m.addr == "/lane_set" => match &m.args[1] {
                    OscType::String(data) => serde_json::from_str(data).ok(),
                    _ => None,
                },
                _ => None,
            })
            .expect("the lane's data");
        let notes = data["notes"].as_array().unwrap();
        assert_eq!(
            notes[0][3]["out"],
            json!(f64::from(bus)),
            "the box's note sounds into the box, not where its key said"
        );
        assert!(
            notes[1][3].get("out").is_none(),
            "a note of no box is left alone"
        );

        // The same notes again: nothing of the box is made twice.
        let again = playback.notes(&placement, &mut ids).unwrap();
        assert_eq!(addrs(&again), ["/lane_set"]);
        // And a sync after it plans the box itself.
        let synced = playback
            .sync(&multitrack, 48_000.0, &HashMap::new(), 1.0, &mut ids)
            .unwrap();
        assert!(
            !addrs(&synced).iter().any(|a| a == "/node_free"),
            "the box stays: {synced:?}"
        );

        // No notes: the box is no clip any more, and its bus is given back.
        let cleared = playback.notes(&Placement::default(), &mut ids).unwrap();
        assert!(addrs(&cleared).iter().any(|a| a == "/node_free"));
        assert!(
            playback
                .applier
                .bus(&crate::instance::voice_bus_handle(20))
                .is_none()
        );
    }

    /// The JSON doors answer steps with 64-bit samples, and an error for what
    /// is not a multitrack.
    #[test]
    fn the_json_doors_answer_steps_or_an_error() {
        let mut playback = MultitrackPlayback::new(Endpoint::default());
        let answer: Value =
            serde_json::from_str(&answer_json(Ok(playback.locate(48_000)))).unwrap();
        assert_eq!(answer["steps"][0]["send"]["args"][0], json!({"i": 0}));
        assert_eq!(answer["steps"][0]["send"]["args"][1], json!({"h": 48_000}));
        let refused: Value = serde_json::from_str(&sync_json(
            &mut playback,
            "no",
            48_000.0,
            "{}",
            1.0,
            &mut spaces(),
        ))
        .unwrap();
        assert!(refused["error"].is_string());
        assert_eq!(meters_json(&playback), "[]");
    }
}
