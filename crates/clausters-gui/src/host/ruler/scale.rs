//! **What a ruler shows**: the scales, each a ladder of ticks over the
//! visible window of its axis -- pure, display-only, and the same on either
//! axis.
//!
//! A time axis, an amplitude axis, a frequency axis, a plain value axis and a
//! meter's decibels need tick positions and labels that stay legible at any
//! zoom. The math is classic editor chrome: each unit owns a ladder of
//! candidate steps -- a 1-2-5 decimal progression (time in seconds or
//! samples; amplitude in normalized/percent/integer sample units; zoomed-in
//! frequency spans), a binary/bar ladder on the musical `beats` axis (labels
//! `bar:beat` off the client's quant grid, via the shared
//! `clausters_core::tempoclock::bar`/`beat_in_bar`), a fixed mirrored dB rung
//! list on the dBFS amplitude axis, a regular step in decibels on a meter's,
//! and decade ticks on the wide frequency axis -- and the layout picks the
//! smallest step whose **measured labels fit**: each candidate is tried
//! against its *own* formatted labels (`font::width`/`height` at the ruler's
//! font scale, in device pixels, so HiDPI is exact), never against a mean
//! width. Every generator lays out over the **visible sub-range** of its axis,
//! so a zoom on either axis reveals finer rungs. The frequency ticks are
//! placed with the **identical** display->bin geometry the spectrogram shader
//! uses (linear, log, mel or bark; the perceptual forms from
//! `clausters_core::scale`), so a tick labeled 1 kHz sits exactly on the
//! 1 kHz row of pixels.
//!
//! **A scale does not know which way it is drawn.** What the axis changes is
//! only how a label is measured against its neighbours -- by its width where
//! labels stand side by side, by a line of text where they stand one over the
//! other ([`Along`]) -- so every scale answers for either axis through
//! [`Scale::ticks`], and where the strip stands and which way it faces is the
//! drawing's ([`super::draw`]). No GPU, no widget types: positions come out
//! as fractions of the visible span.

use clausters_core::scale::{
    bark_to_hz, hz_to_bark, hz_to_mel, hz_to_midi_tuned, mel_to_hz, midi_to_hz_tuned, note_name,
};
use clausters_core::tempoclock;
use clausters_core::tempomap::TempoMap;

use crate::spectrogram::FreqScale;
use crate::waveform::AMP_MARGIN;

use super::Along;
use crate::host::widget::RulerY;
use clausters_gui_draw::font;
use clausters_gui_draw::metrics::Metrics;

/// One ruler tick: its position as a fraction of the visible axis span
/// (0 = start/bottom, 1 = end/top) and its label (`None` for a minor tick).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Tick {
    pub frac: f64,
    pub label: Option<String>,
}

/// How the time ruler labels its ticks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum TimeUnit<'a> {
    /// `h:mm:ss.mmm`-style clock time (needs the sample rate).
    Seconds,
    /// Plain sample counts (the fallback when no rate is known).
    Samples,
    /// Musical time on the client's beat grid: `tempo` in beats per second
    /// (the client `Clock` convention), `beat_at` the beat position of sample
    /// 0, `quant` the beats per bar (`<= 0` = no bar grid). Falls back to
    /// sample counts when the rate or tempo is unknown.
    ///
    /// **A beat is not a length of time**, and `map` is what says where each
    /// one falls. With no map the axis is one tempo, so beats are evenly
    /// spaced and `tempo`/`beat_at` are the whole answer. With one, they are
    /// not: the multitrack's [`TempoMap`] places every tick through `secs_at`, so
    /// the marks crowd where it is fast and spread where it is slow, over an
    /// axis that never stopped measuring samples. The map also supplies its
    /// own anchoring, so `beat_at` says nothing where it is present.
    ///
    /// **`bars` are the page's**, where the axis has them: runs of bars of
    /// one length (`beat length number`, [`bars_in`]), so a meter that
    /// changes, an anacrusis and a bar of another length are counted as a
    /// page counts them. Empty, `quant` is the length of every bar.
    Beats {
        tempo: f64,
        beat_at: f64,
        quant: f64,
        map: Option<&'a TempoMap>,
        bars: &'a [[f64; 3]],
    },
}

/// **The bars a run list puts on `[b0, b1]`**, in beats: `(beat, length,
/// number)` of each bar that starts there, and of the one `b0` is inside.
/// A run is `[beat, length, number]`: bars of `length` beats from `beat`,
/// the first numbered `number`, until the next run begins; the last goes on.
pub(crate) fn bars_in(runs: &[[f64; 3]], b0: f64, b1: f64) -> Vec<(f64, f64, i64)> {
    let mut out = Vec::new();
    for (k, &[from, len, number]) in runs.iter().enumerate() {
        if len <= 0.0 {
            continue;
        }
        let until = runs.get(k + 1).map_or(f64::INFINITY, |next| next[0]);
        if until <= b0 || from > b1 {
            continue;
        }
        let mut i = ((b0 - from) / len).floor().max(0.0) as i64;
        loop {
            let at = from + i as f64 * len;
            if at >= until - 1e-9 || at > b1 || out.len() > 20_000 {
                break;
            }
            out.push((at, len, number as i64 + i));
            i += 1;
        }
    }
    out
}

/// The ruler's own reading of the host's size roles: the `caption_scale` its
/// labels render at -- the layout measures every candidate label at that scale,
/// so what fits in the math is exactly what fits on screen -- plus the
/// `label_gap` between two drawn labels and the `tick_gap` between any two
/// drawn ticks (labels drop before ticks do).
#[derive(Debug, Clone, Copy)]
pub(super) struct Gaps {
    pub(super) scale: f32,
    pub(super) label: f64,
    pub(super) tick: f64,
    pub(super) gap: f64,
    /// The axis the labels are laid along, which decides how one is measured
    /// against the next ([`Gaps::extent`]).
    along: Along,
}

impl Gaps {
    pub(super) fn of(metrics: &Metrics) -> Self {
        Self::along(metrics, Along::Horizontal)
    }

    pub(super) fn along(metrics: &Metrics, along: Along) -> Self {
        Self {
            scale: metrics.caption_scale,
            label: metrics.label_gap as f64,
            tick: metrics.tick_gap as f64,
            gap: metrics.gap as f64,
            along,
        }
    }

    /// **How much of the axis a label takes**: its width where labels stand
    /// side by side, one line of text where they stand one over the other.
    fn extent(&self, label: &str) -> f64 {
        match self.along {
            Along::Horizontal => font::width(label, self.scale) as f64,
            Along::Vertical => font::height(self.scale) as f64,
        }
    }

    /// The clear space two neighbouring labels keep along the axis.
    fn clear(&self) -> f64 {
        match self.along {
            Along::Horizontal => self.label,
            Along::Vertical => self.gap,
        }
    }

    /// The room a label needs from the one before it: its extent and the
    /// clear space -- [`Gaps::label_v`] on a vertical axis.
    fn room(&self, label: &str) -> f64 {
        self.extent(label) + self.clear()
    }

    /// **The 1-2-5 step of a linear axis** whose labels keep clear of each
    /// other at `px_per_value`: one line of text apart on a vertical axis, and
    /// on a horizontal one as wide as the widest label the step itself writes
    /// (`label_at`), found by walking up until the step and its labels agree.
    fn linear_step(&self, px_per_value: f64, widest: &dyn Fn(f64) -> f64) -> f64 {
        let mut step = snap_125(self.label_v() / px_per_value);
        if self.along == Along::Horizontal {
            for _ in 0..8 {
                let need = snap_125((widest(step) + self.label) / px_per_value);
                if need <= step {
                    break;
                }
                step = need;
            }
        }
        step
    }

