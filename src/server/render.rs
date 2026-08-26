//! Offline (NRT) rendering: the same engine, driven by a score instead of
//! cpal.
//!
//! A [`Score`] is a time-ordered list of OSC bundles. On disk it uses the
//! scsynth binary score format — `[i32 big-endian byte count][OSC packet]`
//! repeated — where each bundle's NTP timetag is read as **seconds from the
//! start of the render** (the immediate tag is time 0). Rendering is single
//! threaded and synchronous: async commands (defs, buffers) complete before
//! time advances, like scsynth's NRT mode, while the schedulable subset
//! travels through the engine's own queue, so a bundle landing
//! mid-block splits the block exactly as it would in real time — the offline
//! render of a score is sample-identical to a perfectly timed live take.
//!
//! The render ends at the time of the **last** bundle, whose commands
//! therefore produce no sound: close a score with a dummy bundle (e.g. a
//! final `/node_free`) to set the total duration.

use std::path::Path;
use std::sync::Arc;

use rosc::{OscMessage, OscPacket, OscTime, OscType};

use crate::dsp::{Limits, NUM_AUDIO_BUSES};
#[cfg(all(feature = "faust", not(target_arch = "wasm32")))]
use crate::osc::translate::parse_def_send_faust;
use crate::osc::translate::{CmdTranslator, parse_buffer_gen, parse_buffer_msg};
use crate::server::engine::{
    BLOCK_SIZE, Cmd, DEFAULT_AUDIO_BUSES, DEFAULT_CONTROL_BUSES, Engine, EngineHandle, Garbage,
    NodeEventKind, engine_pair_full,
};
use crate::server::nrt::{NrtAction, run_job, wav_format};

/// One score entry: the messages of a bundle, executed atomically at `time`
/// seconds from the start of the render.
pub struct ScoreEvent {
    pub time: f64,
    pub messages: Vec<OscMessage>,
}

/// A render script: events sorted by time (stable for equal times).
pub struct Score {
    events: Vec<ScoreEvent>,
}

impl Score {
    /// Builds a score from (seconds, messages) pairs; sorts them stably.
    pub fn new(events: impl IntoIterator<Item = (f64, Vec<OscMessage>)>) -> Result<Self, String> {
        let events: Vec<ScoreEvent> = events
            .into_iter()
            .map(|(time, messages)| ScoreEvent { time, messages })
            .collect();
        for ev in &events {
            if !ev.time.is_finite() || ev.time < 0.0 {
                return Err(format!("invalid event time {}", ev.time));
            }
        }
        Ok(Self::sorted(events))
    }

    /// Parses the scsynth binary score format: length-prefixed OSC packets.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        let mut events = Vec::new();
        let mut pos = 0usize;
        let mut n = 0usize;
        while pos < bytes.len() {
            if bytes.len() - pos < 4 {
                return Err(format!(
                    "truncated score: {} stray bytes after packet {n}",
                    bytes.len() - pos
                ));
            }
            let len = i32::from_be_bytes(bytes[pos..pos + 4].try_into().unwrap());
            pos += 4;
            if len <= 0 || !(len as usize).is_multiple_of(4) || pos + len as usize > bytes.len() {
                return Err(format!("packet {n}: bad length {len}"));
            }
            // The single decode entry point for every transport (`crate::osc`).
            let packet = crate::osc::decode_packet(&bytes[pos..pos + len as usize])
                .map_err(|e| format!("packet {n}: {e}"))?;
            pos += len as usize;
            flatten(packet, &mut events);
            n += 1;
        }
        Ok(Self::sorted(events))
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref();
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::from_bytes(&bytes).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Seconds from the start to the last event: the render length.
    pub fn duration(&self) -> f64 {
        self.events.last().map_or(0.0, |ev| ev.time)
    }

    pub fn events(&self) -> &[ScoreEvent] {
        &self.events
    }

