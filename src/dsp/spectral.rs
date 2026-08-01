//! The frequency-domain (`fr`) chain: `FFT` → `PV_*` → `IFFT` (S8).
//!
//! scsynth's spectral processing bookends a chain of `PV_*` (phase-vocoder)
//! UGens between [`Fft`] (window an audio input and transform it to a complex
//! frame) and [`Ifft`] (inverse-transform and overlap-add back to audio). The
//! chain is **not block-rate**: `FFT` emits one spectral frame per **hop**, and
//! the `PV_*` UGens only touch the frame on the blocks a fresh one is ready —
//! the frame-rate (`fr`) substrate, kin to the demand (`dr`) sub-list of S1.
//!
//! ## Where the spectral frame lives (a deliberate deviation from scsynth)
//!
//! scsynth threads the frame through a client-allocated buffer whose bin data
//! the audio thread mutates in place — which would break Clausters' invariant
//! that a pool [`Buffer`](super::buffer::Buffer) is immutable once built. So the
//! frame lives **not** in the sample-buffer pool but in a [`SpectralChain`]:
//! synth-private scratch, allocated when the synth is instantiated (on the
//! network thread, where allocation is legal) and freed with the synth — exactly
//! like the `LocalIn`/`LocalOut` feedback `locals`, and the moral equivalent of
//! SuperCollider's `LocalBuf`. No `/buffer_alloc` is required and the sample pool
//! stays fully immutable. The chain is shared by the chain's UGens through a
//! compile-assigned *slot* the synth resolves for each of them (see
//! `synthdef::instance`); the wire between the UGens only enforces ordering.
//!
//! The [`SpectralChain::advance`] field carries how many input samples the last
//! hop covered, so `Ifft` overlap-adds and emits exactly that many samples per
//! frame — keeping analysis and resynthesis in lockstep regardless of how the
//! hop size relates to the block size (the hop is effectively quantized up to
//! the processing slice, as scsynth computes its FFT at block granularity).
//!
//! The transforms and the windows are the single-sourced
//! [`clausters_core::fft`] / [`clausters_core::window`], shared with the clients
//! for bit-identical analysis. Every per-hop transform reuses pre-allocated
//! scratch, so nothing here allocates on the audio thread.
//!
//! ## Hop-phase stagger (S11)
//!
//! A chain concentrates all its work on the block where its hop closes; chains
//! instantiated on the same block would all hop on the same block, stacking
//! their transform spikes. So each [`Fft`] delays its *first* frame by a
//! deterministic sub-hop offset derived from its node id
//! ([`UGen::set_node_id`], delivered by the engine when the node enters the
//! tree). Only the initial fire shifts — the cadence, the analysis discipline
//! and a chain's own latency-to-content are unchanged — and the same score
//! yields the same ids, so RT and NRT renders stay sample-identical.

use clausters_core::fft;
use clausters_core::pvprog::{BinCtx, PvOp, PvProgram};
use clausters_core::window::Window;

use crate::dsp::registry::UGenConfig;
use crate::dsp::{BLOCK_SIZE, MAX_UGEN_INPUTS, ProcessCtx, UGen, UGenCmd, at, ugen_cmd_selector};

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

fn resolve_hop(winsize: usize, hop: Option<f32>) -> usize {
    let frac = hop.unwrap_or(0.5);
    ((winsize as f32 * frac).round() as usize).clamp(1, winsize)
}

/// The synth-private spectral frame shared by one `FFT`→`PV_*`→`IFFT` chain.
/// Persistent across blocks (like the feedback `locals`); allocated once at
/// synth init. See the module docs for why this replaces scsynth's mutable pool
/// buffer.
pub struct SpectralChain {
    /// The packed complex frame, `winsize` floats in the
    /// [`fft::rfft_into`] layout `[dc, nyquist, re₁, im₁, …]`.
    pub frame: Vec<f32>,
    /// True on the processing slice where `FFT` wrote a fresh frame; the
    /// `PV_*`/`IFFT` UGens act only then. `FFT` clears it each slice.
    pub ready: bool,
    /// Input samples the fresh frame advanced past the previous one (the hop).
    /// `IFFT` overlap-adds and emits this many samples for the frame.
    pub advance: usize,
    /// The frame's transform size.
    pub winsize: usize,
}

