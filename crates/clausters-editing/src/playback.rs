//! **How a piece is played**: its instance, its applier and its transport, as
//! the steps that carry every verb out.
//!
//! [`crate::instance`] answers what has to change on a server for it to hold
//! what a piece says, and [`crate::apply`] what messages that is. What was left
//! over was *playing* it — which tempo a piece that states none is read at,
//! which sample a beat is when the transport is located, what play, pause, stop
//! and cue send, and that a paused meter is zeroed — and it was written once in
//! each client's `Playback` and once more in the GUI host. A standalone host
//! and a script's editor are the same multitrack, so they are the same program:
//! this object, which every one of them holds.
//!
//! **It sends nothing.** Every verb answers [`Step`]s — a message, a `/done` the
//! rest waits for, a barrier — and the caller sends them and waits where they
//! say. That half is the only one a language, or a host's reply loop, owns.
//!
//! **The position is the engine's.** Every reader follows the transport, so a
//! locate is one `/transport_locateSample` and nothing is re-cued; what this
//! keeps is whether it last told the transport to roll, which is what a cue
//! asks, and a caller that learns otherwise (another client rolled it) says so
//! with [`PiecePlayback::set_rolling`].

use std::collections::HashMap;

use clausters_core::ids::{IdError, IdSpaces};
use clausters_core::osc::{OscMessage, OscType};
use clausters_core::tempoclock::{samples_to_secs, secs_to_samples};
use clausters_core::tempomap::TempoMap;
use clausters_document::SourceId;
use clausters_document::multitrack::Multitrack;
use clausters_document::multitrack::nodes::{self, SourceInfo};
use serde_json::{Value, json};

use crate::apply::{Applier, Endpoint, Step, steps_json};
use crate::instance::Instance;

/// **The tempo a piece that states none is played at**, in beats per minute.
///
/// The reader's default and not the document's: a piece that said no tempo did
/// not say one, and writing it into the format would be the crate deciding a
/// musical question. Every endpoint reads and draws a piece at this one.
pub const DEFAULT_BPM: f64 = 60.0;

/// **One piece, as it is playing.**
#[derive(Debug, Clone)]
pub struct PiecePlayback {
    instance: Instance,
    applier: Applier,
    /// The piece's beat → second map, as of the last [`PiecePlayback::sync`].
    tempo: TempoMap,
    /// The rate the piece was last planned at.
    rate: f64,
    /// Whether the transport was last told to roll.
    rolling: bool,
}

impl PiecePlayback {
    /// A playback that has made nothing yet, carrying out its steps for
    /// `endpoint`.
    pub fn new(endpoint: Endpoint) -> PiecePlayback {
        PiecePlayback {
            instance: Instance::new(),
            applier: Applier::new(endpoint),
            tempo: TempoMap::new(DEFAULT_BPM / 60.0),
            rate: 48_000.0,
            rolling: false,
        }
    }

    /// **Makes what sounds be what the piece says**: the plan at `rate`, the
    /// difference from what is made, as steps, allocating from `ids`. It runs
    /// on every change of the piece, whoever made it; a node that did not
    /// change costs nothing.
    pub fn sync(
        &mut self,
        piece: &Multitrack,
        rate: f64,
        sources: &HashMap<SourceId, SourceInfo>,
        gain: f32,
        ids: &mut IdSpaces,
    ) -> Result<Vec<Step>, IdError> {
        self.tempo = nodes::tempo_map(piece, DEFAULT_BPM);
        self.rate = rate;
        let plan = nodes::plan(piece, rate, DEFAULT_BPM, sources);
        let ops = self.instance.reconcile(&plan, gain);
        self.applier.apply(ops, ids)
    }

    /// **Rolls the transport**, or continues a paused pass: the engine keeps
    /// where it stopped, so resuming is the same verb as starting.
    pub fn play(&mut self) -> Vec<Step> {
        self.rolling = true;
        command("/transport_play", vec![])
    }

    /// **Freezes the piece where it stands**, every node's state intact — and
    /// zeroes its meters, because a frozen meter gets no time to fall and would
    /// go on claiming the last level it wrote. The mark goes with the level.
    pub fn pause(&mut self) -> Vec<Step> {
        self.rolling = false;
        let mut steps = command("/transport_stop", vec![]);
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
        let mut steps = self.pause();
        steps.extend(self.locate(mark));
        steps
    }

    /// **Puts the transport at `beat`**, through the piece's own tempo map. The
    /// readers seek in the engine, so what is sounding carries on from there.
    pub fn locate(&mut self, beat: f64) -> Vec<Step> {
        command(
            "/transport_locateSample",
            vec![OscType::Long(self.beats_to_samples(beat))],
        )
    }

    /// **The position cursor moved**: a stopped transport is cued there and a
    /// rolling one is left alone, because moving the mark mid-pass must not
    /// move the music.
    pub fn cue(&mut self, beat: f64) -> Vec<Step> {
        if self.rolling {
            Vec::new()
        } else {
            self.locate(beat)
        }
    }

