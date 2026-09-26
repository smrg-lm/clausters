//! The frequency-domain (`fr`) chain: `FFT` -> `PV_*` -> `IFFT`.
//!
//! scsynth's spectral processing bookends a chain of `PV_*` (phase-vocoder)
//! UGens between [`Fft`] (window an audio input and transform it to a complex
//! frame) and [`Ifft`] (inverse-transform and overlap-add back to audio). The
//! chain is **not block-rate**: `FFT` emits one spectral frame per **hop**, and
//! the `PV_*` UGens only touch the frame on the blocks a fresh one is ready --
//! the frame-rate (`fr`) substrate, kin to the demand (`dr`) rate.
//!
//! ## Where the spectral frame lives (a deliberate deviation from scsynth)
//!
//! scsynth threads the frame through a client-allocated buffer whose bin data
//! the audio thread mutates in place -- which would break Clausters' invariant
//! that a pool [`Buffer`](super::buffer::Buffer) is immutable once built. So the
//! frame lives **not** in the sample-buffer pool but in a [`SpectralChain`]:
//! synth-private scratch, allocated when the synth is instantiated (on the
//! network thread, where allocation is legal) and freed with the synth -- exactly
//! like the `LocalIn`/`LocalOut` feedback `locals`, and the moral equivalent of
//! SuperCollider's `LocalBuf`. No `/buffer_alloc` is required and the sample pool
//! stays fully immutable. The chain is shared by the chain's UGens through a
//! compile-assigned *slot* the synth resolves for each of them (see
//! `synthdef::instance`); the wire between the UGens only enforces ordering.
//!
//! ## Framing is sample-exact
//!
//! The engine cuts a block at every timed event -- of any node -- and a synth
//! may start mid-block, so the slices a chain sees have any length. `Fft`
//! therefore counts its own input samples and takes a frame at **exactly**
//! every `hop` samples, at the sample the hop closes on, whatever slice that
//! falls in; [`SpectralChain::pos`] carries where that frame ends. `Ifft`
//! overlap-adds each frame at that position and reads its output a fixed
//! window behind, so the round trip is the input delayed by exactly one window
//! -- for any hop, not only whole blocks, and however the blocks were cut. A
//! hop is at least one block ([`MIN_HOP`]), which keeps it to at most one
//! frame per slice.
//!
//! The transforms and the windows are the single-sourced
//! [`clausters_core::fft`] / [`clausters_core::window`], shared with the clients
//! for bit-identical analysis. Every per-hop transform reuses pre-allocated
//! scratch, so nothing here allocates on the audio thread.
//!
//! ## Hop-phase stagger
//!
//! A chain concentrates all its work on the block where its hop closes; chains
//! instantiated on the same block would all hop on the same block, stacking
//! their transform spikes. So each [`Fft`] delays its *first* frame by a
//! deterministic sub-hop offset derived from its node id
//! ([`UGen::set_node_id`], delivered by the engine when the node enters the
//! tree). Only the initial fire shifts -- the cadence, the analysis discipline
//! and a chain's own latency-to-content are unchanged -- and the same score
//! yields the same ids, so RT and NRT renders stay sample-identical.

use clausters_core::fft;
use clausters_core::pvprog::{BinCtx, PvOp, PvProgram};
use clausters_core::window::Window;

use crate::dsp::registry::UGenConfig;
use crate::dsp::{
    BLOCK_SIZE, MAX_UGEN_INPUTS, ProcessCtx, ReplyKind, ReplyMsg, UGen, UGenCmd, at,
    ugen_cmd_selector,
};

/// Default FFT window size when a `FFT`/`IFFT` def omits it. A power of two in
/// [`fft::SUPPORTED_SIZES`].
pub const DEFAULT_FFT_SIZE: usize = 1024;

/// Resolves a def's requested FFT size to a supported power of two, falling back
/// to [`DEFAULT_FFT_SIZE`] for an unset or unsupported request. Called at
/// compile time; the compiler has already validated supported sizes, this is
/// the last-resort clamp so a built UGen always has a legal size.
pub fn resolve_fft_size(requested: Option<usize>) -> usize {
    match requested {
        Some(n) if fft::supports(n) => n,
        _ => DEFAULT_FFT_SIZE,
    }
}

/// Resolves a def's hop fraction to the hop in samples the chain runs at.
/// A hop the compiler would reject is clamped into range, so a built UGen
/// always has a legal one.
pub fn resolve_hop(winsize: usize, hop: Option<f32>) -> usize {
    let frac = hop.unwrap_or(0.5);
    ((winsize as f32 * frac).round() as usize).clamp(MIN_HOP, winsize)
}

/// The shortest hop, in samples: one block. A slice is never longer than a
/// block, so a hop this long closes at most once per slice -- and the chain
/// carries one frame per slice. The compiler rejects a shorter one rather
/// than letting it be lengthened silently.
pub const MIN_HOP: usize = BLOCK_SIZE;