    /// Minimum device pixels between labels on a vertical axis: one line of
    /// text plus clear space -- the measured-height counterpart of `label`.
    pub(super) fn label_v(&self) -> f64 {
        font::height(self.scale) as f64 + self.gap
    }
}

/// Snap `x` up to the 1-2-5 progression (1, 2, 5, 10, 20, 50, ...).
pub(super) fn snap_125(x: f64) -> f64 {
    let x = x.max(f64::MIN_POSITIVE);
    let decade = 10f64.powf(x.log10().floor());
    for m in [1.0, 2.0, 5.0, 10.0] {
        if m * decade >= x - f64::EPSILON {
            return m * decade;
        }
    }
    10.0 * decade
}

/// The ascending 1-2-5 candidate steps for an axis spanning `axis_len` units
/// over `width_px` device pixels, starting just below the densest step that
/// could possibly fit and stopping once one step spans the whole axis.
/// `min_step` floors the ladder (1.0 on integral axes like samples).
pub(super) fn decimal_steps(axis_len: f64, width_px: f64, min_step: f64, g: Gaps) -> Vec<f64> {
    let floor_raw = (axis_len * g.tick / width_px.max(1.0))
        .max(min_step)
        .max(f64::MIN_POSITIVE);
    let mut step = 10f64.powf(floor_raw.log10().floor());
    let mut out = Vec::new();
    while out.len() < 64 {
        for m in [1.0, 2.0, 5.0] {
            let s = m * step;
            if s >= min_step {
                out.push(s);
            }
        }
        if step > axis_len {
            break;
        }
        step *= 10.0;
    }
    out
}

/// The smallest candidate step whose formatted labels fit `width_px` without
/// collision -- each candidate tried against its **own** labels via
/// [`labels_fit`]. When nothing fits (a degenerate strip), a step wider than
/// the window leaves at most one visible label, which cannot collide.
pub(super) fn fit_step(
    candidates: &[f64],
    axis_start: f64,
    axis_len: f64,
    width_px: f64,
    pos: &dyn Fn(f64) -> f64,
    fmt: &dyn Fn(f64, f64) -> String,
    g: Gaps,
) -> f64 {
    for &step in candidates {
        if labels_fit(step, axis_start, axis_len, width_px, pos, fmt, g) {
            return step;
        }
    }
    axis_len * 1.001
}

/// Whether the labels of every tick at multiples of `step` inside
/// `[axis_start, axis_start + axis_len]`, measured at the ruler font scale
/// and edge-clamped exactly as the renderer draws them, keep at least the
/// `label` gap of clear space between neighbours.
///
/// It walks **every** pair rather than dividing the axis by the tick count,
/// which is what makes it right on an axis whose ticks are not evenly spaced:
/// under a tempo map the rung that fits is the one the *closest* pair leaves
/// room for, and that pair is found by looking rather than by averaging.
pub(super) fn labels_fit(
    step: f64,
    axis_start: f64,
    axis_len: f64,
    width_px: f64,
    pos: &dyn Fn(f64) -> f64,
    fmt: &dyn Fn(f64, f64) -> String,
    g: Gaps,
) -> bool {
    let mut prev_end = f64::NEG_INFINITY;
    let mut k = (axis_start / step).ceil() as i64;
    loop {
        let t = k as f64 * step;
        if t > axis_start + axis_len + step * 1e-9 {
            return true;
        }
        let x = pos(t) * width_px;
        let w = g.extent(&fmt(t, step));
        let lx = (x - w * 0.5).clamp(0.0, (width_px - w).max(0.0));
        if lx < prev_end + g.clear() {
            return false;
        }
        prev_end = lx + w;
        k += 1;
    }
}

/// **The ruler of an axis with nothing on it**: a file of no samples, which
/// has no duration to show. Its one tick is its start, labelled `0` -- in
/// seconds and in samples alike -- or, on the musical grid, as the grid
/// names the beat the axis starts on.
pub(super) fn origin_ticks(sample_rate: f64, unit: TimeUnit, metrics: &Metrics) -> Vec<Tick> {
    let label = match unit {
        TimeUnit::Beats { .. } if sample_rate > 0.0 => time_ticks_on(
            Gaps::of(metrics),
            0.0,
            sample_rate,
            1000.0,
            sample_rate,
            unit,
        )
        .into_iter()
        .find(|t| t.frac.abs() < 1e-9 && t.label.is_some())
        .and_then(|t| t.label),
        _ => None,
    };
    vec![Tick {
        frac: 0.0,
        label: Some(label.unwrap_or_else(|| "0".into())),
    }]
}

/// The ticks of a time ruler spanning samples `[start, start + len)` over
/// `width_px` device pixels. With `TimeUnit::Seconds` (and a positive
/// `sample_rate`) the steps and labels are in clock time; with
/// `TimeUnit::Beats` on the musical grid (labels `bar:beat`); otherwise in
/// sample counts. The step is the smallest rung of the unit's ladder whose
/// measured labels fit; majors carry labels, minors (a fifth of the step)
/// appear only when they are at least a few pixels apart.
///
/// `g` says which axis its labels are measured along.
pub(super) fn time_ticks_on(
    g: Gaps,
    start: f64,
    len: f64,
    width_px: f64,
    sample_rate: f64,
    unit: TimeUnit,
) -> Vec<Tick> {
    if len <= 0.0 || width_px <= 0.0 {
        return Vec::new();
    }
    if let TimeUnit::Beats {
        tempo,
        beat_at,
        quant,
        map,
        bars,
    } = unit
        && sample_rate > 0.0
        && (tempo > 0.0 || map.is_some())
    {
        if !bars.is_empty() {
            return bar_ticks(
                start,
                len,
                width_px,
                sample_rate,
                tempo,
                beat_at,
                map,
                bars,
                g,
            );
        }
        return beat_ticks(
            start,
            len,
            width_px,
            sample_rate,
            tempo,
            beat_at,
            quant,
            map,
            g,
        );
    }
    let seconds = unit == TimeUnit::Seconds && sample_rate > 0.0;
    let (axis_start, axis_len) = if seconds {
        (start / sample_rate, len / sample_rate)
    } else {
        (start, len)
    };
    let fmt = |t: f64, step: f64| {
        if seconds {
            fmt_time(t, step)
        } else {
            fmt_samples(t)
        }
    };
    let pos = |t: f64| (t - axis_start) / axis_len;
    let min_step = if seconds { 0.0 } else { 1.0 }; // samples are integral
    let candidates = decimal_steps(axis_len, width_px, min_step, g);
    let step = fit_step(&candidates, axis_start, axis_len, width_px, &pos, &fmt, g);
    emit_time_ticks(
        axis_start,
        axis_len,
        width_px,
        step,
        step / 5.0,
        &pos,
        &fmt,
        g,
    )
}