impl SpectralChain {
    pub fn new(winsize: usize) -> Self {
        Self {
            frame: vec![0.0; winsize],
            ready: false,
            advance: 0,
            winsize,
        }
    }
}

/// Windows an audio input and transforms it to a spectral frame once per hop.
///
/// Inputs: `[in, active]` — the audio signal and a control that gates the
/// transform (`> 0` on, `<= 0` off, holding the last frame). The window size,
/// hop and window type are static per-UGen config, not signal inputs, because
/// they size the pre-allocated scratch. The window type is also settable live
/// through `/node_ugenCmd` (selector `window`), the first real consumer of the S6
/// typed per-UGen command surface.
pub struct Fft {
    winsize: usize,
    hop_size: usize,
    window_kind: Window,
    /// Analysis window coefficients (`winsize`); rebuilt when `/node_ugenCmd` changes
    /// the window type — off any hop, so still allocation-free per block.
    window: Vec<f32>,
    /// Sliding input, a circular buffer of the last `winsize` samples.
    inbuf: Vec<f32>,
    write: usize,
    /// Samples seen since start (gates the first frame until the buffer fills).
    filled: usize,
    /// Samples accumulated since the last emitted frame.
    since_hop: usize,
    /// Hop-phase stagger (S11): samples still to elapse before this instance
    /// may emit its *first* frame. Set once from the node id in
    /// [`UGen::set_node_id`] — a deterministic sub-hop offset (a block
    /// multiple) so chains instantiated on the same block spread their
    /// transform spikes across blocks instead of stacking them on one. Only
    /// the first fire shifts; the per-hop cadence and the analysis discipline
    /// are untouched, and the same node id yields the same offset (RT and NRT
    /// renders of one score stay sample-identical).
    stagger: usize,
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
            inbuf: vec![0.0; winsize],
            write: 0,
            filled: 0,
            since_hop: 0,
            stagger: 0,
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
        let input = inputs[0];
        let active = inputs.get(1).map(|a| at(a, 0)).unwrap_or(1.0) > 0.0;
        let frames = input.len();
        // Push this slice's samples into the sliding input buffer.
        for &s in input {
            self.inbuf[self.write] = if active { s } else { 0.0 };
            self.write = (self.write + 1) % self.winsize;
        }
        self.filled = (self.filled + frames).min(self.winsize);
        self.since_hop += frames;
        // Emit at most one frame per slice (the hop is quantized up to the
        // slice length; scsynth likewise transforms at block granularity).
        if active
            && self.stagger == 0
            && self.filled >= self.winsize
            && self.since_hop >= self.hop_size
        {
            // De-circularize: `write` points at the oldest sample.
            for k in 0..self.winsize {
                let s = self.inbuf[(self.write + k) % self.winsize];
                self.scratch[k] = s * self.window[k];
            }
            fft::rfft_into(&self.scratch, &mut chain.frame);
            chain.advance = self.since_hop;
            chain.ready = true;
            self.since_hop = 0;
        } else if self.filled >= self.winsize {
            // Count the stagger down only once the window is full and only on
            // slices that did not fire: it defers the *first* frame by whole
            // elapsed slices past the fill point (see the field docs).
            self.stagger = self.stagger.saturating_sub(frames);
        }
        // The wire only orders the chain; carry the slot marker for debugging.
        if let Some(o) = output.first_mut() {
            *o = if chain.ready { 1.0 } else { 0.0 };
        }
    }

    fn set_node_id(&mut self, id: i32) {
        // S11: derive the deterministic hop-phase stagger — the node id modulo
        // the hop's block count, in whole blocks. A hop no longer than one
        // block cannot stack (at most one frame per slice already), so it
        // keeps offset 0.
        let blocks_per_hop = self.hop_size / BLOCK_SIZE;
        if blocks_per_hop > 1 {
            self.stagger = (id.unsigned_abs() as usize % blocks_per_hop) * BLOCK_SIZE;
        }
    }

    fn command(&mut self, cmd: &UGenCmd) {
        // `/node_ugenCmd <node> <ugen> window <wintype>`: swap the analysis window.
        if cmd.selector == ugen_cmd_selector("window") && cmd.num_args >= 1 {
            let kind = Window::from_wintype(cmd.args[0] as i32);
            if kind != self.window_kind {
                self.window_kind = kind;
                kind.fill(&mut self.window);
            }
        }
    }
}

