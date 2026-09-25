//! **How the audio editor is played**: its nodes, its transport, and the
//! steps that carry every verb out.
//!
//! The multitrack's is [`crate::playback`]; this is the audio editor's, built
//! the same way and over its own defs ([`clausters_core::audio_editor`]). It
//! sends nothing: every verb answers [`Step`]s, and the caller -- a client's
//! editor, or the GUI host -- sends them and waits where they say.
//!
//! # One structure, one file in focus
//!
//! An audio editor works on several files at once and plays one of them. So
//! there is one structure per playback: an editor group that **follows** the
//! editor's transport, holding a group the transport **governs** and the
//! output after it. Every open file is one `ae.play` instance in the governed
//! group, all writing the editor's bus, and **every one but the file in focus
//! is paused** (`/node_run 0`): the transport's freeze is the governed group's
//! own flag and a child's is its own, so a thaw does not wake a paused file.
//! Switching files pauses one instance and runs another; nothing is rebuilt.
//!
//! # The transport is the editor's own
//!
//! [`AUDIO_EDITOR_TRANSPORT`], not the multitrack's: playing a take never moves
//! a multitrack's position. It is given a ramp ([`FADE_SECS`]), so a stop rolls
//! out while the output's declick falls to zero and a play rises from it, and
//! neither edge clicks.
//!
//! # Frames and samples
//!
//! A file is addressed in **its own frames** -- a cursor, a selection, the end
//! of the take -- and the transport in **engine samples**: the two are one
//! number only while the take was recorded at the engine's rate. The reader
//! scales its phase by `BufRateScale`, and every position this sends is the
//! frame at the engine's rate, so a 44.1 kHz take in a 48 kHz session plays at
//! its pitch and ends where it ends.

use std::collections::{BTreeMap, BTreeSet};

use clausters_core::audio_editor as ae;
use clausters_core::ids::{IdError, IdSpaces};
use clausters_core::osc::OscType;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::apply::{Applier, Endpoint, Step};
use crate::instance::{Op, Port, Ports};

/// The transport the audio editor plays on: its own, so a take played never
/// moves a multitrack (which plays on transport 0).
pub const AUDIO_EDITOR_TRANSPORT: i32 = 1;

/// How long a stop and a play ramp, in seconds: short enough that nobody hears
/// it as a fade, long enough that nobody hears an edge as a click.
pub const FADE_SECS: f64 = 0.005;

const EDITOR: &str = "editor";
const GOVERNED: &str = "editor/transport";
const OUTPUT: &str = "editor/output";
const BUS: &str = "editor/bus";
const METER: &str = "editor/meter";

fn play_handle(file: u64) -> String {
    format!("play:{file}")
}

fn reader_handle(file: u64, channel: usize) -> String {
    format!("reader:{file}:{channel}")
}

/// **How a pass over a take ends**: it loops over a span, or it runs until an
/// end and goes back. Frames of the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Pass {
    /// Over and over, inside the half-open span.
    Loop {
        /// The first frame of the span.
        from: u64,
        /// The first frame past it.
        to: u64,
    },
    /// Until `end`, where the transport stops and is located to `back`.
    Until {
        /// The frame it stops on.
        end: u64,
        /// The frame it goes back to.
        back: u64,
    },
}

/// **What the space bar plays over a take**, read off the view: where the
/// pass starts and how it ends.
///
/// A selection plays from its start, and with none the take plays from the
/// position cursor, or from its first frame where none is placed. Looping, a
/// selection repeats and so does a take with none. Not looping, the pass stops
/// at the end of the selection or of the take and goes back to the position
/// cursor -- where a selection's play started, when there is no cursor apart
/// from it.
pub fn space(
    looping: bool,
    selection: Option<(u64, u64)>,
    cursor: Option<u64>,
    frames: u64,
) -> (u64, Pass) {
    let start = selection.map_or(cursor.unwrap_or(0), |(from, _)| from);
    let pass = if looping {
        let (from, to) = selection.unwrap_or((0, frames));
        Pass::Loop { from, to }
    } else {
        Pass::Until {
            end: selection.map_or(frames, |(_, to)| to),
            back: cursor.unwrap_or(start),
        }
    };
    (start, pass)
}

