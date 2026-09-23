//! **The audio editor's turns**: the window over a take made of parts, what one
//! message from the host does to the list, and the steps that make the server
//! draw and play what the list now is.
//!
//! The shape is the samples editor's -- a message read by the conversation, a
//! gesture read in its vocabulary, an acknowledgement, and an entry for the
//! history the caller keeps -- with one difference in what comes back: not a
//! write into the take, but the **steps** that make a new take where a turn
//! needs one and stitch the drawn join over the new list ([`Outcome::steps`]).
//! Each entry states the takes its two lists read, so the history holds them
//! for as long as it can walk to them.

use std::collections::BTreeMap;
use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use clausters_core::osc::{OscMessage, OscType};
use clausters_core::tempoclock::samples_to_secs;
use clausters_document::clipboard::{Clipboard, Content};
use clausters_document::parts::{self, PARTS};
use clausters_document::session::{Location, Part, Source};
use clausters_document::view::NOT_AN_EDIT;
use clausters_document::{Lifetime, Opaque, Range, SourceId, SourceRef};
use clausters_editing::apply::{Step, steps_json};
use clausters_editing::conversation::{self, Answer, Conversation, Correction, Message, Turn};
use clausters_editing::load::stitch_message;
use clausters_editing::samples;
use clausters_editing::sources::{Held, stitch};

use crate::samples::{MEASURES, Window, measures, props, window};
use crate::turn::{Event, Kind, Leg, Record, int, number, text};

/// The most values one write carries when the caller has not said.
pub const DEFAULT_CHUNK: usize = 8192;

/// **What one turn came to.**
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    /// What kind of turn the message was.
    pub turn: Kind,
    /// What to send the host, `silent` for nothing.
    pub answer: Option<Answer>,
    /// The stamp a [`Kind::Step`] is answered with.
    pub seq: i64,
    /// Whether a [`Kind::Step`] walks forward.
    pub redo: bool,
    /// The entry to record.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record: Option<Record>,
    /// Whether the take changed.
    pub changed: bool,
    /// The version after the turn.
    pub version: i64,
    /// **The steps to carry out**, in the JSON a runner walks: a new take made
    /// where the turn needed one, then the drawn join stitched over the list.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub steps: Option<Value>,
    /// Where the position cursor was placed, in seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locate: Option<f64>,
    /// The selection a sweep left, in seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selection: Option<Value>,
}

/// **An audio editor**: the take it opened, the list of parts it is now, the
/// join that list is drawn from, and one view's end of the conversation.
#[derive(Clone, Debug)]
pub struct AudioEditor {
    /// The buffer the take was opened from. Never written, never freed here.
    take: i64,
    channels: u32,
    rate: f64,
    /// The join the window draws: the list, stitched.
    display: i64,
    list: Vec<Part>,
    /// Frames of every take this editor knows -- the one it opened and each it
    /// made -- by buffer.
    frames: BTreeMap<i64, u64>,
    /// Buffers the caller handed over for new takes, next first.
    spare: Vec<i64>,
    chunk: usize,
    name: Option<String>,
    layers: Vec<String>,
    title: String,
    size: (i64, i64),
    window: Option<i32>,
    widget: Option<i32>,
    conversation: Conversation,
}

/// A part reading frames `start..end` of `buffer`.
fn part(buffer: i64, start: u64, end: u64) -> Part {
    Part {
        source: SourceRef {
            source: SourceId(buffer as u64),
            lifetime: Lifetime::Temporary,
            generation: 0,
            range: Some(Range { start, end }),
        },
        fade_in: 0,
        fade_out: 0,
        channels: None,
    }
}

fn send(addr: &str, args: Vec<OscType>) -> Step {
    Step::Send(OscMessage {
        addr: addr.into(),
        args,
    })
}

