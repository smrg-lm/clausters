//! The engine's JS door (the B track): a thin wasm-bindgen shell over the
//! `clausters` server crate, the browser sibling of `clausters-ffi` (the C
//! door) and of the embed C ABI in `src/embed.rs`.
//!
//! The shell owns no logic: everything it exposes is a one-call wrapper over
//! the crate's own entry points (`Score::from_bytes`, `render_to_vec`), so
//! native cargo tests exercise the identical code path the browser runs. The
//! wasm-bindgen attributes exist only on the wasm target; natively this is a
//! plain rlib.
//!
//! Workers are always 0 on wasm: the browser has no worker-pool threads, and
//! `workers = 0` is the sequential in-thread schedule, bit-identical to any
//! worker count by design.
//!
//! Parity caveat (recorded in `docs/decisions.md`): wasm has no flush-to-zero
//! mode, so native↔wasm bit-identity holds where the render stays out of the
//! denormal range — the parity harness asserts on denormal-free scores.

use clausters::server::render::{RenderConfig, Score, render_to_vec};

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

/// The libm a linked Faust module imports from us. Declared for its exports
/// alone — nothing in this crate calls it.
#[cfg(target_arch = "wasm32")]
mod faust_math;

/// The embed / IPC ABI version this build speaks (`clausters_abi_version`).
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn abi_version() -> u32 {
    clausters::embed::clausters_abi_version()
}

/// Renders a binary score (the `--nrt` format: length-prefixed OSC packets,
/// timetags in seconds from the start) synchronously into interleaved `f32`
/// samples (`frames * channels`). The JS side receives a `Float32Array`.
///
/// `seed` is the render's starting seed, or `None` for whatever the engine
/// picks. **On wasm that is a fixed value**: `entropy_seed` has no source
/// here, since `SystemTime` is not implemented on this target. The browser
/// *does* have one, so a caller that wants a fresh take each time passes a
/// word from `crypto.getRandomValues` — the shell forwards entropy from the
/// edge that has it rather than inventing any, which is the same reason it
/// owns no other logic.
fn render_score(
    score: &[u8],
    sample_rate: f64,
    channels: u32,
    seed: Option<u64>,
) -> Result<(Vec<f32>, u64), String> {
    let score = Score::from_bytes(score)?;
    let cfg = RenderConfig {
        sample_rate,
        channels: channels as usize,
        workers: 0,
        seed,
        // The wasm entry point exposes no capacity arguments, so a browser
        // render takes the defaults.
        ..RenderConfig::default()
    };
    render_to_vec(&score, &cfg).map(|(samples, stats)| (samples, stats.seed))
}

/// Native face of [`render`], for the in-crate tests.
#[cfg(not(target_arch = "wasm32"))]
pub fn render(
    score: &[u8],
    sample_rate: f64,
    channels: u32,
    seed: Option<u64>,
) -> Result<(Vec<f32>, u64), String> {
    render_score(score, sample_rate, channels, seed)
}

/// JS face: `render(scoreBytes, sampleRate, channels, seed?) -> Float32Array`,
/// throwing a `JsError` with the render's message on failure. The seed the
/// render used is read back with [`last_render_seed`].
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn render(
    score: &[u8],
    sample_rate: f64,
    channels: u32,
    seed: Option<u64>,
) -> Result<Vec<f32>, JsError> {
    render_score(score, sample_rate, channels, seed)
        .map(|(samples, seed)| {
            LAST_SEED.with(|s| s.set(seed));
            samples
        })
        .map_err(|e| JsError::new(&e))
}

#[cfg(target_arch = "wasm32")]
thread_local! {
    static LAST_SEED: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// The seed the last [`render`] on this thread used — how a caller gets back
/// to a take it liked. Separate from `render`'s return because the JS face
/// returns a bare `Float32Array`; a stats object is the shape to grow into if
/// the web client ever needs the frame, event and level counts too.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn last_render_seed() -> u64 {
    LAST_SEED.with(|s| s.get())
}