/// The synth-private spectral frame shared by one `FFT`->`PV_*`->`IFFT` chain.
/// Persistent across blocks (like the feedback `locals`); allocated once at
/// synth init. See the module docs for why this replaces scsynth's mutable pool
/// buffer.
pub struct SpectralChain {
    /// The packed complex frame, `winsize` floats in the
    /// [`fft::rfft_into`] layout `[dc, nyquist, re1, im1, ...]`.
    pub frame: Vec<f32>,
    /// True on the processing slice where `FFT` wrote a fresh frame; the
    /// `PV_*`/`IFFT` UGens act only then. `FFT` clears it each slice.
    pub ready: bool,
    /// Where the fresh frame ends, in samples of the synth's own input since
    /// it started: the frame is the `winsize` samples before `pos`. `IFFT`
    /// overlap-adds it there.
    pub pos: u64,
    /// The frame's transform size.
    pub winsize: usize,
    /// The window the chain analyses and resynthesizes with -- the chain's,
    /// not either end's: `FFT` changes it, and `IFFT` follows.
    pub window: Window,
    /// The frame end position the current window took effect at (0: from
    /// the start). A frame ending at or after it used `window`, one before
    /// it `prev_window`. Changes are at least a window apart, so no span one
    /// window long ever holds frames of more than these two.
    pub window_from: u64,
    /// The window in force before `window_from`.
    pub prev_window: Window,
}

impl SpectralChain {
    pub fn new(winsize: usize) -> Self {
        Self {
            frame: vec![0.0; winsize],
            ready: false,
            pos: 0,
            winsize,
            window: Window::default(),
            window_from: 0,
            prev_window: Window::default(),
        }
    }
}

/// Windows an audio input and transforms it to a spectral frame once per hop.
///
/// Inputs: `[in, active]` -- the audio signal and a gate on it, read per
/// sample. `active <= 0` gates the input to silence, so the chain analyses
/// the gated signal and a plain round trip is exactly that signal, one window
/// late, through both edges. A frame whose whole window was gated holds
/// nothing but silence, so it is not taken: the chain does no work while it
/// stays off, and its frame reads as silence to anything that combines it.
///
/// The window size, hop and window type are static per-UGen config, not
/// signal inputs, because they size the pre-allocated scratch. The window is
/// also settable live through `/node_ugenCmd` (selector `window`), and it is
/// the chain's: the change is recorded on the [`SpectralChain`], `IFFT`
/// follows it, and it takes effect at a frame, at most once per window length
/// (a later one waits), so the overlap-add can normalize the frames on both
/// sides of it exactly.
pub struct Fft {
    winsize: usize,
    hop_size: usize,
    window_kind: Window,
    /// Analysis window coefficients (`winsize`); refilled in place when a
    /// window change takes effect, so still allocation-free per block.
    window: Vec<f32>,
    /// A window asked for by `/node_ugenCmd` and not yet in force.
    pending_window: Option<Window>,
    /// The frame end position the current window took effect at.
    window_from: u64,
    /// Sliding input, a circular buffer of the last `winsize` samples.
    inbuf: Vec<f32>,
    write: usize,
    /// Input samples seen since the synth started.
    pos: u64,
    /// The input position the next frame ends at: the first is one window
    /// in (plus the stagger), every later one a hop after the previous.
    ///
    /// The hop-phase stagger is set once from the node id in
    /// [`UGen::set_node_id`] -- a deterministic sub-hop offset (a block
    /// multiple) so chains instantiated on the same block spread their
    /// transform spikes across blocks instead of stacking them on one. Only
    /// the first frame moves; the cadence after it is the hop, and the same
    /// node id yields the same offset (RT and NRT renders of one score stay
    /// sample-identical).
    next_frame: u64,
    /// The input position just past the last sample `active` let through: a
    /// frame ending at `p` holds any of the signal only if this is past
    /// `p - winsize`.
    live_until: u64,
    /// Whether the chain's frame has been cleared since the last frame taken,
    /// so a run of skipped frames clears it once.
    frame_cleared: bool,
    /// De-circularized, windowed frame handed to the forward transform.
    scratch: Vec<f32>,
}

impl Fft {
    pub fn new(config: &UGenConfig) -> Self {
        let winsize = resolve_fft_size(config.fft_size);
        let hop_size = resolve_hop(winsize, config.hop);
        let window_kind = Window::from_wintype(config.wintype.unwrap_or(0));
        let mut window = vec![0.0; winsize];
        window_kind.fill(&mut window);
        Self {
            winsize,
            hop_size,
            window_kind,
            window,
            pending_window: None,
            window_from: 0,
            inbuf: vec![0.0; winsize],
            write: 0,
            pos: 0,
            next_frame: winsize as u64,
            live_until: 0,
            frame_cleared: true,
            scratch: vec![0.0; winsize],
        }
    }
}

impl UGen for Fft {
    fn process(&mut self, _ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        // Never reached for the spectral exec mode; a plain call is a no-op.
        output.fill(0.0);
    }

