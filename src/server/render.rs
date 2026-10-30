//! Offline (NRT) rendering: the same engine, driven by a score instead of
//! cpal.
//!
//! An [`OscScore`] is a time-ordered list of OSC bundles. On disk it uses the
//! scsynth binary score format -- `[i32 big-endian byte count][OSC packet]`
//! repeated -- where each bundle's NTP timetag is read as **seconds from the
//! start of the render** (the immediate tag is time 0). Rendering is single
//! threaded and synchronous: async commands (defs, buffers) complete before
//! time advances, like scsynth's NRT mode, while the schedulable subset
//! travels through the engine's own queue, so a bundle landing
//! mid-block splits the block exactly as it would in real time -- the offline
//! render of a score is sample-identical to a perfectly timed live take.
//!
//! The render ends at the time of the **last** bundle, whose commands
//! therefore produce no sound: close a score with a dummy bundle (e.g. a
//! final `/node_free`) to set the total duration.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use rosc::{OscMessage, OscPacket, OscTime, OscType};

use crate::dsp::buffer::Buffer;
use crate::dsp::{Limits, NUM_AUDIO_BUSES, ReplyKind};
#[cfg(feature = "faust")]
use crate::osc::translate::parse_def_send_faust;
use crate::osc::translate::{BUFFER_JOBS, CmdTranslator, parse_buffer_gen, parse_buffer_msg};
use crate::server::engine::{
    BLOCK_SIZE, Cmd, DEFAULT_AUDIO_BUSES, DEFAULT_CONTROL_BUSES, Engine, EngineHandle, Garbage,
    NodeEventKind, engine_pair_full,
};
use crate::server::nrt::{NrtAction, run_job};
use crate::server::render_log::{self, LogLine};

/// One score entry: the messages of a bundle, executed atomically at `time`
/// seconds from the start of the render.
#[derive(Clone)]
pub struct ScoreEvent {
    pub time: f64,
    pub messages: Vec<OscMessage>,
}

/// A render script: events sorted by time (stable for equal times).
///
/// An **OSC score** -- timed bundles -- and not a symbolic score, the notation
/// `clausters_core::notation` engraves as its `Score`; the prefix is what keeps
/// the two apart, and the Python client names this one `OscScore` too.
pub struct OscScore {
    events: Vec<ScoreEvent>,
}

impl OscScore {
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

    /// Every `/def_send faust` in the score, as `(name, format, text)` -- the
    /// same three fields a live compile job carries (`"source"`, `"boxes"` or
    /// `"signals"`).
    ///
    /// What it is for: a host whose Faust compiler answers *later* than the
    /// renderer can wait. A native server has none -- it compiles a def where it
    /// stands -- but a page's compiler is another scope, so the page reads the
    /// score here, compiles and links each def, and the render then finds them
    /// (see `faust::compiler_web`). Reading the score twice is the price of not
    /// writing a second reader of it in another language.
    #[cfg(feature = "faust")]
    pub fn faust_jobs(&self) -> Result<Vec<(String, &'static str, String)>, String> {
        let mut jobs = Vec::new();
        for ev in &self.events {
            for msg in &ev.messages {
                if msg.addr != "/def_send" {
                    continue;
                }
                if !matches!(msg.args.first(), Some(OscType::String(f)) if f == "faust") {
                    continue;
                }
                let (name, def) = parse_def_send_faust(&msg.args[1..])?;
                let payload = crate::faust::CompilePayload::classify(def);
                jobs.push((name, payload.kind(), payload.text().to_string()));
            }
        }
        Ok(jobs)
    }

    /// **Every file the score reads**, once each, in the order they are
    /// first named: the paths of its synth defs' `DiskIn`s.
    ///
    /// What it is for is a page, whose render opens a file only before it
    /// starts: an offline render's host opens each of these first, and the
    /// render reads them through `clausters_core::files`, as a page compiles
    /// a score's Faust defs first.
    pub fn files(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for msg in self.events.iter().flat_map(|ev| &ev.messages) {
            for path in def_paths(msg, &["DiskIn"]) {
                add_once(&mut out, &path);
            }
        }
        out
    }

