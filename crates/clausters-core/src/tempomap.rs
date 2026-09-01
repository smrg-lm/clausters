//! The time map: beats (logical time) ↔ seconds (wall-clock time) under a
//! tempo that changes along the piece.
//!
//! A beat is **not** a unit of time. It is a logical coordinate that only
//! becomes time by passing through this map, and the scale that converts it
//! is itself a function of the coordinate. So the two things the domain calls
//! "tempo" are distinct, and this module keeps them apart:
//!
//! - the **tempo function** `T(b)` — beats per second at beat `b`; the
//!   derivative side, and what a user edits (a tempo track);
//! - the **time map** `M(b) = ∫₀ᵇ db'/T(b')` — the second beat `b` falls on;
//!   the integral, and what everything queries.
//!
//! Storing the integral rather than integrating on each query is Jaffe's 1985
//! proposal (*Ensemble Timing in Computer Music*); the closed forms below
//! follow the same analytical route as Dias, Pinto and Matos' BPMTimeline
//! (WAC 2016). The seconds are cached at every breakpoint, so a query is a
//! binary search plus one closed-form evaluation, never a sum over segments.
//!
//! The consequence that governs every caller: a length in beats is **not** a
//! duration. `Δbeats` has no length until it is told where it sits, so seconds
//! are always `secs_at(b1) - secs_at(b0)` and never a function of `b1 - b0`.
//! [`TempoMap::span_secs`] is the only correct spelling and exists so no
//! caller has to remember.
//!
//! # Relation to [`crate::tempoclock::TempoClock`]
//!
//! A [`TempoClock`](crate::tempoclock::TempoClock) is one affine segment: its
//! `base_seconds + (beats - base_beats) / tempo` is exactly this map's closed
//! form for a single [`Curve::Step`]. A map with one segment answers the
//! identical expression, term for term, which is what lets a clock adopt one
//! without changing a single result. A clock reading its own *now* always
//! lands in the last segment — [`TempoMap::last`] hands back that segment's
//! affine triple so the hot path stays three float operations with no search.

use std::fmt;

use serde::{Deserialize, Serialize};

/// The shape a segment's tempo takes on its way to the next breakpoint.
///
/// The numbers are the envelope shape numbers the clients already use for
/// `Env`, so one vocabulary spells a tempo curve and an amplitude curve — a
/// tempo envelope is written with the same words as any other.
///
/// **Every shape here has a closed integral and a closed inverse**, which is
/// not true of the whole `Env` family: `sin` and `wel` integrate in closed form
/// but invert transcendentally, and `beats_at` is what a running clock calls on
/// every read. The knob that survives is [`Shape::Curvature`], which is
/// continuous through linear at `c = 0` and covers "starts slow" and "starts
/// fast" — the family the excluded shapes belong to.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Shape {
    /// Tempo linear in beats: `T(u) = T₀ + (T₁ - T₀)·u`. The straight
    /// accelerando, and the shape a plain ramp writes.
    Linear,
    /// Tempo geometric in beats: `T(u) = T₀·(T₁/T₀)^u`. Equal *ratios* of
    /// tempo over equal stretches of beat — the musician's accelerando, where
    /// 60→120 and 120→240 feel like the same move.
    Exponential,
    /// `Env`'s curvature knob: `T(u) = A + B·e^{cu}`, linear at `c = 0`,
    /// starting slow for `c > 0` and fast for `c < 0`.
    Curvature(f64),
}

impl Shape {
    /// The envelope shape number, as the clients' `Env` numbers them
    /// (`lin` 1, `exp` 2, a numeric curvature 5).
    pub fn number(self) -> u32 {
        match self {
            Self::Linear => 1,
            Self::Exponential => 2,
            Self::Curvature(_) => 5,
        }
    }

    /// The curvature carried by [`Shape::Curvature`], and zero for the rest —
    /// the seventh number a segment crosses a binding as.
    pub fn curvature(self) -> f64 {
        match self {
            Self::Curvature(c) => c,
            _ => 0.0,
        }
    }

    /// A shape from the pair a binding carries, or `None` for a number no
    /// shape has. A curvature that rounds to zero **is** linear, which is what
    /// makes the knob continuous rather than a special case at its middle.
    pub fn from_parts(number: u32, curvature: f64) -> Option<Self> {
        match number {
            1 => Some(Self::Linear),
            2 => Some(Self::Exponential),
            5 => match curvature.is_finite() {
                true if curvature.abs() < crate::warp::CURVE_EPSILON => Some(Self::Linear),
                true => Some(Self::Curvature(curvature)),
                false => None,
            },
            _ => None,
        }
    }

    /// The tempo at normalised position `u` in `[0, 1]` between `t0` and `t1`.
    ///
    /// Every shape is written over `u` rather than over beats, which is what
    /// makes [`Shape::unit_secs`] independent of how wide the segment is — and
    /// that is what lets an extent be given in seconds (see
    /// [`Shape::beats_for_secs`]).
    fn tempo_at(self, t0: f64, t1: f64, u: f64) -> f64 {
        match self {
            Self::Linear => t0 + (t1 - t0) * u,
            // `exp(u·ln r)` rather than `powf(r, u)`: the two are the same
            // function for `r > 0` (which `T > 0` guarantees), but `powf`'s
            // last bit differs between a native libm and wasm's, and these
            // numbers are compared for **equality** across the bindings.
            Self::Exponential => t0 * (u * (t1 / t0).ln()).exp(),
            Self::Curvature(c) => match u {
                // The ends are given, not computed: `A + B·e^{cu}` reconstructs
                // them to within a rounding, and a gesture written at a
                // breakpoint departs from the tempo *stated* there.
                u if u <= 0.0 => t0,
                u if u >= 1.0 => t1,
                u => match curvature_terms(t0, t1, c) {
                    Some((a, b)) => a + b * (c * u).exp(),
                    // A curvature this flat *is* linear -- the same rule
                    // `Shape::from_parts` applies, held here too so a
                    // hand-built `Curvature(0.0)` bends rather than diverges.
                    None => t0 + (t1 - t0) * u,
                },
            },
        }
    }