    fn process_spectral(
        &mut self,
        _ctx: &mut ProcessCtx,
        inputs: &[&[f32]],
        output: &mut [f32],
        chain: &mut SpectralChain,
    ) {
        chain.ready = false;
        if self.pos == 0 {
            chain.window = self.window_kind;
            chain.prev_window = self.window_kind;
        }
        let input = inputs[0];
        let gate = inputs.get(1).copied();
        for (j, &s) in input.iter().enumerate() {
            let active = gate.is_none_or(|g| at(g, j) > 0.0);
            self.inbuf[self.write] = if active { s } else { 0.0 };
            self.write = (self.write + 1) % self.winsize;
            self.pos += 1;
            if active {
                self.live_until = self.pos;
            }
            // The frame is taken at the sample its hop closes on, wherever
            // that falls in the slice. The hop is at least a block, so this
            // happens at most once per slice.
            if self.pos == self.next_frame {
                self.next_frame += self.hop_size as u64;
                // A window change takes effect at a frame, and not within a
                // window of the last one: every sample is then covered by
                // frames of at most two windows, which the overlap-add
                // normalizes exactly.
                if let Some(wanted) = self.pending_window
                    && (self.window_from == 0 || self.pos >= self.window_from + self.winsize as u64)
                {
                    self.pending_window = None;
                    chain.prev_window = self.window_kind;
                    self.window_kind = wanted;
                    wanted.fill(&mut self.window);
                    self.window_from = self.pos;
                    chain.window = wanted;
                    chain.window_from = self.pos;
                }
                let live = self.live_until + self.winsize as u64 > self.pos;
                if !live {
                    // Nothing but gated silence in the window: skip the
                    // transform, and let a combiner reading this chain read
                    // the silence it holds rather than the last frame taken.
                    if !self.frame_cleared {
                        chain.frame.fill(0.0);
                        self.frame_cleared = true;
                    }
                } else {
                    self.frame_cleared = false;
                    // De-circularize: `write` points at the oldest sample.
                    for k in 0..self.winsize {
                        let s = self.inbuf[(self.write + k) % self.winsize];
                        self.scratch[k] = s * self.window[k];
                    }
                    fft::rfft_into(&self.scratch, &mut chain.frame);
                    chain.pos = self.pos;
                    chain.ready = true;
                }
            }
        }
        // The wire only orders the chain; carry the slot marker for debugging.
        if let Some(o) = output.first_mut() {
            *o = if chain.ready { 1.0 } else { 0.0 };
        }
    }

    fn set_node_id(&mut self, id: i32) {
        // derive the deterministic hop-phase stagger -- the node id modulo
        // the hop's block count, in whole blocks. A hop no longer than one
        // block cannot stack (at most one frame per slice already), so it
        // keeps offset 0.
        let blocks_per_hop = self.hop_size / BLOCK_SIZE;
        if blocks_per_hop > 1 && self.pos == 0 {
            let stagger = (id.unsigned_abs() as usize % blocks_per_hop) * BLOCK_SIZE;
            self.next_frame = (self.winsize + stagger) as u64;
        }
    }

    fn command(&mut self, cmd: &UGenCmd) {
        // `/node_ugenCmd <node> <ugen> window <wintype>`: change the chain's
        // window, at the next frame it may take effect at.
        if cmd.selector != ugen_cmd_selector("window") || cmd.num_args < 1 {
            return;
        }
        let wanted = Window::from_wintype(cmd.args[0] as i32);
        self.pending_window = (wanted != self.window_kind).then_some(wanted);
    }
}

/// Inverse-transforms each fresh spectral frame and overlap-adds it back to
/// audio. Input: `[chain]` -- the chain wire, which only carries ordering (the
/// live frame is the synth-private [`SpectralChain`] the synth passes in). The
/// window size, hop and type are the chain's; the synthesis window is the
/// chain's too, frame by frame, so it matches the analysis window through a
/// live change. A `window` command addressed to `IFFT` does nothing: the
/// window is changed at the chain's `FFT`.
///
/// The output is the input position one window behind the synth's own time:
/// by then every frame that covers that position has been added (the last
/// one ends there), so each sample leaves complete, and the latency is
/// exactly `winsize` however the blocks were sliced.
pub struct Ifft {
    winsize: usize,
    hop_size: usize,
    window_kind: Window,
    window: Vec<f32>,
    /// The window before the chain's last change, and its `norm`: the
    /// samples whose frames straddle the change are normalized with both.
    prev_window: Vec<f32>,
    prev_norm: Vec<f32>,
    /// The chain's `window_from` this `IFFT` has followed.
    window_from: u64,
    /// Overlap-add accumulator indexed by input position (modulo its length,
    /// two windows: a frame writes up to one window ahead of the position
    /// being read, never onto one not read yet). A slot is cleared as it is
    /// read.
    ring: Vec<f32>,
    /// The steady-state overlap-add normalization (COLA), one value per hop
    /// phase: `norm[r] = sum_i window[r + i*hop]^2` over the frames that overlap
    /// output phase `r`. Precomputed at build (constant per render), so dividing
    /// by it never over-amplifies the under-overlapped edges of the startup or a
    /// spectrally modified frame -- unlike a running per-sample window sum.
    norm: Vec<f32>,
    /// Time-domain scratch for the inverse transform.
    time: Vec<f32>,
    /// Samples output since the synth started -- the same clock the chain's
    /// [`SpectralChain::pos`] counts in.
    now: u64,
    /// Where the first frame starts: the origin of the hop phase `norm` is
    /// indexed by. `None` until a frame arrives.
    origin: Option<u64>,
}