impl AudioEditor {
    /// An editor over the take in `take` -- `frames` long, `channels` wide,
    /// at `rate` -- drawn through the join `display`, measured by `layers`,
    /// whose history is at `version`; or the reason the stack is refused.
    pub fn new(
        take: i64,
        frames: u64,
        channels: u32,
        rate: f64,
        display: i64,
        layers: &[String],
        version: i64,
    ) -> Result<Self, String> {
        let mut editor = Self {
            take,
            channels: channels.max(1),
            rate: if rate > 0.0 { rate } else { 48_000.0 },
            display,
            list: Vec::new(),
            frames: BTreeMap::new(),
            spare: Vec::new(),
            chunk: DEFAULT_CHUNK,
            name: None,
            layers: measures(layers)?,
            title: "Audio".into(),
            size: (1000, 520),
            window: None,
            widget: None,
            conversation: Conversation::new(version),
        };
        editor.reopen(take, frames);
        Ok(editor)
    }

    /// Starts over on the take in `take`: the list is the whole of it.
    fn reopen(&mut self, take: i64, frames: u64) {
        self.take = take;
        self.frames.insert(take, frames);
        self.list = if frames > 0 {
            vec![part(take, 0, frames)]
        } else {
            Vec::new()
        };
    }

    /// The list the take is now.
    pub fn list(&self) -> &[Part] {
        &self.list
    }

    /// How many frames the take is now.
    pub fn length(&self) -> u64 {
        parts::length(&self.list).unwrap_or(0)
    }

    /// What the picture measures, innermost last.
    pub fn layers(&self) -> &[String] {
        &self.layers
    }

    /// Measure the picture by `stack` instead, or say why not.
    pub fn set_layers(&mut self, stack: &[String]) -> Result<(), String> {
        self.layers = measures(stack)?;
        Ok(())
    }

    /// **The takes this editor cannot let go of**: the one it opened, which is
    /// the caller's, and every take the list reads now.
    pub fn rooted(&self) -> Vec<SourceId> {
        let mut out = parts::sources(&self.list);
        let opened = SourceId(self.take as u64);
        if !out.contains(&opened) {
            out.push(opened);
        }
        out
    }

    /// How many bytes the take in `buffer` holds, or `None` for one this editor
    /// never knew.
    pub fn bytes(&self, buffer: i64) -> Option<u64> {
        self.frames
            .get(&buffer)
            .map(|frames| frames * u64::from(self.channels) * 4)
    }

    /// Forgets a take the caller is about to free, so nothing here names it.
    pub fn forget(&mut self, buffer: i64) {
        if buffer != self.take {
            self.frames.remove(&buffer);
        }
    }

    /// **The steps that make the window's join what the list is** -- what
    /// opening the editor carries out first, and what every turn ends with.
    pub fn open(&self) -> Value {
        steps_json(&self.stitched())
    }

    /// The list, stitched into the display buffer, and the `/done` awaited.
    fn stitched(&self) -> Vec<Step> {
        let held: HashMap<SourceId, Held> = self
            .frames
            .iter()
            .map(|(buffer, frames)| {
                (
                    SourceId(*buffer as u64),
                    Held {
                        buffer: *buffer as i32,
                        channels: self.channels as usize,
                        frames: *frames,
                        rate: self.rate,
                    },
                )
            })
            .collect();
        let mut source =
            Source::volatile(Lifetime::Temporary).shaped(self.channels, self.length(), self.rate);
        source.location = Location::Segments {
            parts: self.list.clone(),
        };
        let Some(made) = stitch(&source, &held) else {
            return Vec::new();
        };
        let display = self.display as i32;
        vec![
            Step::Send(stitch_message(display, &made)),
            Step::AwaitDone {
                command: "/buffer_stitch".into(),
                index: Some(display),
            },
        ]
    }

    /// **The window**, numbered with the take widget's id, drawing the join.
    pub fn window(&mut self, widget: i32) -> Value {
        self.widget = Some(widget);
        window(&Window {
            buffer: self.display,
            channels: self.channels,
            name: self.name.as_deref(),
            layers: &self.layers,
            rate: self.rate,
            widget,
            title: &self.title,
            size: self.size,
        })
    }