    /// `∫₀^u du'/T(u')` — the seconds a segment one beat wide would take to
    /// reach `u`. The real segment's seconds are this times its width.
    fn secs_at(self, t0: f64, t1: f64, u: f64) -> f64 {
        match self {
            Self::Linear => match t1 == t0 {
                true => u / t0,
                false => (self.tempo_at(t0, t1, u) / t0).ln() / (t1 - t0),
            },
            Self::Exponential => match t1 == t0 {
                true => u / t0,
                false => {
                    // `exp(-u·ln r)`, for the reason `tempo_at` gives.
                    let ln_r = (t1 / t0).ln();
                    (1.0 - (-u * ln_r).exp()) / (t0 * ln_r)
                }
            },
            Self::Curvature(c) => match curvature_terms(t0, t1, c) {
                Some((a, b)) => (u - (((a + b * (c * u).exp()) / (a + b)).ln()) / c) / a,
                // Flat enough to be linear, and the integral has to agree with
                // `tempo_at`'s fallback or the two would answer about
                // different curves.
                None => Self::Linear.secs_at(t0, t1, u),
            },
        }
    }

    /// The inverse of [`Shape::secs_at`]: the `u` reached after `s` seconds of
    /// a segment one beat wide.
    ///
    /// Closed for the two ramps. [`Shape::Curvature`] mixes `u` and `e^{cu}`
    /// and has no closed inverse, so it is solved by a safeguarded Newton
    /// iteration — which is exact to the last bit in practice, deterministic,
    /// and lives here **once**, so every client inverts identically.
    fn u_at_secs(self, t0: f64, t1: f64, s: f64) -> f64 {
        match self {
            Self::Linear => match t1 == t0 {
                true => s * t0,
                false => {
                    let k = t1 - t0;
                    ((k * s).exp() - 1.0) * t0 / k
                }
            },
            Self::Exponential => match t1 == t0 {
                true => s * t0,
                false => {
                    let ln_r = (t1 / t0).ln();
                    -(1.0 - s * t0 * ln_r).ln() / ln_r
                }
            },
            Self::Curvature(_) => self.newton_u(t0, t1, s),
        }
    }

    /// Newton on `secs_at`, whose derivative is `1/T(u) > 0`, safeguarded by a
    /// bracket so a step that leaves `[lo, hi]` bisects instead. `secs_at` is
    /// strictly increasing (the map's `T > 0` invariant), so the bracket only
    /// ever narrows and the iteration cannot wander.
    fn newton_u(self, t0: f64, t1: f64, s: f64) -> f64 {
        let full = self.secs_at(t0, t1, 1.0);
        // NaN spelled out rather than folded into a negated comparison: a
        // second that is not a number lands at the segment's start, like one
        // that is not yet past it.
        if s.is_nan() || s <= 0.0 {
            return 0.0;
        }
        if s >= full {
            return 1.0;
        }
        let (mut lo, mut hi) = (0.0_f64, 1.0_f64);
        let mut u = s / full;
        for _ in 0..40 {
            let f = self.secs_at(t0, t1, u) - s;
            if f > 0.0 {
                hi = u;
            } else {
                lo = u;
            }
            let step = f * self.tempo_at(t0, t1, u); // f / (ds/du)
            let next = u - step;
            let next = match next > lo && next < hi {
                true => next,
                false => 0.5 * (lo + hi),
            };
            if (next - u).abs() <= f64::EPSILON * u.abs() {
                return next;
            }
            u = next;
        }
        u
    }

    /// **The seconds one beat of this shape takes**, `K = ∫₀¹du/T(u)`.
    ///
    /// The whole reason the shapes are written over `u`: `K` does not depend on
    /// how wide the segment is, so a stretch `Δb` beats wide lasts `Δb·K`
    /// seconds — and an extent given in *seconds* inverts by a single division.
    pub fn unit_secs(self, t0: f64, t1: f64) -> f64 {
        self.secs_at(t0, t1, 1.0)
    }

    /// **How many beats wide a stretch must be to last `secs` seconds**, going
    /// from `t0` to `t1` in this shape: `Δb = Δt/K`.
    ///
    /// This is what lets a tempo change be written with its extent in seconds
    /// rather than in beats, and it is exact for every shape rather than
    /// searched for. For a straight ramp `K` is the reciprocal of the
    /// logarithmic mean of the two tempos, so `Δb` is that mean times the
    /// seconds.
    pub fn beats_for_secs(self, t0: f64, t1: f64, secs: f64) -> f64 {
        secs / self.unit_secs(t0, t1)
    }
}

/// Below this a curvature is linear. `Env`'s own threshold, so the knob reads
/// the same in a tempo curve and in an amplitude curve.
/// `T(u) = A + B·e^{cu}` for a curvature `c`, as the two constants.
///
/// The algebra is `warp`'s and is written once there
/// ([`crate::warp::curve_terms_f64`]) — a bend is a bend whether it is a
/// filter sweep or an accelerando, and this used to be a second copy of it,
/// with a second copy of sclang's flatness threshold beside it.
///
/// `None` for a curvature flat enough to be the linear map, which is the case
/// the old copy divided by zero on: `Shape::from_parts` folds a small
/// curvature into [`Shape::Linear`], but nothing stopped a caller writing
/// `Shape::Curvature(0.0)` by hand and getting an infinity.
fn curvature_terms(t0: f64, t1: f64, c: f64) -> Option<(f64, f64)> {
    crate::warp::curve_terms_f64(t0, t1, c).map(|(d, a, _grow)| (a, -d))
}