impl Ifft {
    pub fn new(config: &UGenConfig) -> Self {
        let winsize = resolve_fft_size(config.fft_size);
        let hop_size = resolve_hop(winsize, config.hop);
        let window_kind = Window::from_wintype(config.wintype.unwrap_or(0));
        let mut window = vec![0.0; winsize];
        window_kind.fill(&mut window);
        let mut norm = vec![0.0f32; hop_size];
        fill_cola_norm(&window, &mut norm);
        Self {
            winsize,
            hop_size,
            window_kind,
            prev_window: window.clone(),
            prev_norm: norm.clone(),
            window,
            window_from: 0,
            ring: vec![0.0; 2 * winsize],
            norm,
            time: vec![0.0; winsize],
            now: 0,
            origin: None,
        }
    }
}

impl UGen for Ifft {
    fn process(&mut self, _ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        output.fill(0.0);
    }

    fn process_spectral(
        &mut self,
        _ctx: &mut ProcessCtx,
        _inputs: &[&[f32]],
        output: &mut [f32],
        chain: &mut SpectralChain,
    ) {
        let len = self.ring.len() as u64;
        if chain.window_from != self.window_from {
            // The chain's window changed at a frame: frames from there on
            // were analysed with the new one, so they are resynthesized and
            // normalized with it, and the old one is kept for the samples
            // that frames on both sides cover.
            self.prev_window.copy_from_slice(&self.window);
            self.prev_norm.copy_from_slice(&self.norm);
            self.window_kind = chain.window;
            self.window_kind.fill(&mut self.window);
            fill_cola_norm(&self.window, &mut self.norm);
            self.window_from = chain.window_from;
        }
        if chain.ready {
            fft::irfft_into(&chain.frame, &mut self.time);
            // Overlap-add the windowed reconstruction where the frame sits.
            // It only reaches positions at or after `pos - winsize`, which no
            // output earlier in this slice reads.
            let start = chain.pos - self.winsize as u64;
            self.origin.get_or_insert(start);
            for k in 0..self.winsize {
                let slot = ((start + k as u64) % len) as usize;
                self.ring[slot] += self.time[k] * self.window[k];
            }
        }
        for o in output.iter_mut() {
            // Position `now - winsize` is complete: the last frame covering
            // it ends at `now` at the latest. Normalize by the COLA
            // denominator of the frames on the grid that cover it -- taken or
            // not, and before the first one too. Dividing by that *full*
            // overlap sum (not a running partial one) means an incompletely
            // overlapped startup or a spectrally modified frame fades cleanly
            // instead of blowing up where the window is small.
            *o = match (self.now.checked_sub(self.winsize as u64), self.origin) {
                (Some(j), Some(origin)) if j >= origin => {
                    let slot = (j % len) as usize;
                    let v = self.ring[slot] / self.norm_at(j, origin);
                    self.ring[slot] = 0.0;
                    v
                }
                _ => 0.0,
            };
            self.now += 1;
        }
    }

    fn latency(&self) -> usize {
        self.winsize
    }
}

impl Ifft {
    /// The overlap-add denominator at input position `j`: the sum of the
    /// squared windows of the grid frames covering it (ends in `(j, j +
    /// winsize]`), each with the window it was analysed with. Away from a
    /// window change every such frame used one window, and the precomputed
    /// per-phase sum is that sum; across one, it is added up frame by frame.
    fn norm_at(&self, j: u64, origin: u64) -> f32 {
        let (w, hop) = (self.winsize as u64, self.hop_size as u64);
        let r = ((j - origin) % hop) as usize;
        let from = self.window_from;
        if from == 0 || j + 1 >= from {
            return self.norm[r];
        }
        if j + w < from {
            return self.prev_norm[r];
        }
        // Frame ends sit on `origin + w + k*hop`; the first one past `j`.
        let first = j + 1 + (origin + w + hop * (j / hop + 1) - (j + 1)) % hop;
        let mut sum = 0.0f32;
        let mut end = first;
        while end <= j + w {
            let k = (j + w - end) as usize;
            let c = if end >= from {
                self.window[k]
            } else {
                self.prev_window[k]
            };
            sum += c * c;
            end += hop;
        }
        if sum > 1e-9 { sum } else { 1.0 }
    }
}

/// The steady-state window-power sum per hop phase -- the exact COLA
/// denominator once the overlap is full -- written into `norm`, one slot per
/// phase of the hop. Guarded against a zero phase so the division is always
/// safe. In place, so a window swap on the audio thread can refill it.
fn fill_cola_norm(window: &[f32], norm: &mut [f32]) {
    let hop = norm.len();
    for (r, slot) in norm.iter_mut().enumerate() {
        let mut s = 0.0;
        let mut k = r;
        while k < window.len() {
            s += window[k] * window[k];
            k += hop;
        }
        *slot = if s > 1e-9 { s } else { 1.0 };
    }
}

/// The kind of magnitude threshold a [`PvMag`] filter applies to each bin.
/// One implementation, three registered names -- the mode is a parameter, not
/// a UGen (the stance: no one-UGen-per-op catalog).
#[derive(Clone, Copy)]
pub enum MagMode {
    /// Keep bins whose magnitude is **above** the threshold (`PV_MagAbove`).
    Above,
    /// Keep bins whose magnitude is **below** the threshold (`PV_MagBelow`).
    Below,
    /// Limit each bin's magnitude **to** the threshold, keeping its phase
    /// (`PV_MagClip`).
    Clip,
}