    /// **Every file the score's synths write**, once each: the paths of its
    /// synth defs' `DiskOut`s, which a page's render creates before it
    /// starts, as it opens [`files`](Self::files).
    pub fn outputs(&self) -> Vec<String> {
        let mut out = Vec::new();
        for msg in self.events.iter().flat_map(|ev| &ev.messages) {
            for path in def_paths(msg, &["DiskOut"]) {
                add_once(&mut out, &path);
            }
        }
        out
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
        let bytes =
            clausters_core::files::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
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
/// the OSC immediate tag (seconds 0, fractional <= 1) is time 0.
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
    /// draw one from entropy ([`clausters_core::rng::entropy_seed`]) -- the
    /// default.
    ///
    /// A random process is unpredictable first: an unconfigured render is a
    /// *new take*, the way playing a take with noise in it again gives you
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
    /// costs a compare per sample and saves the caller a second pass -- which
    /// matters most for [`render_to_wav`], where the samples are gone by the
    /// time the call returns and measuring them again would mean reading the
    /// file back.
    pub peak: Vec<f32>,
    /// RMS per output channel over the whole render.
    pub rms: Vec<f32>,
    /// The seed this render's stochastic UGens actually started from --
    /// whatever [`RenderConfig::seed`] asked for, or the one drawn from
    /// entropy when it asked for nothing.
    ///
    /// Reporting it is what makes an unpredictable default usable: you play a
    /// score, you like the take, and this is how you get it back. Feed it to
    /// `RenderConfig::seed` (`--seed`, `seed=`) and the render repeats.
    pub seed: u64,
    /// What the render logged at `info` and above, in order -- a node the
    /// engine rejected, a `Poll`'s line. A render with no logger of its own
    /// (the embed ABI, the wasm entry point) hands these to its caller; see
    /// [`render_log`].
    pub log: Vec<LogLine>,
}

/// Renders a score, handing each processed chunk (interleaved, at most one
/// block) to `sink`. This is the core the WAV and in-memory frontends wrap.
///
/// Side effect: leaves the calling thread in flush-to-zero FPU mode (see
/// [`crate::dsp::denormals`]) -- the same mode the real-time callback runs
/// in, so the offline render stays sample-identical to a live take.
pub fn render(
    score: &OscScore,
    cfg: &RenderConfig,
    mut sink: impl FnMut(&[f32]) -> Result<(), String>,
) -> Result<RenderStats, String> {
    let mut render = OfflineRender::new(score, cfg)?;
    while !render.step(u64::MAX, &mut sink)? {}
    render.finish()
}

/// **An offline render, a run of frames at a time.** [`render`] is one of
/// these driven to its end in one call; a host that has to serve the
/// render's file streams between two runs -- a page, whose files are read by
/// the host and not by the engine (`dsp::disk_web`) -- drives it itself:
/// [`step`](Self::step) as far as it likes, the streams served, and again,
/// until it answers that it is done, then [`finish`](Self::finish). The score
/// is read once and the engine is the same one, so the samples are the same
/// whatever the runs.
///
/// What the render logs is kept across the runs, and comes back in the stats.
pub struct OfflineRender {
    r: Renderer,
    events: Vec<ScoreEvent>,
    /// The next event to schedule.
    next: usize,
    total: u64,
    peak: Vec<f32>,
    sumsq: Vec<f64>,
    seed: u64,
    log: Vec<LogLine>,
}

impl OfflineRender {
    /// Builds the engine for `score` under `cfg`, or says why it cannot.
    pub fn new(score: &OscScore, cfg: &RenderConfig) -> Result<Self, String> {
        let (built, log) = render_log::during(|| Self::build(score, cfg));
        let mut render = built?;
        render.log = log;
        Ok(render)
    }