/// How the tempo behaves across one segment.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Curve {
    /// Constant tempo: `T(b) = tempo`. What a plain tempo change produces, and
    /// the only shape a clock creates on its own.
    Step,
    /// The tempo moves in [`Shape`] from this segment's `tempo` to `end_tempo`,
    /// reached at `end_beats`. The real accelerando — its integral is a
    /// logarithm, not an average of the two tempos.
    ///
    /// The segment carries its own end rather than reading the next
    /// breakpoint, so it answers on its own: a map is built by appending, and a
    /// curve that had to look forward would be evaluated before the breakpoint
    /// it depends on exists. Past `end_beats` the tempo holds at `end_tempo`.
    Shaped {
        shape: Shape,
        end_beats: f64,
        end_tempo: f64,
    },
}

impl Curve {
    /// Whether this is the constant-tempo curve — the one a stored breakpoint
    /// leaves out.
    pub fn is_step(&self) -> bool {
        matches!(self, Self::Step)
    }
}

/// One breakpoint as a map is **written and read back** — the segment without
/// its `secs`.
///
/// `secs` is `M(beats)`, the integral evaluated there: derived, never authored.
/// Writing it out would be writing a cache, and reading one back would let a
/// file assert a second that its own tempi do not produce. So a stored map is
/// its breakpoints, and loading replays them through the same writers a live
/// gesture uses — which is what makes a loaded map one the client could have
/// built, and refuses at the door anything it could not.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Breakpoint {
    /// The beat this breakpoint sits at.
    pub beats: f64,
    /// Beats per second there.
    pub tempo: f64,
    /// How the tempo moves from here to the next one.
    #[serde(default = "step_curve", skip_serializing_if = "Curve::is_step")]
    pub curve: Curve,
}

/// The default a breakpoint with no curve written on it takes.
fn step_curve() -> Curve {
    Curve::Step
}

/// One segment of a [`TempoMap`], starting at `beats` (and at the second
/// `secs`, which is the integral evaluated at that breakpoint).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Segment {
    /// The beat this segment starts at.
    pub beats: f64,
    /// The second that beat falls on — `M(beats)`, cached so no query sums.
    pub secs: f64,
    /// Beats per second at `beats`.
    pub tempo: f64,
    /// How the tempo moves from here to the next breakpoint.
    pub curve: Curve,
}

impl Segment {
    /// The shape and the end this segment curves to, or `None` for a constant
    /// tempo — and also for a degenerate curve with no width or no tempo
    /// change, which is what makes the constant-tempo formula the correct
    /// fallback in every branch below.
    fn curving(&self) -> Option<(Shape, f64, f64, f64)> {
        match self.curve {
            Curve::Step => None,
            Curve::Shaped {
                shape,
                end_beats,
                end_tempo,
            } => match end_beats > self.beats && end_tempo != self.tempo {
                true => Some((shape, end_beats, end_tempo, end_beats - self.beats)),
                false => None,
            },
        }
    }

    /// The tempo slope `k` of a straight ramp, in beats per second per beat.
    /// Zero for everything else, which sends [`Self::secs_into`] and
    /// [`Self::beats_into`] down the affine branch — the one a clock's own
    /// segment always takes.
    fn slope(&self) -> f64 {
        match self.curving() {
            Some((Shape::Linear, _, end_tempo, width)) => (end_tempo - self.tempo) / width,
            _ => 0.0,
        }
    }

    /// Where this segment's curve ends, as `(end_beats, end_tempo, seconds into
    /// the segment at that beat)`. `None` for a constant tempo.
    fn ramp_end(&self) -> Option<(f64, f64, f64)> {
        let (shape, end_beats, end_tempo, width) = self.curving()?;
        let secs = match shape {
            // The straight ramp keeps its own spelling, which is the clock's:
            // `ln(T₁/T₀)/k` term for term, unchanged since before shapes.
            Shape::Linear => (end_tempo / self.tempo).ln() / self.slope(),
            _ => width * shape.unit_secs(self.tempo, end_tempo),
        };
        Some((end_beats, end_tempo, secs))
    }

    /// Seconds elapsed from this segment's start to beat `b` within it.
    ///
    /// `∫ db/T(b)`: `Δb/T` at a constant tempo, and the shape's closed form
    /// across a curve — so a long accelerando costs what a short one does. Past
    /// the curve's end the tempo holds, so the tail is affine again.
    fn secs_into(&self, b: f64) -> f64 {
        let Some((shape, end_beats, end_tempo, width)) = self.curving() else {
            return (b - self.beats) / self.tempo;
        };
        let (_, _, end_secs) = self.ramp_end().expect("a curve implies an end");
        if b > end_beats {
            return end_secs + (b - end_beats) / end_tempo;
        }
        match shape {
            Shape::Linear => {
                let k = self.slope();
                (1.0 + k * (b - self.beats) / self.tempo).ln() / k
            }
            _ => width * shape.secs_at(self.tempo, end_tempo, (b - self.beats) / width),
        }
    }

    /// The inverse of [`Self::secs_into`]: beats reached `ds` seconds after
    /// this segment's start.
    fn beats_into(&self, ds: f64) -> f64 {
        let Some((shape, end_beats, end_tempo, width)) = self.curving() else {
            return ds * self.tempo;
        };
        let (_, _, end_secs) = self.ramp_end().expect("a curve implies an end");
        if ds > end_secs {
            return (end_beats - self.beats) + (ds - end_secs) * end_tempo;
        }
        match shape {
            Shape::Linear => {
                let k = self.slope();
                self.tempo * ((k * ds).exp() - 1.0) / k
            }
            _ => width * shape.u_at_secs(self.tempo, end_tempo, ds / width),
        }
    }

    /// The tempo at beat `b` within this segment.
    fn tempo_into(&self, b: f64) -> f64 {
        let Some((shape, end_beats, end_tempo, width)) = self.curving() else {
            return self.tempo;
        };
        match b >= end_beats {
            true => end_tempo,
            false => shape.tempo_at(self.tempo, end_tempo, (b - self.beats) / width),
        }
    }
}