/// A magnitude-threshold spectral filter: `PV_MagAbove`/`PV_MagBelow`/
/// `PV_MagClip`. Input: `[chain, threshold]`. It transforms the bins failing
/// the test on each fresh frame; other blocks pass the (unchanged) chain
/// through.
pub struct PvMag {
    mode: MagMode,
}

impl PvMag {
    pub fn new(mode: MagMode) -> Self {
        Self { mode }
    }
}

/// Zeroes bin `b` (its slot(s)) in the packed frame.
#[inline]
fn zero_bin(frame: &mut [f32], b: usize, half: usize) {
    if b == 0 {
        frame[0] = 0.0; // DC
    } else if b == half {
        frame[1] = 0.0; // Nyquist
    } else {
        frame[2 * b] = 0.0;
        frame[2 * b + 1] = 0.0;
    }
}

/// Magnitude of bin `b` in the packed frame.
#[inline]
fn bin_mag(frame: &[f32], b: usize, half: usize) -> f32 {
    if b == 0 {
        frame[0].abs()
    } else if b == half {
        frame[1].abs()
    } else {
        (frame[2 * b] * frame[2 * b] + frame[2 * b + 1] * frame[2 * b + 1]).sqrt()
    }
}

/// Bin `b` of the packed frame as a complex pair (DC/Nyquist are real-only).
#[inline]
fn get_bin(frame: &[f32], b: usize, half: usize) -> (f32, f32) {
    if b == 0 {
        (frame[0], 0.0)
    } else if b == half {
        (frame[1], 0.0)
    } else {
        (frame[2 * b], frame[2 * b + 1])
    }
}

/// Writes bin `b` of the packed frame (the imaginary part is dropped on the
/// real-only DC/Nyquist slots).
#[inline]
fn set_bin(frame: &mut [f32], b: usize, half: usize, re: f32, im: f32) {
    if b == 0 {
        frame[0] = re;
    } else if b == half {
        frame[1] = re;
    } else {
        frame[2 * b] = re;
        frame[2 * b + 1] = im;
    }
}

/// Scales bin `b` by the real factor `s` (magnitude change, phase kept).
#[inline]
fn scale_bin(frame: &mut [f32], b: usize, half: usize, s: f32) {
    if b == 0 {
        frame[0] *= s;
    } else if b == half {
        frame[1] *= s;
    } else {
        frame[2 * b] *= s;
        frame[2 * b + 1] *= s;
    }
}

impl UGen for PvMag {
    fn process(&mut self, _ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        output.fill(0.0);
    }

    fn process_spectral(
        &mut self,
        _ctx: &mut ProcessCtx,
        inputs: &[&[f32]],
        output: &mut [f32],
        chain: &mut SpectralChain,
    ) {
        if chain.ready {
            let thresh = at(inputs[1], 0);
            let half = chain.winsize / 2;
            for b in 0..=half {
                let mag = bin_mag(&chain.frame, b, half);
                match self.mode {
                    MagMode::Above if mag < thresh => zero_bin(&mut chain.frame, b, half),
                    MagMode::Below if mag > thresh => zero_bin(&mut chain.frame, b, half),
                    MagMode::Clip if mag > thresh && mag > 0.0 => {
                        scale_bin(&mut chain.frame, b, half, thresh.max(0.0) / mag);
                    }
                    _ => {}
                }
            }
        }
        if let Some(o) = output.first_mut() {
            *o = if chain.ready { 1.0 } else { 0.0 };
        }
    }
}

/// The operator of a [`PvCombine`] two-chain combiner -- a parameter of one
/// implementation, registered under the scsynth-compatible names (the
/// stance: the operator set is data, not a UGen catalog).
#[derive(Clone, Copy)]
pub enum CombineOp {
    /// Complex addition (`PV_Add`).
    Add,
    /// Complex multiplication (`PV_Mul`).
    Mul,
    /// Per bin, keep whichever input has the **smaller** magnitude (`PV_Min`).
    Min,
    /// Per bin, keep whichever input has the **larger** magnitude (`PV_Max`).
    Max,
    /// A's bin scaled by B's magnitude -- A's phases kept (`PV_MagMul`).
    MagMul,
    /// A's magnitudes with B's phases (`PV_CopyPhase`).
    CopyPhase,
}

impl CombineOp {
    /// The name the operator is registered under.
    fn name(self) -> &'static str {
        match self {
            CombineOp::Add => "PV_Add",
            CombineOp::Mul => "PV_Mul",
            CombineOp::Min => "PV_Min",
            CombineOp::Max => "PV_Max",
            CombineOp::MagMul => "PV_MagMul",
            CombineOp::CopyPhase => "PV_CopyPhase",
        }
    }
}

/// The fault a combiner reports (a [`ReplyKind::Fault`] whose id is this):
/// its two chains are analysed with different windows. Values: the two
/// windows' `wintype`s, chain A's first.
pub const FAULT_WINDOWS_DIFFER: i32 = 1;