/// One open file, as it is sounding.
#[derive(Clone, Debug, PartialEq)]
struct File {
    channels: usize,
    buffer: i32,
    frames: u64,
    /// The take's own rate.
    rate: f64,
}

/// **The audio editor, as it is playing.**
#[derive(Debug, Clone)]
pub struct AudioEditorPlayback {
    applier: Applier,
    transport: i32,
    /// The engine's rate, as last synced.
    rate: f64,
    /// The editor bus's width, `0` while nothing is built.
    width: usize,
    /// The defs already sent, by name.
    sent: BTreeSet<String>,
    files: BTreeMap<u64, File>,
    /// The file that plays: every other one is paused.
    focus: Option<u64>,
    /// Whether the transport was last told to roll.
    rolling: bool,
}

impl AudioEditorPlayback {
    /// A playback that has made nothing yet, on transport `transport`,
    /// carrying out its steps for `endpoint`.
    pub fn new(endpoint: Endpoint, transport: i32) -> AudioEditorPlayback {
        AudioEditorPlayback {
            applier: Applier::new(endpoint),
            transport,
            rate: 48_000.0,
            width: 0,
            sent: BTreeSet::new(),
            files: BTreeMap::new(),
            focus: None,
            rolling: false,
        }
    }

    /// The transport it plays on -- what its commands name, and what a view
    /// drawing its play cursor reads.
    pub fn transport(&self) -> i32 {
        self.transport
    }

    /// **Makes file `file` sound as it now is**: `buffer`, holding `frames`
    /// frames of `channels` channels at `take_rate`, on an engine running at
    /// `rate`. The first call builds the editor's structure; a later one sets
    /// only what moved -- a new join is a new buffer and a new length, which
    /// reach the readers already there, so an edit while it plays is heard at
    /// once and nothing restarts. A file of another width is made again.
    ///
    /// A file that is not in focus is made paused; the first file is the
    /// focus.
    #[allow(clippy::too_many_arguments)]
    pub fn sync(
        &mut self,
        file: u64,
        buffer: i32,
        channels: usize,
        frames: u64,
        take_rate: f64,
        rate: f64,
        ids: &mut IdSpaces,
    ) -> Result<Vec<Step>, IdError> {
        let channels = channels.clamp(1, ae::MAX_CHANNELS);
        if rate > 0.0 {
            self.rate = rate;
        }
        let take_rate = if take_rate > 0.0 {
            take_rate
        } else {
            self.rate
        };
        let mut steps = self.defs(channels);
        steps.extend(self.structure(ae::width(channels), ids)?);
        let now = File {
            channels,
            buffer,
            frames,
            rate: take_rate,
        };
        let mut ops = Vec::new();
        match self.files.get(&file) {
            Some(was) if was == &now => {}
            Some(was) if was.channels == channels => {
                let span = self.frames_to_samples(&now, frames) as f64;
                for channel in 0..channels {
                    ops.push(Op::Set {
                        handle: reader_handle(file, channel),
                        ports: ports(&[
                            (ae::BUF, Port::from(f64::from(buffer))),
                            (ae::SPAN, Port::from(span)),
                        ]),
                    });
                }
            }
            was => {
                if let Some(was) = was {
                    ops.push(self.free_file(file, was.channels));
                }
                self.focus.get_or_insert(file);
                ops.extend(self.make_file(file, &now));
            }
        }
        self.files.insert(file, now);
        steps.extend(self.applier.apply(ops, ids)?);
        Ok(steps)
    }

    /// **Frees what file `file` made**; the last file closed frees the
    /// editor's whole structure, so the node tree is the one before the first
    /// file opened.
    pub fn close_file(&mut self, file: u64, ids: &mut IdSpaces) -> Result<Vec<Step>, IdError> {
        let Some(channels) = self.files.get(&file).map(|f| f.channels) else {
            return Ok(Vec::new());
        };
        if self.files.len() == 1 {
            return self.close(ids);
        }
        self.files.remove(&file);
        let mut steps = Vec::new();
        if self.focus == Some(file) {
            self.focus = None;
            if self.rolling {
                steps.extend(self.pause());
            }
        }
        let op = self.free_file(file, channels);
        steps.extend(self.applier.apply(vec![op], ids)?);
        Ok(steps)
    }