    fn build(score: &OscScore, cfg: &RenderConfig) -> Result<Self, String> {
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
                "empty render: a score ends at its last bundle's time -- close it with a \
                 final bundle (e.g. an /node_free) at the total duration"
                    .into(),
            );
        }
        // Resolved once, here, so the number that goes into the translator is
        // the number that comes back in the stats -- a take is only repeatable
        // if the render reports the seed it actually used.
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
        let r = Renderer {
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
                t.offline = true;
                #[cfg(all(
                    any(feature = "plugin-clap", feature = "plugin-vst3"),
                    target_os = "linux"
                ))]
                {
                    t.offline_plugins = true;
                }
                t
            },
            block: vec![0.0; BLOCK_SIZE * cfg.channels],
            now: 0,
            channels: cfg.channels,
            sample_rate: sr,
            dropped: false,
        };
        Ok(Self {
            r,
            events: score.events.clone(),
            next: 0,
            total,
            // Measured on the way past, not in a second pass: the driver
            // already touches every sample handing it to the sink, and for
            // `render_to_wav` the samples are gone once the call returns.
            peak: vec![0.0; cfg.channels],
            sumsq: vec![0.0; cfg.channels],
            seed,
            log: Vec::new(),
        })
    }

    /// **Puts `buffer` at `index` before the render runs**: a buffer the
    /// caller made (one its own reader feeds) where the score names it, as
    /// a completed `/buffer_alloc` would put one.
    pub fn install(&mut self, index: usize, buffer: Arc<Buffer>) -> Result<(), String> {
        let Some(slot) = self.r.translator.buffers.get_mut(index) else {
            return Err(format!("no buffer index {index}"));
        };
        *slot = Some(Arc::clone(&buffer));
        self.r
            .handle
            .send_batch(vec![Cmd::SetBuffer {
                index,
                buffer: Some(buffer),
            }])
            .map_err(|_| "the render's command queue is full".to_string())
    }

    /// How many frames the render comes to.
    pub fn total_frames(&self) -> u64 {
        self.total
    }

    /// How many it has rendered so far.
    pub fn frames_done(&self) -> u64 {
        self.r.now.min(self.total)
    }

    /// **Renders about `frames` more frames** -- whole blocks, so the run may
    /// end a block past it -- handing each block to `sink`, and answers
    /// whether the render is done.
    pub fn step(
        &mut self,
        frames: u64,
        sink: &mut impl FnMut(&[f32]) -> Result<(), String>,
    ) -> Result<bool, String> {
        let (done, lines) = render_log::during(|| self.run(frames, sink));
        self.log.extend(lines);
        done
    }

    fn run(
        &mut self,
        frames: u64,
        sink: &mut impl FnMut(&[f32]) -> Result<(), String>,
    ) -> Result<bool, String> {
        crate::dsp::denormals::flush_to_zero();
        let until = self.r.now.saturating_add(frames);
        let total = self.total;
        let channels = self.r.channels;
        let (peak, sumsq) = (&mut self.peak, &mut self.sumsq);
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
        let sr = self.r.sample_rate;
        while let Some(ev) = self.events.get(self.next) {
            let target = ((ev.time * sr).round() as u64).min(total);
            // The event lands in a future block: render up to it.
            while self.r.now + BLOCK_SIZE as u64 <= target {
                if self.r.now >= until {
                    return Ok(false);
                }
                self.r.process_block(total, &mut measured)?;
            }
            let cmds = self
                .r
                .event_cmds(&ev.messages)
                .map_err(|e| format!("score event at {:.6}s: {e}", ev.time))?;
            let time = ev.time;
            self.next += 1;
            if cmds.is_empty() {
                continue;
            }
            // The engine splits the upcoming block at the exact sample.
            if self
                .r
                .handle
                .send(Cmd::Schedule { time: target, cmds })
                .is_err()
            {
                return Err(format!(
                    "score event at {time:.6}s: command FIFO full (too many bundles inside one block)"
                ));
            }
        }
        while self.r.now < total {
            if self.r.now >= until {
                return Ok(false);
            }
            self.r.process_block(total, &mut measured)?;
        }
        Ok(true)
    }

    /// The render's numbers, once [`step`](Self::step) has said it is done.
    pub fn finish(mut self) -> Result<RenderStats, String> {
        self.r.collect();
        if self.r.dropped {
            return Err(
                "the engine dropped scheduled bundles (queue full): the score is too dense".into(),
            );
        }
        let total = self.total;
        Ok(RenderStats {
            frames: total,
            events: self.events.len(),
            seed: self.seed,
            peak: self.peak,
            rms: self
                .sumsq
                .iter()
                .map(|&s| {
                    if total == 0 {
                        0.0
                    } else {
                        (s / total as f64).sqrt() as f32
                    }
                })
                .collect(),
            log: self.log,
        })
    }
}