    /// **A step of the history, applied**: the list a payload states becomes
    /// the take, and the answer is the steps that stitch it. Nothing when the
    /// payload is not a list.
    pub fn apply(&mut self, payload: &Value) -> Option<Value> {
        self.list = parts::read(&Opaque(payload.clone()))?;
        Some(steps_json(&self.stitched()))
    }

    /// **One message from the host**, read and answered.
    pub fn event(&mut self, event: &Event, version: i64) -> Outcome {
        let args = &event.args;
        let widget = args.first().map_or(0, int);
        let message = Message {
            addr: event.addr.clone(),
            argc: args.len(),
            widget,
            seq: args.get(1).map_or(0, int),
            against: args.get(2).map_or(0, int),
            owns: self.widget.is_some_and(|w| i64::from(w) == widget),
            tag: args.get(3).map(text).unwrap_or_default(),
            version,
            is_window: self.window.is_some()
                && (args.is_empty() || i64::from(self.window.unwrap_or_default()) == widget),
        };
        let mut out = Outcome {
            version,
            ..Outcome::default()
        };
        match self.conversation.read(&message) {
            Turn::Nothing => {}
            Turn::Closed => {
                out.turn = Kind::Closed;
                self.window = None;
            }
            Turn::Step { seq, redo } => {
                out.turn = Kind::Step;
                out.seq = seq;
                out.redo = redo;
            }
            Turn::Stale {
                widget,
                seq,
                reason,
            } => {
                out.turn = Kind::Stale;
                out.answer = Some(conversation::answer(
                    seq,
                    version,
                    Some(reason),
                    resync(widget),
                ));
            }
            Turn::Route { widget, seq } => {
                out.turn = Kind::Route;
                let values = args.get(4..).unwrap_or_default();
                let (reason, corrections) = self.route(widget, &message.tag, values, &mut out);
                self.conversation.applied(out.version);
                out.answer = Some(conversation::answer(seq, out.version, reason, corrections));
            }
        }
        out
    }

    /// **Every widget of the window, corrected** -- what a history step leaves
    /// behind.
    pub fn resync_all(&self, version: i64) -> Answer {
        let corrections = self.widget.map_or_else(Vec::new, |w| resync(i64::from(w)));
        conversation::answer(0, version, None, corrections)
    }

    /// Answers the stamp a [`Kind::Step`] carried, once the caller has walked.
    pub fn acknowledge(&self, seq: i64, version: i64, reason: Option<String>) -> Answer {
        conversation::answer(seq, version, reason, Vec::new())
    }

    fn secs_at(&self, units: f64) -> f64 {
        samples_to_secs(units.round() as i64, self.rate)
    }

    /// One gesture onto the take. Answers the reason and the corrections the
    /// acknowledgement carries.
    fn route(
        &mut self,
        widget: i64,
        tag: &str,
        values: &[Value],
        out: &mut Outcome,
    ) -> (Option<String>, Vec<Correction>) {
        if NOT_AN_EDIT.contains(&tag) {
            self.observe(tag, values, out);
            return (None, Vec::new());
        }
        let edited = match tag {
            "draw" | "sample" => self.stroke(tag, values),
            "cut" => self.cut(values),
            "paste" => self.paste(values),
            "mix" => self.mix(values),
            _ => return (None, Vec::new()),
        };
        match edited {
            Ok(Some((label, list, mut steps))) => {
                let before = std::mem::replace(&mut self.list, list);
                steps.extend(self.stitched());
                out.record = Some(self.record(&label, &before));
                out.steps = Some(steps_json(&steps));
                out.changed = true;
                out.version += 1;
                (None, Vec::new())
            }
            Ok(None) => (None, Vec::new()),
            Err(why) => (Some(why), resync(widget)),
        }
    }

    /// The entry a turn leaves: the list after, the list before, and the takes
    /// each reads.
    fn record(&self, label: &str, before: &[Part]) -> Record {
        let ids = |list: &[Part]| parts::sources(list).iter().map(|s| s.0).collect();
        Record {
            label: label.into(),
            legs: vec![Leg {
                forward: json!({ "edit": parts::payload(&self.list).0 }),
                backward: parts::payload(before).0,
                key: String::new(),
                holds_forward: ids(&self.list),
                holds_backward: ids(before),
            }],
        }
    }