    /// **Plays file `file` from frame `start`**, the pass ending as `pass`
    /// says: the file comes into focus (the one that was pauses), the loop or
    /// the end mark is set, the transport is located and rolls.
    pub fn play(&mut self, file: u64, start: u64, pass: Pass) -> Vec<Step> {
        let mut steps = self.focus_on(file);
        let Some(f) = self.files.get(&file).cloned() else {
            return steps;
        };
        let (looping, end) = match pass {
            Pass::Loop { from, to } => (
                vec![
                    OscType::Long(self.frames_to_samples(&f, from)),
                    OscType::Long(self.frames_to_samples(&f, to)),
                ],
                vec![],
            ),
            Pass::Until { end, back } => (
                vec![],
                vec![
                    OscType::Long(self.frames_to_samples(&f, end)),
                    OscType::Long(self.frames_to_samples(&f, back)),
                ],
            ),
        };
        steps.extend(self.command("/transport_loop", looping));
        steps.extend(self.command("/transport_end", end));
        steps.extend(self.command(
            "/transport_locateSample",
            vec![OscType::Long(self.frames_to_samples(&f, start))],
        ));
        steps.extend(self.resume());
        steps
    }

    /// Rolls the transport again from where it stands, the file in focus
    /// continuing where it paused.
    pub fn resume(&mut self) -> Vec<Step> {
        self.rolling = true;
        self.command("/transport_play", vec![])
    }

    /// **Pauses where it stands**: the stop rolls out its ramp, the output's
    /// declick falls to zero and the readers freeze with their state, so a
    /// resume continues. The meter falls on its own, since the output is not
    /// frozen.
    pub fn pause(&mut self) -> Vec<Step> {
        self.rolling = false;
        self.command("/transport_stop", vec![])
    }

    /// **Stops and goes back to `back`**, a frame of the file in focus: the
    /// next play starts from the position cursor, not from where the pass
    /// ended.
    pub fn stop(&mut self, back: u64) -> Vec<Step> {
        let mut steps = self.pause();
        steps.extend(self.locate(back));
        steps
    }

    /// **The position cursor moved to `frame`**: a stopped transport is
    /// located there, so the play cursor stands on it, and a rolling one is
    /// left alone -- moving the mark mid-pass must not move the music.
    pub fn cue(&mut self, frame: u64) -> Vec<Step> {
        if self.rolling {
            return Vec::new();
        }
        self.locate(frame)
    }

    /// Puts the transport at frame `frame` of the file in focus.
    pub fn locate(&mut self, frame: u64) -> Vec<Step> {
        let at = match self.focus.and_then(|f| self.files.get(&f)) {
            Some(f) => self.frames_to_samples(f, frame),
            None => frame as i64,
        };
        self.command("/transport_locateSample", vec![OscType::Long(at)])
    }

    /// **Loops the transport over frames `[from, to)` of the file in focus**,
    /// or passes straight on with `None` -- what a selection redrawn while the
    /// file plays asks, without moving where it is. The frames are converted
    /// as a locate's are, so a file at another rate than the engine's loops
    /// over the span drawn.
    pub fn set_loop(&mut self, span: Option<(u64, u64)>) -> Vec<Step> {
        let focused = self.focus.and_then(|f| self.files.get(&f));
        let at = |frame: u64| match focused {
            Some(f) => self.frames_to_samples(f, frame),
            None => frame as i64,
        };
        let args = match span {
            Some((from, to)) => vec![OscType::Long(at(from)), OscType::Long(at(to))],
            None => vec![],
        };
        self.command("/transport_loop", args)
    }