/// The paths a `/def_send synth` message's defs give the UGens of `kinds`,
/// in order; none for another message.
fn def_paths(msg: &OscMessage, kinds: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    if msg.addr != "/def_send"
        || !matches!(msg.args.first(), Some(OscType::String(f)) if f == "synth")
    {
        return out;
    }
    for arg in &msg.args[1..] {
        let OscType::String(text) = arg else {
            continue;
        };
        let Ok(def) = serde_json::from_str::<serde_json::Value>(text) else {
            continue;
        };
        for ugen in def["ugens"].as_array().into_iter().flatten() {
            if kinds.iter().any(|kind| ugen["kind"] == *kind)
                && let Some(path) = ugen["path"].as_str()
            {
                out.push(path.to_string());
            }
        }
    }
    out
}

/// Pushes `path` onto `out` unless it is there.
fn add_once(out: &mut Vec<String>, path: &str) {
    if !out.iter().any(|p| p == path) {
        out.push(path.to_string());
    }
}

/// Renders a score into an interleaved in-memory signal (tests, asserts).
pub fn render_to_vec(
    score: &OscScore,
    cfg: &RenderConfig,
) -> Result<(Vec<f32>, RenderStats), String> {
    let mut out = Vec::new();
    let stats = render(score, cfg, |chunk| {
        out.extend_from_slice(chunk);
        Ok(())
    })?;
    Ok((out, stats))
}

/// Renders a score straight to a sound file: a WAV, closed as an RF64 if
/// the render outgrew 4 GB, or a W64 or an RF64 by the path's extension
/// (`dsp::pcm`). `sample_format` is `int16`, `int24` or `float`, like
/// `/buffer_write`.
pub fn render_to_wav(
    score: &OscScore,
    cfg: &RenderConfig,
    path: impl AsRef<Path>,
    sample_format: &str,
) -> Result<RenderStats, String> {
    render_to_wav_with(score, cfg, path, sample_format, |_, _| {})
}

/// **A render whose output is kept nowhere**: what its synths write is its
/// work. `progress` as [`render_to_wav_with`].
pub fn render_discarding(
    score: &OscScore,
    cfg: &RenderConfig,
    progress: impl FnMut(u64, u64),
) -> Result<RenderStats, String> {
    render_discarding_while(score, cfg, &AtomicBool::new(true), progress)
}

/// [`render_discarding`], **while `live` holds**: let go, the render stops
/// at the end of its run and answers that it was called off.
pub fn render_discarding_while(
    score: &OscScore,
    cfg: &RenderConfig,
    live: &AtomicBool,
    progress: impl FnMut(u64, u64),
) -> Result<RenderStats, String> {
    drive(score, cfg, live, |_: &[f32]| Ok(()), progress)
}

/// [`render_to_wav`], telling `progress` the frames done and the frames the
/// render comes to after each second of the score.
pub fn render_to_wav_with(
    score: &OscScore,
    cfg: &RenderConfig,
    path: impl AsRef<Path>,
    sample_format: &str,
    progress: impl FnMut(u64, u64),
) -> Result<RenderStats, String> {
    render_to_wav_while(
        score,
        cfg,
        path,
        sample_format,
        &AtomicBool::new(true),
        progress,
    )
}