/// The live engine in pulled mode: a 1:1 JS face over
/// `clausters::embed::ClaustersHeadless` (which owns all the logic and is
/// exercised by the native `tests/headless.rs` suite). The AudioWorklet
/// processor drives it: OSC packets in over `send`, one `process` per render
/// quantum, replies drained with `poll`.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub struct WebServer {
    inner: clausters::embed::ClaustersHeadless,
    reply_buf: Vec<u8>,
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
impl WebServer {
    /// `unix_epoch`: Unix seconds at sample 0 (JS: `Date.now() / 1000`), the
    /// anchor that lets wall-clocked clients' bundle timetags land on this
    /// server's sample axis.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(constructor))]
    pub fn new(sample_rate: f64, channels: u32, unix_epoch: f64) -> Result<WebServer, JsErrorish> {
        let inner =
            clausters::embed::ClaustersHeadless::new(sample_rate, channels as usize, unix_epoch)
                .map_err(err)?;
        Ok(WebServer {
            inner,
            reply_buf: vec![0; 64 * 1024],
        })
    }

    /// Sets the ceiling on the bus indices one `/bus_stream` subscription may
    /// list — the page's half of the native `--max-stream-buses`, so an
    /// in-page engine is configured on the same axis as a server process
    /// (default 4096). A page whose document holds hundreds of live canvases
    /// subscribes a bus per meter, and the number it may ask for should be its
    /// own decision here as it is a server operator's there.
    ///
    /// What a client actually gets is this clamped by what the ring carries in
    /// one reply, and `/server_query.reply` reports that number to it.
    pub fn set_max_stream_buses(&mut self, n: u32) {
        self.inner.set_max_stream_buses(n as usize);
    }

    /// Pushes one complete OSC packet into the command ring, authored by
    /// `peer`. `false` = momentarily full (backpressure): retry next quantum.
    ///
    /// A page holds **several** independent clients over this one engine — the
    /// script and the GUI host, at least — and the server has to tell them
    /// apart or their `/bus_stream` subscriptions overwrite each other. The tag
    /// is the page's to assign; there is no handshake.
    pub fn send(&self, peer: u32, packet: &[u8]) -> bool {
        self.inner.send_as(peer, packet)
    }

    /// One pending reply as `[peer, ...bytes]`, or `undefined`/`None` when none
    /// is pending: the first byte group says **who the reply is for**, so the
    /// page routes it to that client instead of handing every reply to all of
    /// them.
    ///
    /// One `Vec` rather than a pair because the value crosses to JS: a tuple
    /// would be a JS array holding a second typed array, which costs an extra
    /// object per reply on the hottest path there is (every streamed bus
    /// snapshot). The peer rides as a `u32` little-endian prefix instead, and
    /// `readReply` in the loader unpacks it.
    pub fn poll(&mut self) -> Option<Vec<u8>> {
        let (peer, len) = self.inner.poll_from(&mut self.reply_buf)?;
        let mut out = Vec::with_capacity(4 + len);
        out.extend_from_slice(&peer.to_le_bytes());
        out.extend_from_slice(&self.reply_buf[..len]);
        Some(out)
    }

    /// Renders into `out` (interleaved, a multiple of `block_frames() *
    /// channels` samples): a serving turn before each engine block.
    pub fn process(&mut self, out: &mut [f32]) -> Result<(), JsErrorish> {
        self.inner.process_block(out).map_err(err)
    }

    /// The engine block size in frames (the granularity `process` needs).
    pub fn block_frames(&self) -> u32 {
        clausters::server::engine::BLOCK_SIZE as u32
    }

    /// The engine's sample counter (block-accurate; exact in an f64 for the
    /// first 2^53 samples — thousands of years of audio).
    pub fn clock(&self) -> f64 {
        self.inner.clock() as f64
    }