    fn sorted(mut events: Vec<ScoreEvent>) -> Self {
        // Stable: events at equal times keep their score order.
        events.sort_by(|a, b| a.time.partial_cmp(&b.time).expect("times are finite"));
        Self { events }
    }
}

/// A bundle becomes one atomic event; nested bundles become their own events
/// at their own times (like the real-time server); a bare message is an
/// event at time 0.
fn flatten(packet: OscPacket, out: &mut Vec<ScoreEvent>) {
    match packet {
        OscPacket::Message(msg) => out.push(ScoreEvent {
            time: 0.0,
            messages: vec![msg],
        }),
        OscPacket::Bundle(bundle) => {
            let time = score_time(bundle.timetag);
            let mut messages = Vec::new();
            for inner in bundle.content {
                match inner {
                    OscPacket::Message(msg) => messages.push(msg),
                    nested @ OscPacket::Bundle(_) => flatten(nested, out),
                }
            }
            out.push(ScoreEvent { time, messages });
        }
    }
}

/// In a score the timetag counts seconds **from the start of the render**;
/// the OSC immediate tag (seconds 0, fractional ≤ 1) is time 0.
fn score_time(t: OscTime) -> f64 {
    if t.seconds == 0 && t.fractional <= 1 {
        0.0
    } else {
        t.seconds as f64 + t.fractional as f64 / 2f64.powi(32)
    }
}

pub struct RenderConfig {
    pub sample_rate: f64,
    /// Output channels = how many of the first audio buses land in the file.
    pub channels: usize,
    /// DSP workers for `/group_parallel` groups. Parallel rendering is
    /// bit-identical to sequential (disjoint stages), just faster.
    pub workers: usize,
    /// Where this render's stochastic UGens start their seeds, or `None` to
    /// draw one from entropy ([`clausters_core::rng::entropy_seed`]) — the
    /// default.
    ///
    /// A random process is unpredictable first: an unconfigured render is a
    /// *new take*, the way playing a piece with noise in it again gives you
    /// another performance. Set this to replay one exactly; the seed a render
    /// actually used comes back in [`RenderStats::seed`], so a take you liked
    /// is never lost.
    pub seed: Option<u64>,
    /// Boot-time pool capacities, the offline half of the live server's
    /// `--max-nodes` / `--max-graph-children` / `--max-buffers`.
    ///
    /// A render is meant to be sample-identical to a perfectly timed live take,
    /// and that only holds if both sides are built the same way: a group whose
    /// child capacity is 512 here and 4096 on the server truncates offline where
    /// the live take does not, and the two diverge without either one failing.
    /// So the limits travel with the render config rather than being defaulted
    /// inside the renderer.
    pub limits: Limits,
}

impl Default for RenderConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48000.0,
            channels: 2,
            workers: 0,
            seed: None,
            limits: Limits::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RenderStats {
    pub frames: u64,
    pub events: usize,
    /// Peak magnitude per output channel, measured as the render streams.
    ///
    /// The driver already walks every sample on its way to the sink, so this
    /// costs a compare per sample and saves the caller a second pass — which
    /// matters most for [`render_to_wav`], where the samples are gone by the
    /// time the call returns and measuring them again would mean reading the
    /// file back.
    pub peak: Vec<f32>,
    /// RMS per output channel over the whole render.
    pub rms: Vec<f32>,
    /// The seed this render's stochastic UGens actually started from —
    /// whatever [`RenderConfig::seed`] asked for, or the one drawn from
    /// entropy when it asked for nothing.
    ///
    /// Reporting it is what makes an unpredictable default usable: you play a
    /// score, you like the take, and this is how you get it back. Feed it to
    /// `RenderConfig::seed` (`--seed`, `seed=`) and the render repeats.
    pub seed: u64,
}