/// The `beats` form of the time ruler: the axis converted to beat positions,
/// the step fit on the musical ladder (binary fractions of a beat, whole
/// beats, bars and powers-of-two bars), majors labeled `bar:beat` (1-based)
/// off the quant grid, minors on the binary subdivision.
///
/// **The axis is still samples, and only the marks move.** Without a map a
/// beat is worth a fixed number of them, so the ticks are evenly spaced and
/// the placement is a straight line. With one, each tick is placed by asking
/// the map what second its beat falls on -- majors and minors alike, which is
/// what makes a subdivision inside a ramp land where it sounds instead of
/// halfway between two beats that are themselves moving.
#[allow(clippy::too_many_arguments)]
pub(super) fn beat_ticks(
    start: f64,
    len: f64,
    width_px: f64,
    rate: f64,
    tempo: f64,
    beat_at: f64,
    quant: f64,
    map: Option<&TempoMap>,
    g: Gaps,
) -> Vec<Tick> {
    let (b0, b1) = match map {
        Some(m) => (m.beats_at(start / rate), m.beats_at((start + len) / rate)),
        None => (
            beat_at + start / rate * tempo,
            beat_at + (start + len) / rate * tempo,
        ),
    };
    let blen = b1 - b0;
    if blen <= 0.0 {
        return Vec::new();
    }
    // Where a beat lands on the strip. Through the map it is the multitrack's own
    // answer, run the one way the ruler needs it: beat to second to sample.
    let mapped = |b: f64| (m_secs(map, b) * rate - start) / len;
    let linear = |b: f64| (b - b0) / blen;
    let pos: &dyn Fn(f64) -> f64 = if map.is_some() { &mapped } else { &linear };
    let fmt = |b: f64, step: f64| fmt_bar_beat(b, quant, step);
    let candidates = beat_steps(blen, width_px, quant, g);
    let step = fit_step(&candidates, b0, blen, width_px, pos, &fmt, g);
    emit_time_ticks(b0, blen, width_px, step, step / 2.0, pos, &fmt, g)
}

/// **The `beats` ruler over the page's bars**: the ticks are the bars' own --
/// at every bar, every second, fourth... bar where they are close, or at the
/// beats inside each bar (and their halves, quarters...) where there is room --
/// labelled `bar:beat` as the page numbers its bars. The finest of those whose
/// labels fit is the one drawn, its next finer level the minor ticks.
#[allow(clippy::too_many_arguments)]
pub(super) fn bar_ticks(
    start: f64,
    len: f64,
    width_px: f64,
    rate: f64,
    tempo: f64,
    beat_at: f64,
    map: Option<&TempoMap>,
    runs: &[[f64; 3]],
    g: Gaps,
) -> Vec<Tick> {
    let beat_of = |s: f64| match map {
        Some(m) => m.beats_at(s / rate),
        None => beat_at + s / rate * tempo,
    };
    let (b0, b1) = (beat_of(start), beat_of(start + len));
    if b1 <= b0 {
        return Vec::new();
    }
    let pos = |b: f64| match map {
        Some(m) => (m.secs_at(b) * rate - start) / len,
        None => (b - b0) / (b1 - b0),
    };
    let bars = bars_in(runs, b0, b1);
    // the levels from the finest: a beat divided, the beat, then bars by twos
    let mut levels: Vec<Level> = [0.125, 0.25, 0.5, 1.0]
        .into_iter()
        .map(Level::Beats)
        .collect();
    let mut every = 1;
    while every <= 4096 {
        levels.push(Level::Bars(every));
        every *= 2;
    }
    let ticks_of = |level: Level| -> Vec<(f64, String)> {
        let mut out = Vec::new();
        for &(at, bar_len, number) in &bars {
            match level {
                Level::Beats(step) => {
                    let decimals = (0..=3)
                        .find(|d| (step * 10f64.powi(*d)).fract().abs() < 1e-9)
                        .unwrap_or(3) as usize;
                    let mut j = 0.0;
                    while j < bar_len - 1e-9 {
                        out.push((at + j, format!("{number}:{:.decimals$}", j + 1.0)));
                        j += step;
                    }
                }
                Level::Bars(k) => {
                    if (number - 1).rem_euclid(k) == 0 {
                        out.push((at, number.to_string()));
                    }
                }
            }
        }
        out.retain(|(b, _)| *b >= b0 - 1e-9 && *b <= b1 + 1e-9);
        out
    };
    let fits = |ticks: &[(f64, String)]| {
        let mut prev_end = f64::NEG_INFINITY;
        ticks.iter().all(|(b, label)| {
            let x = pos(*b) * width_px;
            let w = g.extent(label);
            let lx = (x - w * 0.5).clamp(0.0, (width_px - w).max(0.0));
            let clear = lx >= prev_end + g.clear();
            prev_end = lx + w;
            clear
        })
    };
    let Some(at) = levels.iter().position(|level| fits(&ticks_of(*level))) else {
        return Vec::new();
    };
    let majors = ticks_of(levels[at]);
    // the minors: the next finer level, where its ticks are far enough apart
    let minors: Vec<f64> = at
        .checked_sub(1)
        .map(|finer| ticks_of(levels[finer]))
        .unwrap_or_default()
        .into_iter()
        .map(|(b, _)| b)
        .collect();
    let spread = minors
        .windows(2)
        .map(|pair| (pos(pair[1]) - pos(pair[0])) * width_px)
        .fold(f64::INFINITY, f64::min);
    let mut out: Vec<Tick> = majors
        .iter()
        .map(|(b, label)| Tick {
            frac: pos(*b).clamp(0.0, 1.0),
            label: Some(label.clone()),
        })
        .collect();
    if spread >= g.tick {
        out.extend(
            minors
                .into_iter()
                .filter(|b| majors.iter().all(|(m, _)| (m - b).abs() > 1e-9))
                .map(|b| Tick {
                    frac: pos(b).clamp(0.0, 1.0),
                    label: None,
                }),
        );
    }
    out.sort_by(|a, b| a.frac.total_cmp(&b.frac));
    out
}

/// One level of the bars' ruler: a step in beats inside every bar, or every
/// so many bars.
#[derive(Debug, Clone, Copy)]
enum Level {
    Beats(f64),
    Bars(i64),
}

/// The second a beat falls on, per the map (0 without one -- a caller that has
/// no map never reaches this).
pub(super) fn m_secs(map: Option<&TempoMap>, b: f64) -> f64 {
    map.map_or(0.0, |m| m.secs_at(b))
}

/// The ascending musical ladder for a `blen`-beat axis: binary fractions of a
/// beat below 1, whole beats that keep bar lines on majors inside the bar,
/// then bars and powers-of-two bars. Every rung divides the next.
pub(super) fn beat_steps(blen: f64, width_px: f64, quant: f64, g: Gaps) -> Vec<f64> {
    let floor_raw = (blen * g.tick / width_px.max(1.0)).max(1.0 / 1024.0);
    let mut v = 2f64.powf(floor_raw.log2().floor()).min(1.0);
    let mut out = Vec::new();
    while v < 1.0 && out.len() < 64 {
        out.push(v);
        v *= 2.0;
    }
    out.push(1.0);
    // Whole beats inside the bar: 2 beats only when it divides the bar (keeps
    // bar lines on majors), then whole bars, doubling.
    let mut bar = if quant > 1.0 {
        if quant > 2.0 && quant % 2.0 == 0.0 {
            out.push(2.0);
        }
        quant
    } else {
        2.0
    };
    while bar <= blen * 2.0 && out.len() < 64 {
        out.push(bar);
        bar *= 2.0;
    }
    out.push(bar);
    out
}

/// The narrowest gap, in device pixels, between two neighbouring ticks at
/// multiples of `step` -- the axis' own spacing rather than its length divided
/// by a count, which are the same number only while `pos` is a straight line.
pub(super) fn min_gap_px(
    step: f64,
    axis_start: f64,
    axis_len: f64,
    width_px: f64,
    pos: &dyn Fn(f64) -> f64,
) -> f64 {
    let mut narrowest = f64::INFINITY;
    let mut prev = f64::NAN;
    let mut k = (axis_start / step).ceil() as i64;
    loop {
        let t = k as f64 * step;
        if t > axis_start + axis_len + step * 1e-9 {
            break;
        }
        let x = pos(t) * width_px;
        if prev.is_finite() {
            narrowest = narrowest.min(x - prev);
        }
        prev = x;
        k += 1;
    }
    narrowest
}

