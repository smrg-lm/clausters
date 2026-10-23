//! **The renders an audio editor's edits are**, as the scores that make them.
//!
//! A take lives on disk as a list of segments over files that never change,
//! so an edit that makes new samples -- a gain applied, a span reversed, a
//! block mixed in, a stroke of the pencil, a block pasted, a file of silence,
//! and the peaks of a file opened without them -- is an offline render that
//! writes a new file, and the list then names it. Each function here answers
//! the [`Step::Render`] that does one: a binary score for an offline server,
//! and the file it is rendered into, with the peaks beside it written by a
//! `PeaksOut` on the same signal. Nothing here does I/O; the endpoint renders
//! the score on an offline instance of its own.
//!
//! **How a score reads a list.** The list is stitched on the offline server
//! (`/buffer_stitch` with parts over files) and read by one `TransportDiskIn`
//! per channel, on a transport the score binds to a group of its own and
//! rolls from frame 0 (an offline score takes the transport commands the
//! engine carries out itself). Offline the reader waits for what it reads, so
//! the list is there from its first frame. The render runs at the list's
//! rate, one frame a sample, and is exactly as long as what it writes.

use clausters_core::osc::{OscMessage, OscPacket, OscType, bundle, encode, pack_timetag};
use clausters_document::clipboard::SegmentPart;
use clausters_editing::apply::{Render, Step};
use serde_json::{Value, json};

/// The buffer a score stitches the list it reads into.
const LIST: i32 = 1;
/// The buffer a second list (a mix's) is stitched into.
const OTHER: i32 = 2;
/// The buffer a block of samples the score carries is written into.
const BLOCK: i32 = 3;
/// The group the score's transport governs.
const GROUP: i32 = 100;
/// The synth that renders.
const NODE: i32 = 1000;
/// Samples one `/buffer_setRange` of a score carries.
const CHUNK: usize = 8192;

/// **A list a render reads**: its parts, by location, and its shape.
#[derive(Clone, Debug, PartialEq)]
pub struct List {
    /// The parts, in the order they play.
    pub parts: Vec<SegmentPart>,
    /// The list's channels.
    pub channels: u32,
    /// The list's rate, which the render runs at.
    pub rate: f64,
}

impl List {
    /// How many frames the list comes to.
    pub fn frames(&self) -> u64 {
        self.parts.iter().map(|p| p.frames).sum()
    }
}

/// **Where a render writes**: the file, its peaks beside it, and the sample
/// format.
#[derive(Clone, Debug, PartialEq)]
pub struct Output {
    pub path: String,
    pub peaks: String,
    pub format: String,
}

/// One segment of a gain curve: its start in seconds from the list's origin,
/// its level there, and the shape and curvature of the ramp to the next.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GainPoint {
    pub at: f64,
    pub value: f64,
    pub shape: f64,
    pub curve: f64,
}

/// A binary score being written: timed bundles.
#[derive(Default)]
struct Score {
    events: Vec<(f64, Vec<OscMessage>)>,
}

impl Score {
    fn at(&mut self, secs: f64, messages: Vec<OscMessage>) {
        self.events.push((secs, messages));
    }

    /// The `--nrt` format: each bundle after its big-endian byte count.
    fn bytes(mut self) -> Vec<u8> {
        self.events.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut out = Vec::new();
        for (secs, messages) in self.events {
            let packet = encode(&OscPacket::Bundle(bundle(pack_timetag(secs), messages)))
                .expect("a score bundle encodes");
            out.extend_from_slice(&(packet.len() as i32).to_be_bytes());
            out.extend_from_slice(&packet);
        }
        out
    }
}

fn msg(addr: &str, args: Vec<OscType>) -> OscMessage {
    OscMessage {
        addr: addr.into(),
        args,
    }
}

/// A frame count as the wire takes it: an `int32`, or a long past one.
fn frames_arg(n: u64) -> OscType {
    match i32::try_from(n) {
        Ok(n) => OscType::Int(n),
        Err(_) => OscType::Long(n as i64),
    }
}

