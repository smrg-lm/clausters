//! The UGen bench: one place that turns a **subject** into a running def.
//!
//! `tests/common/subjects.json` declares the UGens under test, one entry per
//! row. This module is the Rust half of that file's contract — it assembles a
//! subject into a SynthDef, renders it, and drives the rules from the
//! `audio-testing` skill that are the same for every UGen:
//!
//! * **rule 5, the block split** — a scheduled bundle cuts a block in two, and
//!   a stateful UGen must not notice. [`assert_split_agrees`].
//! * **rule 4, the long run** — ten seconds of output stays finite and bounded.
//!   The cheap half of the rule, the one that catches an `f64` state regressing
//!   to `f32` by the NaN it eventually produces; the *numerical* half (does the
//!   filter still have its analytic gain, did the ramp land on its target) is
//!   family-specific and stays in the milestone's own file.
//!   [`assert_long_run_is_bounded`].
//! * finiteness over every subject of a milestone. [`assert_renders_finite`].
//!
//! Everything a family claims about *its own* signal — an alias SNR, a transfer
//! function, a decay envelope — stays in that family's suite, where it can be
//! written against the closed form. This module deliberately knows no DSP.
//!
//! The other half of the contract is `examples/audition.py`, which reads the
//! same file and sends the same fragment to a server, so a row that gained an
//! assert here is audible there with no further work.
//!
//! Include it with `#[path = "common/bench.rs"] mod bench;`.

// A shared test-support module: each suite uses the handful of entry points it
// needs, so anything unused *there* is still used elsewhere.
#![allow(dead_code)]

use std::sync::Arc;

use clausters::clausters_core::rng::SEED_STRIDE;
use clausters::dsp::{BLOCK_SIZE, Buses, ControlBuses, ProcessCtx};
use clausters::node::SynthNode;
use clausters::synthdef::instance::UGenSynth;
use clausters::synthdef::{SynthDefSpec, compile};
use serde::Deserialize;
use serde_json::{Map, Value, json};

/// The sample rate every U suite renders at.
pub const SR: f32 = 48_000.0;

/// Audio bus the driving signal is written to, and the one a subject's `In`
/// reads. Above the output buses so a multi-channel subject cannot collide
/// with its own input.
pub const IN_BUS: usize = 8;

/// Audio buses a rendered def gets. Output channels take `0..channels`;
/// [`IN_BUS`] sits above them.
const AUDIO_BUSES: usize = 16;

// ---- the subject table ----

/// One UGen under test, as declared in `tests/common/subjects.json`. See that
/// file's `_doc` for what each field means.
#[derive(Debug, Clone, Deserialize)]
pub struct Subject {
    /// Audition handle: lowercase, unique across the file.
    pub name: String,
    /// The registry row this exercises.
    pub kind: String,
    /// A signal wired in front as UGen 0, which the inputs reach as
    /// `{"ugen": 0}`. See [`source_signal`] for the shapes.
    #[serde(default)]
    pub source: Option<String>,
    /// UGens the subject needs *before* it — a demand source feeding a driver.
    /// They follow the source, so their indices start at 1 when there is one.
    #[serde(default)]
    pub prelude: Vec<Value>,
    /// The subject's wire inputs, in wire order, minus the trailing channel
    /// index when [`channels`](Self::channels) is more than one.
    pub inputs: Vec<Value>,
    /// Non-signal fields merged into the row (`max_delay`, ...).
    #[serde(default, rename = "static")]
    pub statics: Map<String, Value>,
    /// How many rows the family emits for one logical UGen. A panner is two
    /// rows differing only in a trailing `chan` index, which the bench appends
    /// — the same thing the Python builder does.
    #[serde(default = "one")]
    pub channels: usize,
    #[serde(default)]
    pub traits: Vec<String>,
    /// The fundamental a spectral measurement should use, in Hz.
    #[serde(default)]
    pub hz: Option<f32>,
}

fn one() -> usize {
    1
}

impl Subject {
    pub fn has(&self, t: &str) -> bool {
        self.traits.iter().any(|x| x == t)
    }