/// Emits the ticks of a horizontal axis: majors (labeled by `fmt`) at
/// multiples of `step`, minors at multiples of `minor` when they clear the
/// minimum tick gap. `pos` places an axis value as a fraction of the strip --
/// a straight line for every unit but the mapped beat.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_time_ticks(
    axis_start: f64,
    axis_len: f64,
    width_px: f64,
    step: f64,
    minor: f64,
    pos: &dyn Fn(f64) -> f64,
    fmt: &dyn Fn(f64, f64) -> String,
    g: Gaps,
) -> Vec<Tick> {
    let draw_minors = min_gap_px(minor, axis_start, axis_len, width_px, pos) >= g.tick;
    let fine = if draw_minors { minor } else { step };
    let mut out = Vec::new();
    let mut k = (axis_start / fine).ceil() as i64;
    loop {
        let t = k as f64 * fine;
        if t > axis_start + axis_len + fine * 1e-9 {
            break;
        }
        let major = (t / step - (t / step).round()).abs() < 1e-6;
        out.push(Tick {
            frac: pos(t).clamp(0.0, 1.0),
            label: major.then(|| fmt(t, step)),
        });
        k += 1;
    }
    out
}

/// A beat position as a `bar:beat` label (1-based, DAW style) on the `quant`
/// grid, with just enough decimals on the beat for a fractional `step`;
/// without a grid (`quant <= 0`), the plain beat number.
pub(super) fn fmt_bar_beat(beats: f64, quant: f64, step: f64) -> String {
    // Enough decimals that the step's binary fraction prints exactly (0.5 ->
    // 1, 0.25 -> 2, ...), capped at 4.
    let decimals = (0..=4)
        .find(|d| (step * 10f64.powi(*d)).fract().abs() < 1e-9)
        .unwrap_or(4) as usize;
    if quant <= 0.0 {
        return format!("{beats:.decimals$}");
    }
    let bar = tempoclock::bar(beats, quant) + 1.0;
    let beat = tempoclock::beat_in_bar(beats, quant) + 1.0;
    format!("{}:{beat:.decimals$}", bar as i64)
}

/// `secs` as adaptive clock time: `h:mm:ss` / `m:ss` above a minute, plain
/// seconds below, with just enough decimals for `step` (down to microseconds
/// at deep zoom).
pub(super) fn fmt_time(secs: f64, step: f64) -> String {
    let decimals = if step >= 1.0 {
        0
    } else {
        (-step.log10()).ceil().clamp(1.0, 6.0) as usize
    };
    let total = secs.max(0.0);
    let h = (total / 3600.0).floor() as u64;
    let m = ((total % 3600.0) / 60.0).floor() as u64;
    let s = total % 60.0;
    if h > 0 {
        format!(
            "{h}:{m:02}:{s:0width$.decimals$}",
            width = decimals + if decimals > 0 { 3 } else { 2 }
        )
    } else if m > 0 {
        format!(
            "{m}:{s:0width$.decimals$}",
            width = decimals + if decimals > 0 { 3 } else { 2 }
        )
    } else {
        format!("{s:.decimals$}")
    }
}

/// A sample count, compacted with K/M above the thousands.
pub(super) fn fmt_samples(v: f64) -> String {
    let v = v.round();
    if v.abs() >= 1e6 && (v % 1e5).abs() < 0.5 {
        format!("{:.1}M", v / 1e6).replace(".0M", "M")
    } else if v.abs() >= 1e4 && (v % 1e3).abs() < 0.5 {
        format!("{}K", (v / 1e3).round() as i64)
    } else {
        format!("{}", v as i64)
    }
}

/// **The notes a note axis spans**: MIDI note 0 at its bottom and 127 at its
/// top, an even step apiece -- the range a note has, about 8 Hz to 12.5 kHz
/// at A4 = 440, rather than a log axis's floor up to Nyquist.
pub(crate) const NOTE_SPAN: f64 = 127.0;

/// **The normalized floor of a frequency axis** -- the `f_lo` its display
/// mapping starts from, over a Nyquist of `nyquist`: MIDI note 0 at the
/// reference tuning `a4` for a note axis, 20 Hz for every other. One
/// function, because the texture, the curve, the ruler and the gesture must
/// not disagree about where a hertz sits.
pub(crate) fn axis_floor(scale: FreqScale, a4: f64, nyquist: f64) -> f64 {
    let floor = match scale {
        FreqScale::Note => clausters_core::scale::midi_to_hz_tuned(0.0, a4),
        _ => 20.0f64.min(nyquist * 0.5).max(1.0),
    };
    (floor / nyquist.max(1.0)).clamp(1e-5, 0.5)
}

/// **The reference tuning a note axis was laid out at**: its floor is MIDI
/// note 0 at that tuning, 69 semitones under A4.
pub(crate) fn tuning_of(nyquist: f64, f_lo_norm: f64) -> f64 {
    f_lo_norm.clamp(1e-5, 0.5) * nyquist * 2f64.powf(69.0 / 12.0)
}

/// **A frequency as the note it is nearest** at reference `a4`, and the cents
/// it is off it: `A4`, `C#5 +12`, `G2 -30`.
pub(crate) fn note_readout(f: f64, a4: f64) -> String {
    let midi = hz_to_midi_tuned(f, a4);
    let nearest = midi.round();
    let cents = ((midi - nearest) * 100.0).round() as i64;
    let name = note_name(nearest as i32);
    match cents {
        0 => name,
        c => format!("{name} {c:+}"),
    }
}

/// **The ticks of a note axis**: every C labelled with its name, and as many
/// of the other notes as fit, the naturals named where their labels fit and
/// the rest as marks, at reference `a4` over the frequencies `f_bot..f_top`.
pub(super) fn note_ticks(
    a4: f64,
    (f_bot, f_top): (f64, f64),
    height_px: f64,
    to_frac: &impl Fn(f64) -> f64,
    g: Gaps,
) -> Vec<Tick> {
    let first = hz_to_midi_tuned(f_bot.max(1.0), a4).ceil() as i32;
    let last = hz_to_midi_tuned(f_top, a4).floor() as i32;
    let gap_l = g.label_v();
    // How far apart a semitone is where they are closest, which is the same
    // everywhere on a note axis: the axis is geometric.
    let semitone = (to_frac(midi_to_hz_tuned(70.0, a4)) - to_frac(midi_to_hz_tuned(69.0, a4)))
        .abs()
        * height_px;
    let natural = |m: i32| !clausters_core::scale::is_black_key(m);
    let named = |m: i32| {
        let class = m.rem_euclid(12);
        if semitone * 12.0 < gap_l {
            // under an octave a label: every other C
            class == 0 && m.div_euclid(12) % 2 == 0
        } else if semitone * 12.0 < 3.0 * gap_l {
            class == 0
        } else if semitone * 2.0 < gap_l {
            matches!(class, 0 | 4 | 7)
        } else if semitone < gap_l {
            natural(m)
        } else {
            true
        }
    };
    let marked = |m: i32| semitone >= g.tick || natural(m) && semitone * 2.0 >= g.tick;
    (first..=last)
        .filter_map(|m| {
            let d = to_frac(midi_to_hz_tuned(f64::from(m), a4));
            if !(-1e-9..=1.0 + 1e-9).contains(&d) {
                return None;
            }
            let label = named(m).then(|| note_name(m));
            (label.is_some() || marked(m)).then(|| Tick {
                frac: d.clamp(0.0, 1.0),
                label,
            })
        })
        .collect()
}

/// A frequency in Hz, compacted with K above the thousands.
pub(super) fn fmt_hz(f: f64) -> String {
    if f >= 1000.0 {
        let k = f / 1000.0;
        if (k - k.round()).abs() < 1e-9 {
            format!("{}K", k.round() as i64)
        } else {
            format!("{k:.1}K")
        }
    } else {
        format!("{}", f.round() as i64)
    }
}