/// The sentence for a spectral UGen's fault, built on the network thread.
pub fn describe_fault(msg: &ReplyMsg) -> String {
    let window =
        |i: usize| Window::from_wintype(msg.values().get(i).copied().unwrap_or(0.0) as i32);
    match msg.id {
        FAULT_WINDOWS_DIFFER => format!(
            "{}: its chains are analysed with different windows ({:?} and {:?}); \
             it plays silence until they match (change both FFTs' window in one bundle)",
            msg.name(),
            window(0),
            window(1)
        ),
        code => format!("{}: fault {code} {:?}", msg.name(), msg.values()),
    }
}

/// A two-chain spectral combiner (`SpectralRole::Filter2`): inputs
/// `[chain_a, chain_b]`, the result written into chain A bin by bin. It acts
/// on the slices where **A** has a fresh frame, reading B's *latest* frame
/// (the frame is persistent chain state; two same-config `FFT`s in one synth
/// hop on the same blocks anyway, it staggering included -- the offset is
/// per-node, not per-UGen).
///
/// The compiler holds the two chains to one window; a live change to one of
/// them alone breaks that, and B's share would be resynthesized with A's
/// window. So while the two differ the combiner writes silence into chain A
/// and reports it once.
pub struct PvCombine {
    op: CombineOp,
    /// A fault waiting for the synth to drain it.
    pending: Option<ReplyMsg>,
    /// Whether the current mismatch has been reported.
    reported: bool,
}

impl PvCombine {
    pub fn new(op: CombineOp) -> Self {
        Self {
            op,
            pending: None,
            reported: false,
        }
    }
}

impl UGen for PvCombine {
    fn process(&mut self, _ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        output.fill(0.0);
    }

    fn process_spectral_pair(
        &mut self,
        _ctx: &mut ProcessCtx,
        _inputs: &[&[f32]],
        output: &mut [f32],
        a: &mut SpectralChain,
        b: &mut SpectralChain,
    ) {
        if a.ready && a.window != b.window {
            a.frame.fill(0.0);
            if !self.reported {
                self.reported = true;
                let mut msg = ReplyMsg::new(ReplyKind::Fault, FAULT_WINDOWS_DIFFER, self.op.name());
                msg.push_value(a.window.wintype() as f32);
                msg.push_value(b.window.wintype() as f32);
                self.pending = Some(msg);
            }
        } else if a.ready {
            self.reported = false;
            let half = a.winsize / 2;
            for k in 0..=half {
                let (ar, ai) = get_bin(&a.frame, k, half);
                let (br, bi) = get_bin(&b.frame, k, half);
                let (re, im) = match self.op {
                    CombineOp::Add => (ar + br, ai + bi),
                    CombineOp::Mul => (ar * br - ai * bi, ar * bi + ai * br),
                    CombineOp::Min | CombineOp::Max => {
                        let (ma, mb) = (ar * ar + ai * ai, br * br + bi * bi);
                        let take_b = match self.op {
                            CombineOp::Min => mb < ma,
                            _ => mb > ma,
                        };
                        if take_b { (br, bi) } else { (ar, ai) }
                    }
                    CombineOp::MagMul => {
                        let mb = (br * br + bi * bi).sqrt();
                        (ar * mb, ai * mb)
                    }
                    CombineOp::CopyPhase => {
                        let ma = (ar * ar + ai * ai).sqrt();
                        let mb = (br * br + bi * bi).sqrt();
                        if mb > 0.0 {
                            (br * ma / mb, bi * ma / mb)
                        } else {
                            (ma, 0.0) // B is silent: keep A's magnitude at phase 0.
                        }
                    }
                };
                set_bin(&mut a.frame, k, half, re, im);
            }
        }
        if let Some(o) = output.first_mut() {
            *o = if a.ready { 1.0 } else { 0.0 };
        }
    }

    fn is_reply(&self) -> bool {
        true
    }

    fn drain_replies(&mut self, node_id: i32, sink: &mut dyn FnMut(ReplyMsg)) {
        if let Some(mut msg) = self.pending.take() {
            msg.node_id = node_id;
            sink(msg);
        }
    }
}

/// Freezes the frame's magnitudes (`PV_MagFreeze`). Input: `[chain, freeze]`.
/// While `freeze <= 0` it stores each fresh frame's magnitudes and passes the
/// chain through; while `freeze > 0` every bin is rescaled to the stored
/// magnitude, phases left running -- the spectral envelope holds while the
/// texture keeps moving.
pub struct PvMagFreeze {
    /// Stored magnitudes, one per bin (`half + 1`), captured un-frozen.
    mags: Vec<f32>,
}

impl PvMagFreeze {
    pub fn new(config: &UGenConfig) -> Self {
        let winsize = resolve_fft_size(config.fft_size);
        Self {
            mags: vec![0.0; winsize / 2 + 1],
        }
    }
}

impl UGen for PvMagFreeze {
    fn process(&mut self, _ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        output.fill(0.0);
    }

    fn process_spectral(
        &mut self,
        _ctx: &mut ProcessCtx,
        inputs: &[&[f32]],
        output: &mut [f32],
        chain: &mut SpectralChain,
    ) {
        if chain.ready {
            let freeze = at(inputs[1], 0) > 0.0;
            let half = chain.winsize / 2;
            for b in 0..=half {
                let mag = bin_mag(&chain.frame, b, half);
                if freeze {
                    if mag > 0.0 {
                        scale_bin(&mut chain.frame, b, half, self.mags[b] / mag);
                    }
                    // A silent bin stays silent: there is no phase to rescale.
                } else {
                    self.mags[b] = mag;
                }
            }
        }
        if let Some(o) = output.first_mut() {
            *o = if chain.ready { 1.0 } else { 0.0 };
        }
    }
}