/// The unit an envelope's extents are measured in.
///
/// The two are not two spellings of one number: a stretch of beats and a
/// stretch of seconds are different stretches under any tempo but a constant
/// one, which is the whole reason this enum exists rather than a conversion at
/// the call site.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Extent {
    /// Extents are stretches of the beat axis.
    Beats,
    /// Extents are stretches of wall clock.
    Seconds,
}

/// What a map refuses to be built out of.
///
/// Every one of these breaks invertibility or ordering, which every query and
/// the binary search itself rely on — so they are rejected at the edit rather
/// than producing a map that answers nonsense.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TempoError {
    /// A tempo that is zero, negative or not finite. `T > 0` everywhere is
    /// what makes `M` strictly increasing, and therefore invertible.
    Tempo,
    /// A breakpoint beat that is not finite, or does not come after the last
    /// one. Segments are ordered and non-overlapping by construction.
    Beats,
    /// An envelope whose three lists do not agree: one tempo more than extents,
    /// one shape per extent, and at least one segment.
    Envelope,
}

impl fmt::Display for TempoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tempo => write!(f, "tempo must be finite and greater than zero"),
            Self::Beats => write!(f, "a breakpoint must be finite and after the previous one"),
            Self::Envelope => write!(
                f,
                "an envelope needs one more tempo than extents, one shape each, and a segment"
            ),
        }
    }
}

impl std::error::Error for TempoError {}

/// The piece's beat→second map: an ordered list of tempo segments with the
/// seconds cached at every breakpoint.
///
/// It is a **pure function**, not a running thing: it knows nothing of now,
/// answers the same for the same beat forever, and is meaningful for a piece
/// that has never been played. That is what lets an editor, an offline render
/// and a live clock share one.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(try_from = "Vec<Breakpoint>", into = "Vec<Breakpoint>")]
pub struct TempoMap {
    segments: Vec<Segment>,
    /// Bumped by every write. A holder that cached anything derived from this
    /// map compares it and re-reads when it moved.
    ///
    /// It is **not** stored: a version counts edits made in this process, and a
    /// loaded map has had none. Two maps that hold the same breakpoints are
    /// equal whatever their versions, which is why `PartialEq` is written out
    /// below rather than derived.
    version: u64,
}

impl PartialEq for TempoMap {
    fn eq(&self, other: &Self) -> bool {
        self.segments == other.segments
    }
}

impl From<TempoMap> for Vec<Breakpoint> {
    fn from(map: TempoMap) -> Self {
        map.breakpoints()
    }
}

impl TryFrom<Vec<Breakpoint>> for TempoMap {
    type Error = TempoError;

    fn try_from(points: Vec<Breakpoint>) -> Result<Self, TempoError> {
        Self::from_breakpoints(&points)
    }
}

impl TempoMap {
    /// A map of one constant-tempo segment with beat 0 at second 0 — the
    /// affine clock every piece starts as.
    ///
    /// A non-positive or non-finite tempo falls back to 1.0 rather than
    /// failing, so an infallible constructor stays infallible; [`Self::try_new`]
    /// is the checking spelling.
    pub fn new(tempo: f64) -> Self {
        Self::try_new(tempo).unwrap_or_else(|_| Self::try_new(1.0).expect("1.0 is a valid tempo"))
    }

    /// A map of one constant-tempo segment, refusing a tempo that would not
    /// be invertible.
    pub fn try_new(tempo: f64) -> Result<Self, TempoError> {
        check_tempo(tempo)?;
        Ok(Self {
            segments: vec![Segment {
                beats: 0.0,
                secs: 0.0,
                tempo,
                curve: Curve::Step,
            }],
            version: 1,
        })
    }

    /// A map anchored like a running clock: constant `tempo`, with `base_beats`
    /// falling on `base_seconds`.
    ///
    /// The bridge for adopting a map where an affine triple already lives: the
    /// map it builds answers `secs_at` with the identical expression that
    /// triple did, so nothing downstream moves.
    pub fn anchored(tempo: f64, base_beats: f64, base_seconds: f64) -> Result<Self, TempoError> {
        check_tempo(tempo)?;
        if !base_beats.is_finite() || !base_seconds.is_finite() {
            return Err(TempoError::Beats);
        }
        Ok(Self {
            segments: vec![Segment {
                beats: base_beats,
                secs: base_seconds,
                tempo,
                curve: Curve::Step,
            }],
            version: 1,
        })
    }

    /// The segments, in order.
    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    /// The breakpoints, in order — the map without its derived seconds, which
    /// is what a save writes and what [`Self::from_breakpoints`] reads back.
    pub fn breakpoints(&self) -> Vec<Breakpoint> {
        self.segments
            .iter()
            .map(|s| Breakpoint {
                beats: s.beats,
                tempo: s.tempo,
                curve: s.curve,
            })
            .collect()
    }

    /// A map rebuilt from breakpoints, **replayed through the ordinary
    /// writers**.
    ///
    /// The integral is recomputed rather than trusted, and every rule a live
    /// gesture obeys applies here: an unusable tempo, a breakpoint below the
    /// one before it, a curve that ends where it starts. So a stored map that
    /// loads is one this client could have written, and the door that reads a
    /// file is the door that checks it.
    ///
    /// An empty list is refused: a map always maps.
    pub fn from_breakpoints(points: &[Breakpoint]) -> Result<Self, TempoError> {
        let first = points.first().ok_or(TempoError::Beats)?;
        let mut map = Self::anchored(first.tempo, first.beats, 0.0)?;
        check_curve(first.beats, first.curve)?;
        map.segments[0].curve = first.curve;
        for point in &points[1..] {
            map.push_curve(point.beats, point.tempo, point.curve)?;
        }
        map.version = 1;
        Ok(map)
    }