    /// Whether a `/server_quit` arrived; the page decides what closing means.
    pub fn quit_requested(&self) -> bool {
        self.inner.quit_requested()
    }

    /// Data-plane control-bus write (no command round trip).
    pub fn ctl_set(&self, index: u32, value: f32) {
        self.inner.ctl_set(index as usize, value);
    }

    /// Data-plane control-bus read.
    pub fn ctl_get(&self, index: u32) -> f32 {
        self.inner.ctl_get(index as usize)
    }

    /// Installs host-decoded samples as buffer `index` (the browser's
    /// `/buffer_allocRead` replacement: fetch + `decodeAudioData`, then this).
    pub fn buffer_load(
        &mut self,
        index: u32,
        channels: u32,
        sample_rate: f64,
        data: &[f32],
    ) -> Result<(), JsErrorish> {
        self.inner
            .buffer_load(index as usize, channels as usize, sample_rate, data)
            .map_err(err)
    }

    /// Begins a **staged** load and returns its ticket: the destination is
    /// allocated, no samples are copied.
    ///
    /// [`buffer_load`](Self::buffer_load) copies the whole take in one call,
    /// on this thread — which is the AudioWorklet's, the one that owes the next
    /// quantum. Measured natively, a five-minute stereo take is some fourteen
    /// times the quantum's budget (`examples/measure_turn.rs`), so a long take
    /// is loaded in runs instead: `begin`, `chunk` as often as the caller
    /// likes, `end`. Nothing is visible under `index` until `end`.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = bufferLoadBegin))]
    pub fn buffer_load_begin(
        &mut self,
        index: u32,
        channels: u32,
        sample_rate: f64,
        frames: u32,
    ) -> Result<f64, JsErrorish> {
        self.inner
            .buffer_load_begin(
                index as usize,
                channels as usize,
                sample_rate,
                frames as usize,
            )
            // JavaScript has no u64; a ticket is a small counter and a double
            // carries it exactly, which is the same trade the clock doors make.
            .map(|ticket| ticket as f64)
            .map_err(err)
    }

    /// Copies one run of interleaved samples into a staged load, at flat
    /// sample offset `at`. Costs what it copies: the caller picks the run, and
    /// therefore the deadline it fits in.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = bufferLoadChunk))]
    pub fn buffer_load_chunk(
        &mut self,
        ticket: f64,
        at: u32,
        data: &[f32],
    ) -> Result<(), JsErrorish> {
        self.inner
            .buffer_load_chunk(ticket as u64, at as usize, data)
            .map_err(err)
    }

    /// Installs a staged load: one pointer swap, the samples being already in.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = bufferLoadEnd))]
    pub fn buffer_load_end(&mut self, ticket: f64) -> Result<(), JsErrorish> {
        self.inner.buffer_load_end(ticket as u64).map_err(err)
    }

    /// Discards a staged load without installing it.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = bufferLoadCancel))]
    pub fn buffer_load_cancel(&mut self, ticket: f64) {
        self.inner.buffer_load_cancel(ticket as u64);
    }

    /// How many frames one [`buffer_load_chunk`](Self::buffer_load_chunk)
    /// should carry — the serving budget's number, read from the engine rather
    /// than repeated in JavaScript.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = installFrames))]
    pub fn install_frames(&self) -> u32 {
        clausters::osc::server::ServeBudget::default().install_frames as u32
    }

    /// Hands the jobs the host does better over to it — reading a soundfile,
    /// whose filesystem is the page's (OPFS, reachable only from a Worker) and
    /// not the engine's. Call it once, at boot, if the page has a Worker to do
    /// them; without it every job runs here, as before.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = delegateJobs))]
    pub fn delegate_jobs(&mut self) {
        self.inner.delegate_jobs();
    }