    /// Frees everything the editor made, and the transport's marks with it.
    pub fn close(&mut self, ids: &mut IdSpaces) -> Result<Vec<Step>, IdError> {
        if self.width == 0 {
            return Ok(Vec::new());
        }
        let mut steps = Vec::new();
        if self.rolling {
            steps.extend(self.pause());
        }
        steps.extend(self.command("/transport_loop", vec![]));
        steps.extend(self.command("/transport_end", vec![]));
        let mut forget = vec![GOVERNED.to_string(), OUTPUT.to_string()];
        for (&file, f) in &self.files {
            forget.push(play_handle(file));
            forget.extend((0..f.channels).map(|c| reader_handle(file, c)));
        }
        steps.extend(self.applier.apply(
            vec![
                Op::Free {
                    handle: EDITOR.into(),
                    forget,
                },
                Op::FreeBus { handle: BUS.into() },
                Op::FreeBus {
                    handle: METER.into(),
                },
            ],
            ids,
        )?);
        self.files.clear();
        self.focus = None;
        self.width = 0;
        Ok(steps)
    }

    /// Says whether the transport is rolling, when the caller learned it from
    /// the engine rather than from this -- a pass that ended on its mark.
    pub fn set_rolling(&mut self, rolling: bool) {
        self.rolling = rolling;
    }

    /// Whether the transport was last told to roll.
    pub fn rolling(&self) -> bool {
        self.rolling
    }

    /// The files it holds, in order.
    pub fn files(&self) -> Vec<u64> {
        self.files.keys().copied().collect()
    }

    /// The file that plays.
    pub fn focus(&self) -> Option<u64> {
        self.focus
    }

    /// **Where the level is read from**: the first of the control buses the
    /// meter writes and how many there are, one per channel of the editor's
    /// bus; `None` before anything is built.
    pub fn meters(&self) -> Option<(i32, usize)> {
        self.applier.bus(METER)
    }

    /// How many nodes it holds.
    pub fn node_count(&self) -> usize {
        self.applier.node_count()
    }

    /// Frame `frame` of `file` as a sample of the transport.
    fn frames_to_samples(&self, file: &File, frame: u64) -> i64 {
        (frame as f64 * self.rate / file.rate).round() as i64
    }

    /// The defs a file of `channels` needs that were not sent yet, and the
    /// barrier after them: the SynthDefs, then the graphs that name them.
    fn defs(&mut self, channels: usize) -> Vec<Step> {
        let Ok(defs) = ae::defs_for(channels) else {
            return Vec::new();
        };
        let mut ops = Vec::new();
        let synth = defs.synth.into_iter().map(|d| ("synth", d));
        let graph = defs.graph.into_iter().map(|d| ("graph", d));
        for (family, spec) in synth.chain(graph) {
            let name = spec["name"].as_str().unwrap_or_default().to_string();
            if self.sent.insert(name) {
                ops.push(Op::Def {
                    family: family.into(),
                    spec,
                });
            }
        }
        if !ops.is_empty() {
            ops.push(Op::Barrier);
        }
        self.apply_plain(ops)
    }

    /// Ops that allocate nothing -- a def, a barrier, a `/node_run` -- as
    /// steps.
    fn apply_plain(&mut self, ops: Vec<Op>) -> Vec<Step> {
        let mut none = IdSpaces::new(
            clausters_core::ids::ServerShape::DEFAULT,
            clausters_core::ids::IdShare::WHOLE,
        );
        self.applier.apply(ops, &mut none).unwrap_or_default()
    }

    /// **The editor's structure, at least `width` wide**: the group that
    /// follows the transport, the governed group inside it, the transport's
    /// ramp, the editor's bus and the meter's, and the output after the
    /// governed group. A file wider than the bus rebuilds the output over a
    /// wider one and points every file at it.
    fn structure(&mut self, width: usize, ids: &mut IdSpaces) -> Result<Vec<Step>, IdError> {
        if width <= self.width {
            return Ok(Vec::new());
        }
        let mut steps = Vec::new();
        if self.width == 0 {
            steps.extend(self.applier.apply(
                vec![
                    Op::Follow {
                        handle: EDITOR.into(),
                        transport: self.transport,
                    },
                    Op::Governed {
                        handle: GOVERNED.into(),
                        parent: EDITOR.into(),
                        transport: self.transport,
                    },
                ],
                ids,
            )?);
            let fade = (FADE_SECS * self.rate).round() as i64;
            steps.extend(self.command("/transport_fade", vec![OscType::Long(fade)]));
        } else {
            steps.extend(self.applier.apply(
                vec![
                    Op::Free {
                        handle: OUTPUT.into(),
                        forget: Vec::new(),
                    },
                    Op::FreeBus { handle: BUS.into() },
                    Op::FreeBus {
                        handle: METER.into(),
                    },
                ],
                ids,
            )?);
        }
        self.width = width;
        let mut output = Ports::new();
        for channel in 0..width {
            output.insert(ae::bus_port("in", channel), bus(BUS, channel));
            output.insert(ae::meter_port(channel), bus(METER, channel));
        }
        let mut ops = vec![
            Op::AudioBus {
                handle: BUS.into(),
                channels: width,
            },
            Op::Bus {
                handle: METER.into(),
                channels: width,
            },
            Op::Graph {
                handle: OUTPUT.into(),
                parent: EDITOR.into(),
                graph: ae::output_name(width),
                ports: output,
            },
        ];
        for (&file, f) in &self.files {
            ops.push(Op::Set {
                handle: play_handle(file),
                ports: out_ports(ae::width(f.channels)),
            });
        }
        steps.extend(self.applier.apply(ops, ids)?);
        Ok(steps)
    }

