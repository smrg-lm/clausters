//! Panning, stereo-field transforms and selection (U7): one pan law, one
//! two-by-two matrix, one crossfade.
//!
//! **One law, computed rather than looked up.** Every equal-power row here —
//! `Pan2`, `Balance2`, `XFade2`, `SelectX`, and `PanAz`'s window — asks the same
//! question: what pair of gains holds `l^2 + r^2` at one while moving a source
//! from one side to the other. scsynth answers it with a rounded lookup into a
//! 2049-entry sine table, whose worst-case gain error is about `3.8e-4`. Here
//! is a fifth-term polynomial in the position ([`quarter_sin`]), worst-case
//! `2.6e-7` and exact at both ends — three orders of magnitude closer for
//! about ten flops, no table, and nothing to initialize.
//!
//! **The pair is symmetric by construction.** A gain pair is
//! `(quarter_sin(1 - t), quarter_sin(t))`, the *same* function read from both
//! ends, so panning to `-pos` gives exactly the mirror of panning to `pos` —
//! a property of the expression, not something the tests have to keep honest to
//! within a tolerance.
//!
//! **Position is evaluated per sample when it is a signal.** This is the one
//! place the track's block-rate stance does not apply. Interpolating the two
//! gains linearly across a block — what a filter coefficient does here, and what
//! scsynth's `CALCSLOPE` does for its own amplitudes — puts `0.5` where the law
//! wants `0.707` if the position sweeps a full block, a 3 dB hole in the middle
//! of the block. So a scalar position computes its gains once per block, and an
//! audio-rate one computes them per sample; the polynomial is what makes the
//! second affordable.
//!
//! **The per-sample mix stays in `f64` because the law is, and that is a
//! consistency choice rather than a requirement.** The gains come out of an
//! `f64` polynomial, and every row here then *combines* its `f32` inputs in
//! `f64` too before rounding once to `f32` — `(x as f64 * cx + y as f64 * cy)
//! as f32` and its cousins in [`Pan`], [`Rotate`] and [`PanAz`]. Only the first
//! half of that is load-bearing: the law's exactness properties (an exact
//! endpoint, an exact quadrant reduction, a symmetric pair) live in the
//! coefficients, not in how the two products are added.
//!
//! Combining in `f32` instead was measured rather than argued about. It is
//! **2.30× faster on the mix loop alone** — the `f64` version vectorizes two
//! lanes wide (`mulpd`), the `f32` one four — and it costs at most `5.96e-8`
//! of absolute disagreement over a sweep of 2001 angles, which is half an ulp
//! at full scale, or -144 dBFS. (The *relative* error over that sweep reads a
//! frightening -66 dB, but only where the output is itself near zero through
//! cancellation — the side channel of a near-mono pair. That figure is an
//! artifact of dividing by nothing, not an audio number.)
//!
//! It stays `f64` anyway, because the engine cannot see the difference: on
//! `Sine → Pan2 → 2× Out` the whole-graph throughput is unchanged. A row's
//! arithmetic is a small part of a block that spends most of its time in its
//! sources, which is the same reason the fused rows kept their naive loops
//! (`docs/decisions.md`). Anyone revisiting this should get an engine-level
//! number first — the isolated 2.30× is real and has never been worth anything.
//!
//! **Rotation and width are different operations, and only one of them is
//! scsynth's.** `Rotate2` rotates the plane the two signals span: it moves the
//! stereo image without changing its size, and at a quarter turn the rotation
//! *is* the change of basis between left/right and mid/side. Width scales the
//! side axis: it changes the size of the image without moving it, and no angle
//! produces it. Both are the same two-by-two matrix with a different
//! parameterization, so [`Rotate`] carries all three rows —
//! `Rotate2`, `MidSide` and `StereoWidth`.

use std::f64::consts::FRAC_PI_2;

use crate::dsp::{ProcessCtx, UGen, at};

// --- the pan law ---------------------------------------------------------

/// Polynomial coefficients of `sin(t * pi/2)` on `t` in `[0, 1]`, in odd powers
/// of `t`. The first four are the Taylor series'; the fifth is *defined* as
/// whatever makes the five sum to one, which lands within `3.5e-6` of the
/// Taylor coefficient and buys an exact `quarter_sin(1) == 1`. Forcing that
/// sum also cancels most of the truncation error across the range, which is
/// where the `2.6e-7` worst case comes from.
///
/// That endpoint matters more than its size suggests: it is the gain of a
/// hard-panned source in the channel it is panned *to*. The other end,
/// `quarter_sin(0) == 0`, is exact for free — it is the bare factor `t` — and
/// is the gain in the channel it is panned *away* from, so a hard pan is
/// digital silence on the far side rather than -110 dB of it.
const A1: f64 = FRAC_PI_2;
const A3: f64 = -FRAC_PI_2 * FRAC_PI_2 * FRAC_PI_2 / 6.0;
const A5: f64 = FRAC_PI_2 * FRAC_PI_2 * FRAC_PI_2 * FRAC_PI_2 * FRAC_PI_2 / 120.0;
const A7: f64 =
    -FRAC_PI_2 * FRAC_PI_2 * FRAC_PI_2 * FRAC_PI_2 * FRAC_PI_2 * FRAC_PI_2 * FRAC_PI_2 / 5040.0;