/// Renders a score, handing each processed chunk (interleaved, at most one
/// block) to `sink`. This is the core the WAV and in-memory frontends wrap.
///
/// Side effect: leaves the calling thread in flush-to-zero FPU mode (see
/// [`crate::dsp::denormals`]) — the same mode the real-time callback runs
/// in, so the offline render stays sample-identical to a live take.
pub fn render(
    score: &Score,
    cfg: &RenderConfig,
    mut sink: impl FnMut(&[f32]) -> Result<(), String>,
) -> Result<RenderStats, String> {
    if cfg.channels == 0 || cfg.channels > NUM_AUDIO_BUSES {
        return Err(format!("channels must be 1-{NUM_AUDIO_BUSES}"));
    }
    if !(cfg.sample_rate.is_finite() && cfg.sample_rate > 0.0) {
        return Err("sample rate must be positive".into());
    }
    let sr = cfg.sample_rate;
    let total = (score.duration() * sr).round() as u64;
    if total == 0 {
        return Err(
            "empty render: a score ends at its last bundle's time — close it with a \
             final bundle (e.g. an /node_free) at the total duration"
                .into(),
        );
    }

    // Measured on the way past, not in a second pass: the driver already
    // touches every sample handing it to the sink, and for `render_to_wav`
    // the samples are gone once the call returns.
    let channels = cfg.channels;
    let mut peak = vec![0.0f32; channels];
    let mut sumsq = vec![0.0f64; channels];

    // Resolved once, here, so the number that goes into the translator is the
    // number that comes back in the stats — a take is only repeatable if the
    // render reports the seed it actually used.
    let seed = cfg.seed.unwrap_or_else(clausters_core::rng::entropy_seed);

    crate::dsp::denormals::flush_to_zero();
    let (engine, handle) = engine_pair_full(
        sr as f32,
        cfg.channels,
        cfg.workers,
        None,
        DEFAULT_AUDIO_BUSES,
        DEFAULT_CONTROL_BUSES,
        cfg.limits,
    );
    let mut r = Renderer {
        engine,
        handle,
        translator: {
            let mut t = CmdTranslator::with_limits(
                sr as f32,
                DEFAULT_AUDIO_BUSES,
                DEFAULT_CONTROL_BUSES,
                cfg.limits,
            );
            t.set_seed(seed);
            t
        },
        block: vec![0.0; BLOCK_SIZE * cfg.channels],
        now: 0,
        channels: cfg.channels,
        sample_rate: sr,
        dropped: false,
    };

    let mut measured = |chunk: &[f32]| -> Result<(), String> {
        for (i, &x) in chunk.iter().enumerate() {
            let c = i % channels;
            let a = x.abs();
            if a > peak[c] {
                peak[c] = a;
            }
            sumsq[c] += (x as f64) * (x as f64);
        }
        sink(chunk)
    };

    for ev in &score.events {
        let target = ((ev.time * sr).round() as u64).min(total);
        // The event lands in a future block: render up to it.
        while r.now + BLOCK_SIZE as u64 <= target {
            r.process_block(total, &mut measured)?;
        }
        let cmds = r
            .event_cmds(&ev.messages)
            .map_err(|e| format!("score event at {:.6}s: {e}", ev.time))?;
        if cmds.is_empty() {
            continue;
        }
        // The engine splits the upcoming block at the exact sample.
        if r.handle.send(Cmd::Schedule { time: target, cmds }).is_err() {
            return Err(format!(
                "score event at {:.6}s: command FIFO full (too many bundles inside one block)",
                ev.time
            ));
        }
    }
    while r.now < total {
        r.process_block(total, &mut measured)?;
    }
    r.collect();
    if r.dropped {
        return Err(
            "the engine dropped scheduled bundles (queue full): the score is too dense".into(),
        );
    }
    Ok(RenderStats {
        frames: total,
        events: score.events.len(),
        seed,
        peak,
        rms: sumsq
            .iter()
            .map(|&s| {
                if total == 0 {
                    0.0
                } else {
                    (s / total as f64).sqrt() as f32
                }
            })
            .collect(),
    })
}