    /// The edit count. A holder that cached anything derived from this map
    /// compares this and re-reads when it moved — which is the whole of what a
    /// shared map needs, since every reader re-evaluates from the map itself.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// How many segments the map holds (always at least one).
    pub fn len(&self) -> usize {
        self.segments.len()
    }

    /// Always false — a map holds at least one segment by construction. Present
    /// because [`Self::len`] exists.
    pub fn is_empty(&self) -> bool {
        false
    }

    /// The last segment: the one a running clock always reads, handed back so
    /// the caller can cache its affine triple and skip the search entirely.
    pub fn last(&self) -> Segment {
        *self
            .segments
            .last()
            .expect("a map holds at least one segment")
    }

    /// The tempo in effect at beat `b`.
    pub fn tempo_at(&self, b: f64) -> f64 {
        let i = self.index_at(b);
        let seg = self.segments[i];
        seg.tempo_into(b)
    }

    /// Appends a constant-tempo change at beat `b`: the tempo before `b` is
    /// untouched, `T(b)` becomes `tempo`, and **no discontinuity in seconds**
    /// is introduced — the breakpoint's second is what the map already said.
    ///
    /// This is what a clock's tempo change becomes: it used to overwrite the
    /// one anchor it had, and now it records it. A `b` that lands exactly on
    /// the last breakpoint restates it; one *before* it is rewriting history,
    /// which [`Self::truncate_from`] is for, and is refused.
    pub fn push(&mut self, b: f64, tempo: f64) -> Result<(), TempoError> {
        self.push_curve(b, tempo, Curve::Step)
    }

    /// Appends a segment with an explicit [`Curve`]. The curve governs from `b`
    /// until the next breakpoint (or forever, for the last segment — where a
    /// [`Curve::Shaped`] has no end and behaves as a step).
    pub fn push_curve(&mut self, b: f64, tempo: f64, curve: Curve) -> Result<(), TempoError> {
        check_tempo(tempo)?;
        check_curve(b, curve)?;
        let last = self.last();
        if !b.is_finite() || b < last.beats {
            return Err(TempoError::Beats);
        }
        if b == last.beats {
            // A breakpoint already sits here: restate it rather than refuse.
            // Its second is already the map's answer for `b` and must not move
            // — that is what keeps the change free of a discontinuity.
            let seg = self.segments.last_mut().expect("a map holds a segment");
            seg.tempo = tempo;
            seg.curve = curve;
            self.version += 1;
            return Ok(());
        }
        let secs = self.secs_at(b);
        self.segments.push(Segment {
            beats: b,
            secs,
            tempo,
            curve,
        });
        self.version += 1;
        Ok(())
    }

    /// Writes a straight tempo ramp over `[from_beats, to_beats]`, going from
    /// `from_tempo` to `to_tempo`, and keeps `to_tempo` after it.
    ///
    /// The composer's spelling of an accelerando or a ritardando: one call
    /// leaves two breakpoints, so the tempo after the ramp is stated rather
    /// than left to whatever the ramp's last value was.
    pub fn ramp(
        &mut self,
        from_beats: f64,
        to_beats: f64,
        from_tempo: f64,
        to_tempo: f64,
    ) -> Result<(), TempoError> {
        self.shaped(from_beats, to_beats, from_tempo, to_tempo, Shape::Linear)
    }

    /// [`TempoMap::ramp`] in an explicit [`Shape`].
    pub fn shaped(
        &mut self,
        from_beats: f64,
        to_beats: f64,
        from_tempo: f64,
        to_tempo: f64,
        shape: Shape,
    ) -> Result<(), TempoError> {
        if !to_beats.is_finite() || to_beats <= from_beats {
            return Err(TempoError::Beats);
        }
        self.push_curve(
            from_beats,
            from_tempo,
            Curve::Shaped {
                shape,
                end_beats: to_beats,
                end_tempo: to_tempo,
            },
        )?;
        self.push_curve(to_beats, to_tempo, Curve::Step)
    }

    /// **Writes a whole tempo envelope from beat `at`**: `tempos` (one more
    /// than the rest), one `extent` and one `shape` per segment.
    ///
    /// The envelope is of **finite duration** — it has as many segments as it
    /// has extents, and after the last one the tempo it reached simply holds.
    /// There is no sustain and no loop: those make sense for a gate, and a
    /// piece's tempo has no gate to hold.
    ///
    /// `unit` says what the extents measure. In [`Extent::Beats`] each one is a
    /// stretch of the beat axis; in [`Extent::Seconds`] it is a stretch of wall
    /// clock, and each segment's width in beats is solved exactly by
    /// [`Shape::beats_for_secs`] — no iteration, no approximation, and no
    /// per-shape special case.
    ///
    /// One call rather than a chain of them, and that is not only convenience:
    /// the map is appended to, so a chain has to write every segment in order
    /// anyway, and a shape written as one call cannot get that order wrong.
    pub fn write_env(
        &mut self,
        at: f64,
        tempos: &[f64],
        extents: &[f64],
        shapes: &[Shape],
        unit: Extent,
    ) -> Result<(), TempoError> {
        if tempos.len() != extents.len() + 1 || shapes.len() != extents.len() || extents.is_empty()
        {
            return Err(TempoError::Envelope);
        }
        for t in tempos {
            check_tempo(*t)?;
        }
        // Solve the beat width of every segment *before* writing any of them,
        // so an envelope that is refused leaves the map untouched rather than
        // half-written.
        let mut edges = Vec::with_capacity(extents.len() + 1);
        edges.push(at);
        for (i, extent) in extents.iter().enumerate() {
            if !extent.is_finite() || *extent <= 0.0 {
                return Err(TempoError::Beats);
            }
            let width = match unit {
                Extent::Beats => *extent,
                Extent::Seconds => shapes[i].beats_for_secs(tempos[i], tempos[i + 1], *extent),
            };
            if !width.is_finite() || width <= 0.0 {
                return Err(TempoError::Beats);
            }
            edges.push(edges[i] + width);
        }
        for i in 0..extents.len() {
            self.push_curve(
                edges[i],
                tempos[i],
                Curve::Shaped {
                    shape: shapes[i],
                    end_beats: edges[i + 1],
                    end_tempo: tempos[i + 1],
                },
            )?;
        }
        self.push_curve(edges[extents.len()], tempos[extents.len()], Curve::Step)
    }