/// [`render_to_wav_with`], **while `live` holds**: let go, the render stops
/// at the end of its run, leaves what it wrote for its caller to delete, and
/// answers that it was called off. A save of hours is one render, so this is
/// how a hand that cancels it is heard before it ends.
pub fn render_to_wav_while(
    score: &OscScore,
    cfg: &RenderConfig,
    path: impl AsRef<Path>,
    sample_format: &str,
    live: &AtomicBool,
    progress: impl FnMut(u64, u64),
) -> Result<RenderStats, String> {
    let path = path.as_ref().to_string_lossy().into_owned();
    let mut writer =
        crate::dsp::pcm::Writer::create(&path, cfg.channels, cfg.sample_rate, sample_format)?;
    let stats = drive(
        score,
        cfg,
        live,
        |chunk: &[f32]| writer.write(chunk),
        progress,
    )?;
    writer.finalize()?;
    Ok(stats)
}

/// What a render that was let go answers.
pub const CALLED_OFF: &str = "called off";

/// **A render driven to its end into `sink`, while `live` holds**, the
/// `buffers` the caller made put where the score names them first: each run
/// of frames handed to `sink` as it is made, `progress` told the frames done
/// and the frames it comes to after each. What a caller that does more with
/// the samples than write them (measures them as they go), or that feeds a
/// buffer the render reads, drives.
pub fn render_into_while(
    score: &OscScore,
    cfg: &RenderConfig,
    buffers: Vec<(usize, Arc<Buffer>)>,
    live: &AtomicBool,
    sink: impl FnMut(&[f32]) -> Result<(), String>,
    progress: impl FnMut(u64, u64),
) -> Result<RenderStats, String> {
    drive_with(score, cfg, buffers, live, sink, progress)
}

/// **A render driven to its end** by the helpers above: a second of the
/// score a run, `progress` told after each. In a page the render's file
/// streams are served between two runs ([`page_streams`]), where natively
/// each has a thread of its own.
fn drive(
    score: &OscScore,
    cfg: &RenderConfig,
    live: &AtomicBool,
    sink: impl FnMut(&[f32]) -> Result<(), String>,
    progress: impl FnMut(u64, u64),
) -> Result<RenderStats, String> {
    drive_with(score, cfg, Vec::new(), live, sink, progress)
}

/// [`drive`], `buffers` installed first.
fn drive_with(
    score: &OscScore,
    cfg: &RenderConfig,
    buffers: Vec<(usize, Arc<Buffer>)>,
    live: &AtomicBool,
    mut sink: impl FnMut(&[f32]) -> Result<(), String>,
    mut progress: impl FnMut(u64, u64),
) -> Result<RenderStats, String> {
    #[cfg(all(target_arch = "wasm32", feature = "synth"))]
    let mut served = page_streams::Served::install(cfg.sample_rate);
    let mut render = OfflineRender::new(score, cfg)?;
    for (index, buffer) in buffers {
        render.install(index, buffer)?;
    }
    let run = cfg.sample_rate.max(1.0) as u64;
    // under what an output stream's ring holds between two drains
    #[cfg(all(target_arch = "wasm32", feature = "synth"))]
    let run = run.min(page_streams::RUN_FRAMES);
    while !render.step(run, &mut sink)? {
        #[cfg(all(target_arch = "wasm32", feature = "synth"))]
        served.drain();
        progress(render.frames_done(), render.total_frames());
        if !live.load(Ordering::Acquire) {
            #[cfg(all(target_arch = "wasm32", feature = "synth"))]
            served.close();
            return Err(CALLED_OFF.into());
        }
    }
    progress(render.total_frames(), render.total_frames());
    let stats = render.finish();
    // what the synths that just went left behind
    #[cfg(all(target_arch = "wasm32", feature = "synth"))]
    served.close();
    stats
}

/// **A render's file streams in a page, served through the files layer.**
///
/// A page's engine reaches no file from inside a block (`dsp::disk_web`): its
/// input streams read through the files installed for a render, and its
/// output (`DiskOut`) waits in its ring for whoever drains
/// them. Natively a thread per stream does both. Here the thread a render
/// runs on reaches the page's files synchronously (`clausters_core::files`:
/// an application's thread through the page's file worker, the page's render
/// Worker through the files it opened), so the render serves its own streams: the inputs read through that layer
/// the moment they need to, and between two runs the outputs are drained
/// into the files a native render would write, in the same formats.
#[cfg(all(target_arch = "wasm32", feature = "synth"))]
mod page_streams {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::io::{Read, Seek, SeekFrom};
    use std::rc::Rc;