/// Renders a score into an interleaved in-memory signal (tests, asserts).
pub fn render_to_vec(score: &Score, cfg: &RenderConfig) -> Result<(Vec<f32>, RenderStats), String> {
    let mut out = Vec::new();
    let stats = render(score, cfg, |chunk| {
        out.extend_from_slice(chunk);
        Ok(())
    })?;
    Ok((out, stats))
}

/// Renders a score straight to a WAV file. `sample_format` is `int16`,
/// `int24` or `float`, like `/buffer_write`.
pub fn render_to_wav(
    score: &Score,
    cfg: &RenderConfig,
    path: impl AsRef<Path>,
    sample_format: &str,
) -> Result<RenderStats, String> {
    let path = path.as_ref();
    let err = |e: hound::Error| format!("{}: {e}", path.display());
    let (bits, format) = wav_format(sample_format)?;
    let spec = hound::WavSpec {
        channels: cfg.channels as u16,
        sample_rate: cfg.sample_rate as u32,
        bits_per_sample: bits,
        sample_format: format,
    };
    let mut writer = hound::WavWriter::create(path, spec).map_err(err)?;
    let scale = ((1u64 << (bits - 1)) - 1) as f32;
    let stats = render(score, cfg, |chunk| {
        for &s in chunk {
            match format {
                hound::SampleFormat::Float => writer.write_sample(s),
                hound::SampleFormat::Int => {
                    writer.write_sample((s.clamp(-1.0, 1.0) * scale).round() as i32)
                }
            }
            .map_err(err)?;
        }
        Ok(())
    })?;
    writer.finalize().map_err(err)?;
    Ok(stats)
}

/// The offline counterpart of the server: owns both engine halves and drives
/// them on one thread.
struct Renderer {
    engine: Engine,
    handle: EngineHandle,
    translator: CmdTranslator,
    block: Vec<f32>,
    /// Frames processed so far; tracked here rather than read back from the
    /// engine's published clock.
    now: u64,
    channels: usize,
    sample_rate: f64,
    /// Set when the engine rejected a scheduled bundle (queue full).
    dropped: bool,
}

impl Renderer {
    fn process_block(
        &mut self,
        total: u64,
        sink: &mut impl FnMut(&[f32]) -> Result<(), String>,
    ) -> Result<(), String> {
        self.engine.process_block(&mut self.block);
        // The last block is truncated to the requested length.
        let frames = (total - self.now).min(BLOCK_SIZE as u64) as usize;
        sink(&self.block[..frames * self.channels])?;
        self.now += BLOCK_SIZE as u64;
        self.collect();
        Ok(())
    }

    /// Translates one event's messages into engine commands, running async
    /// commands (defs, buffers) synchronously right now — scsynth NRT
    /// semantics: they complete before time advances.
    fn event_cmds(&mut self, messages: &[OscMessage]) -> Result<Vec<Cmd>, String> {
        let mut cmds = Vec::new();
        for msg in messages {
            self.message_cmds(msg, &mut cmds)
                .map_err(|e| format!("{}: {e}", msg.addr))?;
        }
        Ok(cmds)
    }