    /// The ops that make file `file`: its play graph in the governed group,
    /// one reader per channel, and paused unless it is the focus.
    fn make_file(&self, file: u64, f: &File) -> Vec<Op> {
        let handle = play_handle(file);
        let mut ops = vec![Op::Graph {
            handle: handle.clone(),
            parent: GOVERNED.into(),
            graph: ae::play_name(f.channels),
            ports: out_ports(ae::width(f.channels)),
        }];
        let span = self.frames_to_samples(f, f.frames) as f64;
        for channel in 0..f.channels {
            ops.push(Op::Slot {
                handle: reader_handle(file, channel),
                target: handle.clone(),
                slot: ae::SOURCE_SLOT.into(),
                ports: ports(&[
                    (ae::BUF, Port::from(f64::from(f.buffer))),
                    (ae::CHAN, Port::from(channel as f64)),
                    (ae::SPAN, Port::from(span)),
                ]),
            });
        }
        if self.focus != Some(file) {
            ops.push(Op::Run { handle, run: false });
        }
        ops
    }

    /// The op that frees file `file`'s play graph, and its readers with it.
    fn free_file(&self, file: u64, channels: usize) -> Op {
        Op::Free {
            handle: play_handle(file),
            forget: (0..channels).map(|c| reader_handle(file, c)).collect(),
        }
    }

    /// The steps that bring `file` into focus: the one in focus pauses and
    /// this one runs.
    fn focus_on(&mut self, file: u64) -> Vec<Step> {
        if self.focus == Some(file) || !self.files.contains_key(&file) {
            return Vec::new();
        }
        let mut ops = Vec::new();
        if let Some(was) = self.focus.replace(file) {
            ops.push(Op::Run {
                handle: play_handle(was),
                run: false,
            });
        }
        ops.push(Op::Run {
            handle: play_handle(file),
            run: true,
        });
        self.apply_plain(ops)
    }

    /// A transport command on this playback's transport, awaited.
    fn command(&self, addr: &str, args: Vec<OscType>) -> Vec<Step> {
        crate::apply::transport_command(self.transport, addr, args)
    }
}

fn bus(handle: &str, offset: usize) -> Port {
    Port::Bus {
        bus: handle.into(),
        offset,
    }
}

fn ports(pairs: &[(&str, Port)]) -> Ports {
    pairs
        .iter()
        .map(|(name, port)| (name.to_string(), port.clone()))
        .collect()
}

/// The ports that point a play graph `outs` wide at the editor's bus.
fn out_ports(outs: usize) -> Ports {
    (0..outs)
        .map(|channel| (ae::bus_port("out", channel), bus(BUS, channel)))
        .collect()
}

