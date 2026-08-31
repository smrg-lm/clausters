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

/// How the tempo behaves across one segment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Curve {
    /// Constant tempo: `T(b) = tempo`. What a plain tempo change produces, and
    /// the only shape a clock creates on its own.
    Step,
    /// Tempo linear **in beats** from this segment's `tempo` to `end_tempo`,
    /// reached at `end_beats`: `T(b) = T₀ + k·(b - b₀)`. The real accelerando
    /// — its integral is a logarithm, not an average of the two tempos.
    ///
    /// The ramp carries its own end rather than reading the next breakpoint,
    /// so a segment answers on its own: a map is built by appending, and a
    /// ramp that had to look forward would be evaluated before the breakpoint
    /// it depends on exists. Past `end_beats` the tempo holds at `end_tempo`.
    Linear { end_beats: f64, end_tempo: f64 },
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
    /// The tempo slope `k`, in beats per second per beat. Zero for a step and
    /// for a degenerate ramp (no width), which is what makes the constant-tempo
    /// formula the correct fallback in every branch below.
    fn slope(&self) -> f64 {
        match self.curve {
            Curve::Step => 0.0,
            Curve::Linear {
                end_beats,
                end_tempo,
            } => match end_beats > self.beats {
                true => (end_tempo - self.tempo) / (end_beats - self.beats),
                false => 0.0,
            },
        }
    }

    /// Where this segment's ramp ends, as `(end_beats, end_tempo, seconds into
    /// the segment at that beat)`. `None` for a constant tempo.
    fn ramp_end(&self) -> Option<(f64, f64, f64)> {
        match self.curve {
            Curve::Step => None,
            Curve::Linear {
                end_beats,
                end_tempo,
            } => {
                let k = self.slope();
                if k == 0.0 {
                    return None;
                }
                Some((end_beats, end_tempo, (end_tempo / self.tempo).ln() / k))
            }
        }
    }

    /// Seconds elapsed from this segment's start to beat `b` within it.
    ///
    /// `∫ db/T(b)`: `Δb/T` at a constant tempo, and `ln(T(b)/T₀)/k` across a
    /// ramp — the closed form, so a long accelerando costs what a short one
    /// does. Past the ramp's end the tempo holds, so the tail is affine again.
    fn secs_into(&self, b: f64) -> f64 {
        let k = self.slope();
        if k == 0.0 {
            return (b - self.beats) / self.tempo;
        }
        let (end_beats, end_tempo, end_secs) = self.ramp_end().expect("a slope implies a ramp");
        if b <= end_beats {
            return (1.0 + k * (b - self.beats) / self.tempo).ln() / k;
        }
        end_secs + (b - end_beats) / end_tempo
    }

    /// The inverse of [`Self::secs_into`]: beats reached `ds` seconds after
    /// this segment's start.
    fn beats_into(&self, ds: f64) -> f64 {
        let k = self.slope();
        if k == 0.0 {
            return ds * self.tempo;
        }
        let (end_beats, end_tempo, end_secs) = self.ramp_end().expect("a slope implies a ramp");
        if ds <= end_secs {
            return self.tempo * ((k * ds).exp() - 1.0) / k;
        }
        (end_beats - self.beats) + (ds - end_secs) * end_tempo
    }

    /// The tempo at beat `b` within this segment.
    fn tempo_into(&self, b: f64) -> f64 {
        let k = self.slope();
        if k == 0.0 {
            return self.tempo;
        }
        let (end_beats, end_tempo, _) = self.ramp_end().expect("a slope implies a ramp");
        match b >= end_beats {
            true => end_tempo,
            false => self.tempo + k * (b - self.beats),
        }
    }
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
}

impl fmt::Display for TempoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tempo => write!(f, "tempo must be finite and greater than zero"),
            Self::Beats => write!(f, "a breakpoint must be finite and after the previous one"),
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
#[derive(Clone, Debug, PartialEq)]
pub struct TempoMap {
    segments: Vec<Segment>,
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
        })
    }

    /// The segments, in order.
    pub fn segments(&self) -> &[Segment] {
        &self.segments
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
    /// [`Curve::Linear`] has no end and behaves as a step).
    pub fn push_curve(&mut self, b: f64, tempo: f64, curve: Curve) -> Result<(), TempoError> {
        check_tempo(tempo)?;
        if let Curve::Linear {
            end_beats,
            end_tempo,
        } = curve
        {
            check_tempo(end_tempo)?;
            if !end_beats.is_finite() || end_beats <= b {
                return Err(TempoError::Beats);
            }
        }
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
            return Ok(());
        }
        let secs = self.secs_at(b);
        self.segments.push(Segment {
            beats: b,
            secs,
            tempo,
            curve,
        });
        Ok(())
    }

    /// Writes a tempo ramp over `[from_beats, to_beats]`, going from
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
        if !to_beats.is_finite() || to_beats <= from_beats {
            return Err(TempoError::Beats);
        }
        self.push_curve(
            from_beats,
            from_tempo,
            Curve::Linear {
                end_beats: to_beats,
                end_tempo: to_tempo,
            },
        )?;
        self.push_curve(to_beats, to_tempo, Curve::Step)
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
fn check_tempo(tempo: f64) -> Result<(), TempoError> {
    match tempo.is_finite() && tempo > 0.0 {
        true => Ok(()),
        false => Err(TempoError::Tempo),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tempoclock::TempoClock;

    fn close(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-9, "{a} != {b}");
    }

    #[test]
    fn one_segment_is_the_affine_clock_bit_for_bit() {
        // The property the whole adoption rests on: a one-segment map computes
        // the same expression a TempoClock does, not merely a close number.
        for (tempo, base_b, base_s) in [(1.0, 0.0, 0.0), (2.0, 3.0, 1.5), (0.75, -2.0, 4.0)] {
            let map = TempoMap::anchored(tempo, base_b, base_s).unwrap();
            // The clock's own anchoring: set_tempo pins the instant it is
            // called at, which is the same (beats, secs) pair the map holds.
            let mut clk = TempoClock::new(tempo);
            clk.set_tempo(tempo, base_s);
            for b in [-1.0, 0.0, 0.5, 7.25, 100.0] {
                assert_eq!(map.secs_at(b), base_s + (b - base_b) / tempo);
                // And the clock's expression, offset by its own base.
                assert_eq!(
                    map.secs_at(b) - base_s,
                    clk.beats_to_secs(clk.secs_to_beats(base_s) + (b - base_b)) - base_s
                );
            }
        }
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