    fn message_cmds(&mut self, msg: &OscMessage, cmds: &mut Vec<Cmd>) -> Result<(), String> {
        match msg.addr.as_str() {
            "/def_send" => {
                let Some(OscType::String(family)) = msg.args.first() else {
                    return Err(
                        "expected a family string (\"synth\", \"faust\" or \"graph\")".into(),
                    );
                };
                let rest = &msg.args[1..];
                match family.as_str() {
                    "synth" => self.translator.d_recv(rest).map(|_| ()),
                    "faust" => self.d_faust(rest),
                    "graph" => self.translator.d_graph(rest).map(|_| ()),
                    other => Err(format!("unknown def family '{other}'")),
                }
            }
            "/def_free" => self.translator.d_free(&msg.args),
            "/buffer_alloc"
            | "/buffer_allocRead"
            | "/buffer_read"
            | "/buffer_write"
            | "/buffer_zero"
            | "/buffer_gen"
            | "/buffer_set"
            | "/buffer_setRange"
            | "/buffer_gain"
            | "/buffer_reverse"
            | "/buffer_fill"
            | "/buffer_readChannel"
            | "/buffer_allocReadChannel"
            | "/buffer_free" => {
                let (index, job) = if msg.addr == "/buffer_gen" {
                    parse_buffer_gen(&msg.args, &self.translator.buffers)?
                } else {
                    parse_buffer_msg(
                        msg.addr.as_str(),
                        &msg.args,
                        &self.translator.buffers,
                        self.sample_rate,
                    )?
                };
                // The buffer is built now, but installed sample-accurately
                // with the rest of the event's commands. It lands in the
                // translator's pool — the same one `make_synth` reads to fill a
                // Faust `soundfile("<bufnum>", n)` zone, so soundfile resolves
                // offline exactly as it does on the live server.
                match run_job(job)? {
                    NrtAction::Install(buffer) => {
                        self.translator.buffers[index as usize] = Some(Arc::clone(&buffer));
                        cmds.push(Cmd::SetBuffer {
                            index: index as usize,
                            buffer: Some(buffer),
                        });
                    }
                    // Offline there is no segment, no region and therefore no
                    // overview to follow: a write in place has already landed
                    // in the cells the render reads.
                    NrtAction::Wrote { .. } => {}
                    NrtAction::Clear => {
                        self.translator.buffers[index as usize] = None;
                        cmds.push(Cmd::SetBuffer {
                            index: index as usize,
                            buffer: None,
                        });
                    }
                    NrtAction::None => {}
                }
                Ok(())
            }
            _ => self.translator.translate(msg, cmds),
        }
    }

    #[cfg(all(feature = "faust", not(target_arch = "wasm32")))]
    fn d_faust(&mut self, args: &[rosc::OscType]) -> Result<(), String> {
        use crate::faust::compiler::{CompilePayload, compile};
        let (name, def) = parse_def_send_faust(args)?;
        let payload = CompilePayload::classify(def);
        let def = compile(&name, &payload)?;
        self.translator.faust_defs.insert(name, Arc::new(def));
        Ok(())
    }

    /// Offline rendering compiles a def where it stands, and in a page the
    /// Faust compiler is a different scope answering later (see
    /// `faust::compiler_web`), so there is nothing to call here. A tab renders
    /// a Faust def by sending it to the live engine first.
    #[cfg(all(feature = "faust", target_arch = "wasm32"))]
    fn d_faust(&mut self, _args: &[rosc::OscType]) -> Result<(), String> {
        Err(
            "an offline render in a page cannot compile a Faust def: send it to the engine first"
                .into(),
        )
    }

    #[cfg(not(feature = "faust"))]
    fn d_faust(&mut self, _args: &[rosc::OscType]) -> Result<(), String> {
        Err("server built without faust support".into())
    }

    fn collect(&mut self) {
        while let Some(g) = self.handle.pop_garbage() {
            match g {
                Garbage::FreedSynth { id, .. } => self.translator.forget_node(id),
                Garbage::SpentBundle(cmds) => {
                    if !cmds.is_empty() {
                        self.dropped = true;
                    }
                }
                Garbage::RejectedSynth { id, .. } | Garbage::RejectedGroup { id, .. } => {
                    self.translator.release_node_id(id);
                    tracing::warn!(
                        "nrt render: engine rejected node {id} (duplicate ID, bad target or full table)"
                    );
                }
                Garbage::FreedGroup { .. } | Garbage::FreedBuffer(_) => {}
            }
        }
        // Offline there is no client to notify, but the server-owned id
        // ranges still recycle on death, same as live.
        while let Some(ev) = self.handle.pop_event() {
            if ev.kind == NodeEventKind::End {
                self.translator.release_node_id(ev.id);
            }
        }
    }
}