/// The cursor-readout form of a sample position in clock time -- millisecond
/// precision, refined to the view's pixel resolution (`secs_per_px`) when a
/// pixel spans less than a millisecond, so at deep zoom the readout never
/// shows fewer decimals than the ruler labels -- falling back to a sample
/// count when no rate is known.
pub(crate) fn readout_time(sample: f64, sample_rate: f64, secs_per_px: f64) -> String {
    if sample_rate > 0.0 {
        let step = if secs_per_px > 0.0 {
            secs_per_px.min(1e-3)
        } else {
            1e-3
        };
        fmt_time(sample.max(0.0) / sample_rate, step)
    } else {
        readout_samples(sample)
    }
}

/// The cursor-readout form of a sample position as a plain sample count.
pub(crate) fn readout_samples(sample: f64) -> String {
    format!("{}", sample.max(0.0).round() as i64)
}

/// The decimals a cursor readout needs to resolve `step` (one pixel of the
/// view, in the readout's unit), floored at the unit's base precision so a
/// coarse view never loses it and capped at the matching ruler labels' own
/// cap -- the readout never shows fewer decimals than the ruler.
pub(super) fn readout_decimals(step: f64, floor: usize, cap: usize) -> usize {
    if step > 0.0 {
        (-step.log10()).ceil().clamp(floor as f64, cap as f64) as usize
    } else {
        floor
    }
}

/// The cursor-readout form of a sample position on the beat grid:
/// `bar:beat` (plain beats without a grid) with two decimals, refined to the
/// view's pixel resolution up to the ruler labels' own four-decimal cap,
/// falling back to the sample count when the rate or tempo is unknown.
///
/// It reads the axis' `map` where there is one, for the reason the ticks do:
/// a read-out naming a different beat from the tick under it would be two
/// answers about one pixel. It takes `secs_per_px` rather than beats per
/// pixel because **under a map that is a local rate** -- how much of a beat a
/// pixel is worth depends on the tempo where the cursor is, not on an average
/// of the multitrack -- and deriving it here is what keeps the two read-outs (the
/// waveform's and the roll's) from each doing that conversion their own way.
#[allow(clippy::too_many_arguments)]
pub(crate) fn readout_beats(
    sample: f64,
    sample_rate: f64,
    tempo: f64,
    beat_at: f64,
    quant: f64,
    map: Option<&TempoMap>,
    secs_per_px: f64,
) -> String {
    if sample_rate <= 0.0 || (tempo <= 0.0 && map.is_none()) {
        return readout_samples(sample);
    }
    let secs = sample.max(0.0) / sample_rate;
    let beats = match map {
        Some(m) => m.beats_at(secs),
        None => beat_at + secs * tempo,
    };
    let here = map.map_or(tempo, |m| m.tempo_at(beats));
    let decimals = readout_decimals(secs_per_px * here, 2, 4);
    if quant <= 0.0 {
        return format!("{beats:.decimals$}");
    }
    let bar = tempoclock::bar(beats, quant) + 1.0;
    let beat = tempoclock::beat_in_bar(beats, quant) + 1.0;
    format!("{}:{beat:.decimals$}", bar as i64)
}

/// The cursor-readout form of a normalized amplitude in the vertical-ruler
/// unit (`Norm` for the unit-less kinds). The linear units (`Norm`,
/// `Percent`) refine their decimals to the view's pixel resolution
/// (`amp_per_px`, normalized amplitude per device pixel) under vertical
/// zoom; `Db` and `Bits` already out-resolve their integer ruler labels.
pub(crate) fn readout_amp(amp: f64, unit: RulerY, bit_depth: u32, amp_per_px: f64) -> String {
    match unit {
        RulerY::Db => {
            let mag = amp.abs();
            if mag < 1e-5 {
                "-INF DB".to_string()
            } else {
                format!("{:.1} DB", 20.0 * mag.log10())
            }
        }
        RulerY::Bits => {
            let full = 2f64.powi(bit_depth.saturating_sub(1) as i32);
            format!("{}", (amp * full).round() as i64)
        }
        RulerY::Percent => {
            let decimals = readout_decimals(amp_per_px * 100.0, 0, 6);
            format!("{:.decimals$}%", amp * 100.0)
        }
        _ => {
            let decimals = readout_decimals(amp_per_px, 2, 6);
            format!("{amp:+.decimals$}")
        }
    }
}

/// A decimal label with just enough decimals for `step` (so deep zoom never
/// prints two ticks the same), trailing zeros trimmed (`0.5`, `-0.25`, `1`).
pub(crate) fn fmt_decimal(v: f64, step: f64) -> String {
    let decimals = if step >= 1.0 {
        0
    } else {
        (-step.log10()).ceil().clamp(1.0, 6.0) as usize
    };
    let s = format!("{v:.decimals$}");
    let s = if decimals > 0 {
        s.trim_end_matches('0').trim_end_matches('.')
    } else {
        &s
    };
    if s == "-0" {
        "0".to_string()
    } else {
        s.to_string()
    }
}

/// The dBFS rung candidates of the amplitude axis, walking outward from full
/// scale -- round values the greedy spacing filter thins to what fits.
pub(super) const DB_RUNGS: [f64; 17] = [
    -96.0, -72.0, -60.0, -48.0, -36.0, -30.0, -24.0, -18.0, -12.0, -10.0, -8.0, -6.0, -5.0, -4.0,
    -3.0, -2.0, -1.0, // 0 dB is handled as the walk's endpoint below.
];

