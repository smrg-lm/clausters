//! **How a multitrack is played**: its instance, its applier and its transport, as
//! the steps that carry every verb out.
//!
//! [`crate::instance`] answers what has to change on a server for it to hold
//! what a multitrack says, and [`crate::apply`] what messages that is. What was left
//! over was *playing* it — which sample a second of the multitrack is when the
//! transport is located, what play, pause, stop and cue send, and that a paused
//! meter is zeroed — and it was written once in
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
//! with [`MultitrackPlayback::set_rolling`].

use std::collections::HashMap;

use clausters_core::ids::{IdError, IdSpaces};
use clausters_core::osc::{OscMessage, OscType};
use clausters_core::tempoclock::{samples_to_secs, secs_to_samples};
use clausters_document::SourceId;
use clausters_document::multitrack::Multitrack;
use clausters_document::multitrack::nodes::{self, SourceInfo};
use serde_json::{Value, json};

use crate::apply::{Applier, Endpoint, Step, steps_json};
use crate::instance::Instance;

/// **One multitrack, as it is playing.**
#[derive(Debug, Clone)]
pub struct MultitrackPlayback {
    instance: Instance,
    applier: Applier,
    /// The rate the multitrack was last planned at.
    rate: f64,
    /// Whether the transport was last told to roll.
    rolling: bool,
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
        }
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
        self.applier.apply(ops, ids)
    }

    /// **Rolls the transport**, or continues a paused pass: the engine keeps
    /// where it stopped, so resuming is the same verb as starting.
    pub fn play(&mut self) -> Vec<Step> {
        self.rolling = true;
        command("/transport_play", vec![])
    }

    /// **Freezes the multitrack where it stands**, every node's state intact — and
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

    /// **Puts the transport at `secs`** of the multitrack. The readers seek in
    /// the engine, so what is sounding carries on from there.
    pub fn locate(&mut self, secs: f64) -> Vec<Step> {
        command(
            "/transport_locateSample",
            vec![OscType::Long(self.secs_to_samples(secs))],
        )
    }

    /// **The position cursor moved**: a stopped transport is cued there and a
    /// rolling one is left alone, because moving the mark mid-pass must not
    /// move the music.
    pub fn cue(&mut self, secs: f64) -> Vec<Step> {
        if self.rolling {
            Vec::new()
        } else {
            self.locate(secs)
        }
    }

    /// Frees everything the multitrack made. The multitrack itself is untouched: what a
    /// playback holds is nodes, and nodes are not the document.
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

    /// Whether anything of the multitrack is made.
    pub fn is_sounding(&self) -> bool {
        self.instance.is_sounding()
    }

    /// **The transport's group**, once the multitrack has made it: where an
    /// endpoint puts anything else that has to follow the transport, as the GUI
    /// host's take monitor does.
    pub fn group(&self) -> Option<i32> {
        self.applier.node(crate::instance::TRANSPORT)
    }

    /// How many nodes the multitrack holds.
    pub fn node_count(&self) -> usize {
        self.applier.node_count()
    }

    /// **A second of the multitrack as a sample**, at the rate it was last
    /// planned at and with the core's seconds → samples rounding.
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
            send("/transport_locateSample", vec![OscType::Long(96_000)])
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

    /// The JSON doors answer steps with 64-bit samples, and an error for what
    /// is not a multitrack.
    #[test]
    fn the_json_doors_answer_steps_or_an_error() {
        let mut playback = MultitrackPlayback::new(Endpoint::default());
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