    use clausters_core::files::File;

    use crate::dsp::disk::{self, Direction, PageFiles};
    use crate::dsp::pcm::Writer;

    /// Frames per run: a `DiskOut`'s ring holds more than this many samples
    /// between two drains.
    pub(super) const RUN_FRAMES: u64 = 16384;

    /// The files the render reads, each opened once.
    #[derive(Default)]
    struct LayerFiles {
        open: RefCell<HashMap<String, File>>,
    }

    impl LayerFiles {
        fn with<T>(
            &self,
            path: &str,
            f: impl FnOnce(&mut File) -> std::io::Result<T>,
        ) -> Result<T, String> {
            let mut open = self.open.borrow_mut();
            if !open.contains_key(path) {
                let file = File::open(path).map_err(|e| format!("{path}: {e}"))?;
                open.insert(path.to_string(), file);
            }
            let file = open.get_mut(path).expect("opened above");
            f(file).map_err(|e| format!("{path}: {e}"))
        }
    }

    impl PageFiles for LayerFiles {
        fn size(&self, path: &str) -> Result<u64, String> {
            self.with(path, |file| file.len())
        }

        fn read(&self, path: &str, offset: u64, len: usize) -> Result<Vec<u8>, String> {
            self.with(path, |file| {
                file.seek(SeekFrom::Start(offset))?;
                let mut out = vec![0u8; len];
                let mut got = 0;
                while got < len {
                    match file.read(&mut out[got..])? {
                        0 => break,
                        n => got += n,
                    }
                }
                out.truncate(got);
                Ok(out)
            })
        }
    }

    /// The render's outputs being drained: each `DiskOut` into its file.
    pub(super) struct Served {
        rate: f64,
        recordings: HashMap<u32, Option<Writer>>,
        scratch: Vec<f32>,
    }

    impl Served {
        /// The render's files installed for its input streams, which read
        /// through them from the moment the render is built.
        pub(super) fn install(rate: f64) -> Served {
            disk::install_files(Some(Rc::new(LayerFiles::default())));
            Served {
                rate,
                recordings: HashMap::new(),
                scratch: Vec::new(),
            }
        }

        /// Empties the outputs into their files, and writes the files of the
        /// streams whose synths are gone.
        pub(super) fn drain(&mut self) {
            for row in disk::poll() {
                match row.direction {
                    Direction::Out => self.record(&row),
                    Direction::In => {}
                }
            }
        }

        fn pull(&mut self, id: u32, samples: usize) -> &[f32] {
            self.scratch.resize(samples, 0.0);
            let got = disk::pull(id, &mut self.scratch);
            &self.scratch[..got]
        }

        fn record(&mut self, row: &disk::StreamRequest) {
            if !self.recordings.contains_key(&row.id) {
                let format = if row.format.is_empty() {
                    "int16"
                } else {
                    &row.format
                };
                // a file that cannot be made is told once, and the stream is
                // drained into nothing, as a native `DiskOut`'s thread ends
                let writer = Writer::create(&row.path, 1, self.rate, format)
                    .inspect_err(|e| tracing::warn!("DiskOut: {e}"))
                    .ok();
                self.recordings.insert(row.id, writer);
            }
            if row.samples > 0 {
                let run = self.pull(row.id, row.samples).to_vec();
                if let Some(Some(writer)) = self.recordings.get_mut(&row.id)
                    && let Err(e) = writer.write(&run)
                {
                    tracing::warn!("DiskOut: {e}");
                    self.recordings.insert(row.id, None);
                }
            }
            // `looping` says the synth is gone
            if row.looping {
                disk::release(row.id);
                if let Some(Some(writer)) = self.recordings.remove(&row.id)
                    && let Err(e) = writer.finalize()
                {
                    tracing::warn!("DiskOut: {e}");
                }
            }
        }