    /// Frees everything the piece made. The piece itself is untouched: what a
    /// playback holds is nodes, and nodes are not the composition.
    pub fn close(&mut self, ids: &mut IdSpaces) -> Result<Vec<Step>, IdError> {
        self.rolling = false;
        let ops = self.instance.teardown();
        self.applier.apply(ops, ids)
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

    /// Whether anything of the piece is made.
    pub fn is_sounding(&self) -> bool {
        self.instance.is_sounding()
    }

    /// How many nodes the piece holds.
    pub fn node_count(&self) -> usize {
        self.applier.node_count()
    }

    /// **A beat as a sample of the piece**, through its tempo map and the
    /// core's seconds → samples rounding.
    pub fn beats_to_samples(&self, beat: f64) -> i64 {
        secs_to_samples(self.tempo.secs_at(beat.max(0.0)), self.rate)
    }

    /// A sample of the piece as a beat: the same map, read the other way.
    pub fn samples_to_beats(&self, samples: i64) -> f64 {
        self.tempo.beats_at(samples_to_secs(samples, self.rate))
    }

    /// The meters the piece writes, as `(track id, first bus, channels)`: a run
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

/// A command whose `/done` the rest waits for.
fn command(addr: &str, args: Vec<OscType>) -> Vec<Step> {
    vec![
        send(addr, args),
        Step::AwaitDone {
            command: addr.into(),
            index: None,
        },
    ]
}

fn send(addr: &str, args: Vec<OscType>) -> Step {
    Step::Send(OscMessage {
        addr: addr.into(),
        args,
    })
}

/// **Steps as the JSON a client is handed**: `{"steps": [...]}`, or
/// `{"error": "..."}` when an id space was exhausted.
pub fn answer_json(steps: Result<Vec<Step>, IdError>) -> String {
    match steps {
        Ok(steps) => json!({ "steps": steps_json(&steps) }).to_string(),
        Err(e) => json!({ "error": e.to_string() }).to_string(),
    }
}

/// [`PiecePlayback::sync`] over JSON: the piece as the document's own JSON and
/// the source table as [`crate::instance::sources_table`] reads it.
pub fn sync_json(
    playback: &mut PiecePlayback,
    piece: &str,
    rate: f64,
    sources: &str,
    gain: f32,
    ids: &mut IdSpaces,
) -> String {
    let piece = match serde_json::from_str::<Multitrack>(piece) {
        Ok(piece) => piece,
        Err(e) => return json!({ "error": format!("not a piece: {e}") }).to_string(),
    };
    let table = crate::instance::sources_table(sources);
    answer_json(playback.sync(&piece, rate, &table, gain, ids))
}

/// [`PiecePlayback::meters`] as JSON: `[{"track", "bus", "channels"}]`.
pub fn meters_json(playback: &PiecePlayback) -> String {
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

    fn piece() -> Multitrack {
        Multitrack {
            tracks: vec![Track::new(NodeId(10), NodeId(11))],
            ..Multitrack::default()
        }
    }

    /// **A piece that states no tempo is played at the one default**, and a
    /// locate is that tempo's sample.
    #[test]
    fn a_locate_is_the_beat_s_sample_at_the_piece_s_tempo() {
        let mut playback = PiecePlayback::new(Endpoint::default());
        playback
            .sync(&piece(), 48_000.0, &HashMap::new(), 1.0, &mut spaces())
            .unwrap();
        assert_eq!(
            playback.beats_to_samples(2.0),
            96_000,
            "60 bpm: a beat a second"
        );
        let steps = playback.locate(2.0);
        assert_eq!(
            steps[0],
            send("/transport_locateSample", vec![OscType::Long(96_000)])
        );
        assert!(
            matches!(steps[1], Step::AwaitDone { .. }),
            "and waits for it"
        );

        let mut faster = piece();
        faster.tempo = vec![Tempo {
            at: Beat(0.0),
            bpm: 120.0,
            ramp: false,
            extra: Default::default(),
        }];
        playback
            .sync(&faster, 48_000.0, &HashMap::new(), 1.0, &mut spaces())
            .unwrap();
        assert_eq!(
            playback.beats_to_samples(2.0),
            48_000,
            "the piece's own tempo"
        );
        assert_eq!(playback.samples_to_beats(48_000), 2.0);
    }

    /// **A cue moves a stopped transport and leaves a rolling one alone**, and
    /// a stop goes back to the mark.
    #[test]
    fn a_cue_waits_for_a_stopped_transport_and_a_stop_returns_to_the_mark() {
        let mut playback = PiecePlayback::new(Endpoint::default());
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

    /// The JSON doors answer steps with 64-bit samples, and an error for what
    /// is not a piece.
    #[test]
    fn the_json_doors_answer_steps_or_an_error() {
        let mut playback = PiecePlayback::new(Endpoint::default());
        let answer: Value = serde_json::from_str(&answer_json(Ok(playback.locate(1.0)))).unwrap();
        assert_eq!(answer["steps"][0]["send"]["args"][0], json!({"h": 48_000}));
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