    /// Drops every breakpoint at or after beat `b`, so the segment covering `b`
    /// governs from there on. The first segment is never dropped — a map always
    /// maps.
    ///
    /// What an edit to a tempo track needs before rewriting a stretch of it.
    pub fn truncate_from(&mut self, b: f64) {
        let keep = self
            .segments
            .iter()
            .take_while(|s| s.beats < b)
            .count()
            .max(1);
        self.segments.truncate(keep);
        self.version += 1;
    }

    /// The index of the segment governing beat `b` (the first one for a beat
    /// before the map starts, which extrapolates backwards on its slope).
    fn index_at(&self, b: f64) -> usize {
        // Saturating at 0: a beat before the first breakpoint is governed by
        // the first segment, extrapolated backwards on its own tempo.
        self.segments
            .partition_point(|s| s.beats <= b)
            .saturating_sub(1)
    }

    /// The index of the segment governing second `s`.
    fn index_at_secs(&self, s: f64) -> usize {
        self.segments
            .partition_point(|g| g.secs <= s)
            .saturating_sub(1)
    }

    /// **The time map**: the second beat `b` falls on.
    ///
    /// Defined for every finite beat, before and after the piece: a beat
    /// earlier than the first breakpoint extrapolates on the first segment's
    /// tempo, which is what makes a map built mid-performance still answer
    /// about the music that already happened.
    pub fn secs_at(&self, b: f64) -> f64 {
        let i = self.index_at(b);
        let seg = self.segments[i];
        seg.secs + seg.secs_into(b)
    }

    /// The inverse: the beat falling on second `s`.
    pub fn beats_at(&self, s: f64) -> f64 {
        let i = self.index_at_secs(s);
        let seg = self.segments[i];
        seg.beats + seg.beats_into(s - seg.secs)
    }

    /// **How long the stretch from `b0` to `b1` lasts, in seconds.**
    ///
    /// The only correct way to turn a length in beats into a length in time:
    /// it takes both ends, because under a changing tempo the same `b1 - b0`
    /// lasts differently depending on where it sits. Anything shaped like
    /// `beats / tempo` is the bug this exists to prevent.
    pub fn span_secs(&self, b0: f64, b1: f64) -> f64 {
        self.secs_at(b1) - self.secs_at(b0)
    }

    /// How many beats fit in `secs` seconds starting at beat `b0` — the same
    /// question from the other side, and equally position-dependent.
    pub fn span_beats(&self, b0: f64, secs: f64) -> f64 {
        self.beats_at(self.secs_at(b0) + secs) - b0
    }
}

/// `T > 0` and finite: the condition that makes the map invertible.
/// The rules a [`Curve`] obeys wherever it is written: a usable end tempo, an
/// end strictly after its start, a finite curvature. One function, so a live
/// gesture and a loaded file are checked by the same code.
fn check_curve(b: f64, curve: Curve) -> Result<(), TempoError> {
    if let Curve::Shaped {
        shape,
        end_beats,
        end_tempo,
    } = curve
    {
        check_tempo(end_tempo)?;
        if !end_beats.is_finite() || end_beats <= b || !shape.curvature().is_finite() {
            return Err(TempoError::Beats);
        }
    }
    Ok(())
}