    /// The full def this subject renders as: the source (if any), the prelude,
    /// one row per channel, and an `Out` per channel onto buses `0..channels`.
    pub fn def_json(&self) -> String {
        let mut ugens: Vec<Value> = Vec::new();
        if self.source.is_some() {
            ugens.push(json!({"kind": "In", "inputs": [{"const": IN_BUS as f32}]}));
        }
        ugens.extend(self.prelude.iter().cloned());
        let first = ugens.len();
        for chan in 0..self.channels {
            let mut inputs = self.inputs.clone();
            if self.channels > 1 {
                inputs.push(json!({ "const": chan as f32 }));
            }
            let mut row = json!({"kind": self.kind, "inputs": inputs});
            let obj = row.as_object_mut().expect("row is an object");
            for (k, v) in &self.statics {
                obj.insert(k.clone(), v.clone());
            }
            ugens.push(row);
        }
        for chan in 0..self.channels {
            ugens.push(json!({"kind": "Out", "inputs": [
                {"const": chan as f32}, {"ugen": first + chan}]}));
        }
        json!({"name": self.name, "ugens": ugens}).to_string()
    }
}

/// The subject table, verbatim. Both this module and `examples/audition.py`
/// read it; keeping it a file rather than a Rust literal is what lets the ear
/// and the asserts stay on the same declaration.
pub const SUBJECTS_JSON: &str = include_str!("subjects.json");

/// Every subject of one milestone (`"U1"`, ... `"U8"`), in declaration order.
pub fn subjects(milestone: &str) -> Vec<Subject> {
    let table: Map<String, Value> =
        serde_json::from_str(SUBJECTS_JSON).expect("subjects.json is valid JSON");
    let rows = table
        .get(milestone)
        .unwrap_or_else(|| panic!("subjects.json has no {milestone} section"));
    serde_json::from_value(rows.clone())
        .unwrap_or_else(|e| panic!("{milestone} does not deserialize: {e}"))
}

/// Every subject in the file, paired with its milestone.
pub fn all_subjects() -> Vec<(String, Subject)> {
    let table: Map<String, Value> =
        serde_json::from_str(SUBJECTS_JSON).expect("subjects.json is valid JSON");
    let mut out = Vec::new();
    for (milestone, rows) in &table {
        // `_doc` is the file's own prose, not a milestone.
        if milestone.starts_with('_') {
            continue;
        }
        let rows: Vec<Subject> = serde_json::from_value(rows.clone())
            .unwrap_or_else(|e| panic!("{milestone} does not deserialize: {e}"));
        out.extend(rows.into_iter().map(|s| (milestone.clone(), s)));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// The driving signals a subject's `source` may name. Deterministic to the
/// sample, so a whole render and a split one are comparable.
pub fn source_signal(kind: &str, n: usize, hz: f32) -> Vec<f32> {
    match kind {
        "noise" => {
            let mut rng = clausters_core::rng::WhiteNoise::from_seed(0x5EED);
            (0..n).map(|_| rng.next_sample()).collect()
        }
        "impulse" => {
            let mut x = vec![0.0f32; n];
            if !x.is_empty() {
                x[0] = 1.0;
            }
            x
        }
        // A trigger every 1/hz seconds: what a trigger UGen needs to have
        // anything to do.
        "impulse_train" => {
            let period = (SR / hz).round().max(1.0) as usize;
            (0..n)
                .map(|i| if i % period == 0 { 1.0 } else { 0.0 })
                .collect()
        }
        "sine" => (0..n)
            .map(|i| (std::f32::consts::TAU * hz * i as f32 / SR).sin())
            .collect(),
        "dc" => vec![1.0f32; n],
        "silence" => vec![0.0f32; n],
        other => panic!("unknown source {other:?} in subjects.json"),
    }
}

// ---- rendering ----

/// How to render a def: the knobs the eight U suites turn between them.
pub struct Run<'a> {
    /// Output samples wanted, per channel.
    pub samples: usize,
    pub sample_rate: f32,
    /// Output channels to collect, from bus 0 up.
    pub channels: usize,
    /// Written to [`IN_BUS`] block by block, for a def whose UGen 0 is an `In`.
    pub input: Option<&'a [f32]>,
    /// Split every block at this sample, the way a scheduled bundle does.
    /// Rule 5's whole point: a stateful UGen must not be able to tell.
    pub cut: Option<usize>,
}

impl Default for Run<'_> {
    fn default() -> Self {
        Self {
            samples: BLOCK_SIZE,
            sample_rate: SR,
            channels: 1,
            input: None,
            cut: None,
        }
    }
}