/// **The playback's verbs as JSON**, one door for every binding: a request
/// `{"verb": ...}` answers `{"steps": [...]}`, a query's own object, or
/// `{"error": ...}`.
///
/// - `sync` -- `file`, `buffer`, `channels`, `frames`, `takeRate`, `rate`
/// - `closeFile` -- `file`
/// - `play` -- `file`, `start`, `pass` (`{"kind": "loop", "from", "to"}` or
///   `{"kind": "until", "end", "back"}`)
/// - `resume`, `pause`, `stop` (`back`), `locate` (`frame`), `cue`
///   (`frame`: a locate while stopped, nothing while rolling), `close`
/// - `setRolling` -- `rolling`
/// - `state` -- answers `{"transport", "rolling", "focus", "meters", "nodes"}`,
///   `meters` being `{"bus", "channels"}` or `null`
/// - `space` -- `looping`, `selection` (`[from, to]` or `null`), `cursor`,
///   `frames`: answers `{"start", "pass"}`
pub fn call_json(playback: &mut AudioEditorPlayback, request: &str, ids: &mut IdSpaces) -> String {
    let Ok(request) = serde_json::from_str::<Value>(request) else {
        return json!({"error": "not a request"}).to_string();
    };
    let int = |key: &str| request.get(key).and_then(Value::as_u64).unwrap_or(0);
    let answer = |steps: Result<Vec<Step>, IdError>| crate::playback::answer_json(steps);
    match request.get("verb").and_then(Value::as_str).unwrap_or("") {
        "sync" => answer(playback.sync(
            int("file"),
            int("buffer") as i32,
            int("channels") as usize,
            int("frames"),
            request.get("takeRate").and_then(Value::as_f64).unwrap_or(0.0),
            request.get("rate").and_then(Value::as_f64).unwrap_or(0.0),
            ids,
        )),
        "closeFile" => answer(playback.close_file(int("file"), ids)),
        "play" => match request
            .get("pass")
            .cloned()
            .map(serde_json::from_value::<Pass>)
        {
            Some(Ok(pass)) => answer(Ok(playback.play(int("file"), int("start"), pass))),
            _ => json!({"error": "a play needs a pass"}).to_string(),
        },
        "resume" => answer(Ok(playback.resume())),
        "pause" => answer(Ok(playback.pause())),
        "stop" => answer(Ok(playback.stop(int("back")))),
        "locate" => answer(Ok(playback.locate(int("frame")))),
        "cue" => answer(Ok(playback.cue(int("frame")))),
        "close" => answer(playback.close(ids)),
        "setRolling" => {
            playback.set_rolling(
                request
                    .get("rolling")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            );
            answer(Ok(Vec::new()))
        }
        "state" => json!({
            "transport": playback.transport(),
            "rolling": playback.rolling(),
            "focus": playback.focus(),
            "meters": playback.meters().map(|(bus, channels)| json!({"bus": bus, "channels": channels})),
            "nodes": playback.node_count(),
        })
        .to_string(),
        "space" => {
            let selection = request
                .get("selection")
                .and_then(Value::as_array)
                .and_then(|s| Some((s.first()?.as_u64()?, s.get(1)?.as_u64()?)));
            let (start, pass) = space(
                request
                    .get("looping")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                selection,
                request.get("cursor").and_then(Value::as_u64),
                int("frames"),
            );
            json!({"start": start, "pass": pass}).to_string()
        }
        other => json!({"error": format!("no verb {other:?}")}).to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clausters_core::ids::{IdShare, ServerShape};

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

    fn sent<'a>(steps: &'a [Step], addr: &str) -> Vec<&'a clausters_core::osc::OscMessage> {
        steps
            .iter()
            .filter_map(|step| match step {
                Step::Send(m) if m.addr == addr => Some(m),
                _ => None,
            })
            .collect()
    }

    /// **The first file builds the structure**: the defs, the group that
    /// follows the transport, the governed group inside it, the ramp, the
    /// output, the play graph and one reader per channel.
    #[test]
    fn the_first_file_builds_the_editors_structure() {
        let mut playback = AudioEditorPlayback::new(Endpoint::default(), AUDIO_EDITOR_TRANSPORT);
        let mut ids = spaces();
        let steps = playback
            .sync(7, 12, 2, 48_000, 48_000.0, 48_000.0, &mut ids)
            .unwrap();
        let order: Vec<String> = addrs(&steps)
            .into_iter()
            .filter(|a| a != "/def_send")
            .collect();
        assert_eq!(
            order,
            [
                "/group_new",
                "/transport_follow",
                "/group_new",
                "/transport_group",
                "/transport_fade",
                "/graph_new",
                "/graph_new",
                "/graph_addSlot",
                "/graph_addSlot",
            ]
        );
        let fade = sent(&steps, "/transport_fade")[0];
        assert_eq!(fade.args, vec![OscType::Int(1), OscType::Long(240)]);
        let (bus, channels) = playback.meters().expect("a meter");
        assert_eq!(channels, 2);
        assert!(bus >= 0);
        assert_eq!(playback.focus(), Some(7));
        let slot = sent(&steps, "/graph_addSlot")[1];
        assert!(
            slot.args.contains(&OscType::String("span".into()))
                && slot.args.contains(&OscType::Float(48_000.0)),
            "the reader's window is the take: {:?}",
            slot.args
        );
    }

    /// **An edit reaches the readers already there**: a new join is a new
    /// buffer and a new length, set, and nothing is rebuilt.
    #[test]
    fn an_edit_sets_the_readers_and_rebuilds_nothing() {
        let mut playback = AudioEditorPlayback::new(Endpoint::default(), 1);
        let mut ids = spaces();
        playback
            .sync(7, 12, 1, 100, 48_000.0, 48_000.0, &mut ids)
            .unwrap();
        let nodes = playback.node_count();
        let steps = playback
            .sync(7, 13, 1, 80, 48_000.0, 48_000.0, &mut ids)
            .unwrap();
        assert_eq!(addrs(&steps), ["/node_set"]);
        assert_eq!(playback.node_count(), nodes);
        let same = playback
            .sync(7, 13, 1, 80, 48_000.0, 48_000.0, &mut ids)
            .unwrap();
        assert!(same.is_empty(), "nothing moved, nothing sent");
    }

    /// **A second file is made paused, and playing it switches the focus**:
    /// the one in focus pauses and this one runs.
    #[test]
    fn one_file_plays_and_the_rest_are_paused() {
        let mut playback = AudioEditorPlayback::new(Endpoint::default(), 1);
        let mut ids = spaces();
        playback
            .sync(1, 10, 1, 100, 48_000.0, 48_000.0, &mut ids)
            .unwrap();
        let second = playback
            .sync(2, 11, 2, 100, 48_000.0, 48_000.0, &mut ids)
            .unwrap();
        let paused = sent(&second, "/node_run");
        assert_eq!(paused.len(), 1);
        assert_eq!(paused[0].args[1], OscType::Int(0));
        let played = playback.play(2, 0, Pass::Loop { from: 0, to: 50 });
        let runs = sent(&played, "/node_run");
        assert_eq!(runs.len(), 2, "one paused, one run");
        assert_eq!(playback.focus(), Some(2));
        assert_eq!(
            addrs(&played)[2..],
            [
                "/transport_loop",
                "/transport_end",
                "/transport_locateSample",
                "/transport_play"
            ]
        );
    }

    /// **Positions are the frame at the engine's rate**: a 44.1 kHz take in a
    /// 48 kHz engine ends where it ends.
    #[test]
    fn a_take_at_another_rate_is_located_at_the_engines_sample() {
        let mut playback = AudioEditorPlayback::new(Endpoint::default(), 1);
        let mut ids = spaces();
        playback
            .sync(1, 10, 1, 44_100, 44_100.0, 48_000.0, &mut ids)
            .unwrap();
        let steps = playback.play(
            1,
            0,
            Pass::Until {
                end: 44_100,
                back: 22_050,
            },
        );
        let end = sent(&steps, "/transport_end")[0];
        assert_eq!(
            end.args,
            vec![
                OscType::Int(1),
                OscType::Long(48_000),
                OscType::Long(24_000)
            ]
        );
    }

    /// **A loop set while playing is converted as a locate is**: a selection
    /// of a 44.1 kHz take in a 48 kHz engine loops over the span drawn, and
    /// `None` clears it.
    #[test]
    fn a_loop_set_while_playing_is_in_the_engines_samples() {
        let mut playback = AudioEditorPlayback::new(Endpoint::default(), 1);
        let mut ids = spaces();
        playback
            .sync(1, 10, 1, 44_100, 44_100.0, 48_000.0, &mut ids)
            .unwrap();
        playback.play(
            1,
            0,
            Pass::Until {
                end: 44_100,
                back: 0,
            },
        );
        let steps = playback.set_loop(Some((22_050, 44_100)));
        let set = sent(&steps, "/transport_loop")[0];
        assert_eq!(
            set.args,
            vec![
                OscType::Int(1),
                OscType::Long(24_000),
                OscType::Long(48_000)
            ]
        );
        let steps = playback.set_loop(None);
        assert_eq!(
            sent(&steps, "/transport_loop")[0].args,
            vec![OscType::Int(1)]
        );
    }

    /// **Closing the last file frees everything**, and nothing is held after.
    #[test]
    fn closing_the_last_file_frees_the_structure() {
        let mut playback = AudioEditorPlayback::new(Endpoint::default(), 1);
        let mut ids = spaces();
        playback
            .sync(1, 10, 1, 100, 48_000.0, 48_000.0, &mut ids)
            .unwrap();
        playback
            .sync(2, 11, 1, 100, 48_000.0, 48_000.0, &mut ids)
            .unwrap();
        let one = playback.close_file(1, &mut ids).unwrap();
        assert_eq!(addrs(&one), ["/node_free"]);
        let last = playback.close_file(2, &mut ids).unwrap();
        assert!(addrs(&last).contains(&"/node_free".to_string()));
        assert_eq!(playback.node_count(), 0);
        assert_eq!(playback.meters(), None);
    }

    /// **A cue moves a stopped transport and leaves a rolling one alone.**
    #[test]
    fn a_cue_locates_only_a_stopped_transport() {
        let mut playback = AudioEditorPlayback::new(Endpoint::default(), 1);
        let mut ids = spaces();
        playback
            .sync(1, 10, 1, 100, 48_000.0, 48_000.0, &mut ids)
            .unwrap();
        assert_eq!(addrs(&playback.cue(40)), ["/transport_locateSample"]);
        playback.resume();
        assert!(
            playback.cue(40).is_empty(),
            "rolling: the mark moves, not the music"
        );
    }

    /// **The space bar's pass, read off the view.**
    #[test]
    fn the_space_bar_plays_the_selection_or_from_the_cursor() {
        assert_eq!(
            space(false, None, Some(10), 100),
            (10, Pass::Until { end: 100, back: 10 })
        );
        assert_eq!(
            space(false, Some((20, 40)), None, 100),
            (20, Pass::Until { end: 40, back: 20 })
        );
        assert_eq!(
            space(true, None, None, 100),
            (0, Pass::Loop { from: 0, to: 100 })
        );
        assert_eq!(
            space(true, Some((20, 40)), Some(5), 100),
            (20, Pass::Loop { from: 20, to: 40 })
        );
    }

    /// The JSON door answers steps, the state, and an error for an unknown
    /// verb.
    #[test]
    fn the_json_door_answers() {
        let mut playback = AudioEditorPlayback::new(Endpoint::default(), 1);
        let mut ids = spaces();
        let synced: Value = serde_json::from_str(&call_json(
            &mut playback,
            r#"{"verb": "sync", "file": 3, "buffer": 4, "channels": 1, "frames": 10,
                "takeRate": 48000, "rate": 48000}"#,
            &mut ids,
        ))
        .unwrap();
        assert!(synced["steps"].is_array());
        let state: Value =
            serde_json::from_str(&call_json(&mut playback, r#"{"verb": "state"}"#, &mut ids))
                .unwrap();
        assert_eq!(state["focus"], 3);
        assert_eq!(state["meters"]["channels"], 2);
        let space: Value = serde_json::from_str(&call_json(
            &mut playback,
            r#"{"verb": "space", "looping": true, "selection": [2, 5], "frames": 10}"#,
            &mut ids,
        ))
        .unwrap();
        assert_eq!(space["pass"], json!({"kind": "loop", "from": 2, "to": 5}));
        let played: Value = serde_json::from_str(&call_json(
            &mut playback,
            r#"{"verb": "play", "file": 3, "start": 0, "pass": {"kind": "until", "end": 10, "back": 0}}"#,
            &mut ids,
        ))
        .unwrap();
        assert!(played["steps"].is_array());
        let unknown: Value =
            serde_json::from_str(&call_json(&mut playback, r#"{"verb": "no"}"#, &mut ids)).unwrap();
        assert!(unknown["error"].is_string());
    }
}
