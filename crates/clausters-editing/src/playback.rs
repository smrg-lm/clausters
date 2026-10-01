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

use std::collections::HashMap;

use clausters_core::ids::{IdError, IdSpaces};
use clausters_core::osc::OscType;
use clausters_core::tempoclock::{samples_to_secs, secs_to_samples};
use clausters_document::SourceId;
use clausters_document::multitrack::Multitrack;
use clausters_document::multitrack::nodes::{self, SourceInfo};
use serde_json::{Value, json};

use crate::apply::{Applier, Endpoint, MULTITRACK_TRANSPORT, Step, send, steps_json};
use crate::instance::Instance;
use crate::note_curves::{self, NoteCurves};
use crate::notes_playback::Placement;

/// **Where a pass ends**, the same three ways for every playback on a
/// transport -- the multitrack's and the notes editor's.
///
/// - `Open`: it does not, and the transport rolls on past the contents until
///   it is stopped, as a multitrack is played to record onto or to hear a
///   tail. The default.
/// - `Contents`: where the contents end -- the last region, the last note's
///   end -- going back to the position cursor, as an audio editor's pass does.
/// - `At`: an **end marker**, at a place of the playback's own axis (seconds
///   of a multitrack, beats of a sequence), going back the same way.
///
/// What it sends is the transport's end mark, only when that moves; a loop
/// set on the transport wins over it. As JSON: `null`, `"contents"` or the
/// number.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum End {
    /// The transport rolls on.
    #[default]
    Open,
    /// Where the contents end.
    Contents,
    /// An end marker.
    At(f64),
}

impl End {
    /// The end a JSON value names, or `None` for one that names none.
    pub fn from_json(value: &Value) -> Option<End> {
        match value {
            Value::Null => Some(End::Open),
            Value::String(word) if word == "contents" => Some(End::Contents),
            Value::Number(n) => n.as_f64().map(|at| End::At(at.max(0.0))),
            _ => None,
        }
    }

    /// Its JSON form.
    pub fn to_json(self) -> Value {
        match self {
            End::Open => Value::Null,
            End::Contents => json!("contents"),
            End::At(at) => json!(at),
        }
    }

    /// Where a pass ends, given where the contents do, or `None` for one that
    /// rolls on -- and for contents that end nowhere, since a pass over
    /// nothing has no end to stop on.
    pub fn at(self, contents: f64) -> Option<f64> {
        match self {
            End::Open => None,
            End::Contents => (contents > 0.0).then_some(contents),
            End::At(at) => Some(at),
        }
    }
}

/// **A pass as JSON**, `{"range": [start, end] | null, "looping": bool}`: the
/// one reading every door of [`MultitrackPlayback::play_pass`] shares.
pub fn pass_of(json: &str) -> (Option<(f64, f64)>, bool) {
    let value: Value = serde_json::from_str(json).unwrap_or(Value::Null);
    let range = value
        .get("range")
        .and_then(Value::as_array)
        .and_then(|r| Some((r.first()?.as_f64()?, r.get(1)?.as_f64()?)));
    let looping = value
        .get("looping")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    (range, looping)
}