/// Inverse-transforms each fresh spectral frame and overlap-adds it back to
/// audio. Input: `[chain]` — the chain wire, which only carries ordering (the
/// live frame is the synth-private [`SpectralChain`] the synth passes in). The
/// window size and type are static config; the synthesis window matches the
/// analysis window for correct overlap-add.
pub struct Ifft {
    winsize: usize,
    hop_size: usize,
    window_kind: Window,
    window: Vec<f32>,
    /// Overlap-add tail: `olabuf[k]` accumulates the windowed reconstruction.
    olabuf: Vec<f32>,
    /// The steady-state overlap-add normalization (COLA), one value per hop
    /// phase: `norm[r] = Σ_i window[r + i·hop]²` over the frames that overlap
    /// output phase `r`. Precomputed at build (constant per render), so dividing
    /// by it never over-amplifies the under-overlapped edges of the startup or a
    /// spectrally modified frame — unlike a running per-sample window sum.
    norm: Vec<f32>,
    /// Time-domain scratch for the inverse transform.
    time: Vec<f32>,
    /// Finalized samples awaiting output, a ring drained `frames` per slice.
    fifo: Vec<f32>,
    fifo_head: usize,
    fifo_tail: usize,
    fifo_len: usize,
    /// Absolute output position of `olabuf[0]`, modulo the hop — the phase into
    /// [`norm`](Self::norm), tracked so the COLA denominator stays aligned even
    /// if a frame's `advance` is not a multiple of the hop.
    phase: usize,
}

impl Ifft {
    pub fn new(config: &UGenConfig) -> Self {
        let winsize = resolve_fft_size(config.fft_size);
        let hop_size = resolve_hop(winsize, config.hop);
        let window_kind = Window::from_wintype(config.wintype.unwrap_or(0));
        let mut window = vec![0.0; winsize];
        window_kind.fill(&mut window);
        // Steady-state window-power sum per hop phase (the exact COLA
        // denominator once the overlap is full). Guarded against a zero phase so
        // the division is always safe.
        let mut norm = vec![0.0f32; hop_size];
        for (r, slot) in norm.iter_mut().enumerate() {
            let mut s = 0.0;
            let mut k = r;
            while k < winsize {
                s += window[k] * window[k];
                k += hop_size;
            }
            *slot = if s > 1e-9 { s } else { 1.0 };
        }
        // The FIFO holds at most a couple of hops' worth of finalized samples
        // between the frame that produces them and the slices that drain them.
        let fifo = vec![0.0; 4 * winsize];
        Self {
            winsize,
            hop_size,
            window_kind,
            window,
            olabuf: vec![0.0; winsize],
            norm,
            time: vec![0.0; winsize],
            fifo,
            fifo_head: 0,
            fifo_tail: 0,
            fifo_len: 0,
            phase: 0,
        }
    }

    #[inline]
    fn fifo_push(&mut self, v: f32) {
        if self.fifo_len < self.fifo.len() {
            self.fifo[self.fifo_tail] = v;
            self.fifo_tail = (self.fifo_tail + 1) % self.fifo.len();
            self.fifo_len += 1;
        }
    }