        /// The last drain, once the render's synths are gone, and the files
        /// taken back.
        pub(super) fn close(mut self) {
            self.drain();
            for (_, writer) in self.recordings.drain() {
                if let Some(writer) = writer
                    && let Err(e) = writer.finalize()
                {
                    tracing::warn!("DiskOut: {e}");
                }
            }
        }
    }

    impl Drop for Served {
        fn drop(&mut self) {
            disk::install_files(None);
        }
    }
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
        // A UGen that refused what it was given plays silence; offline, where
        // nobody reads a log, that would be a wrong file with no error, so the
        // render stops on it. A `Poll` posts its line exactly as the live
        // server does; the replies meant for clients have no one to reach.
        while let Some(msg) = self.handle.pop_reply() {
            match msg.kind {
                ReplyKind::PluginNoteEnd | ReplyKind::PluginControl => {}
                ReplyKind::Fault => {
                    return Err(format!(
                        "at {:.6}s, node {}: {}",
                        self.now as f64 / self.sample_rate,
                        msg.node_id,
                        crate::dsp::describe_fault(&msg)
                    ));
                }
                ReplyKind::Poll => {
                    tracing::info!(
                        target: crate::logging::OSC_TARGET,
                        "{}",
                        crate::dsp::poll_line(&msg)
                    );
                }
                ReplyKind::Trig | ReplyKind::Reply => {}
            }
        }
        // The last block is truncated to the requested length.
        let frames = (total - self.now).min(BLOCK_SIZE as u64) as usize;
        sink(&self.block[..frames * self.channels])?;
        self.now += BLOCK_SIZE as u64;
        self.collect();
        Ok(())
    }

    /// Translates one event's messages into engine commands, running async
    /// commands (defs, buffers) synchronously right now -- scsynth NRT
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
                        "expected a family string (\"synth\", \"faust\", \"graph\" or \"plugin\")"
                            .into(),
                    );
                };
                let rest = &msg.args[1..];
                match family.as_str() {
                    "synth" => self.translator.d_recv(rest).map(|_| ()),
                    "faust" => self.d_faust(rest),
                    "graph" => self.translator.d_graph(rest).map(|_| ()),
                    "plugin" => self.d_plugin(rest),
                    other => Err(format!("unknown def family '{other}'")),
                }
            }
            "/def_free" => self.translator.d_free(&msg.args),
            addr if BUFFER_JOBS.contains(&addr) => {
                let (index, job) = if msg.addr == "/buffer_gen" {
                    parse_buffer_gen(&msg.args, &self.translator.buffers, self.sample_rate)?
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
                // translator's pool -- the same one `make_synth` reads to fill a
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
            // **The transports, offline.** A render has no network thread to
            // keep a transport's grid and broadcast it, so what a score can
            // ask of one is what the engine itself does: bind the group it
            // governs, roll it, stop it and stand it at a sample -- each at
            // its exact sample, like the rest of the event -- which is what a
            // render that plays a fed take at the transport's position
            // (`TransportDiskIn`) needs. A play from a beat needs a grid,
            // which a render keeps none of, and is refused.
            "/transport_group"
            | "/transport_play"
            | "/transport_stop"
            | "/transport_locateSample" => {
                let transport = match msg.args.first() {
                    Some(OscType::Int(k))
                        if *k >= 0 && (*k as usize) < crate::dsp::DEFAULT_TRANSPORTS =>
                    {
                        *k as usize
                    }
                    _ => {
                        return Err(format!(
                            "expected a transport index below {}",
                            crate::dsp::DEFAULT_TRANSPORTS
                        ));
                    }
                };
                let rest = &msg.args[1..];
                cmds.push(match (msg.addr.as_str(), rest) {
                    ("/transport_group", [OscType::Int(id)]) => {
                        Cmd::TransportGroup { transport, id: *id }
                    }
                    ("/transport_play", []) => Cmd::TransportRun {
                        transport,
                        rolling: true,
                    },
                    ("/transport_play", [_]) => {
                        return Err(
                            "an offline render keeps no grid, so a transport plays from where it \
                             stands: locate it with /transport_locateSample first"
                                .into(),
                        );
                    }
                    ("/transport_stop", []) => Cmd::TransportRun {
                        transport,
                        rolling: false,
                    },
                    ("/transport_locateSample", [OscType::Int(n)]) => Cmd::TransportLocate {
                        transport,
                        position: (*n).max(0) as u64,
                    },
                    ("/transport_locateSample", [OscType::Long(n)]) => Cmd::TransportLocate {
                        transport,
                        position: (*n).max(0) as u64,
                    },
                    _ => return Err("unexpected arguments".into()),
                });
                Ok(())
            }
            _ => self.translator.translate(msg, cmds),
        }
    }

    #[cfg(all(
        any(feature = "plugin-clap", feature = "plugin-vst3"),
        target_os = "linux"
    ))]
    fn d_plugin(&mut self, args: &[OscType]) -> Result<(), String> {
        let Some(OscType::String(name)) = args.first() else {
            return Err("expected plugin name and reference JSON".into());
        };
        let reference = serde_json::from_slice(crate::osc::args::json_payload(&args[1..])?)
            .map_err(|e| format!("invalid plugin reference: {e}"))?;
        let def = crate::plugin::PluginDef::probe(reference, self.sample_rate as f32)?;
        self.translator.d_free(&[OscType::String(name.clone())])?;
        self.translator
            .plugin_defs
            .insert(name.clone(), Arc::new(def));
        Ok(())
    }

    #[cfg(not(all(
        any(feature = "plugin-clap", feature = "plugin-vst3"),
        target_os = "linux"
    )))]
    fn d_plugin(&mut self, _args: &[OscType]) -> Result<(), String> {
        Err("server built without native CLAP support".into())
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
    /// `faust::compiler_web`) -- there is no turn in which a result could
    /// arrive, because time does not advance until the def is loaded.
    ///
    /// So a page does the same work in the other order: the host reads the
    /// score's Faust defs with [`OscScore::faust_jobs`] *before* the render
    /// starts, compiles and links each one, and this becomes the lookup. A def
    /// that is not there was not offered to the host, which is a host bug and
    /// says so rather than failing as a Faust error.
    #[cfg(all(feature = "faust", target_arch = "wasm32"))]
    fn d_faust(&mut self, args: &[rosc::OscType]) -> Result<(), String> {
        let (name, _) = parse_def_send_faust(args)?;
        match crate::faust::compiler::prelinked(&name) {
            Some(def) => {
                self.translator.faust_defs.insert(name, def);
                Ok(())
            }
            None => Err(format!(
                "the Faust def \"{name}\" was not compiled before this render: a page compiles \
                 the score's defs first (see the score's faust jobs) and links them into the \
                 renderer"
            )),
        }
    }

    #[cfg(not(feature = "faust"))]
    fn d_faust(&mut self, _args: &[rosc::OscType]) -> Result<(), String> {
        Err("server built without faust support".into())
    }

    fn collect(&mut self) {
        while let Some(g) = self.handle.pop_garbage() {
            match g {
                Garbage::RetiredCompensation(_) => {}
                Garbage::FreedSynth { id, synth, .. } => {
                    self.translator.forget_freed_synth(id, synth.incarnation())
                }
                Garbage::SpentBundle(_) => {}
                Garbage::RejectedBundle(_) => self.dropped = true,
                Garbage::RejectedSynth { id, why, .. } | Garbage::RejectedGroup { id, why, .. } => {
                    self.translator.release_node_id(id);
                    crate::server::engine::report_rejected(id, why, "nrt render");
                }
                // A score has no lanes: nothing here sends one an entry.
                Garbage::FreedGroup { .. }
                | Garbage::RetiredSynth(_)
                | Garbage::FreedBuffer(_)
                | Garbage::EventLaneSpent { .. }
                | Garbage::TransportEnded { .. }
                | Garbage::TransportCued { .. } => {}
            }
        }
        let mut compensation = Vec::new();
        self.translator.prepare_compensation(&mut compensation, 0);
        if self.handle.send_batch(compensation).is_err() {
            self.dropped = true;
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