/// The ticks of an amplitude ruler beside a waveform row of `height_px`
/// device pixels, in the vertical unit `unit` (`Off`/`Hz` yield none), laid
/// out over the visible display window `[y_start, y_start + y_len)` of the
/// row's vertical axis (`0, 1` = no zoom). The positions respect the same
/// `AMP_MARGIN` the waveform geometry applies, so a tick sits exactly on the
/// amplitude it names. The linear units (`Norm`, `Percent`, `Bits`) share the
/// 1-2-5 geometry and differ only in labels; `Db` walks the fixed dBFS rung
/// list outward from the (silence) center line, labeling the rungs that clear
/// one line of text and keeping crowded ones as minors.
///
/// `g` says which axis its labels are measured along.
pub(super) fn amp_ticks_on(
    g: Gaps,
    unit: RulerY,
    height_px: f64,
    bit_depth: u32,
    y_start: f64,
    y_len: f64,
) -> Vec<Tick> {
    if height_px <= 0.0 || y_len <= 0.0 {
        return Vec::new();
    }
    let margin = AMP_MARGIN as f64;
    // Absolute display coordinate of an amplitude (0 = the row's bottom at no
    // zoom), then mapped through the visible window into the row's fraction.
    let frac_of = |amp: f64| ((amp * margin + 1.0) / 2.0 - y_start) / y_len;
    let visible = |f: f64| (-1e-9..=1.0 + 1e-9).contains(&f);
    match unit {
        RulerY::Off | RulerY::Hz => Vec::new(),
        RulerY::Db => {
            let mut out = Vec::new();
            let center = frac_of(0.0);
            if visible(center) {
                out.push(Tick {
                    frac: center.clamp(0.0, 1.0),
                    label: Some("-INF".to_string()),
                });
            }
            // Walk outward from the center: spacing grows with amplitude, so
            // the crowded rungs drop first. Off-screen rungs still consume
            // their ladder slot, so panning never reshuffles the kept set.
            let px_of = |amp: f64| frac_of(amp) * height_px;
            let (mut last_any, mut last_label) = (px_of(0.0), px_of(0.0));
            for db in DB_RUNGS.into_iter().chain([0.0]) {
                let amp = 10f64.powf(db / 20.0);
                let p = px_of(amp);
                if p - last_any < g.tick {
                    continue;
                }
                let text = format!("{}", db as i64);
                let label = (p - last_label >= g.room(&text)).then_some(text);
                if label.is_some() {
                    last_label = p;
                }
                last_any = p;
                for frac in [frac_of(amp), frac_of(-amp)] {
                    if visible(frac) {
                        out.push(Tick {
                            frac: frac.clamp(0.0, 1.0),
                            label: label.clone(),
                        });
                    }
                }
            }
            out
        }
        _ => {
            let full = if unit == RulerY::Bits {
                2f64.powi(bit_depth.saturating_sub(1) as i32)
            } else {
                1.0
            };
            let label_of = |v: f64, step: f64| match unit {
                // Signed: 0..100% above the zero line, 0..-100% below it.
                RulerY::Percent => {
                    format!("{}%", fmt_decimal(v / full * 100.0, step / full * 100.0))
                }
                RulerY::Bits => fmt_samples(v),
                _ => fmt_decimal(v, step),
            };
            // The visible value range and its on-screen density decide the
            // step: the smallest 1-2-5 rung whose labels (one line of text
            // each) keep clear space.
            let amp_at = |d: f64| (2.0 * d - 1.0) / margin;
            let v_lo = (amp_at(y_start) * full).max(-full);
            let v_hi = (amp_at(y_start + y_len) * full).min(full);
            let px_per_value = height_px * margin / (2.0 * y_len) / full;
            let widest = |step: f64| {
                [v_lo, v_hi]
                    .map(|v| g.extent(&label_of((v / step).round() * step, step)))
                    .into_iter()
                    .fold(0.0, f64::max)
            };
            let mut step = g.linear_step(px_per_value, &widest).min(full);
            if unit == RulerY::Bits {
                step = step.max(1.0);
            }
            let mut out = Vec::new();
            let mut k = (v_lo / step).ceil() as i64;
            while k as f64 * step <= v_hi + step * 1e-9 {
                let v = k as f64 * step;
                let frac = frac_of((v / full).clamp(-1.0, 1.0));
                if visible(frac) {
                    out.push(Tick {
                        frac: frac.clamp(0.0, 1.0),
                        label: Some(label_of(v, step)),
                    });
                }
                k += 1;
            }
            // The full-scale endpoints when the decimal step misses them (a
            // 16-bit axis stepping 10000 still labels +/-32768) and their label
            // clears the nearest stepped one by at least a line of text.
            let remainder = full - (full / step).floor() * step;
            if remainder > step * 1e-9
                && remainder * px_per_value >= g.extent(&label_of(full, step))
            {
                for sign in [-1.0, 1.0] {
                    let frac = frac_of(sign);
                    if visible(frac) {
                        out.push(Tick {
                            frac: frac.clamp(0.0, 1.0),
                            label: Some(label_of(sign * full, step)),
                        });
                    }
                }
            }
            out
        }
    }
}

/// The ticks of a plain linear **value axis** over `[lo, hi]` -- any range, not
/// tied to an amplitude convention (no margin, no full-scale): the `plot`'s
/// vertical ruler for arbitrary numeric sequences. The step is the smallest
/// 1-2-5 rung whose labels (one line of text each) keep clear space over
/// `height_px`; majors are labeled with [`fmt_decimal`], minors (a fifth of
/// the step) appear when they clear the minimum tick gap. `frac` is 0 at `lo`
/// (the bottom), 1 at `hi`.
///
/// `g` says which axis its labels are measured along.
pub(super) fn value_ticks_on(g: Gaps, lo: f64, hi: f64, height_px: f64) -> Vec<Tick> {
    let span = hi - lo;
    if span <= 0.0 || height_px <= 0.0 {
        return Vec::new();
    }
    let px_per_value = height_px / span;
    let widest = |step: f64| {
        [lo, hi]
            .map(|v| g.extent(&fmt_decimal((v / step).round() * step, step)))
            .into_iter()
            .fold(0.0, f64::max)
    };
    let step = g.linear_step(px_per_value, &widest);
    let minor = step / 5.0;
    let draw_minors = minor * px_per_value >= g.tick;
    let fine = if draw_minors { minor } else { step };
    let mut out = Vec::new();
    let mut k = (lo / fine).ceil() as i64;
    loop {
        let v = k as f64 * fine;
        if v > hi + fine * 1e-9 {
            break;
        }
        let major = (v / step - (v / step).round()).abs() < 1e-6;
        out.push(Tick {
            frac: ((v - lo) / span).clamp(0.0, 1.0),
            label: major.then(|| fmt_decimal(v, step)),
        });
        k += 1;
    }
    out
}

/// The cursor-readout form of an arbitrary value on a `[lo, hi]` axis: enough
/// decimals to resolve about a thousandth of the span (so a zoomed-in narrow
/// range still reads distinct values), trailing zeros trimmed.
pub(crate) fn readout_value(v: f64, span: f64) -> String {
    let step = (span.abs() / 1000.0).max(f64::MIN_POSITIVE);
    fmt_decimal(v, step)
}

/// Display coordinate `d` (0 = axis bottom, 1 = Nyquist) -> frequency in Hz,
/// the exact mapping the spectrogram shader applies per scale -- the single
/// inversion the ruler ticks and the cursor readout both use. `f_lo_norm` is
/// the shader's normalized log-axis floor (~20 Hz / Nyquist).
pub(crate) fn display_to_hz(d: f64, nyquist: f64, scale: FreqScale, f_lo_norm: f64) -> f64 {
    match scale {
        FreqScale::Linear => nyquist * d,
        FreqScale::Log => nyquist * f_lo_norm.clamp(1e-5, 0.5).powf(1.0 - d),
        FreqScale::Note => nyquist * f_lo_norm.clamp(1e-5, 0.5) * 2f64.powf(NOTE_SPAN * d / 12.0),
        FreqScale::Mel => mel_to_hz(d * hz_to_mel(nyquist)),
        FreqScale::Bark => {
            let z0 = hz_to_bark(0.0);
            bark_to_hz(z0 + d * (hz_to_bark(nyquist) - z0))
        }
    }
}

/// Frequency in Hz -> display coordinate under `scale`, the inverse of
/// [`display_to_hz`] (the tick-placement direction).
pub(crate) fn hz_to_display(f: f64, nyquist: f64, scale: FreqScale, f_lo_norm: f64) -> f64 {
    match scale {
        FreqScale::Linear => f / nyquist,
        FreqScale::Log => {
            let f_lo = f_lo_norm.clamp(1e-5, 0.5);
            1.0 - (f / nyquist).ln() / f_lo.ln()
        }
        FreqScale::Note => {
            let f_lo = f_lo_norm.clamp(1e-5, 0.5) * nyquist;
            12.0 * (f / f_lo).log2() / NOTE_SPAN
        }
        FreqScale::Mel => hz_to_mel(f) / hz_to_mel(nyquist),
        FreqScale::Bark => {
            let z0 = hz_to_bark(0.0);
            (hz_to_bark(f) - z0) / (hz_to_bark(nyquist) - z0)
        }
    }
}