/// `/buffer_stitch` of `list` into buffer `index`, each part a span of its
/// file with its fades and its whole channel map.
fn stitch(index: i32, list: &List) -> OscMessage {
    let mut args = vec![
        OscType::Int(index),
        OscType::Int(list.channels as i32),
        OscType::Float(list.rate as f32),
    ];
    for part in &list.parts {
        args.push(OscType::String(part.path.clone()));
        args.push(frames_arg(part.start));
        args.push(frames_arg(part.frames));
        args.push(OscType::Int(part.fade_in as i32));
        args.push(OscType::Int(part.fade_out as i32));
        args.push(OscType::Int(part.fade_in_shape.shape));
        args.push(OscType::Float(part.fade_in_shape.curve as f32));
        args.push(OscType::Int(part.fade_out_shape.shape));
        args.push(OscType::Float(part.fade_out_shape.curve as f32));
        // Every part spells its whole map: a narrower file repeats, as a
        // join reads it.
        for c in 0..list.channels as i32 {
            let picked = match &part.channels {
                Some(map) => map.get(c as usize).copied().unwrap_or(-1),
                None => c % part.source_channels.max(1) as i32,
            };
            args.push(OscType::Int(picked));
        }
    }
    msg("/buffer_stitch", args)
}

/// The transport rolled from frame 0, governing a group of the score's own.
fn roll() -> Vec<OscMessage> {
    vec![
        msg(
            "/group_new",
            vec![OscType::Int(GROUP), OscType::Int(0), OscType::Int(0)],
        ),
        msg(
            "/transport_group",
            vec![OscType::Int(0), OscType::Int(GROUP)],
        ),
        msg("/transport_play", vec![OscType::Int(0)]),
    ]
}

/// The writes that put `samples` into buffer `index` (`channels` wide).
fn block(index: i32, channels: u32, samples: &[f32]) -> Vec<OscMessage> {
    let frames = samples.len() / channels.max(1) as usize;
    let mut out = vec![msg(
        "/buffer_alloc",
        vec![
            OscType::Int(index),
            frames_arg(frames as u64),
            OscType::Int(channels as i32),
        ],
    )];
    for (i, run) in samples.chunks(CHUNK).enumerate() {
        out.push(msg(
            "/buffer_setRange",
            vec![
                OscType::Int(index),
                OscType::Int((i * CHUNK) as i32),
                OscType::Blob(clausters_core::osc::sample_blob(run)),
            ],
        ));
    }
    out
}

/// A def being written: its UGens, and the index each is at.
struct Def {
    ugens: Vec<Value>,
}

impl Def {
    fn new() -> Self {
        Def { ugens: Vec::new() }
    }

    fn push(&mut self, ugen: Value) -> usize {
        self.ugens.push(ugen);
        self.ugens.len() - 1
    }

    /// One channel of the list in buffer `index`, at the transport.
    fn list_channel(&mut self, index: i32, channel: u32) -> usize {
        self.push(json!({"kind": "TransportDiskIn", "inputs": [
            {"const": index as f64}, {"const": channel as f64},
            {"const": 0.0}, {"const": 0.0}, {"const": 0.0}]}))
    }

    /// One channel of the block in buffer `index`, at the transport, from its
    /// `start`th frame at `rate` frames a frame (`-1` backwards).
    fn block_channel(&mut self, index: i32, channel: u32, rate: f64, start: f64) -> usize {
        self.push(json!({"kind": "TransportBufRd", "inputs": [
            {"const": index as f64}, {"const": channel as f64}, {"const": 0.0},
            {"const": rate}, {"const": start}, {"const": 0.0}]}))
    }

    fn binary(&mut self, kind: &str, a: usize, b: usize) -> usize {
        self.push(json!({"kind": kind, "inputs": [{"ugen": a}, {"ugen": b}]}))
    }

    /// The channels out on buses `0..`, and their peaks into `peaks`.
    fn finish(mut self, name: &str, channels: &[usize], peaks: &str) -> Value {
        for (bus, ugen) in channels.iter().enumerate() {
            self.push(json!({"kind": "Out", "inputs": [{"const": bus as f64}, {"ugen": ugen}]}));
        }
        let inputs: Vec<Value> = channels.iter().map(|u| json!({"ugen": u})).collect();
        self.push(json!({"kind": "PeaksOut", "inputs": inputs, "path": peaks}));
        json!({"name": name, "ugens": self.ugens})
    }
}

/// **The render a score is**: the def sent and played from the top, the
/// synth freed at `frames`, the list's or block's setup before it.
fn render(
    setup: Vec<OscMessage>,
    def: Value,
    frames: u64,
    rate: f64,
    channels: u32,
    out: &Output,
) -> Step {
    let mut score = Score::default();
    let name = def["name"].as_str().unwrap_or("x").to_string();
    let mut first = setup;
    first.push(msg(
        "/def_send",
        vec![
            OscType::String("synth".into()),
            OscType::String(def.to_string()),
        ],
    ));
    first.extend(roll());
    first.push(msg(
        "/synth_new",
        vec![
            OscType::String(name),
            OscType::Int(NODE),
            OscType::Int(0),
            OscType::Int(GROUP),
        ],
    ));
    score.at(0.0, first);
    score.at(
        frames as f64 / rate,
        vec![msg("/node_free", vec![OscType::Int(NODE)])],
    );
    Step::Render(Render {
        score: score.bytes(),
        path: out.path.clone(),
        rate,
        channels: channels as usize,
        format: out.format.clone(),
    })
}