    /// A buffer for a new take, or the reason there is none.
    fn next_buffer(&mut self) -> Result<i64, String> {
        if self.spare.is_empty() {
            return Err("no buffer was handed over for the take this edit writes".into());
        }
        Ok(self.spare.remove(0))
    }

    /// The steps that make `buffer` a take `frames` long at the editor's rate.
    fn allocate(&self, buffer: i64, frames: u64) -> Vec<Step> {
        vec![
            send(
                "/buffer_alloc",
                vec![
                    OscType::Int(buffer as i32),
                    OscType::Int(frames as i32),
                    OscType::Int(self.channels as i32),
                    OscType::Float(self.rate as f32),
                ],
            ),
            Step::Sync(buffer as i32),
        ]
    }

    /// **A pencil stroke is a new take the size of the stroke**, made of the
    /// frames it was drawn over with the drawn channel written in, and spliced
    /// over those frames.
    fn stroke(&mut self, tag: &str, values: &[Value]) -> Result<Edited, String> {
        let taken = samples::intake(tag, values);
        let Some(payload) = taken.payloads.first() else {
            return match taken.refusal {
                Some(why) => Err(why),
                None => Ok(None),
            };
        };
        let start = number(payload.get("start").unwrap_or(&Value::Null)).max(0.0) as u64;
        let channel = number(payload.get("channel").unwrap_or(&Value::Null)).max(0.0) as u32;
        let mut drawn: Vec<Value> = payload
            .get("values")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let total = self.length();
        if start >= total || channel >= self.channels {
            return Err("the stroke is outside the take".into());
        }
        drawn.truncate((total - start) as usize);
        let frames = drawn.len() as u64;
        let buffer = self.next_buffer()?;
        let mut steps = self.allocate(buffer, frames);
        if self.channels > 1 {
            // The frames it was drawn over, every channel: the other channels
            // keep what they held, and the server is what copies them.
            let width = i64::from(self.channels);
            steps.push(send(
                "/buffer_gen",
                vec![
                    OscType::Int(buffer as i32),
                    OscType::String("copy".into()),
                    OscType::Int(0),
                    OscType::Int(self.display as i32),
                    OscType::Int((start as i64 * width) as i32),
                    OscType::Int((frames as i64 * width) as i32),
                ],
            ));
            steps.push(Step::AwaitDone {
                command: "/buffer_gen".into(),
                index: Some(buffer as i32),
            });
        }
        steps.extend(samples::write_steps(
            buffer as i32,
            self.channels,
            &json!({ "channel": channel, "start": 0, "values": drawn }),
            self.chunk,
        ));
        self.frames.insert(buffer, frames);
        let list = parts::replace(
            &self.list,
            start,
            start + frames,
            &[part(buffer, 0, frames)],
        )?;
        Ok(Some((taken.label, list, steps)))
    }

    /// **A cut takes the span out of the list**, and moves no samples.
    fn cut(&mut self, values: &[Value]) -> Result<Edited, String> {
        let at = |i: usize| values.get(i).map_or(0.0, number).round().max(0.0) as u64;
        let (start, len) = (at(0), at(1));
        if len == 0 {
            return Ok(None);
        }
        let list = parts::remove(&self.list, start, start + len)?;
        if list.is_empty() {
            return Err("a take cannot be cut down to nothing".into());
        }
        Ok(Some(("cut".into(), list, Vec::new())))
    }

    /// **A paste is a new take holding the block**, put in at the position.
    ///
    /// The block travels with the gesture -- the clipboard is the host's -- so
    /// its samples are written into a take of their own; it is refused when
    /// its width or its rate is not the take's, since fitting either would be
    /// an edit nobody asked for.
    fn paste(&mut self, values: &[Value]) -> Result<Edited, String> {
        let (position, samples) = self.block(values)?;
        let frames = samples.len() as u64 / u64::from(self.channels);
        if frames == 0 {
            return Ok(None);
        }
        let buffer = self.next_buffer()?;
        let mut steps = self.allocate(buffer, frames);
        steps.extend(self.fill(buffer, &samples));
        self.frames.insert(buffer, frames);
        let list = parts::insert(&self.list, position, &[part(buffer, 0, frames)])?;
        Ok(Some(("paste".into(), list, steps)))
    }