/// Averages each bin's magnitude over its neighbors (`PV_MagSmear`). Input:
/// `[chain, bins]` -- `bins` neighbors on each side (0 = pass through), phases
/// untouched. O(bins^2)-free: a prefix sum over the magnitudes makes every
/// window average O(1).
pub struct PvMagSmear {
    /// Prefix sums of the frame's magnitudes (`half + 2` entries).
    prefix: Vec<f32>,
}

impl PvMagSmear {
    pub fn new(config: &UGenConfig) -> Self {
        let winsize = resolve_fft_size(config.fft_size);
        Self {
            prefix: vec![0.0; winsize / 2 + 2],
        }
    }
}

impl UGen for PvMagSmear {
    fn process(&mut self, _ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        output.fill(0.0);
    }

    fn process_spectral(
        &mut self,
        _ctx: &mut ProcessCtx,
        inputs: &[&[f32]],
        output: &mut [f32],
        chain: &mut SpectralChain,
    ) {
        if chain.ready {
            let bins = (at(inputs[1], 0).max(0.0)) as usize;
            let half = chain.winsize / 2;
            if bins > 0 {
                // prefix[b+1] = sum mag[0..=b], so a clamped window average is
                // one subtraction and one divide per bin.
                self.prefix[0] = 0.0;
                for b in 0..=half {
                    self.prefix[b + 1] = self.prefix[b] + bin_mag(&chain.frame, b, half);
                }
                for b in 0..=half {
                    let lo = b.saturating_sub(bins);
                    let hi = (b + bins).min(half);
                    let avg = (self.prefix[hi + 1] - self.prefix[lo]) / (hi - lo + 1) as f32;
                    let mag = bin_mag(&chain.frame, b, half);
                    if mag > 0.0 {
                        scale_bin(&mut chain.frame, b, half, avg / mag);
                    } else {
                        set_bin(&mut chain.frame, b, half, avg, 0.0);
                    }
                }
            }
        }
        if let Some(o) = output.first_mut() {
            *o = if chain.ready { 1.0 } else { 0.0 };
        }
    }
}

/// Remaps bin positions (`PV_BinShift` / `PV_MagShift`): destination bin
/// `round(b*stretch + shift)`, colliding bins summed, out-of-range bins
/// dropped. Inputs: `[chain, stretch, shift]`. One implementation, two
/// registered names -- `PV_BinShift` moves the full complex bins (phases
/// travel with their magnitudes), `PV_MagShift` (`mags_only`) remaps only the
/// magnitude envelope onto the frame's original phases.
pub struct PvBinShift {
    mags_only: bool,
    /// Remap scratch: a full packed frame (complex mode) or `half + 1`
    /// magnitudes (`mags_only`); sized at build, zeroed per fresh frame.
    scratch: Vec<f32>,
}

impl PvBinShift {
    pub fn new(config: &UGenConfig, mags_only: bool) -> Self {
        let winsize = resolve_fft_size(config.fft_size);
        Self {
            mags_only,
            scratch: vec![0.0; winsize],
        }
    }
}

impl UGen for PvBinShift {
    fn process(&mut self, _ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        output.fill(0.0);
    }

    fn process_spectral(
        &mut self,
        _ctx: &mut ProcessCtx,
        inputs: &[&[f32]],
        output: &mut [f32],
        chain: &mut SpectralChain,
    ) {
        if chain.ready {
            let stretch = at(inputs[1], 0);
            let shift = at(inputs[2], 0);
            let half = chain.winsize / 2;
            self.scratch.fill(0.0);
            for b in 0..=half {
                let t = (b as f32 * stretch + shift).round();
                if t < 0.0 || t > half as f32 {
                    continue;
                }
                let t = t as usize;
                if self.mags_only {
                    self.scratch[t] += bin_mag(&chain.frame, b, half);
                } else {
                    let (re, im) = get_bin(&chain.frame, b, half);
                    let (tr, ti) = get_bin(&self.scratch, t, half);
                    set_bin(&mut self.scratch, t, half, tr + re, ti + im);
                }
            }
            if self.mags_only {
                // Remapped magnitude envelope over the original phases.
                for b in 0..=half {
                    let mag = bin_mag(&chain.frame, b, half);
                    if mag > 0.0 {
                        scale_bin(&mut chain.frame, b, half, self.scratch[b] / mag);
                    } else {
                        set_bin(&mut chain.frame, b, half, self.scratch[b], 0.0);
                    }
                }
            } else {
                chain.frame.copy_from_slice(&self.scratch);
            }
        }
        if let Some(o) = output.first_mut() {
            *o = if chain.ready { 1.0 } else { 0.0 };
        }
    }
}