    /// The next job for the host, as JSON, or `undefined` if none is waiting:
    /// `{ticket, index, kind: "allocRead", path, fileStart, numFrames,
    /// channels}`. A delegated job blocks the buffer queue behind it, so this
    /// hands out at most one at a time.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = takeDelegated))]
    pub fn take_delegated(&mut self) -> Option<String> {
        use clausters::server::nrt::DelegatedKind;
        let job = self.inner.take_delegated()?;
        let DelegatedKind::AllocRead {
            path,
            file_start,
            num_frames,
            channels,
        } = job.kind;
        // Printed by hand rather than through serde: one shape, one place, and
        // the shell keeps carrying no dependency it does not need.
        let channels = channels
            .iter()
            .map(|c| c.to_string())
            .collect::<Vec<_>>()
            .join(",");
        Some(format!(
            r#"{{"ticket":{},"index":{},"kind":"allocRead","path":{},"fileStart":{},"numFrames":{},"channels":[{}]}}"#,
            job.ticket,
            job.index,
            json_string(&path),
            file_start,
            num_frames,
            channels,
        ))
    }

    /// Tells a `DiskIn` stream how many channels its file turned out to have.
    ///
    /// Natively the UGen opens the file and knows on the spot; here reading is
    /// asynchronous and belongs to another thread, so a stream is born
    /// shapeless, reports `channels: 0` in [`disk_poll`](Self::disk_poll), and
    /// plays silence until this arrives. Nothing is declared up front — a
    /// declaration would be a call the other client has no counterpart for.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = diskShape))]
    pub fn disk_shape(&mut self, id: u32, channels: u32) {
        #[cfg(target_arch = "wasm32")]
        clausters::dsp::disk::set_shape(id, channels as usize);
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (id, channels);
        }
    }

    /// Every open disk stream and what it wants right now, as JSON: an array of
    /// `{id, direction: "in"|"out", path, channels, looping, format, samples}`.
    /// `samples` is room to fill for an `in`, and samples waiting for an `out`.
    ///
    /// This is the whole interface between the graph and whatever is reading
    /// files: the host walks it each turn, fills what is hungry with
    /// [`disk_push`](Self::disk_push) and empties what is full with
    /// [`disk_pull`](Self::disk_pull).
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = diskPoll))]
    pub fn disk_poll(&mut self) -> String {
        #[cfg(target_arch = "wasm32")]
        {
            use clausters::dsp::disk::Direction;
            let rows: Vec<String> = clausters::dsp::disk::poll()
                .into_iter()
                .map(|r| {
                    format!(
                        r#"{{"id":{},"direction":"{}","path":{},"channels":{},"looping":{},"format":{},"samples":{}}}"#,
                        r.id,
                        if r.direction == Direction::In { "in" } else { "out" },
                        json_string(&r.path),
                        r.channels,
                        r.looping,
                        json_string(&r.format),
                        r.samples,
                    )
                })
                .collect();
            format!("[{}]", rows.join(","))
        }
        #[cfg(not(target_arch = "wasm32"))]
        "[]".to_string()
    }

    /// Pushes interleaved frames into a `DiskIn` stream; returns how many
    /// samples were taken. Fewer than offered means the ring filled and the
    /// rest is the caller's to offer again.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = diskPush))]
    pub fn disk_push(&mut self, id: u32, samples: &[f32]) -> u32 {
        #[cfg(target_arch = "wasm32")]
        {
            clausters::dsp::disk::push(id, samples) as u32
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (id, samples);
            0
        }
    }

    /// Pulls what a `DiskOut` stream has recorded, up to `max` samples.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = diskPull))]
    pub fn disk_pull(&mut self, id: u32, max: u32) -> Vec<f32> {
        #[cfg(target_arch = "wasm32")]
        {
            let mut out = vec![0.0f32; max as usize];
            let got = clausters::dsp::disk::pull(id, &mut out);
            out.truncate(got);
            out
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (id, max);
            Vec::new()
        }
    }

    /// Answers a delegated job: an empty `error` once the host has installed
    /// the result through a staged load, otherwise the message the command
    /// fails with. Emits the `/done` or `/fail` and unblocks the queue.
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen(js_name = finishDelegated))]
    pub fn finish_delegated(&mut self, ticket: f64, error: Option<String>) {
        let outcome = match error {
            Some(message) if !message.is_empty() => Err(message),
            _ => Ok(()),
        };
        self.inner.finish_delegated(ticket as u64, outcome);
    }

    /// The Faust compilations waiting for this page's compiler, as a JSON
    /// array (empty when there are none): `[{ticket, name, kind, def}]`, where
    /// `kind` is `"source"`, `"boxes"` or `"signals"` — which of the three def
    /// formats `def` is in.
    ///
    /// A page's Faust compiler is not a thread but the host: it compiles with
    /// `libfaust-wasm` in its Worker, strips the emitted module's data section,
    /// instantiates it against this engine's own memory and
    /// `__indirect_function_table` with its math imports bound to this
    /// engine's exports, and answers with [`finish_faust`](Self::finish_faust).
    /// Until it does, the `/def_send faust` is simply still in flight.
    ///
    /// wasm32 only, unlike the rest of this shell: the compiler queue exists
    /// only where the compiler is the host (see `clausters::faust`).
    #[cfg(target_arch = "wasm32")]
    #[wasm_bindgen(js_name = takeFaustJobs)]
    pub fn take_faust_jobs(&mut self) -> String {
        let jobs = self.inner.take_faust_jobs();
        let mut out = String::from("[");
        for (i, job) in jobs.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            // Printed by hand rather than through serde: one shape, one place,
            // and the shell keeps carrying no dependency it does not need.
            out.push_str(&format!(
                r#"{{"ticket":{},"name":{},"kind":{},"def":{}}}"#,
                job.ticket,
                json_string(&job.name),
                json_string(job.kind),
                json_string(&job.def),
            ));
        }
        out.push(']');
        out
    }

    /// Answers one compilation. `report` is the link report — a JSON object
    /// `{compute, init, size, inputs, outputs, params: [{name, index, init,
    /// min, max, step}]}` where `compute` and `init` are the table slots the
    /// module's exports were appended at and the rest is the compiler's own
    /// JSON — and emits `/done`. Pass `error` instead (and no report) to emit
    /// `/fail` with the compiler's message verbatim.
    ///
    /// **The report is trusted.** The slots must belong to a module
    /// instantiated against *this* engine's memory and table, with the shape
    /// its own JSON declared; a wrong one writes into the engine's memory
    /// rather than failing. Only the host that linked the module may call this.
    ///
    /// wasm32 only, for the same reason as
    /// [`take_faust_jobs`](Self::take_faust_jobs).
    #[cfg(target_arch = "wasm32")]
    #[wasm_bindgen(js_name = finishFaust)]
    pub fn finish_faust(&mut self, ticket: f64, report: Option<String>, error: Option<String>) {
        match (report, error) {
            (_, Some(message)) if !message.is_empty() => {
                self.inner.finish_faust_error(ticket as u64, message);
            }
            (Some(report), _) => {
                // SAFETY: delegated to the host by construction — this method
                // is the door it answers through, and its contract is the
                // paragraph above.
                unsafe { self.inner.finish_faust_linked(ticket as u64, &report) };
            }
            _ => self.inner.finish_faust_error(
                ticket as u64,
                "the host answered with neither a linked module nor an error".into(),
            ),
        }
    }
}