/// The ticks of a frequency ruler over the visible display window
/// `[y_start, y_start + y_len)` of the axis (bottom = the axis floor, top =
/// Nyquist at no zoom), matching the spectrogram shader's display->bin mapping
/// for `scale`. A **wide** window (a decade or more) uses the classic decade
/// scheme -- 1/2/5 multiples labeled, the remaining integer multiples as
/// minors, thinned by the measured label height (the log and perceptual
/// scales compress unevenly). A **narrow** (zoomed) window, and the linear
/// axis at any zoom, fits a plain 1-2-5 ladder in hertz against its measured
/// labels, so zooming in keeps revealing finer round frequencies.
pub(super) fn hz_ticks(
    nyquist: f64,
    scale: FreqScale,
    f_lo_norm: f64,
    height_px: f64,
    y_start: f64,
    y_len: f64,
    metrics: &Metrics,
) -> Vec<Tick> {
    let g = Gaps::of(metrics);
    if nyquist <= 0.0 || height_px <= 0.0 || y_len <= 0.0 {
        return Vec::new();
    }
    let to_frac = |f: f64| (hz_to_display(f, nyquist, scale, f_lo_norm) - y_start) / y_len;
    let visible = |f: f64| (-1e-9..=1.0 + 1e-9).contains(&f);
    let f_bot = display_to_hz(y_start.max(0.0), nyquist, scale, f_lo_norm).max(0.0);
    let f_top = display_to_hz((y_start + y_len).min(1.0), nyquist, scale, f_lo_norm).min(nyquist);
    if f_top <= f_bot {
        return Vec::new();
    }
    if scale == FreqScale::Note {
        return note_ticks(
            tuning_of(nyquist, f_lo_norm),
            (f_bot, f_top),
            height_px,
            &to_frac,
            g,
        );
    }
    let mut out = Vec::new();
    let gap_l = g.label_v();
    if scale != FreqScale::Linear && f_top / f_bot.max(1.0) >= 10.0 {
        // Decade scheme over a wide window.
        let (mut last_label, mut last_any) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
        if scale != FreqScale::Log {
            // The perceptual scales have a true 0 at the axis bottom.
            let d = to_frac(0.0);
            if visible(d) {
                out.push(Tick {
                    frac: d.clamp(0.0, 1.0),
                    label: Some("0".to_string()),
                });
                last_label = d * height_px;
                last_any = last_label;
            }
        }
        let floor = if scale == FreqScale::Log {
            (f_lo_norm.clamp(1e-5, 0.5) * nyquist).max(f_bot)
        } else {
            f_bot.max(1.0)
        };
        let mut decade = 10f64.powf(floor.log10().floor()).max(1.0);
        while decade <= f_top {
            for mult in 1..10 {
                let f = decade * mult as f64;
                if f < floor || f > f_top {
                    continue;
                }
                let d = to_frac(f);
                if !visible(d) {
                    continue;
                }
                let p = d * height_px;
                let major = matches!(mult, 1 | 2 | 5);
                if major && p - last_label >= gap_l && p - last_any >= g.tick {
                    out.push(Tick {
                        frac: d.clamp(0.0, 1.0),
                        label: Some(fmt_hz(f)),
                    });
                    last_label = p;
                    last_any = p;
                } else if p - last_any >= g.tick {
                    out.push(Tick {
                        frac: d.clamp(0.0, 1.0),
                        label: None,
                    });
                    last_any = p;
                }
            }
            decade *= 10.0;
        }
    } else {
        // A 1-2-5 ladder in hertz over the visible range, fit against the
        // measured label heights at the *actual* (possibly nonlinear) tick
        // positions -- the worst local gap decides.
        let span = f_top - f_bot;
        let candidates = decimal_steps(span, height_px, 1.0, g);
        let fits = |step: f64| {
            let mut prev = f64::NEG_INFINITY;
            let mut k = (f_bot / step).ceil() as i64;
            loop {
                let f = k as f64 * step;
                if f > f_top + step * 1e-9 {
                    return true;
                }
                let p = to_frac(f) * height_px;
                if p - prev < gap_l {
                    return false;
                }
                prev = p;
                k += 1;
            }
        };
        let step = candidates
            .iter()
            .copied()
            .find(|&s| fits(s))
            .unwrap_or(span * 1.001);
        let minor = step / 5.0;
        let mut prev_any = f64::NEG_INFINITY;
        let mut k = (f_bot / minor).ceil() as i64;
        loop {
            let f = k as f64 * minor;
            if f > f_top + minor * 1e-9 {
                break;
            }
            let d = to_frac(f);
            k += 1;
            if !visible(d) {
                continue;
            }
            let p = d * height_px;
            let major = (f / step - (f / step).round()).abs() < 1e-6;
            if !major && p - prev_any < g.tick {
                continue;
            }
            prev_any = p;
            out.push(Tick {
                frac: d.clamp(0.0, 1.0),
                label: major.then(|| fmt_hz(f)),
            });
        }
    }
    out
}

/// The ticks of a **horizontal** frequency axis over the visible display
/// window `[x_start, x_start + x_len)` of `[0, Nyquist]`, across `width_px`
/// device pixels -- the spectral x ruler. The horizontal twin of [`hz_ticks`],
/// down to the two schemes it picks between: a **wide** window (a decade or
/// more, on a non-linear scale) walks the decades with 1/2/5 labeled, and a
/// **narrow** (zoomed) one -- like the linear axis at any zoom -- fits a plain
/// 1-2-5 ladder in hertz over what is actually visible, so zooming keeps
/// revealing finer round frequencies. What differs is only how a label is
/// measured: by its *width*, edge-clamped exactly as the renderer draws it,
/// since these sit side by side. `frac` is 0 at the window's left edge, 1 at
/// its right; `x_start = 0, x_len = 1` is the whole axis.
pub(super) fn hz_ticks_h(
    nyquist: f64,
    scale: FreqScale,
    f_lo_norm: f64,
    width_px: f64,
    x_start: f64,
    x_len: f64,
    metrics: &Metrics,
) -> Vec<Tick> {
    if nyquist <= 0.0 || width_px <= 0.0 || x_len <= 0.0 {
        return Vec::new();
    }
    let g = Gaps::of(metrics);
    let to_px = |f: f64| (hz_to_display(f, nyquist, scale, f_lo_norm) - x_start) / x_len * width_px;
    // What the window actually shows, in hertz: the range every scheme below
    // walks, so a zoomed ruler never spends its candidates on rungs off screen.
    let f_lo_vis = display_to_hz(x_start.max(0.0), nyquist, scale, f_lo_norm).max(0.0);
    let f_hi_vis =
        display_to_hz((x_start + x_len).min(1.0), nyquist, scale, f_lo_norm).min(nyquist);
    if f_hi_vis <= f_lo_vis {
        return Vec::new();
    }
    // A label centered at `p`, edge-clamped into the strip: its drawn span.
    let span_of = |label: &str, p: f64| {
        let w = font::width(label, g.scale) as f64;
        let lx = (p - w * 0.5).clamp(0.0, (width_px - w).max(0.0));
        (lx, lx + w)
    };
    let mut out = Vec::new();
    let mut prev_end = f64::NEG_INFINITY;
    let mut last_any = f64::NEG_INFINITY;
    let mut push = |f: f64, want_label: bool, out: &mut Vec<Tick>| {
        let p = to_px(f);
        if !(-1e-9..=width_px + 1e-9).contains(&p) {
            return;
        }
        let label = if want_label {
            let text = fmt_hz(f);
            let (lx, rx) = span_of(&text, p);
            (lx >= prev_end + g.label).then(|| {
                prev_end = rx;
                text
            })
        } else {
            None
        };
        if label.is_none() && p - last_any < g.tick {
            return;
        }
        last_any = p;
        out.push(Tick {
            frac: (p / width_px).clamp(0.0, 1.0),
            label,
        });
    };
    if scale != FreqScale::Linear && f_hi_vis / f_lo_vis.max(1.0) >= 10.0 {
        // Decade scheme: 1/2/5 multiples labeled (as they fit), the rest minors.
        if scale != FreqScale::Log {
            push(0.0, true, &mut out); // the perceptual scales reach a true 0
        }
        let axis_floor = if scale == FreqScale::Log {
            (f_lo_norm.clamp(1e-5, 0.5) * nyquist).max(1.0)
        } else {
            1.0
        };
        let floor = axis_floor.max(f_lo_vis);
        let mut decade = 10f64.powf(floor.log10().floor()).max(1.0);
        while decade <= f_hi_vis {
            for mult in 1..10 {
                let f = decade * mult as f64;
                if f < floor || f > f_hi_vis {
                    continue;
                }
                push(f, matches!(mult, 1 | 2 | 5), &mut out);
            }
            decade *= 10.0;
        }
    } else {
        // A 1-2-5 ladder in hertz over the visible range: the smallest step
        // whose labels fit at their **actual** (possibly nonlinear) positions,
        // which is what the log scale needs once it is zoomed past a decade.
        let span = f_hi_vis - f_lo_vis;
        let candidates = decimal_steps(span, width_px, 1.0, g);
        let fits = |step: f64| {
            let mut prev_end = f64::NEG_INFINITY;
            let mut k = (f_lo_vis / step).ceil() as i64;
            loop {
                let f = k as f64 * step;
                if f > f_hi_vis + step * 1e-9 {
                    return true;
                }
                let (lx, rx) = span_of(&fmt_hz(f), to_px(f));
                if lx < prev_end + g.label {
                    return false;
                }
                prev_end = rx;
                k += 1;
            }
        };
        let step = candidates
            .iter()
            .copied()
            .find(|&s| fits(s))
            .unwrap_or(span * 1.001);
        let minor = step / 5.0;
        let mut k = (f_lo_vis / minor).floor() as i64;
        loop {
            let f = k as f64 * minor;
            if f > f_hi_vis + minor * 1e-9 {
                break;
            }
            let major = (f / step - (f / step).round()).abs() < 1e-6;
            push(f, major, &mut out);
            k += 1;
        }
    }
    out
}