/// **The peaks of a file opened without them**: the file read whole into a
/// `PeaksOut`, the render's own output kept nowhere.
pub fn measure(file: &SegmentPart, peaks: &str) -> Step {
    let list = List {
        parts: vec![SegmentPart {
            start: 0,
            frames: file.source_frames,
            fade_in: 0,
            fade_out: 0,
            fade_in_shape: Default::default(),
            fade_out_shape: Default::default(),
            channels: None,
            ..file.clone()
        }],
        channels: file.source_channels,
        rate: file.source_rate,
    };
    let mut def = Def::new();
    let channels: Vec<usize> = (0..list.channels)
        .map(|c| def.list_channel(LIST, c))
        .collect();
    let def = def.finish("ae.measure", &channels, peaks);
    let out = Output {
        path: String::new(),
        peaks: peaks.into(),
        format: "float".into(),
    };
    render(
        vec![stitch(LIST, &list)],
        def,
        list.frames(),
        list.rate,
        list.channels,
        &out,
    )
}

/// **A gain curve applied to the whole list**: each channel times the
/// curve, which holds its first value before its first point and its last
/// after its last, each ramp of the shape its point names.
pub fn gain(list: &List, curve: &[GainPoint], out: &Output) -> Step {
    let mut def = Def::new();
    let env = envelope(curve);
    let level = def.push(json!({"kind": "EnvGen", "inputs": env}));
    let channels: Vec<usize> = (0..list.channels)
        .map(|c| {
            let read = def.list_channel(LIST, c);
            def.binary("Mul", read, level)
        })
        .collect();
    let def = def.finish("ae.gain_render", &channels, &out.peaks);
    render(
        vec![stitch(LIST, list)],
        def,
        list.frames(),
        list.rate,
        list.channels,
        out,
    )
}

/// The `EnvGen` inputs of a gain curve from time 0: the gate held open, no
/// release and no loop, a step to the first point's level held to it, and a
/// segment between each two points.
fn envelope(curve: &[GainPoint]) -> Vec<Value> {
    let first = curve.first().map_or(1.0, |p| p.value);
    let mut segments: Vec<[f64; 4]> = Vec::new();
    // Held at the first level up to the first point.
    if let Some(p) = curve.first()
        && p.at > 0.0
    {
        segments.push([first, p.at, 1.0, 0.0]);
    }
    for pair in curve.windows(2) {
        segments.push([
            pair[1].value,
            (pair[1].at - pair[0].at).max(0.0),
            pair[0].shape,
            pair[0].curve,
        ]);
    }
    let mut inputs: Vec<Value> = [
        1.0,
        1.0,
        0.0,
        1.0,
        0.0,
        first,
        segments.len() as f64,
        -99.0,
        -99.0,
    ]
    .iter()
    .map(|v| json!({"const": v}))
    .collect();
    for segment in segments {
        inputs.extend(segment.iter().map(|v| json!({"const": v})));
    }
    inputs
}

/// **A span reversed**: the list read into memory on the offline server (a
/// join of the parts' spans, read into buffers there) and played backwards.
/// What it holds is the span, which is the work in hand; a reader that runs
/// backwards would hold nothing (the root plan's "A list reader plays
/// backwards").
pub fn reverse(list: &List, out: &Output) -> Step {
    let mut setup = Vec::new();
    let mut pool = Vec::with_capacity(list.parts.len());
    for (i, part) in list.parts.iter().enumerate() {
        let index = 10 + i as i32;
        // The frames of the file the part reads, at the file's own rate.
        let ratio = part.source_rate / list.rate;
        let read = ((part.frames as f64) * ratio).ceil() as u64;
        setup.push(msg(
            "/buffer_allocRead",
            vec![
                OscType::Int(index),
                OscType::String(part.path.clone()),
                frames_arg(part.start),
                frames_arg(read.min(part.source_frames.saturating_sub(part.start))),
            ],
        ));
        pool.push((index, part));
    }
    // The join over the buffers read, with the parts' fades and maps.
    let mut args = vec![
        OscType::Int(LIST),
        OscType::Int(list.channels as i32),
        OscType::Float(list.rate as f32),
    ];
    for (index, part) in &pool {
        args.push(OscType::Int(*index));
        args.push(OscType::Int(0));
        args.push(frames_arg(part.frames));
        args.push(OscType::Int(part.fade_in as i32));
        args.push(OscType::Int(part.fade_out as i32));
        args.push(OscType::Int(part.fade_in_shape.shape));
        args.push(OscType::Float(part.fade_in_shape.curve as f32));
        args.push(OscType::Int(part.fade_out_shape.shape));
        args.push(OscType::Float(part.fade_out_shape.curve as f32));
        for c in 0..list.channels as i32 {
            let picked = match &part.channels {
                Some(map) => map.get(c as usize).copied().unwrap_or(-1),
                None => c % part.source_channels.max(1) as i32,
            };
            args.push(OscType::Int(picked));
        }
    }
    setup.push(msg("/buffer_stitch", args));
    let frames = list.frames();
    let mut def = Def::new();
    let last = frames.saturating_sub(1) as f64;
    let channels: Vec<usize> = (0..list.channels)
        .map(|c| def.block_channel(LIST, c, -1.0, last))
        .collect();
    let def = def.finish("ae.reverse", &channels, &out.peaks);
    render(setup, def, frames, list.rate, list.channels, out)
}