impl<'a> Run<'a> {
    pub fn new(samples: usize) -> Self {
        Self {
            samples,
            ..Self::default()
        }
    }

    pub fn input(mut self, x: &'a [f32]) -> Self {
        self.input = Some(x);
        self.samples = x.len();
        self
    }

    pub fn cut(mut self, at: usize) -> Self {
        self.cut = Some(at);
        self
    }

    pub fn channels(mut self, n: usize) -> Self {
        self.channels = n;
        self
    }
}

/// Renders a whole def, one vector per output channel.
///
/// A `cut` render processes every block as two calls sharing the block's input
/// — offsets `0..cut` and `cut..BLOCK_SIZE` — which is exactly what the engine
/// does when a timed bundle lands mid-block.
pub fn render_def(def_json: &str, run: &Run) -> Vec<Vec<f32>> {
    let spec: SynthDefSpec = serde_json::from_str(def_json)
        .unwrap_or_else(|e| panic!("subject def is not a valid spec: {e}\n{def_json}"));
    let def = compile(spec).unwrap_or_else(|e| panic!("subject def does not compile: {e}"));
    let mut synth = UGenSynth::new(Arc::new(def), run.sample_rate, SEED_STRIDE);
    let mut buses = Buses::new(ControlBuses::new(16), AUDIO_BUSES);

    let mut out = vec![Vec::with_capacity(run.samples); run.channels];
    let mut pos = 0;
    while pos < run.samples {
        buses.clear_audio();
        let n = BLOCK_SIZE.min(run.samples - pos);
        if let Some(x) = run.input {
            // SAFETY: single-threaded test, sole owner of the input bus.
            let drive = unsafe { buses.audio_mut(IN_BUS) };
            let take = n.min(x.len().saturating_sub(pos));
            drive[..take].copy_from_slice(&x[pos..pos + take]);
        }
        let slices = match run.cut {
            // A cut only means anything on a whole block; the tail runs plain.
            Some(at) if at > 0 && at < n && n == BLOCK_SIZE => vec![(0, at), (at, n - at)],
            _ => vec![(0, BLOCK_SIZE)],
        };
        for (offset, frames) in slices {
            let mut ctx = ProcessCtx {
                sample_rate: run.sample_rate,
                full_sample_rate: run.sample_rate,
                buses: &buses,
                buffers: &[],
                offset,
                frames,
                transport: Default::default(),
            };
            synth.process(&mut ctx);
        }
        for (chan, dst) in out.iter_mut().enumerate() {
            dst.extend_from_slice(&buses.audio(chan)[..n]);
        }
        pos += n;
    }
    out
}

/// Renders one subject, one vector per channel. `cut` splits every block.
pub fn render_subject(s: &Subject, samples: usize, cut: Option<usize>) -> Vec<Vec<f32>> {
    let hz = s.hz.unwrap_or(220.0);
    let input = s.source.as_ref().map(|k| source_signal(k, samples, hz));
    let mut run = Run::new(samples).channels(s.channels);
    run.input = input.as_deref();
    run.cut = cut;
    render_def(&s.def_json(), &run)
}

// ---- the convenience forms the milestone suites render through ----

/// `n` samples of a single free-running UGen, written straight to bus 0.
pub fn render(ugen_json: &str, n: usize) -> Vec<f32> {
    render_def(&wrap(ugen_json, false), &Run::new(n)).remove(0)
}

/// The same, with every block cut at `at`.
pub fn render_split(ugen_json: &str, n: usize, at: usize) -> Vec<f32> {
    render_def(&wrap(ugen_json, false), &Run::new(n).cut(at)).remove(0)
}

/// A UGen driven by `input`: the def's UGen 0 is an `In`, so the fragment
/// reaches the signal as `{"ugen": 0}`.
pub fn render_with_input(ugen_json: &str, input: &[f32]) -> Vec<f32> {
    render_def(&wrap(ugen_json, true), &Run::default().input(input)).remove(0)
}

/// The same, with every block cut at `at`.
pub fn render_with_input_split(ugen_json: &str, input: &[f32], at: usize) -> Vec<f32> {
    render_def(&wrap(ugen_json, true), &Run::default().input(input).cut(at)).remove(0)
}