/// The display name of a frequency scale, as the spectral views' corner
/// read-out tags it -- three letters each, so the tag's footprint is constant
/// across scales.
pub(crate) fn scale_tag(scale: FreqScale) -> &'static str {
    match scale {
        FreqScale::Linear => "LIN",
        FreqScale::Log => "LOG",
        FreqScale::Mel => "MEL",
        FreqScale::Bark => "BRK",
        FreqScale::Note => "TET",
    }
}

/// **The decibel ladder of a meter**: the marks from full scale down to
/// `floor_db`, over a column `height_px` device pixels tall.
///
/// A meter's axis is linear *in decibels*, so its ladder is a regular step and
/// not the 1-2-5 progression a value axis walks or the outward crowd
/// [`amp_ticks_on`] thins -- the same reason the three exist separately. The step
/// is the coarsest of the field's (3, 6, 12, 24, 48 dB) that still gives every
/// mark a line of text to itself, so a tall meter is read in sixes and a short
/// one in twenty-fours rather than in a ladder nobody can read. The floor is
/// always marked, since where a meter bottoms out is the one number that says
/// what scale it is on at all.
pub(super) fn db_ticks(g: Gaps, floor_db: f64, height_px: f64) -> Vec<Tick> {
    let floor = floor_db.min(-1.0);
    if height_px <= 0.0 {
        return Vec::new();
    }
    let px_per_db = height_px / -floor;
    // the widest label the ladder writes is its floor's
    let room = g.room(&format!("{}", floor.round() as i64));
    const STEPS: [f64; 6] = [3.0, 6.0, 12.0, 24.0, 48.0, 96.0];
    let step = STEPS
        .into_iter()
        .find(|s| s * px_per_db >= room)
        .unwrap_or(-floor);
    let mut out = Vec::new();
    let mut db = 0.0;
    // The floor is always marked, so the last rung above it stops where it
    // would crowd the floor's own label rather than at a fixed fraction of the
    // step: at the bottom of the scale those two are the only pair whose gap is
    // not the step, and a number written over another is worse than a rung
    // missing.
    while db > floor && (db - floor) * px_per_db >= room {
        out.push(Tick {
            frac: ((db - floor) / -floor).clamp(0.0, 1.0),
            label: Some(format!("{}", db.round() as i64)),
        });
        db -= step;
    }
    out.push(Tick {
        frac: 0.0,
        label: Some(format!("{}", floor.round() as i64)),
    });
    out
}

/// **What a ruler shows**: one of the project's scales, with what each needs
/// to know about its axis. Every ruler is one of these drawn on a side
/// ([`super::Ruler`]); a view, a plot, a scope and a meter name theirs and
/// draw nothing of their own.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Scale<'a> {
    /// **Time**, over samples: seconds, samples or `bar:beat` ([`TimeUnit`])
    /// at `rate` samples a second. `empty` is an axis bounded by a take of no
    /// samples, which shows its start alone ([`origin_ticks`]). Its window is
    /// samples, `(start, len)`.
    Time {
        rate: f64,
        unit: TimeUnit<'a>,
        empty: bool,
    },
    /// **Full-scale amplitude**, labelled in `unit` (normalized, percent,
    /// sample values at `bit_depth`, dBFS) and placed with the waveform's own
    /// margin. Its window is the display window of the axis.
    Amplitude { unit: RulerY, bit_depth: u32 },
    /// **A plain linear value** over `[lo, hi]`: a curve's parameter, a
    /// plot's numbers, a spectrum's decibels. Its window is a fraction of the
    /// range.
    Value { lo: f64, hi: f64 },
    /// **Frequency**, placed through the display mapping `scale` over
    /// `[f_lo, nyquist]` (`f_lo` normalized): hertz on a linear, log, mel or
    /// bark axis, note names on a note axis. Its window is the display window.
    Hertz {
        nyquist: f64,
        scale: FreqScale,
        f_lo: f64,
    },
    /// **A meter's decibels**, linear in decibels from `floor` up to full
    /// scale ([`db_ticks`]). Its window is a fraction of the range.
    Decibels { floor: f64 },
}

impl Scale<'_> {
    /// **The ticks of this scale over `window` of its axis**, laid `along`
    /// an axis `length_px` device pixels long -- the fractions the drawing
    /// places, `0` at the window's start (its left, or its bottom).
    ///
    /// The window is in the scale's own coordinate (samples for time, the
    /// display window for amplitude and frequency, a fraction of the range for
    /// the rest), so a zoom on either axis is the same question.
    pub(crate) fn ticks(
        &self,
        along: Along,
        (start, len): (f64, f64),
        length_px: f64,
        metrics: &Metrics,
    ) -> Vec<Tick> {
        let g = Gaps::along(metrics, along);
        match *self {
            Scale::Time { rate, unit, empty } => {
                if empty {
                    origin_ticks(rate, unit, metrics)
                } else {
                    time_ticks_on(g, start, len, length_px, rate, unit)
                }
            }
            Scale::Amplitude { unit, bit_depth } => {
                amp_ticks_on(g, unit, length_px, bit_depth, start, len)
            }
            Scale::Value { lo, hi } => {
                let at = |f: f64| lo + f * (hi - lo);
                value_ticks_on(g, at(start), at(start + len), length_px)
            }
            Scale::Hertz {
                nyquist,
                scale,
                f_lo,
            } => match along {
                Along::Vertical => hz_ticks(nyquist, scale, f_lo, length_px, start, len, metrics),
                Along::Horizontal => {
                    hz_ticks_h(nyquist, scale, f_lo, length_px, start, len, metrics)
                }
            },
            Scale::Decibels { floor } => {
                // the whole ladder at the zoomed length, then the window of it
                if len <= 0.0 {
                    return Vec::new();
                }
                db_ticks(g, floor, length_px / len)
                    .into_iter()
                    .filter_map(|t| {
                        let frac = (t.frac - start) / len;
                        (-1e-9..=1.0 + 1e-9).contains(&frac).then(|| Tick {
                            frac: frac.clamp(0.0, 1.0),
                            ..t
                        })
                    })
                    .collect()
            }
        }
    }
}