fn check_tempo(tempo: f64) -> Result<(), TempoError> {
    match tempo.is_finite() && tempo > 0.0 {
        true => Ok(()),
        false => Err(TempoError::Tempo),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-9, "{a} != {b}");
    }

    #[test]
    fn one_segment_is_the_affine_clock_bit_for_bit() {
        // The property the whole adoption rests on: a one-segment map computes
        // the affine expression itself, not merely a close number. The oracle
        // is the formula, written out here -- there is no second implementation
        // of it to compare against, which is the point.
        for (tempo, base_b, base_s) in [(1.0, 0.0, 0.0), (2.0, 3.0, 1.5), (0.75, -2.0, 4.0)] {
            let map = TempoMap::anchored(tempo, base_b, base_s).unwrap();
            for b in [-1.0, 0.0, 0.5, 7.25, 100.0] {
                assert_eq!(map.secs_at(b), base_s + (b - base_b) / tempo);
                assert_eq!(map.beats_at(base_s), base_b);
            }
        }
    }

    #[test]
    fn a_curvature_flat_enough_to_be_linear_is_linear() {
        // `Shape::from_parts` folds a small curvature into `Linear`, but
        // nothing stops a caller building `Curvature(0.0)` in Rust, and the
        // coefficients divide by `1 - e^c`. It used to give an infinity; the
        // shared `warp` terms answer `None` there, and both the tempo and its
        // integral fall back to the linear shape -- which they must do
        // together, or the two would describe different curves.
        for c in [0.0, 0.0005, -0.0009] {
            let bent = Shape::Curvature(c);
            for u in [0.0, 0.25, 0.5, 0.75, 1.0] {
                assert_eq!(
                    bent.tempo_at(1.0, 2.0, u),
                    Shape::Linear.tempo_at(1.0, 2.0, u)
                );
                assert_eq!(
                    bent.secs_at(1.0, 2.0, u),
                    Shape::Linear.secs_at(1.0, 2.0, u)
                );
            }
        }
    }

    #[test]
    fn a_map_round_trips_through_its_breakpoints() {
        // Every shape, written and read back: the breakpoints come back
        // identical and so does every second the map answers, because the
        // integral is recomputed from the same writers rather than stored.
        let mut map = TempoMap::new(1.0);
        map.shaped(2.0, 6.0, 1.0, 2.0, Shape::Linear).unwrap();
        map.shaped(8.0, 12.0, 2.0, 0.5, Shape::Exponential).unwrap();
        map.shaped(14.0, 20.0, 0.5, 3.0, Shape::Curvature(-2.5))
            .unwrap();
        let json = serde_json::to_string(&map).unwrap();
        let back: TempoMap = serde_json::from_str(&json).unwrap();
        assert_eq!(back.breakpoints(), map.breakpoints());
        assert_eq!(back, map);
        for b in [-1.0, 0.0, 2.0, 4.5, 9.0, 15.5, 30.0] {
            assert_eq!(back.secs_at(b), map.secs_at(b));
            assert_eq!(back.beats_at(map.secs_at(b)), map.beats_at(map.secs_at(b)));
        }
    }

    #[test]
    fn a_stored_map_is_checked_by_the_door_that_reads_it() {
        // Breakpoints out of order, an unusable tempo, a curve that ends where
        // it starts, and an empty map: each is refused on load by the same
        // rule that refuses it on a live write.
        let out_of_order = r#"[{"beats":0.0,"tempo":1.0},{"beats":-2.0,"tempo":2.0}]"#;
        let bad_tempo = r#"[{"beats":0.0,"tempo":0.0}]"#;
        let bad_curve = r#"[{"beats":0.0,"tempo":1.0,"curve":{"shaped":
            {"shape":"linear","end_beats":0.0,"end_tempo":2.0}}}]"#;
        for json in [out_of_order, bad_tempo, bad_curve, "[]"] {
            assert!(
                serde_json::from_str::<TempoMap>(json).is_err(),
                "accepted {json}"
            );
        }
    }

    #[test]
    fn a_stored_map_holds_no_seconds_and_no_version() {
        // The seconds are the cache and the version counts this process's
        // edits; neither is authored, so neither is written.
        let mut map = TempoMap::new(2.0);
        map.push(4.0, 3.0).unwrap();
        let json = serde_json::to_string(&map).unwrap();
        assert!(!json.contains("secs"), "{json}");
        assert!(!json.contains("version"), "{json}");
        // A constant-tempo breakpoint leaves its curve out entirely.
        assert!(!json.contains("curve"), "{json}");
        assert_eq!(
            serde_json::from_str::<TempoMap>(&json).unwrap().version(),
            1
        );
    }

    #[test]
    fn every_write_moves_the_version_and_a_read_does_not() {
        // What a shared map gives a holder: something to compare. Every reader
        // re-evaluates from the map itself, so this is the whole of the
        // machinery a second clock needs.
        let mut map = TempoMap::new(1.0);
        let start = map.version();
        assert_eq!(map.secs_at(4.0), 4.0);
        assert_eq!(map.version(), start);
        map.push(2.0, 2.0).unwrap();
        assert!(map.version() > start);
        let after = map.version();
        assert!(map.push(1.0, 2.0).is_err());
        assert_eq!(map.version(), after, "a refused write moves nothing");
        map.truncate_from(1.0);
        assert!(map.version() > after);
    }

    #[test]
    fn a_step_change_keeps_the_instant_and_records_the_past() {
        // Tempo 1.0 from beat 0, doubled at beat 2 (second 2.0).
        let mut map = TempoMap::new(1.0);
        map.push(2.0, 2.0).unwrap();
        close(map.secs_at(2.0), 2.0); // no discontinuity at the breakpoint
        close(map.secs_at(8.0), 5.0); // 2.0 + 6/2
        close(map.secs_at(1.0), 1.0); // the past stays true -- the whole point
        close(map.beats_at(5.0), 8.0);
        close(map.beats_at(1.0), 1.0);
    }

    #[test]
    fn the_same_beat_span_lasts_differently_where_it_sits() {
        let mut map = TempoMap::new(1.0);
        map.push(4.0, 2.0).unwrap();
        close(map.span_secs(0.0, 2.0), 2.0); // two beats before the change
        close(map.span_secs(4.0, 6.0), 1.0); // the same two beats after it
        close(map.span_beats(4.0, 1.0), 2.0);
    }

    #[test]
    fn a_linear_ramp_integrates_to_a_logarithm() {
        // Tempo 1 -> 2 beats/s over beats [0, 4]: ln(2)/k with k = 0.25.
        let mut map = TempoMap::new(1.0);
        map.ramp(0.0, 4.0, 1.0, 2.0).unwrap();
        close(map.secs_at(4.0), (2.0f64).ln() / 0.25);
        close(map.tempo_at(2.0), 1.5);
        close(map.tempo_at(4.0), 2.0);
        // Not the average of the two tempos (which would be 4/1.5 = 2.667 s).
        assert!((map.secs_at(4.0) - 4.0 / 1.5).abs() > 0.05);
        // And after the ramp the tempo it ended on governs.
        close(map.span_secs(4.0, 6.0), 1.0);
    }

    #[test]
    fn the_inverse_round_trips_through_a_ramp() {
        let mut map = TempoMap::new(1.5);
        map.ramp(2.0, 10.0, 1.5, 0.5).unwrap();
        for b in [0.0, 1.0, 2.0, 5.5, 10.0, 20.0] {
            close(map.beats_at(map.secs_at(b)), b);
        }
    }

    #[test]
    fn the_map_is_strictly_increasing_so_it_inverts() {
        let mut map = TempoMap::new(1.0);
        map.ramp(1.0, 3.0, 1.0, 4.0).unwrap();
        map.push(9.0, 0.25).unwrap();
        let mut prev = f64::NEG_INFINITY;
        for i in 0..400 {
            let s = map.secs_at(i as f64 * 0.1);
            assert!(s > prev, "not increasing at {i}");
            prev = s;
        }
    }

    #[test]
    fn a_bad_tempo_or_a_backwards_breakpoint_is_refused() {
        let mut map = TempoMap::new(1.0);
        assert_eq!(map.push(1.0, 0.0), Err(TempoError::Tempo));
        assert_eq!(map.push(1.0, f64::NAN), Err(TempoError::Tempo));
        map.push(4.0, 2.0).unwrap();
        assert_eq!(map.push(2.0, 1.0), Err(TempoError::Beats));
        // Refusals leave the map as it was.
        assert_eq!(map.len(), 2);
        // Restating the last breakpoint is not a refusal: it replaces it, and
        // the second it falls on does not move.
        map.push(4.0, 8.0).unwrap();
        assert_eq!(map.len(), 2);
        close(map.secs_at(4.0), 4.0);
        close(map.tempo_at(4.0), 8.0);
    }

    #[test]
    fn truncate_drops_the_rewritten_tail_and_never_the_first() {
        let mut map = TempoMap::new(1.0);
        map.push(4.0, 2.0).unwrap();
        map.push(8.0, 4.0).unwrap();
        map.truncate_from(5.0);
        assert_eq!(map.len(), 2);
        map.truncate_from(0.0);
        assert_eq!(map.len(), 1);
        close(map.secs_at(4.0), 4.0);
    }

    #[test]
    fn every_shape_integrates_to_its_closed_form() {
        // Checked against a 2e6-point midpoint integration of `db/T(b)` when
        // this was designed; the constants are those results.
        let cases = [
            (Shape::Exponential, 4.328_085_122_667),
            (Shape::Curvature(-4.0), 2.655_981_971_0),
        ];
        for (shape, want) in cases {
            let mut m = TempoMap::new(1.0);
            m.shaped(0.0, 8.0, 1.0, 4.0, shape).unwrap();
            close(m.secs_at(8.0), want);
            // and the tempo it holds after the curve is the one it reached
            close(m.tempo_at(20.0), 4.0);
        }
    }

    #[test]
    fn a_curvature_of_zero_is_the_straight_ramp() {
        // The knob is continuous through its middle rather than a shape apart,
        // which is what lets a client hand a curvature straight through.
        assert_eq!(Shape::from_parts(5, 0.0), Some(Shape::Linear));
        assert_eq!(Shape::from_parts(5, 0.000_1), Some(Shape::Linear));
        assert_eq!(Shape::from_parts(5, -4.0), Some(Shape::Curvature(-4.0)));
        assert_eq!(Shape::from_parts(3, 0.0), None); // `sin` is not a tempo shape
    }

    #[test]
    fn the_inverse_round_trips_through_every_shape() {
        // `beats_at` is what a running clock calls on every read, so each shape
        // has to invert -- the curvature by iteration, and to the same place.
        for shape in [Shape::Linear, Shape::Exponential, Shape::Curvature(3.0)] {
            let mut m = TempoMap::new(1.0);
            m.shaped(2.0, 10.0, 1.0, 3.0, shape).unwrap();
            for b in [0.5, 2.0, 3.7, 6.0, 9.99, 10.0, 14.0] {
                close(m.beats_at(m.secs_at(b)), b);
            }
        }
    }

    #[test]
    fn an_extent_in_seconds_lands_on_the_second_it_asked_for() {
        // Delta-b = Delta-t / K, exact for every shape and not searched for.
        for shape in [Shape::Linear, Shape::Exponential, Shape::Curvature(-2.0)] {
            let mut m = TempoMap::new(1.0);
            m.write_env(0.0, &[1.0, 4.0], &[3.0], &[shape], Extent::Seconds)
                .unwrap();
            let end = m.segments()[1].beats;
            close(m.secs_at(end), 3.0);
            close(m.tempo_at(end), 4.0);
        }
    }

    #[test]
    fn an_envelope_is_finite_and_holds_what_it_reached() {
        let mut m = TempoMap::new(1.0);
        m.write_env(
            0.0,
            &[1.0, 2.0, 2.0, 0.5],
            &[4.0, 8.0, 2.0],
            &[Shape::Linear, Shape::Linear, Shape::Exponential],
            Extent::Beats,
        )
        .unwrap();
        assert_eq!(m.len(), 4); // three segments plus the step that holds
        close(m.segments()[3].beats, 14.0);
        close(m.tempo_at(14.0), 0.5);
        close(m.tempo_at(1_000.0), 0.5); // finite duration: it holds, it does not loop
        // The flat middle is a ramp between equal tempos, which is a constant.
        close(m.span_secs(4.0, 12.0), 4.0);
    }

    #[test]
    fn a_malformed_envelope_leaves_the_map_untouched() {
        let mut m = TempoMap::new(1.0);
        m.push(4.0, 2.0).unwrap();
        let before = m.segments().to_vec();
        assert_eq!(
            m.write_env(8.0, &[1.0, 2.0], &[], &[], Extent::Beats),
            Err(TempoError::Envelope)
        );
        assert_eq!(
            m.write_env(
                8.0,
                &[1.0, 2.0, 3.0],
                &[1.0],
                &[Shape::Linear],
                Extent::Beats
            ),
            Err(TempoError::Envelope)
        );
        assert_eq!(
            m.write_env(8.0, &[1.0, -2.0], &[1.0], &[Shape::Linear], Extent::Beats),
            Err(TempoError::Tempo)
        );
        assert_eq!(
            m.segments(),
            &before[..],
            "a refused envelope writes nothing"
        );
    }

    #[test]
    fn the_last_segment_is_the_clocks_affine_triple() {
        let mut map = TempoMap::new(1.0);
        map.push(2.0, 2.0).unwrap();
        let last = map.last();
        assert_eq!((last.beats, last.secs, last.tempo), (2.0, 2.0, 2.0));
        // Which is what makes the hot path search-free: reading "now" through
        // the cached triple gives what the map gives.
        for b in [2.0, 3.0, 40.0] {
            assert_eq!(map.secs_at(b), last.secs + (b - last.beats) / last.tempo);
        }
    }
}