const A9: f64 = 1.0 - A1 - A3 - A5 - A7;

/// `sin(t * pi/2)` for `t` in `[0, 1]` — the quarter sine the whole module is
/// built on. Outside that range the polynomial is meaningless, so every caller
/// clamps or reduces first.
#[inline]
pub fn quarter_sin(t: f64) -> f64 {
    let t2 = t * t;
    t * (A1 + t2 * (A3 + t2 * (A5 + t2 * (A7 + t2 * A9))))
}

/// `(sin, cos)` of `pi * p` for **any** real `p`, by reducing to a quadrant and
/// reading [`quarter_sin`] from one end or the other.
///
/// The reduction is exact at every quadrant boundary, so a half turn is exactly
/// a sign flip and a quarter turn is exactly the mid/side basis.
#[inline]
pub fn sin_cos_pi(p: f64) -> (f64, f64) {
    // Quarter turns since zero: the integer part picks the quadrant, the
    // fraction indexes into it.
    let u = (p * 2.0).rem_euclid(4.0);
    let k = u as u32; // 0..=3
    let t = u - k as f64;
    let (s, c) = (quarter_sin(t), quarter_sin(1.0 - t));
    match k {
        0 => (s, c),
        1 => (c, -s),
        2 => (-s, -c),
        _ => (-c, s),
    }
}

/// `sin(pi * t)` for `t` in `[0, 1]` — one lobe, used as `PanAz`'s window.
#[inline]
fn half_sin(t: f64) -> f64 {
    if t <= 0.5 {
        quarter_sin(t * 2.0)
    } else {
        quarter_sin((1.0 - t) * 2.0)
    }
}

/// The equal-power gain pair for a position in `[-1, 1]`: `l^2 + r^2 == 1`,
/// `(0.707, 0.707)` at the centre.
#[inline]
fn equal_power(pos: f64) -> (f64, f64) {
    let t = (pos * 0.5 + 0.5).clamp(0.0, 1.0);
    (quarter_sin(1.0 - t), quarter_sin(t))
}

/// The constant-**amplitude** gain pair: `l + r == 1`, `(0.5, 0.5)` at the
/// centre. Two channels carrying it and summing coherently — a mono listener,
/// or a fold-down — stay at one level, at the price of a 3 dB dip in the middle
/// for anything that sums by power instead.
#[inline]
fn linear(pos: f64) -> (f64, f64) {
    let t = (pos * 0.5 + 0.5).clamp(0.0, 1.0);
    (1.0 - t, t)
}

// --- the two-channel pan / crossfade family ------------------------------

/// Which row of the pan family an instance is. All five compute one gain pair
/// and apply it; they differ in how many sources they read, which law they use,
/// and whether the two channels leave separately or summed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PanKind {
    /// One source placed between two channels, equal power: `Pan2`.
    Pan2,
    /// One source placed between two channels, constant amplitude: `LinPan2`.
    LinPan2,
    /// A stereo pair with one side attenuated, equal power: `Balance2`.
    Balance2,
    /// Two sources faded into one output, equal power: `XFade2`.
    XFade2,
    /// Two sources faded into one output, constant amplitude: `LinXFade2`.
    LinXFade2,
}

impl PanKind {
    /// Whether this row reads a second source. `Pan2`/`LinPan2` place one
    /// signal; the rest take a pair.
    #[inline]
    fn stereo_source(self) -> bool {
        !matches!(self, PanKind::Pan2 | PanKind::LinPan2)
    }

    /// Whether the two channels leave summed on one output (a crossfade) or
    /// separately (a pan).
    #[inline]
    fn sums(self) -> bool {
        matches!(self, PanKind::XFade2 | PanKind::LinXFade2)
    }

    /// This row's gain pair at `pos`.
    #[inline]
    fn gains(self, pos: f64) -> (f64, f64) {
        match self {
            PanKind::LinPan2 | PanKind::LinXFade2 => linear(pos),
            _ => equal_power(pos),
        }
    }

    /// Wire inputs before `pos`.
    #[inline]
    fn sources(self) -> usize {
        if self.stereo_source() { 2 } else { 1 }
    }
}