    /// **A mix adds the block onto the frames it lands on**, and the result is
    /// a new take over those frames.
    ///
    /// The frames under the block are copied out of the join into a take of
    /// their own, the block is written into a scratch buffer, and the server
    /// adds one into the other (`/buffer_mix`) -- the sum is an operation over
    /// samples, so it is the server's. The scratch buffer is freed by the same
    /// steps and goes back to the ones this editor holds. A block that runs
    /// past the end of the take is mixed as far as the take goes.
    fn mix(&mut self, values: &[Value]) -> Result<Edited, String> {
        let (position, mut samples) = self.block(values)?;
        let width = u64::from(self.channels);
        let total = self.length();
        if position >= total {
            return Err("the mix lands past the end of the take".into());
        }
        let frames = (samples.len() as u64 / width).min(total - position);
        samples.truncate((frames * width) as usize);
        if frames == 0 {
            return Ok(None);
        }
        if self.spare.len() < 2 {
            return Err("a mix needs two buffers handed over: the take and a scratch one".into());
        }
        let (take, scratch) = (self.spare.remove(0), self.spare.remove(0));
        let mut steps = self.allocate(take, frames);
        steps.push(send(
            "/buffer_gen",
            vec![
                OscType::Int(take as i32),
                OscType::String("copy".into()),
                OscType::Int(0),
                OscType::Int(self.display as i32),
                OscType::Int((position * width) as i32),
                OscType::Int((frames * width) as i32),
            ],
        ));
        steps.push(Step::AwaitDone {
            command: "/buffer_gen".into(),
            index: Some(take as i32),
        });
        steps.extend(self.allocate(scratch, frames));
        steps.extend(self.fill(scratch, &samples));
        steps.push(send(
            "/buffer_mix",
            vec![
                OscType::Int(take as i32),
                OscType::Int(0),
                OscType::Int(scratch as i32),
                OscType::Int(0),
                OscType::Int(frames as i32),
                OscType::Float(1.0),
            ],
        ));
        steps.push(Step::AwaitDone {
            command: "/buffer_mix".into(),
            index: Some(take as i32),
        });
        steps.push(send("/buffer_free", vec![OscType::Int(scratch as i32)]));
        steps.push(Step::AwaitDone {
            command: "/buffer_free".into(),
            index: Some(scratch as i32),
        });
        self.spare.push(scratch);
        self.frames.insert(take, frames);
        let list = parts::replace(
            &self.list,
            position,
            position + frames,
            &[part(take, 0, frames)],
        )?;
        Ok(Some(("mix".into(), list, steps)))
    }

    /// **The block a paste or a mix carries**: where it goes, and its samples
    /// interleaved at the take's width -- or why it does not fit this take.
    fn block(&self, values: &[Value]) -> Result<(u64, Vec<f32>), String> {
        let position = values.first().map_or(0.0, number).round().max(0.0) as u64;
        let doc = values.get(2).map(text).unwrap_or_default();
        let Ok(clip) = serde_json::from_str::<Clipboard>(&doc) else {
            return Err("the clipboard did not travel with the paste".into());
        };
        let Content::Samples {
            channels,
            frames,
            sample_rate,
            blob,
        } = clip.content
        else {
            return Err(format!(
                "a take takes samples, and the clipboard holds {}",
                clip.content.kind()
            ));
        };
        if channels != self.channels {
            return Err(format!(
                "a block of {channels} channels does not paste into a take of {}",
                self.channels
            ));
        }
        if sample_rate > 0.0 && (sample_rate - self.rate).abs() > f64::EPSILON {
            return Err(format!(
                "a block at {sample_rate} Hz does not paste into a take at {} Hz",
                self.rate
            ));
        }
        let mut samples: Vec<f32> = match values.get(3 + blob) {
            Some(Value::Array(items)) => items.iter().map(|v| number(v) as f32).collect(),
            _ => return Err("the clipboard's samples did not travel with it".into()),
        };
        let width = u64::from(self.channels);
        let frames = frames.min(samples.len() as u64 / width);
        samples.truncate((frames * width) as usize);
        Ok((position, samples))
    }