/// The general per-frame mechanism (`PV_Kernel`): interprets a pair of
/// compile-validated bin-expression programs (`clausters_core::pvprog`) over
/// every bin of each fresh frame -- magnitude and phase each get one program
/// mapping `(mag, phase, bin, nbins, binfreq, p0...)` to the bin's new value.
/// Inputs: `[chain, p0, p1, ...]` -- the parameters are ordinary signal inputs
/// sampled at the hop, so they can be controls, LFOs, anything.
///
/// An omitted program is the identity, and the identity *phase* program takes
/// the exact scaling path of the curated magnitude ops (`scale_bin`, no
/// `atan2`/`cos`/`sin` round trip): a pure magnitude map is both cheap and
/// bit-identical to a hand-written `PV_*` filter computing the same formula.
/// The polar phase is only computed when some program actually reads it.
///
/// The programs are a **per-bin map** -- no state across bins or frames, no
/// bin remapping. Those stay curated implementations (`PV_MagFreeze`,
/// `PV_BinShift`, ...) per the stance; see `docs/decisions.md`.
pub struct PvKernel {
    mag: PvProgram,
    phase: PvProgram,
    /// Shared evaluation stack, sized at build to the deeper program.
    stack: Vec<f32>,
}

impl PvKernel {
    pub fn new(config: &UGenConfig) -> Self {
        let mag = config
            .mag_prog
            .clone()
            .unwrap_or_else(|| PvProgram::identity(PvOp::Mag));
        let phase = config
            .phase_prog
            .clone()
            .unwrap_or_else(|| PvProgram::identity(PvOp::Phase));
        let stack = vec![0.0; mag.stack_depth().max(phase.stack_depth())];
        Self { mag, phase, stack }
    }
}

impl UGen for PvKernel {
    fn process(&mut self, _ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        output.fill(0.0);
    }

    fn process_spectral(
        &mut self,
        ctx: &mut ProcessCtx,
        inputs: &[&[f32]],
        output: &mut [f32],
        chain: &mut SpectralChain,
    ) {
        if chain.ready {
            // Parameters: inputs 1.. sampled at the hop (block-rate reads).
            let mut params = [0.0f32; MAX_UGEN_INPUTS];
            let n_params = inputs.len().saturating_sub(1);
            for (p, input) in params.iter_mut().zip(&inputs[1..]) {
                *p = at(input, 0);
            }
            let half = chain.winsize / 2;
            // The engine's rate, not this UGen's: a `PV_*` runs at `kr` but the
            // spectrum it edits is of an audio-rate signal.
            let hz_per_bin = ctx.full_sample_rate / chain.winsize as f32;
            // The identity phase program keeps each bin's phase by *scaling*
            // the complex pair -- exact, and no polar conversion unless a
            // program reads `phase`.
            let keep_phase = self.phase.is_identity(PvOp::Phase);
            let need_phase = !keep_phase || self.mag.uses_phase();
            for b in 0..=half {
                let (re, im) = get_bin(&chain.frame, b, half);
                let mag = (re * re + im * im).sqrt();
                let bin_ctx = BinCtx {
                    mag,
                    phase: if need_phase { im.atan2(re) } else { 0.0 },
                    bin: b as f32,
                    nbins: (half + 1) as f32,
                    binfreq: b as f32 * hz_per_bin,
                    params: &params[..n_params],
                };
                let new_mag = self.mag.eval(&bin_ctx, &mut self.stack);
                if keep_phase {
                    if mag > 0.0 {
                        scale_bin(&mut chain.frame, b, half, new_mag / mag);
                    } else {
                        set_bin(&mut chain.frame, b, half, new_mag, 0.0);
                    }
                } else {
                    let new_phase = self.phase.eval(&bin_ctx, &mut self.stack);
                    set_bin(
                        &mut chain.frame,
                        b,
                        half,
                        new_mag * new_phase.cos(),
                        new_mag * new_phase.sin(),
                    );
                }
            }
        }
        if let Some(o) = output.first_mut() {
            *o = if chain.ready { 1.0 } else { 0.0 };
        }
    }
}

/// A brick-wall band limiter: `PV_BrickWall`. Input: `[chain, wipe]` with
/// `wipe` in `-1..1`. `wipe > 0` zeroes the top `wipe` fraction of bins (a low
/// pass); `wipe < 0` zeroes the bottom `|wipe|` fraction (a high pass);
/// `wipe == 0` passes everything.
pub struct PvBrickWall;

impl UGen for PvBrickWall {
    fn process(&mut self, _ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        output.fill(0.0);
    }

    fn process_spectral(
        &mut self,
        _ctx: &mut ProcessCtx,
        inputs: &[&[f32]],
        output: &mut [f32],
        chain: &mut SpectralChain,
    ) {
        if chain.ready {
            let wipe = at(inputs[1], 0).clamp(-1.0, 1.0);
            let half = chain.winsize / 2;
            let nbins = (half + 1) as f32;
            if wipe > 0.0 {
                // Low pass: zero bins above the cutoff.
                let cutoff = (nbins * (1.0 - wipe)).round() as usize;
                for b in cutoff..=half {
                    zero_bin(&mut chain.frame, b, half);
                }
            } else if wipe < 0.0 {
                // High pass: zero bins below the cutoff.
                let cutoff = (nbins * (-wipe)).round() as usize;
                for b in 0..cutoff.min(half + 1) {
                    zero_bin(&mut chain.frame, b, half);
                }
            }
        }
        if let Some(o) = output.first_mut() {
            *o = if chain.ready { 1.0 } else { 0.0 };
        }
    }
}