/// One JSON string literal, escaped. A path can carry a quote or a backslash.
fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The error type of the JS-facing results: a real `JsError` on wasm, the
/// plain message natively (so the same methods compile and test on both).
#[cfg(target_arch = "wasm32")]
type JsErrorish = JsError;
#[cfg(not(target_arch = "wasm32"))]
type JsErrorish = String;

#[cfg(target_arch = "wasm32")]
fn err(e: String) -> JsErrorish {
    JsError::new(&e)
}
#[cfg(not(target_arch = "wasm32"))]
fn err(e: String) -> JsErrorish {
    e
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use clausters::rosc::{OscBundle, OscMessage, OscPacket, OscTime, OscType, encoder};

    /// One length-prefixed `/synth_new` + end bundle in the binary score format.
    fn tiny_score() -> Vec<u8> {
        let packet = |secs: u32, msgs: Vec<OscMessage>| {
            let bundle = OscPacket::Bundle(OscBundle {
                timetag: OscTime {
                    seconds: secs,
                    fractional: 0,
                },
                content: msgs.into_iter().map(OscPacket::Message).collect(),
            });
            encoder::encode(&bundle).unwrap()
        };
        let s_new = OscMessage {
            addr: "/synth_new".into(),
            args: vec![
                OscType::String("default".into()),
                OscType::Int(1000),
                OscType::Int(0),
                OscType::Int(0),
                OscType::String("freq".into()),
                OscType::Float(440.0),
            ],
        };
        let n_free = OscMessage {
            addr: "/node_free".into(),
            args: vec![OscType::Int(1000)],
        };
        let mut score = Vec::new();
        for bytes in [packet(0, vec![s_new]), packet(1, vec![n_free])] {
            score.extend_from_slice(&(bytes.len() as i32).to_be_bytes());
            score.extend_from_slice(&bytes);
        }
        score
    }

    /// The shell renders through the same path the browser calls: a one-second
    /// default-def note comes out with signal in it.
    #[test]
    fn shell_renders_a_score() {
        let (out, seed) = super::render(&tiny_score(), 48000.0, 2, None).unwrap();
        assert_eq!(out.len(), 2 * 48000);
        let rms = (out.iter().map(|x| x * x).sum::<f32>() / out.len() as f32).sqrt();
        assert!(rms > 0.05, "audible signal expected, rms = {rms}");
        assert!(out.iter().all(|x| x.is_finite()));
        // Whatever the shell picked, it says which: a take is repeatable.
        let (again, _) = super::render(&tiny_score(), 48000.0, 2, Some(seed)).unwrap();
        assert_eq!(out, again);
    }

    /// A malformed score reports, not panics.
    #[test]
    fn shell_reports_bad_scores() {
        assert!(super::render(&[1, 2, 3], 48000.0, 2, None).is_err());
    }

    /// The live face, exactly as the worklet drives it: send an `/synth_new`,
    /// pull a second of quanta, hear the tone, drain the `/done`s.
    /// The page's client tag for these tests. One client here, so any tag does.
    const PEER: u32 = 1;

    #[test]
    fn web_server_pulls_a_tone() {
        let mut server = super::WebServer::new(48000.0, 2, 0.0).unwrap();
        let s_new = OscMessage {
            addr: "/synth_new".into(),
            args: vec![
                OscType::String("default".into()),
                OscType::Int(1000),
                OscType::Int(0),
                OscType::Int(0),
            ],
        };
        assert!(server.send(PEER, &encoder::encode(&OscPacket::Message(s_new)).unwrap()));
        let block = server.block_frames() as usize * 2;
        let mut quantum = vec![0.0f32; 128 * 2];
        assert_eq!(
            quantum.len() % block,
            0,
            "128-frame quanta hold whole blocks"
        );
        let mut energy = 0.0f32;
        for _ in 0..375 {
            server.process(&mut quantum).unwrap();
            energy += quantum.iter().map(|x| x * x).sum::<f32>();
        }
        assert_eq!(server.clock(), 48000.0, "one second of quanta");
        let rms = (energy / (375.0 * quantum.len() as f32)).sqrt();
        assert!(rms > 0.05, "audible tone expected, rms = {rms}");
        assert!(server.poll().is_none(), "/synth_new is fire-and-forget");
        assert!(!server.quit_requested());
    }
}
