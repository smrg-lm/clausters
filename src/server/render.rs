//! Offline (NRT) rendering: the same engine, driven by a score instead of
//! cpal (M7).
//!
//! A [`Score`] is a time-ordered list of OSC bundles. On disk it uses the
//! scsynth binary score format — `[i32 big-endian byte count][OSC packet]`
//! repeated — where each bundle's NTP timetag is read as **seconds from the
//! start of the render** (the immediate tag is time 0). Rendering is single
//! threaded and synchronous: async commands (defs, buffers) complete before
//! time advances, like scsynth's NRT mode, while the schedulable subset
//! travels through the engine's own queue (M6), so a bundle landing
//! mid-block splits the block exactly as it would in real time — the offline
//! render of a score is sample-identical to a perfectly timed live take.
//!
//! The render ends at the time of the **last** bundle, whose commands
//! therefore produce no sound: close a score with a dummy bundle (e.g. a
//! final `/n_free`) to set the total duration.

use std::path::Path;
use std::sync::Arc;

use rosc::{OscMessage, OscPacket, OscTime};

use crate::dsp::NUM_AUDIO_BUSES;
use crate::dsp::buffer::{BufferPool, empty_pool};
#[cfg(feature = "faust")]
use crate::osc::translate::parse_d_faust;
use crate::osc::translate::{CmdTranslator, parse_buffer_msg};
use crate::server::engine::{
    BLOCK_SIZE, Cmd, Engine, EngineHandle, Garbage, engine_pair_with_workers,
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
    /// M13 DSP workers for `/g_parallel` groups. Parallel rendering is
    /// bit-identical to sequential (disjoint stages), just faster.
    pub workers: usize,
}

impl Default for RenderConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48000.0,
            channels: 2,
            workers: 0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RenderStats {
    pub frames: u64,
    pub events: usize,
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
             final bundle (e.g. an /n_free) at the total duration"
                .into(),
        );
    }

    crate::dsp::denormals::flush_to_zero();
    let (engine, handle) = engine_pair_with_workers(sr as f32, cfg.channels, cfg.workers);
    let mut r = Renderer {
        engine,
        handle,
        translator: CmdTranslator::new(sr as f32),
        buffers: empty_pool(),
        block: vec![0.0; BLOCK_SIZE * cfg.channels],
        now: 0,
        channels: cfg.channels,
        sample_rate: sr,
        dropped: false,
    };

    for ev in &score.events {
        let target = ((ev.time * sr).round() as u64).min(total);
        // The event lands in a future block: render up to it.
        while r.now + BLOCK_SIZE as u64 <= target {
            r.process_block(total, &mut sink)?;
        }
        let cmds = r
            .event_cmds(&ev.messages)
            .map_err(|e| format!("score event at {:.6}s: {e}", ev.time))?;
        if cmds.is_empty() {
            continue;
        }
        // The engine splits the upcoming block at the exact sample (M6).
        if r.handle.send(Cmd::Schedule { time: target, cmds }).is_err() {
            return Err(format!(
                "score event at {:.6}s: command FIFO full (too many bundles inside one block)",
                ev.time
            ));
        }
    }
    while r.now < total {
        r.process_block(total, &mut sink)?;
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
/// `int24` or `float`, like `/b_write`.
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
    /// Mirror of the engine's buffer pool (the renderer's "network side").
    buffers: BufferPool,
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
            "/d_recv" => self.translator.d_recv(&msg.args).map(|_| ()),
            "/d_free" => self.translator.d_free(&msg.args),
            "/d_faust" => self.d_faust(&msg.args),
            "/d_graph" => self.translator.d_graph(&msg.args).map(|_| ()),
            "/b_alloc" | "/b_allocRead" | "/b_read" | "/b_write" | "/b_zero" | "/b_free" => {
                let (index, job) = parse_buffer_msg(
                    msg.addr.as_str(),
                    &msg.args,
                    &self.buffers,
                    self.sample_rate,
                )?;
                // The buffer is built now, but installed sample-accurately
                // with the rest of the event's commands.
                match run_job(job)? {
                    NrtAction::Install(buffer) => {
                        self.buffers[index as usize] = Some(Arc::clone(&buffer));
                        cmds.push(Cmd::SetBuffer {
                            index: index as usize,
                            buffer: Some(buffer),
                        });
                    }
                    NrtAction::Clear => {
                        self.buffers[index as usize] = None;
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

    #[cfg(feature = "faust")]
    fn d_faust(&mut self, args: &[rosc::OscType]) -> Result<(), String> {
        use crate::faust::compiler::{CompilePayload, compile};
        let (name, def) = parse_d_faust(args)?;
        let payload = CompilePayload::classify(def);
        let def = compile(&name, &payload)?;
        self.translator.faust_defs.insert(name, Arc::new(def));
        Ok(())
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
                    tracing::warn!(
                        "nrt render: engine rejected node {id} (duplicate ID, bad target or full table)"
                    );
                }
                Garbage::FreedGroup { .. } | Garbage::FreedBuffer(_) => {}
            }
        }
        while self.handle.pop_event().is_some() {}
    }
}