/// **A list mixed into a span of another**: `onto`, the frames the mix
/// lands on, plus `added` from its first frame, as long as `onto`.
pub fn mix_lists(onto: &List, added: &List, out: &Output) -> Step {
    let mut def = Def::new();
    let channels: Vec<usize> = (0..onto.channels)
        .map(|c| {
            let a = def.list_channel(LIST, c);
            let b = def.list_channel(OTHER, c);
            def.binary("Add", a, b)
        })
        .collect();
    let def = def.finish("ae.mix", &channels, &out.peaks);
    render(
        vec![stitch(LIST, onto), stitch(OTHER, added)],
        def,
        onto.frames(),
        onto.rate,
        onto.channels,
        out,
    )
}

/// **A block of samples mixed into a span of a list**: the block,
/// interleaved at the list's width and rate, added to `onto` from its first
/// frame. The render is as long as `onto`.
pub fn mix_block(onto: &List, samples: &[f32], out: &Output) -> Step {
    let mut def = Def::new();
    let channels: Vec<usize> = (0..onto.channels)
        .map(|c| {
            let a = def.list_channel(LIST, c);
            let b = def.block_channel(BLOCK, c, 1.0, 0.0);
            def.binary("Add", a, b)
        })
        .collect();
    let def = def.finish("ae.mix_block", &channels, &out.peaks);
    let mut setup = vec![stitch(LIST, onto)];
    setup.extend(block(BLOCK, onto.channels, samples));
    render(setup, def, onto.frames(), onto.rate, onto.channels, out)
}

/// **A block of samples, as a file**: what a paste of a block the host
/// carried is, at the block's own rate and width.
pub fn block_file(samples: &[f32], channels: u32, rate: f64, out: &Output) -> Step {
    let frames = (samples.len() / channels.max(1) as usize) as u64;
    let mut def = Def::new();
    let read: Vec<usize> = (0..channels)
        .map(|c| def.block_channel(BLOCK, c, 1.0, 0.0))
        .collect();
    let def = def.finish("ae.block", &read, &out.peaks);
    render(
        block(BLOCK, channels, samples),
        def,
        frames,
        rate,
        channels,
        out,
    )
}

/// **A pencil stroke over a span of a list**: channel `drawn` is the values
/// drawn, the others what the list holds there. As long as the stroke.
pub fn stroke(over: &List, drawn: u32, values: &[f32], out: &Output) -> Step {
    let mut def = Def::new();
    let channels: Vec<usize> = (0..over.channels)
        .map(|c| {
            if c == drawn {
                def.block_channel(BLOCK, 0, 1.0, 0.0)
            } else {
                def.list_channel(LIST, c)
            }
        })
        .collect();
    let def = def.finish("ae.stroke", &channels, &out.peaks);
    let mut setup = vec![stitch(LIST, over)];
    setup.extend(block(BLOCK, 1, values));
    render(
        setup,
        def,
        values.len() as u64,
        over.rate,
        over.channels,
        out,
    )
}

/// **A file of silence**: what a new file is before anything is put in it.
pub fn silence(frames: u64, channels: u32, rate: f64, out: &Output) -> Step {
    let zeros: Vec<Value> = (0..channels).map(|_| json!({"const": 0.0})).collect();
    let def = json!({"name": "ae.silence", "ugens": [
        {"kind": "PeaksOut", "inputs": zeros, "path": out.peaks}]});
    render(Vec::new(), def, frames.max(1), rate, channels, out)
}