    /// The steps that write `samples`, interleaved, into `buffer` from its
    /// first frame, and wait for the last write.
    fn fill(&self, buffer: i64, samples: &[f32]) -> Vec<Step> {
        let chunk = self.chunk.max(1);
        let mut steps: Vec<Step> = samples
            .chunks(chunk)
            .enumerate()
            .map(|(i, run)| {
                send(
                    "/buffer_setRange",
                    vec![
                        OscType::Int(buffer as i32),
                        OscType::Int((i * chunk) as i32),
                        OscType::Blob(run.iter().flat_map(|v| v.to_le_bytes()).collect()),
                    ],
                )
            })
            .collect();
        steps.push(Step::AwaitDone {
            command: "/buffer_setRange".into(),
            index: Some(buffer as i32),
        });
        steps
    }

    /// A tag that says what the view is looking at rather than what changed.
    fn observe(&mut self, tag: &str, values: &[Value], out: &mut Outcome) {
        match tag {
            "locate" if !values.is_empty() => {
                out.locate = Some(self.secs_at(number(&values[0])));
            }
            "selection" => {
                let at = |i: usize| values.get(i).map_or(0.0, |v| self.secs_at(number(v)));
                out.selection = Some(json!({ "start": at(0), "len": at(1) }));
            }
            _ => {}
        }
    }
}

/// What a gesture made: its label, the list after it and the steps that make
/// its new takes -- or nothing at all.
type Edited = Option<(String, Vec<Part>, Vec<Step>)>;

/// What `widget` should be drawing: read the take again.
fn resync(widget: i64) -> Vec<Correction> {
    let Ok(id) = i32::try_from(widget) else {
        return Vec::new();
    };
    vec![Correction {
        widget,
        props: Value::Object(props(id)),
    }]
}

/// The facts a caller hands over, in [`new_json`] and in `sync`.
#[derive(Deserialize, Default)]
#[serde(default)]
struct Facts {
    take: Option<i64>,
    frames: Option<u64>,
    channels: Option<u32>,
    rate: Option<f64>,
    display: Option<i64>,
    /// Buffers for new takes, appended to what the editor already holds.
    buffers: Option<Vec<i64>>,
    chunk: Option<usize>,
    #[serde(deserialize_with = "present")]
    name: Option<Option<String>>,
    layers: Option<Vec<String>>,
    title: Option<String>,
    w: Option<i64>,
    h: Option<i64>,
    #[serde(deserialize_with = "present")]
    window: Option<Option<i32>>,
    version: Option<i64>,
}

/// A key that is present, whatever its value -- `null` included.
fn present<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

impl AudioEditor {
    fn adopt(&mut self, facts: Facts) {
        if let Some(channels) = facts.channels {
            self.channels = channels.max(1);
        }
        if let Some(rate) = facts.rate.filter(|r| *r > 0.0) {
            self.rate = rate;
        }
        if let Some(take) = facts.take {
            let frames = facts
                .frames
                .unwrap_or_else(|| self.frames.get(&take).copied().unwrap_or(0));
            self.reopen(take, frames);
        } else if let Some(frames) = facts.frames {
            self.reopen(self.take, frames);
        }
        if let Some(display) = facts.display {
            self.display = display;
        }
        if let Some(buffers) = facts.buffers {
            self.spare.extend(buffers);
        }
        if let Some(chunk) = facts.chunk {
            self.chunk = chunk.max(1);
        }
        if let Some(name) = facts.name {
            self.name = name;
        }
        if let Some(title) = facts.title {
            self.title = title;
        }
        if let Some(w) = facts.w {
            self.size.0 = w;
        }
        if let Some(h) = facts.h {
            self.size.1 = h;
        }
        if let Some(window) = facts.window {
            self.window = window;
        }
    }
}