/// `Pan2`, `LinPan2`, `Balance2`, `XFade2`, `LinXFade2`.
///
/// Wire order is scsynth's — the sources, then `pos`, then `level` — with the
/// output channel index **last**, where the builder puts it and the reader
/// never has to look. A summing row has none.
///
/// That index is a wire input read at index 0, not a build-time field, for the
/// same reason `PlayBuf`'s channel is: it is what a def writes down, and the
/// build closure never sees the def's values.
pub struct Pan {
    kind: PanKind,
}

impl Pan {
    pub fn new(kind: PanKind) -> Self {
        Self { kind }
    }
}

impl UGen for Pan {
    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        // A one-source row feeds the same signal to both gains, which is
        // exactly what panning a mono source means.
        let src = inputs[0];
        let (a, b) = if self.kind.stereo_source() {
            (inputs[0], inputs[1])
        } else {
            (src, src)
        };
        let base = self.kind.sources();
        let pos = inputs[base];
        let level = inputs[base + 1];

        // A scalar position is the common case (a control, a constant): the
        // polynomial runs once for the block instead of once per sample.
        let fixed = (pos.len() == 1).then(|| self.kind.gains(pos[0] as f64));
        let sums = self.kind.sums();
        // A summing row has no channel index on the wire, so the slice is only
        // reached once the first operand has ruled that case out.
        let right = !sums && at(inputs[base + 2], 0) >= 0.5;

        for (i, s) in output.iter_mut().enumerate() {
            let (gl, gr) = match fixed {
                Some(g) => g,
                None => self.kind.gains(at(pos, i) as f64),
            };
            // `f64` by consistency with the law, not by need: see the module doc.
            let v = if sums {
                at(a, i) as f64 * gl + at(b, i) as f64 * gr
            } else if right {
                at(b, i) as f64 * gr
            } else {
                at(a, i) as f64 * gl
            };
            *s = (v * at(level, i) as f64) as f32;
        }
    }
}

// --- the stereo-field matrix ---------------------------------------------

/// Which parameterization of the two-by-two matrix an instance is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RotKind {
    /// Rotate the plane by `pi * pos` — scsynth's `Rotate2`. On a stereo pair
    /// it turns the image; on the mid/side pair of a B-format signal it turns
    /// the sound field.
    Rotate2,
    /// The quarter-turn basis change between left/right and mid/side,
    /// normalized so it is **its own inverse**: `MidSide`.
    MidSide,
    /// Scale the side axis, leaving the mid alone: `StereoWidth`.
    Width,
}

impl RotKind {
    /// Whether this row reads a parameter after its two signals.
    #[inline]
    fn has_param(self) -> bool {
        !matches!(self, RotKind::MidSide)
    }
}

/// `Rotate2`, `MidSide`, `StereoWidth` — one matrix, three parameterizations.
///
/// Like [`Pan`], one instance per output channel, the index last on the wire.
pub struct Rotate {
    kind: RotKind,
}

impl Rotate {
    pub fn new(kind: RotKind) -> Self {
        Self { kind }
    }

    /// The matrix row for output channel `chan`, at one parameter value.
    ///
    /// `MidSide` is `Rotate2` at a quarter turn with the second row negated —
    /// a reflection rather than a rotation, which is precisely what makes it an
    /// involution: `(a + b)/sqrt2` and `(a - b)/sqrt2` applied twice give back
    /// `a` and `b` exactly.
    #[inline]
    fn row(&self, chan: usize, param: f64) -> (f64, f64) {
        match self.kind {
            RotKind::Rotate2 => {
                let (sin, cos) = sin_cos_pi(param);
                if chan == 0 { (cos, sin) } else { (-sin, cos) }
            }
            RotKind::MidSide => {
                let k = std::f64::consts::FRAC_1_SQRT_2;
                if chan == 0 { (k, k) } else { (k, -k) }
            }
            // Encode, scale the side, decode — collapsed into the matrix it
            // amounts to. Width 1 is the identity, 0 is mono in both channels,
            // 2 is `1.5*this - 0.5*that`.
            RotKind::Width => {
                let (keep, cross) = ((1.0 + param) * 0.5, (1.0 - param) * 0.5);
                if chan == 0 {
                    (keep, cross)
                } else {
                    (cross, keep)
                }
            }
        }
    }
}

impl UGen for Rotate {
    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let (x, y) = (inputs[0], inputs[1]);
        // `MidSide` takes no parameter, so its channel index sits one slot
        // earlier than the other two rows'.
        let (param, chan_slot) = if self.kind.has_param() {
            (inputs[2], 3)
        } else {
            (&[0.0][..], 2)
        };
        let chan = usize::from(at(inputs[chan_slot], 0) >= 0.5);
        let fixed = (param.len() == 1).then(|| self.row(chan, param[0] as f64));