/// Wraps a bare UGen fragment into a one-output def, optionally behind an `In`.
fn wrap(ugen_json: &str, with_input: bool) -> String {
    if with_input {
        format!(
            r#"{{"name": "bench", "ugens": [
                {{"kind": "In", "inputs": [{{"const": {IN_BUS}.0}}]}},
                {ugen_json},
                {{"kind": "Out", "inputs": [{{"const": 0.0}}, {{"ugen": 1}}]}}]}}"#
        )
    } else {
        format!(
            r#"{{"name": "bench", "ugens": [{ugen_json},
                {{"kind": "Out", "inputs": [{{"const": 0.0}}, {{"ugen": 0}}]}}]}}"#
        )
    }
}

// ---- the generic rules ----

/// Rule 5. Renders the subject whole and with every block cut at `at`, and
/// requires the two to agree.
///
/// The tolerance is not a fudge factor: both renders execute the same
/// arithmetic in the same order, so a correct UGen agrees to well under it. It
/// leaves room for the one place a split legitimately reorders work — a
/// coefficient recomputed once per slice instead of once per block.
///
/// **A stochastic source cannot be driven from here**, and asking is an error
/// rather than a skip. Comparing two renders means comparing two synth
/// instances, and each instance of a noise UGen seeds from a shared counter on
/// purpose (correlated noise summed with itself is a comb filter) — while the
/// wire has no seed input to pin, since a def has no way to name one. So the
/// two renders differ at sample 0 for a reason that is not a bug. The split
/// rule still applies to those rows; `tests/noise.rs` discharges it one level
/// down, against `with_seed` constructors, where the stream *is* pinned.
pub fn assert_split_agrees(s: &Subject, samples: usize, at: usize) {
    assert!(
        !s.has("stochastic"),
        "{}: a stochastic subject cannot be split-tested through the def path \
         (each instance draws its own seed); do it seeded, at the struct level",
        s.name
    );
    let samples = samples.next_multiple_of(BLOCK_SIZE);
    let whole = render_subject(s, samples, None);
    let split = render_subject(s, samples, Some(at));
    let tol = 1e-6;
    for (chan, (w, p)) in whole.iter().zip(&split).enumerate() {
        for (i, (a, b)) in w.iter().zip(p).enumerate() {
            assert!(
                (a - b).abs() <= tol,
                "{} channel {chan} differs at sample {i} when the block is cut \
                 at {at}: {a} vs {b}",
                s.name
            );
        }
    }
}

/// Rule 4, the cheap half: `seconds` of output stays finite and bounded.
///
/// A long run is where a state variable narrowed to `f32` finally shows: the
/// accumulated error either walks off to a NaN or parks the signal against a
/// rail. `bound` is generous on purpose — this assert is about the numbers
/// still being numbers, not about the level being right. What the level *should*
/// be is a claim only the family can make, and it makes it in its own suite.
pub fn assert_long_run_is_bounded(s: &Subject, seconds: f32, bound: f32) {
    let samples = (SR * seconds) as usize;
    for (chan, x) in render_subject(s, samples, None).iter().enumerate() {
        for (i, v) in x.iter().enumerate() {
            assert!(
                v.is_finite(),
                "{} channel {chan} is not finite at sample {i} of a {seconds} s \
                 run ({v}) -- {:.2} s in",
                s.name,
                i as f32 / SR
            );
            assert!(
                v.abs() <= bound,
                "{} channel {chan} reached {v} at sample {i} of a {seconds} s \
                 run, past the bound {bound}",
                s.name
            );
        }
    }
}

/// Every subject of a milestone renders finite output. The floor under all the
/// rest: a row that panics or produces a NaN from its own declared inputs is
/// broken before any measurement is worth making.
pub fn assert_renders_finite(milestone: &str, samples: usize) {
    for s in subjects(milestone) {
        for (chan, x) in render_subject(&s, samples, None).iter().enumerate() {
            if let Some((i, v)) = x.iter().enumerate().find(|(_, v)| !v.is_finite()) {
                panic!("{} channel {chan}: sample {i} is not finite ({v})", s.name);
            }
        }
    }
}