/// **An editor built from a JSON request** -- `take`, `frames`, `channels`,
/// `rate`, `display`, `buffers`, `chunk`, `name`, `layers`, `title`, `w`, `h`
/// and `version` -- or the reason it cannot be.
pub fn new_json(request: &str) -> Result<AudioEditor, String> {
    let facts: Facts =
        serde_json::from_str(request).map_err(|e| format!("not an audio editor request: {e}"))?;
    let layers = facts
        .layers
        .clone()
        .unwrap_or_else(|| MEASURES.iter().map(|m| m.to_string()).collect());
    let mut editor = AudioEditor::new(
        facts.take.unwrap_or(0),
        facts.frames.unwrap_or(0),
        facts.channels.unwrap_or(1),
        facts.rate.unwrap_or(0.0),
        facts.display.unwrap_or(0),
        &layers,
        facts.version.unwrap_or(0),
    )?;
    editor.adopt(Facts {
        layers: None,
        take: None,
        frames: None,
        ..facts
    });
    Ok(editor)
}

/// **One verb of an editor, over JSON** -- the door both clients bind.
///
/// - `sync` -- any of the facts [`new_json`] reads: handed over before the
///   verbs that read them; `buffers` are added to the ones the editor holds.
///   Answers `{}`.
/// - `layers` -- `stack`, optional: `{"layers", "measure"}` or `{"error"}`.
/// - `window` -- `widget`: the window, as a GuiDef.
/// - `props` -- `widget`: the correction.
/// - `open` -- `{"steps"}`: what makes the drawn join the take.
/// - `event` -- `addr`, `args`, `version`: an [`Outcome`].
/// - `apply` -- `payload`: `{"steps"}` for a list the history handed back, or
///   `{}` for a payload that is not one.
/// - `parts` -- `{"parts", "frames"}`: the list, and how long it is.
/// - `acknowledge` -- `seq`, `version`, `reason`: an [`Answer`].
///
/// An unknown verb answers `{}`.
pub fn call_json(editor: &mut AudioEditor, request: &str) -> String {
    let Ok(request) = serde_json::from_str::<Value>(request) else {
        return "{}".into();
    };
    let get = |key: &str| request.get(key).cloned().unwrap_or(Value::Null);
    let widget = int(&get("widget")) as i32;
    let version = int(&get("version"));
    match request
        .get("verb")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "sync" => {
            if let Ok(facts) = serde_json::from_value::<Facts>(request.clone()) {
                editor.adopt(Facts {
                    layers: None,
                    ..facts
                });
            }
            "{}".into()
        }
        "layers" => {
            if let Some(stack) = request.get("stack") {
                let stack: Vec<String> = serde_json::from_value(stack.clone()).unwrap_or_default();
                if let Err(error) = editor.set_layers(&stack) {
                    return json!({ "error": error }).to_string();
                }
            }
            json!({ "layers": editor.layers(), "measure": editor.layers().join(" ") }).to_string()
        }
        "window" => editor.window(widget).to_string(),
        "props" => Value::Object(props(widget)).to_string(),
        "open" => json!({ "steps": editor.open() }).to_string(),
        "event" => {
            let event = serde_json::from_value::<Event>(request.clone()).unwrap_or_default();
            serde_json::to_string(&editor.event(&event, version)).unwrap_or_else(|_| "{}".into())
        }
        "apply" => match editor.apply(&get("payload")) {
            Some(steps) => json!({ "steps": steps }).to_string(),
            None => "{}".into(),
        },
        "parts" => json!({ "parts": editor.list(), "frames": editor.length() }).to_string(),
        "acknowledge" => serde_json::to_string(&editor.acknowledge(
            int(&get("seq")),
            version,
            get("reason").as_str().map(str::to_string),
        ))
        .unwrap_or_else(|_| "{}".into()),
        _ => "{}".into(),
    }
}

/// The domain an audio editor's structure is registered under.
pub const DOMAIN: &str = PARTS;

#[cfg(test)]
mod tests;