        for (i, s) in output.iter_mut().enumerate() {
            let (cx, cy) = match fixed {
                Some(r) => r,
                None => self.row(chan, at(param, i) as f64),
            };
            // `f64` by consistency with the law, not by need: see the module doc.
            *s = (at(x, i) as f64 * cx + at(y, i) as f64 * cy) as f32;
        }
    }
}

// --- the ring panner -----------------------------------------------------

/// `PanAz`: one source placed on a ring of `numchans` channels.
///
/// Each channel is one instance carrying its own index, and computes only its
/// own gain — a raised sine lobe `width` channels wide, centred on the source
/// and wrapped around the ring. At the default width of two, neighbouring
/// lobes are a sine and a cosine of the same angle, so any pair the source sits
/// between holds equal power, and a source parked on a channel gives that
/// channel exactly unity.
pub struct PanAz;

impl UGen for PanAz {
    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let (sig, pos, level) = (inputs[0], inputs[1], inputs[2]);
        let (width, orientation) = (inputs[3], inputs[4]);
        // A ring's size and this instance's place on it are fixed for the
        // node's life: read once, not per sample.
        let chans = (at(inputs[5], 0) as f64).max(1.0);
        let i_chan = (at(inputs[6], 0) as f64).max(0.0);

        let fixed = (pos.len() == 1 && width.len() == 1 && orientation.len() == 1).then(|| {
            gain_az(
                pos[0] as f64,
                width[0] as f64,
                orientation[0] as f64,
                chans,
                i_chan,
            )
        });

        for (i, s) in output.iter_mut().enumerate() {
            let g = match fixed {
                Some(g) => g,
                None => gain_az(
                    at(pos, i) as f64,
                    at(width, i) as f64,
                    at(orientation, i) as f64,
                    chans,
                    i_chan,
                ),
            };
            // `f64` by consistency with the law, not by need: see the module doc.
            *s = (at(sig, i) as f64 * g * at(level, i) as f64) as f32;
        }
    }
}

/// One channel's gain on the ring. `pos` spans the whole ring over `[-1, 1]`;
/// `orientation` shifts it in channels (scsynth's 0.5 puts the front between
/// two speakers, which is what an even ring wants).
///
/// A width at or below zero would divide by zero and is clamped to a lobe
/// narrower than any ring spacing — the audible result, silence except when the
/// source lands exactly on a channel, is what asking for a zero-width lobe
/// means.
#[inline]
fn gain_az(pos: f64, width: f64, orientation: f64, chans: f64, chan: f64) -> f64 {
    let width = width.max(1e-6);
    let rwidth = 1.0 / width;
    let range = chans * rwidth;
    // Half a lobe of lead centres the window on the source rather than
    // starting it there.
    let p = pos * 0.5 * chans + width * 0.5 + orientation;
    let mut chanpos = (p - chan) * rwidth;
    chanpos -= range * (chanpos / range).floor();
    if chanpos >= 1.0 {
        0.0
    } else {
        half_sin(chanpos)
    }
}

// --- selection -----------------------------------------------------------

/// Whether a selector jumps between its sources or crossfades across them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SelectKind {
    /// `Select`: the index truncates, and the output is that source verbatim.
    Pick,
    /// `SelectX`: the index's fraction crossfades to the next source with the
    /// equal-power law.
    Cross,
}

/// `Select`, `SelectX`: input 0 is the index, the rest are the sources.
///
/// Every source runs whether or not it is selected — they are UGens in the
/// graph, not branches — so this chooses what is heard, never what is computed.
pub struct Select {
    kind: SelectKind,
}

impl Select {
    pub fn new(kind: SelectKind) -> Self {
        Self { kind }
    }
}

impl UGen for Select {
    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let which = inputs[0];
        let sources = &inputs[1..];
        let last = sources.len().saturating_sub(1);
        if sources.is_empty() {
            output.fill(0.0);
            return;
        }

        for (i, s) in output.iter_mut().enumerate() {
            let w = at(which, i) as f64;
            *s = match self.kind {
                // Truncation and clamping, scsynth's rule: an index off either
                // end holds the source at that end rather than wrapping to the
                // other one.
                SelectKind::Pick => {
                    let idx = (w as i64).clamp(0, last as i64) as usize;
                    at(sources[idx], i)
                }
                SelectKind::Cross => {
                    let w = w.clamp(0.0, last as f64);
                    let lo = (w.floor() as usize).min(last.saturating_sub(1));
                    let frac = w - lo as f64;
                    let (ga, gb) = equal_power(frac * 2.0 - 1.0);
                    let a = at(sources[lo], i) as f64;
                    let b = at(sources[(lo + 1).min(last)], i) as f64;
                    (a * ga + b * gb) as f32
                }
            };
        }
    }
}