/// **One multitrack, as it is playing.**
#[derive(Debug, Clone)]
pub struct MultitrackPlayback {
    instance: Instance,
    applier: Applier,
    /// The rate the multitrack was last planned at.
    rate: f64,
    /// Whether the transport was last told to roll.
    rolling: bool,
    /// Where a pass ends ([`Self::set_end`]).
    end: End,
    /// Where the contents end, in seconds of the multitrack: the last region's
    /// end on any track and any lane, as of the last [`Self::sync`].
    content_end: f64,
    /// The position cursor, in seconds -- where a pass that stops at the end
    /// goes back to. Moved by [`Self::cue`] and [`Self::stop`].
    mark: f64,
    /// The end mark last sent, in samples, so it is sent only when it moves.
    end_sent: Option<(i64, i64)>,
    /// The transport's ramp last sent, in samples.
    fade_sent: Option<i64>,
    /// **The event lane the boxes over sequences play from**, once made: named
    /// by the tracks' group's id, its notes made in the transport's group
    /// around the multitrack, which a stop does not freeze.
    lane: Option<i32>,
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
        MultitrackPlayback {
            instance: Instance::new(),
            applier: Applier::new(endpoint),
            rate: 48_000.0,
            rolling: false,
            end: End::Open,
            content_end: 0.0,
            mark: 0.0,
            end_sent: None,
            fade_sent: None,
            lane: None,
            curves: NoteCurves::new("mt/notes"),
            passed: false,
        }
    }

    /// **What the boxes over sequences play**, as the event lane's data on the
    /// multitrack's transport: `placed` is `crate::multitrack::placed_notes`,
    /// in seconds of the multitrack, and the server plays it by the position --
    /// a locate, the loop and a stop are the transport's. The lane is made the
    /// first time there is something to play, in the tracks' group, and sent
    /// its data again on every call; a multitrack that never had notes makes
    /// nothing. Its notes sound through their own `out`, outside the tracks'
    /// strips, and a track's mute and solo decide what is placed at all.
    ///
    /// **The curves are heard through graphs** (`crate::note_curves`), made in
    /// the transport's group beside the notes, allocating from `ids`.
    pub fn notes(&mut self, placed: &Placement, ids: &mut IdSpaces) -> Result<Vec<Step>, IdError> {
        let Some(group) = self.applier.node(crate::instance::TRACKS) else {
            return Ok(Vec::new());
        };
        if placed.events.is_empty() && self.lane.is_none() {
            return Ok(Vec::new());
        }
        let mut steps = Vec::new();
        if self.lane != Some(group) {
            // A tracks' group made again is a new lane.
            steps.extend(self.free_lane());
            // Named by the tracks' group, its notes made in the transport's
            // group around the multitrack, which follows the transport and
            // is not frozen by a stop: a stop releases them, and their
            // releases ring out.
            let target = self
                .applier
                .node(crate::instance::TRANSPORT)
                .unwrap_or(group);
            steps.extend(transport_command(
                "/lane_new",
                vec![OscType::Int(group), OscType::Int(target)],
            ));
            self.lane = Some(group);
        }
        let plan = note_curves::plan(placed, self.rate).scoped(group);
        let ops = self.curves.ops(
            &plan,
            crate::instance::TRANSPORT,
            std::mem::take(&mut self.passed),
        );
        steps.extend(self.applier.apply(ops, ids)?);
        let slots = self.curves.slots(&plan, &self.applier);
        let data = crate::notes_playback::data(&placed.events, self.rate, &slots).to_string();
        steps.push(send(
            "/lane_set",
            vec![OscType::Int(group), OscType::String(data)],
        ));
        steps.push(Step::AwaitDone {
            command: "/lane_set".into(),
            index: None,
        });
        Ok(steps)
    }

    /// Frees the lane, when there is one: its notes are released.
    fn free_lane(&mut self) -> Vec<Step> {
        let Some(lane) = self.lane.take() else {
            return Vec::new();
        };
        vec![
            send("/lane_free", vec![OscType::Int(lane)]),
            Step::AwaitDone {
                command: "/lane_free".into(),
                index: None,
            },
        ]
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
        self.rate = rate;
        let plan = nodes::plan(multitrack, rate, sources);
        let ops = self.instance.reconcile(&plan, gain);
        let mut steps = self.applier.apply(ops, ids)?;
        // **The transport's ramp**, so a stop rolls the tracks out while the
        // master's way out fades them and a play fades them in: the master is
        // outside the governed group, and this is what it declicks across.
        let fade = secs_to_samples(crate::audio_playback::FADE_SECS, rate);
        if self.fade_sent != Some(fade) {
            self.fade_sent = Some(fade);
            steps.extend(transport_command(
                "/transport_fade",
                vec![OscType::Long(fade)],
            ));
        }
        // An edit that moves the last region moves where a pass stops.
        self.content_end = multitrack.end().0;
        steps.extend(self.end_steps());
        Ok(steps)
    }

    /// **Where a pass ends** ([`End`]): open by default, or where the last
    /// region ends on any track and any lane, or at an end marker in seconds
    /// -- going back to the position cursor.
    pub fn set_end(&mut self, end: End) -> Vec<Step> {
        self.end = end;
        self.end_steps()
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

    /// The end mark the end, the contents and the cursor ask for, sent when
    /// it differs from the one last sent.
    fn end_steps(&mut self) -> Vec<Step> {
        let want = self
            .end
            .at(self.content_end)
            .map(|end| (self.secs_to_samples(end), self.secs_to_samples(self.mark)));
        if want == self.end_sent {
            return Vec::new();
        }
        self.end_sent = want;
        transport_command(
            "/transport_end",
            want.map_or_else(Vec::new, |(end, back)| {
                vec![OscType::Long(end), OscType::Long(back)]
            }),
        )
    }

    /// **Rolls the transport**, or continues a paused pass: the engine keeps
    /// where it stopped, so resuming is the same verb as starting.
    pub fn play(&mut self) -> Vec<Step> {
        self.rolling = true;
        self.passed = true;
        transport_command("/transport_play", vec![])
    }

    /// **The loop switch changed while it plays**: the pass in progress now
    /// loops over `range` (`[start, end]` in seconds), or the whole
    /// multitrack with none -- or, switched off, goes on to the range's end,
    /// or to where [`End`] says, and back to the mark. From where the
    /// transport stands: nothing is located and nothing restarts. Stopped, it
    /// answers nothing, since the next play reads the switch.
    pub fn set_loop(&mut self, range: Option<(f64, f64)>, looping: bool) -> Vec<Step> {
        if !self.rolling {
            return Vec::new();
        }
        let span = range
            .or_else(|| (looping && self.content_end > 0.0).then_some((0.0, self.content_end)));
        if let (true, Some((from, to))) = (looping, span) {
            let mut steps = transport_command(
                "/transport_loop",
                vec![
                    OscType::Long(self.secs_to_samples(from)),
                    OscType::Long(self.secs_to_samples(to.max(from))),
                ],
            );
            // A loop does not stop on an end mark the pass began with.
            if self.end_sent.take().is_some() {
                steps.extend(transport_command("/transport_end", vec![]));
            }
            return steps;
        }
        let mut steps = transport_command("/transport_loop", vec![]);
        match range {
            Some((from, to)) => {
                let want = (
                    self.secs_to_samples(to.max(from)),
                    self.secs_to_samples(self.mark),
                );
                self.end_sent = Some(want);
                steps.extend(transport_command(
                    "/transport_end",
                    vec![OscType::Long(want.0), OscType::Long(want.1)],
                ));
            }
            None => steps.extend(self.end_steps()),
        }
        steps
    }

    /// **The space bar's play: the audio editor's pass over a multitrack.**
    ///
    /// With a time range (`[start, end]` in seconds) the pass starts at its
    /// start and ends at its end, going back to the mark; with the loop switch
    /// the transport loops the range -- or, with none, the whole multitrack --
    /// and with neither it plays from where the transport stands (the mark,
    /// after a stop) to wherever [`End`] says. A pass over a range sets the end
    /// mark for that pass alone: the next one without a range puts back the
    /// end that was asked for.
    pub fn play_pass(&mut self, range: Option<(f64, f64)>, looping: bool) -> Vec<Step> {
        let mut steps = Vec::new();
        let span = range
            .or_else(|| (looping && self.content_end > 0.0).then_some((0.0, self.content_end)));
        match (span, looping) {
            (Some((from, to)), true) => {
                steps.extend(transport_command(
                    "/transport_loop",
                    vec![
                        OscType::Long(self.secs_to_samples(from)),
                        OscType::Long(self.secs_to_samples(to.max(from))),
                    ],
                ));
                if range.is_some() {
                    steps.extend(self.locate(from));
                }
            }
            _ => {
                steps.extend(transport_command("/transport_loop", vec![]));
                match range {
                    Some((from, to)) => {
                        let want = (
                            self.secs_to_samples(to.max(from)),
                            self.secs_to_samples(self.mark),
                        );
                        self.end_sent = Some(want);
                        steps.extend(transport_command(
                            "/transport_end",
                            vec![OscType::Long(want.0), OscType::Long(want.1)],
                        ));
                        steps.extend(self.locate(from));
                    }
                    None => steps.extend(self.end_steps()),
                }
            }
        }
        steps.extend(self.play());
        steps
    }

    /// **Freezes the multitrack where it stands**, every node's state intact -- and
    /// zeroes its meters, because a frozen meter gets no time to fall and would
    /// go on claiming the last level it wrote. The mark goes with the level.
    pub fn pause(&mut self) -> Vec<Step> {
        self.rolling = false;
        let mut steps = transport_command("/transport_stop", vec![]);
        for (_, bus, channels) in self.meters() {
            steps.push(send(
                "/bus_fill",
                vec![
                    OscType::Int(bus),
                    OscType::Int(2 * channels as i32),
                    OscType::Float(0.0),
                ],
            ));
        }
        steps
    }

    /// **Halts and goes back to the mark**, not to the top: the next play
    /// starts from where the position cursor is.
    pub fn stop(&mut self, mark: f64) -> Vec<Step> {
        self.mark = mark;
        let mut steps = self.pause();
        steps.extend(self.locate(mark));
        steps.extend(self.end_steps());
        steps
    }

    /// **Puts the transport at `secs`** of the multitrack. The readers seek in
    /// the engine, so what is sounding carries on from there.
    pub fn locate(&mut self, secs: f64) -> Vec<Step> {
        transport_command(
            "/transport_locateSample",
            vec![OscType::Long(self.secs_to_samples(secs))],
        )
    }

    /// **The position cursor moved**: a stopped transport is cued there and a
    /// rolling one is left alone, because moving the mark mid-pass must not
    /// move the music.
    pub fn cue(&mut self, secs: f64) -> Vec<Step> {
        self.mark = secs;
        let mut steps = if self.rolling {
            Vec::new()
        } else {
            self.locate(secs)
        };
        // Where a pass that stops at the end goes back to is the cursor, so
        // the mark follows it -- mid-pass too, since it moves no music.
        steps.extend(self.end_steps());
        steps
    }

    /// Frees everything the multitrack made. The multitrack itself is untouched: what a
    /// playback holds is nodes, and nodes are not the document.
    pub fn close(&mut self, ids: &mut IdSpaces) -> Result<Vec<Step>, IdError> {
        self.rolling = false;
        self.fade_sent = None;
        let mut steps = self.free_lane();
        let mut ops = self.curves.teardown();
        ops.extend(self.instance.teardown());
        steps.extend(self.applier.apply(ops, ids)?);
        Ok(steps)
    }

    /// Says whether the transport is rolling, when the caller learned it from
    /// the engine rather than from this.
    pub fn set_rolling(&mut self, rolling: bool) {
        self.rolling = rolling;
    }

    /// Whether the transport was last told to roll.
    pub fn rolling(&self) -> bool {
        self.rolling
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

/// A transport command on the multitrack's transport.
fn transport_command(addr: &str, args: Vec<OscType>) -> Vec<Step> {
    crate::apply::transport_command(MULTITRACK_TRANSPORT, addr, args)
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
        use clausters_document::multitrack::{Content, Region};
        use clausters_document::{Lifetime, Second, SegmentRef, SegmentSource, SourceRef};

        let mut multitrack = multitrack();
        let window = SegmentRef {
            source: SegmentSource::Samples(SourceRef {
                source: SourceId(1),
                lifetime: Lifetime::Session,
                generation: 0,
                range: None,
            }),
            start: 0.0,
            duration: end,
        };
        multitrack.tracks[0].lanes[0].place(Region::new(
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

    /// **A pass is open by default**; at the contents, the end mark is where
    /// the last region ends and the return is the position cursor, sent only
    /// when one of the two moves; and an end marker is its own place.
    #[test]
    fn a_pass_ends_where_it_is_asked_to() {
        let mut playback = MultitrackPlayback::new(Endpoint::default());
        let steps = playback
            .sync(
                &ending_at(5.0),
                48_000.0,
                &HashMap::new(),
                1.0,
                &mut spaces(),
            )
            .unwrap();
        assert_eq!(end_mark(&steps), None, "off: nothing is marked");

        let on = playback.set_end(End::Contents);
        assert_eq!(
            end_mark(&on),
            Some(vec![OscType::Long(240_000), OscType::Long(0)]),
            "the last region's end, back to the cursor"
        );
        let cued = playback.cue(1.0);
        assert_eq!(
            end_mark(&cued),
            Some(vec![OscType::Long(240_000), OscType::Long(48_000)]),
            "the return follows the cursor"
        );
        let same = playback
            .sync(
                &ending_at(5.0),
                48_000.0,
                &HashMap::new(),
                1.0,
                &mut spaces(),
            )
            .unwrap();
        assert_eq!(end_mark(&same), None, "sent only when it moves");
        let longer = playback
            .sync(
                &ending_at(7.0),
                48_000.0,
                &HashMap::new(),
                1.0,
                &mut spaces(),
            )
            .unwrap();
        assert_eq!(
            end_mark(&longer),
            Some(vec![OscType::Long(336_000), OscType::Long(48_000)]),
            "an edit that moves the last region moves it"
        );
        assert_eq!(
            end_mark(&playback.set_end(End::At(2.5))),
            Some(vec![OscType::Long(120_000), OscType::Long(48_000)]),
            "an end marker is where it says"
        );
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

    /// **The space bar's pass over a range** ends at the range's end and goes
    /// back to the mark; the loop switch loops the range, or with none the
    /// whole multitrack; and a pass with neither puts back the end asked for.
    #[test]
    fn a_pass_over_a_range_ends_there_and_the_loop_switch_loops_it() {
        let args_of = |steps: &[Step], addr: &str| {
            steps.iter().find_map(|step| match step {
                Step::Send(m) if m.addr == addr => Some(m.args[1..].to_vec()),
                _ => None,
            })
        };
        let mut playback = MultitrackPlayback::new(Endpoint::default());
        playback
            .sync(
                &ending_at(5.0),
                48_000.0,
                &HashMap::new(),
                1.0,
                &mut spaces(),
            )
            .unwrap();
        playback.cue(0.5);
        let steps = playback.play_pass(Some((1.0, 2.0)), false);
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
        let looped = playback.play_pass(None, true);
        assert_eq!(
            args_of(&looped, "/transport_loop"),
            Some(vec![OscType::Long(0), OscType::Long(240_000)]),
            "the whole multitrack"
        );
        let plain = playback.play_pass(None, false);
        assert_eq!(args_of(&plain, "/transport_loop"), Some(vec![]));
        assert_eq!(
            args_of(&plain, "/transport_end"),
            Some(vec![]),
            "the range's end mark was for its pass: open again"
        );
    }

    /// **The loop switch changes the pass in progress**: on, it loops the
    /// range or the whole multitrack from where it stands; off, it ends at the
    /// range's end and goes back to the mark; stopped, nothing.
    #[test]
    fn the_loop_switch_changes_the_pass_in_progress() {
        let args_of = |steps: &[Step], addr: &str| {
            steps.iter().find_map(|step| match step {
                Step::Send(m) if m.addr == addr => Some(m.args[1..].to_vec()),
                _ => None,
            })
        };
        let mut playback = MultitrackPlayback::new(Endpoint::default());
        playback
            .sync(
                &ending_at(5.0),
                48_000.0,
                &HashMap::new(),
                1.0,
                &mut spaces(),
            )
            .unwrap();
        assert!(playback.set_loop(None, true).is_empty(), "nothing playing");
        playback.cue(0.5);
        playback.play_pass(Some((1.0, 2.0)), false);
        let on = playback.set_loop(Some((1.0, 2.0)), true);
        assert_eq!(
            args_of(&on, "/transport_loop"),
            Some(vec![OscType::Long(48_000), OscType::Long(96_000)])
        );
        assert!(
            args_of(&on, "/transport_play").is_none(),
            "nothing restarted"
        );
        let off = playback.set_loop(Some((1.0, 2.0)), false);
        assert_eq!(args_of(&off, "/transport_loop"), Some(vec![]));
        assert_eq!(
            args_of(&off, "/transport_end"),
            Some(vec![OscType::Long(96_000), OscType::Long(24_000)])
        );
        let whole = playback.set_loop(None, true);
        assert_eq!(
            args_of(&whole, "/transport_loop"),
            Some(vec![OscType::Long(0), OscType::Long(240_000)])
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
        let steps = playback.locate(2.0);
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

    /// **A cue moves a stopped transport and leaves a rolling one alone**, and
    /// a stop goes back to the mark.
    #[test]
    fn a_cue_waits_for_a_stopped_transport_and_a_stop_returns_to_the_mark() {
        let mut playback = MultitrackPlayback::new(Endpoint::default());
        assert_eq!(addrs(&playback.cue(1.0)), ["/transport_locateSample"]);
        assert_eq!(addrs(&playback.play()), ["/transport_play"]);
        assert!(
            playback.cue(1.0).is_empty(),
            "rolling: the mark moves, not the music"
        );
        assert_eq!(
            addrs(&playback.stop(3.0)),
            ["/transport_stop", "/transport_locateSample"]
        );
        assert!(!playback.rolling());
        playback.set_rolling(true);
        assert!(playback.cue(1.0).is_empty(), "another client rolled it");
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

    /// The JSON doors answer steps with 64-bit samples, and an error for what
    /// is not a multitrack.
    #[test]
    fn the_json_doors_answer_steps_or_an_error() {
        let mut playback = MultitrackPlayback::new(Endpoint::default());
        let answer: Value = serde_json::from_str(&answer_json(Ok(playback.locate(1.0)))).unwrap();
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