    #[inline]
    fn fifo_pop(&mut self) -> f32 {
        if self.fifo_len == 0 {
            return 0.0;
        }
        let v = self.fifo[self.fifo_head];
        self.fifo_head = (self.fifo_head + 1) % self.fifo.len();
        self.fifo_len -= 1;
        v
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
        if chain.ready {
            let advance = chain.advance.min(self.winsize);
            fft::irfft_into(&chain.frame, &mut self.time);
            // Overlap-add the windowed reconstruction into the tail.
            for k in 0..self.winsize {
                self.olabuf[k] += self.time[k] * self.window[k];
            }
            // The first `advance` samples are final (no later frame overlaps
            // them): normalize by the steady-state COLA denominator for the
            // sample's hop phase and emit them. Dividing by the *full* overlap
            // sum (not a running partial one) means an incompletely overlapped
            // startup or a spectrally modified frame fades cleanly instead of
            // blowing up where the window is small. `advance` is a multiple of
            // the hop, so phase 0 stays aligned to `norm[0]`.
            for k in 0..advance {
                let r = (self.phase + k) % self.hop_size;
                self.fifo_push(self.olabuf[k] / self.norm[r]);
            }
            self.phase = (self.phase + advance) % self.hop_size;
            // Shift the tail left by `advance`, zeroing the vacated end.
            let keep = self.winsize.saturating_sub(advance);
            self.olabuf.copy_within(advance.., 0);
            for k in keep..self.winsize {
                self.olabuf[k] = 0.0;
            }
        }
        for o in output.iter_mut() {
            *o = self.fifo_pop();
        }
    }

    fn command(&mut self, cmd: &UGenCmd) {
        if cmd.selector == ugen_cmd_selector("window") && cmd.num_args >= 1 {
            let kind = Window::from_wintype(cmd.args[0] as i32);
            if kind != self.window_kind {
                self.window_kind = kind;
                kind.fill(&mut self.window);
            }
        }
    }
}

/// The kind of magnitude threshold a [`PvMag`] filter applies to each bin.
/// One implementation, three registered names — the mode is a parameter, not
/// a UGen (the M27 stance: no one-UGen-per-op catalog).
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

/// The operator of a [`PvCombine`] two-chain combiner — a parameter of one
/// implementation, registered under the scsynth-compatible names (the M27
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
    /// A's bin scaled by B's magnitude — A's phases kept (`PV_MagMul`).
    MagMul,
    /// A's magnitudes with B's phases (`PV_CopyPhase`).
    CopyPhase,
}

/// A two-chain spectral combiner (`SpectralRole::Filter2`): inputs
/// `[chain_a, chain_b]`, the result written into chain A bin by bin. It acts
/// on the slices where **A** has a fresh frame, reading B's *latest* frame
/// (the frame is persistent chain state; two same-config `FFT`s in one synth
/// hop on the same blocks anyway, S11 staggering included — the offset is
/// per-node, not per-UGen).
pub struct PvCombine {
    op: CombineOp,
}

impl PvCombine {
    pub fn new(op: CombineOp) -> Self {
        Self { op }
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
        if a.ready {
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
}

/// Freezes the frame's magnitudes (`PV_MagFreeze`). Input: `[chain, freeze]`.
/// While `freeze <= 0` it stores each fresh frame's magnitudes and passes the
/// chain through; while `freeze > 0` every bin is rescaled to the stored
/// magnitude, phases left running — the spectral envelope holds while the
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
/// `[chain, bins]` — `bins` neighbors on each side (0 = pass through), phases
/// untouched. O(bins²)-free: a prefix sum over the magnitudes makes every
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
                // prefix[b+1] = Σ mag[0..=b], so a clamped window average is
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
/// `round(b·stretch + shift)`, colliding bins summed, out-of-range bins
/// dropped. Inputs: `[chain, stretch, shift]`. One implementation, two
/// registered names — `PV_BinShift` moves the full complex bins (phases
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
/// every bin of each fresh frame — magnitude and phase each get one program
/// mapping `(mag, phase, bin, nbins, binfreq, p0…)` to the bin's new value.
/// Inputs: `[chain, p0, p1, …]` — the parameters are ordinary signal inputs
/// sampled at the hop, so they can be controls, LFOs, anything.
///
/// An omitted program is the identity, and the identity *phase* program takes
/// the exact scaling path of the curated magnitude ops (`scale_bin`, no
/// `atan2`/`cos`/`sin` round trip): a pure magnitude map is both cheap and
/// bit-identical to a hand-written `PV_*` filter computing the same formula.
/// The polar phase is only computed when some program actually reads it.
///
/// The programs are a **per-bin map** — no state across bins or frames, no
/// bin remapping. Those stay curated implementations (`PV_MagFreeze`,
/// `PV_BinShift`, …) per the M27 stance; see `docs/decisions.md`.
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
            // the complex pair — exact, and no polar conversion unless a
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
