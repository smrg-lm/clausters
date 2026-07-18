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
//! SuperCollider's `LocalBuf`. No `/b_alloc` is required and the sample pool
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
use clausters_core::window::Window;

use crate::dsp::registry::UGenConfig;
use crate::dsp::{BLOCK_SIZE, ProcessCtx, UGen, UGenCmd, at, ugen_cmd_selector};

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
/// through `/u_cmd` (selector `window`), the first real consumer of the S6
/// typed per-UGen command surface.
pub struct Fft {
    winsize: usize,
    hop_size: usize,
    window_kind: Window,
    /// Analysis window coefficients (`winsize`); rebuilt when `/u_cmd` changes
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
        // `/u_cmd <node> <ugen> window <wintype>`: swap the analysis window.
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
#[derive(Clone, Copy)]
pub enum MagMode {
    /// Keep bins whose magnitude is **above** the threshold (`PV_MagAbove`).
    Above,
    /// Keep bins whose magnitude is **below** the threshold (`PV_MagBelow`).
    Below,
}

/// A magnitude-threshold spectral filter: `PV_MagAbove`/`PV_MagBelow`. Input:
/// `[chain, threshold]`. It zeroes the bins failing the test on each fresh
/// frame; other blocks pass the (unchanged) chain through.
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
                let keep = match self.mode {
                    MagMode::Above => bin_mag(&chain.frame, b, half) >= thresh,
                    MagMode::Below => bin_mag(&chain.frame, b, half) <= thresh,
                };
                if !keep {
                    zero_bin(&mut chain.frame, b, half);
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
