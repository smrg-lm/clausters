//! Adaptive ruler tick math for the editor-grade views — pure, display-only.
//!
//! A time axis under the waveform/spectrogram, an amplitude axis beside the
//! waveform and a frequency axis beside the spectrogram need tick positions
//! and labels that stay legible at any zoom. The math is classic editor
//! chrome: each unit owns a ladder of candidate steps — a 1-2-5 decimal
//! progression (time in seconds or samples; amplitude in normalized/percent/
//! integer sample units; zoomed-in frequency spans), a binary/bar ladder on
//! the musical `beats` axis (labels `bar:beat` off the client's quant grid,
//! via the shared `clausters_core::tempoclock::bar`/`beat_in_bar`), a fixed
//! mirrored dB rung list on the dBFS amplitude axis, and decade ticks on the
//! wide frequency axis — and the layout picks the smallest step whose
//! **measured labels fit**: each candidate is tried against its *own*
//! formatted labels (`font::width`/`height` at the ruler's font scale, in
//! device pixels, so HiDPI is exact), never against a mean width. Every
//! generator lays out over the **visible sub-range** of its axis, so vertical
//! zoom/pan reveal finer rungs exactly like horizontal zoom does. The
//! frequency ticks are placed with the **identical** display→bin geometry the
//! spectrogram shader uses (linear, log, mel or bark; the perceptual forms
//! from `clausters_core::scale`), so a tick labeled 1 kHz sits exactly on the
//! 1 kHz row of pixels. No GPU, no widget types: positions come out as
//! fractions of the visible span, and the two strip painters here
//! ([`draw_ticks_h`]/[`draw_ticks_v`]) turn them into mesh geometry — one
//! drawing of a ruler strip, shared by every ruled view (the editor-grade
//! frames and the plot).

use clausters_core::scale::{bark_to_hz, hz_to_bark, hz_to_mel, mel_to_hz};
use clausters_core::tempoclock;

use crate::spectrogram::FreqScale;
use crate::waveform::AMP_MARGIN;

use super::font;
use super::layout::Rect;
use super::metrics::Metrics;
use super::paint::Draw;
use super::widget::RulerY;

/// One ruler tick: its position as a fraction of the visible axis span
/// (0 = start/bottom, 1 = end/top) and its label (`None` for a minor tick).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Tick {
    pub frac: f64,
    pub label: Option<String>,
}

/// How the time ruler labels its ticks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum TimeUnit {
    /// `h:mm:ss.mmm`-style clock time (needs the sample rate).
    Seconds,
    /// Plain sample counts (the fallback when no rate is known).
    Samples,
    /// Musical time on the client's beat grid: `tempo` in beats per second
    /// (the client `Clock` convention), `beat_at` the beat position of sample
    /// 0, `quant` the beats per bar (`<= 0` = no bar grid). Falls back to
    /// sample counts when the rate or tempo is unknown.
    Beats {
        tempo: f64,
        beat_at: f64,
        quant: f64,
    },
}

/// The ruler's own reading of the host's size roles: the `caption_scale` its
/// labels render at — the layout measures every candidate label at that scale,
/// so what fits in the math is exactly what fits on screen — plus the
/// `label_gap` between two drawn labels and the `tick_gap` between any two
/// drawn ticks (labels drop before ticks do).
#[derive(Debug, Clone, Copy)]
struct Gaps {
    scale: f32,
    label: f64,
    tick: f64,
    gap: f64,
}

impl Gaps {
    fn of(metrics: &Metrics) -> Self {
        Self {
            scale: metrics.caption_scale,
            label: metrics.label_gap as f64,
            tick: metrics.tick_gap as f64,
            gap: metrics.gap as f64,
        }
    }

    /// Minimum device pixels between labels on a vertical axis: one line of
    /// text plus clear space — the measured-height counterpart of `label`.
    fn label_v(&self) -> f64 {
        font::height(self.scale) as f64 + self.gap
    }
}

/// Snap `x` up to the 1-2-5 progression (1, 2, 5, 10, 20, 50, ...).
fn snap_125(x: f64) -> f64 {
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
fn decimal_steps(axis_len: f64, width_px: f64, min_step: f64, g: Gaps) -> Vec<f64> {
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
/// collision — each candidate tried against its **own** labels via
/// [`labels_fit`]. When nothing fits (a degenerate strip), a step wider than
/// the window leaves at most one visible label, which cannot collide.
fn fit_step(
    candidates: &[f64],
    axis_start: f64,
    axis_len: f64,
    width_px: f64,
    fmt: &dyn Fn(f64, f64) -> String,
    g: Gaps,
) -> f64 {
    for &step in candidates {
        if labels_fit(step, axis_start, axis_len, width_px, fmt, g) {
            return step;
        }
    }
    axis_len * 1.001
}

/// Whether the labels of every tick at multiples of `step` inside
/// `[axis_start, axis_start + axis_len]`, measured at the ruler font scale
/// and edge-clamped exactly as the renderer draws them, keep at least the
/// `label` gap of clear space between neighbours.
fn labels_fit(
    step: f64,
    axis_start: f64,
    axis_len: f64,
    width_px: f64,
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
        let x = (t - axis_start) / axis_len * width_px;
        let w = font::width(&fmt(t, step), g.scale) as f64;
        let lx = (x - w * 0.5).clamp(0.0, (width_px - w).max(0.0));
        if lx < prev_end + g.label {
            return false;
        }
        prev_end = lx + w;
        k += 1;
    }
}

/// The ticks of a time ruler spanning samples `[start, start + len)` over
/// `width_px` device pixels. With `TimeUnit::Seconds` (and a positive
/// `sample_rate`) the steps and labels are in clock time; with
/// `TimeUnit::Beats` on the musical grid (labels `bar:beat`); otherwise in
/// sample counts. The step is the smallest rung of the unit's ladder whose
/// measured labels fit; majors carry labels, minors (a fifth of the step)
/// appear only when they are at least a few pixels apart.
pub(crate) fn time_ticks(
    start: f64,
    len: f64,
    width_px: f64,
    sample_rate: f64,
    unit: TimeUnit,
    metrics: &Metrics,
) -> Vec<Tick> {
    let g = Gaps::of(metrics);
    if len <= 0.0 || width_px <= 0.0 {
        return Vec::new();
    }
    if let TimeUnit::Beats {
        tempo,
        beat_at,
        quant,
    } = unit
        && sample_rate > 0.0
        && tempo > 0.0
    {
        return beat_ticks(start, len, width_px, sample_rate, tempo, beat_at, quant, g);
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
    let min_step = if seconds { 0.0 } else { 1.0 }; // samples are integral
    let candidates = decimal_steps(axis_len, width_px, min_step, g);
    let step = fit_step(&candidates, axis_start, axis_len, width_px, &fmt, g);
    emit_time_ticks(axis_start, axis_len, width_px, step, step / 5.0, &fmt, g)
}

/// The `beats` form of the time ruler: the axis converted to beat positions,
/// the step fit on the musical ladder (binary fractions of a beat, whole
/// beats, bars and powers-of-two bars), majors labeled `bar:beat` (1-based)
/// off the quant grid, minors on the binary subdivision.
#[allow(clippy::too_many_arguments)]
fn beat_ticks(
    start: f64,
    len: f64,
    width_px: f64,
    rate: f64,
    tempo: f64,
    beat_at: f64,
    quant: f64,
    g: Gaps,
) -> Vec<Tick> {
    let b0 = beat_at + start / rate * tempo;
    let blen = len / rate * tempo;
    if blen <= 0.0 {
        return Vec::new();
    }
    let fmt = |b: f64, step: f64| fmt_bar_beat(b, quant, step);
    let candidates = beat_steps(blen, width_px, quant, g);
    let step = fit_step(&candidates, b0, blen, width_px, &fmt, g);
    emit_time_ticks(b0, blen, width_px, step, step / 2.0, &fmt, g)
}

/// The ascending musical ladder for a `blen`-beat axis: binary fractions of a
/// beat below 1, whole beats that keep bar lines on majors inside the bar,
/// then bars and powers-of-two bars. Every rung divides the next.
fn beat_steps(blen: f64, width_px: f64, quant: f64, g: Gaps) -> Vec<f64> {
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

/// Emits the ticks of a linear horizontal axis: majors (labeled by `fmt`) at
/// multiples of `step`, minors at multiples of `minor` when they clear the
/// minimum tick gap.
fn emit_time_ticks(
    axis_start: f64,
    axis_len: f64,
    width_px: f64,
    step: f64,
    minor: f64,
    fmt: &dyn Fn(f64, f64) -> String,
    g: Gaps,
) -> Vec<Tick> {
    let draw_minors = minor / axis_len * width_px >= g.tick;
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
            frac: ((t - axis_start) / axis_len).clamp(0.0, 1.0),
            label: major.then(|| fmt(t, step)),
        });
        k += 1;
    }
    out
}

/// A beat position as a `bar:beat` label (1-based, DAW style) on the `quant`
/// grid, with just enough decimals on the beat for a fractional `step`;
/// without a grid (`quant <= 0`), the plain beat number.
fn fmt_bar_beat(beats: f64, quant: f64, step: f64) -> String {
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
fn fmt_time(secs: f64, step: f64) -> String {
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
fn fmt_samples(v: f64) -> String {
    let v = v.round();
    if v.abs() >= 1e6 && (v % 1e5).abs() < 0.5 {
        format!("{:.1}M", v / 1e6).replace(".0M", "M")
    } else if v.abs() >= 1e4 && (v % 1e3).abs() < 0.5 {
        format!("{}K", (v / 1e3).round() as i64)
    } else {
        format!("{}", v as i64)
    }
}

/// A frequency in Hz, compacted with K above the thousands.
fn fmt_hz(f: f64) -> String {
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

/// The cursor-readout form of a sample position in clock time — millisecond
/// precision, refined to the view's pixel resolution (`secs_per_px`) when a
/// pixel spans less than a millisecond, so at deep zoom the readout never
/// shows fewer decimals than the ruler labels — falling back to a sample
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
/// cap — the readout never shows fewer decimals than the ruler.
fn readout_decimals(step: f64, floor: usize, cap: usize) -> usize {
    if step > 0.0 {
        (-step.log10()).ceil().clamp(floor as f64, cap as f64) as usize
    } else {
        floor
    }
}

/// The cursor-readout form of a sample position on the beat grid:
/// `bar:beat` (plain beats without a grid) with two decimals, refined to the
/// view's pixel resolution (`beats_per_px`) up to the ruler labels' own
/// four-decimal cap, falling back to the sample count when the rate or tempo
/// is unknown.
pub(crate) fn readout_beats(
    sample: f64,
    sample_rate: f64,
    tempo: f64,
    beat_at: f64,
    quant: f64,
    beats_per_px: f64,
) -> String {
    if sample_rate <= 0.0 || tempo <= 0.0 {
        return readout_samples(sample);
    }
    let decimals = readout_decimals(beats_per_px, 2, 4);
    let beats = beat_at + sample.max(0.0) / sample_rate * tempo;
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
/// scale — round values the greedy spacing filter thins to what fits.
const DB_RUNGS: [f64; 17] = [
    -96.0, -72.0, -60.0, -48.0, -36.0, -30.0, -24.0, -18.0, -12.0, -10.0, -8.0, -6.0, -5.0, -4.0,
    -3.0, -2.0, -1.0, // 0 dB is handled as the walk's endpoint below.
];

/// The ticks of an amplitude ruler beside a waveform lane of `height_px`
/// device pixels, in the vertical unit `unit` (`Off`/`Hz` yield none), laid
/// out over the visible display window `[y_start, y_start + y_len)` of the
/// lane's vertical axis (`0, 1` = no zoom). The positions respect the same
/// `AMP_MARGIN` the waveform geometry applies, so a tick sits exactly on the
/// amplitude it names. The linear units (`Norm`, `Percent`, `Bits`) share the
/// 1-2-5 geometry and differ only in labels; `Db` walks the fixed dBFS rung
/// list outward from the (silence) center line, labeling the rungs that clear
/// one line of text and keeping crowded ones as minors.
pub(crate) fn amp_ticks(
    unit: RulerY,
    height_px: f64,
    bit_depth: u32,
    y_start: f64,
    y_len: f64,
    metrics: &Metrics,
) -> Vec<Tick> {
    let g = Gaps::of(metrics);
    if height_px <= 0.0 || y_len <= 0.0 {
        return Vec::new();
    }
    let margin = AMP_MARGIN as f64;
    // Absolute display coordinate of an amplitude (0 = lane bottom at no
    // zoom), then mapped through the visible window into the lane fraction.
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
                let label = (p - last_label >= g.label_v()).then(|| format!("{}", db as i64));
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
            let mut step = snap_125(g.label_v() / px_per_value).min(full);
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
            // 16-bit axis stepping 10000 still labels ±32768) and their label
            // clears the nearest stepped one by at least a line of text.
            let remainder = full - (full / step).floor() * step;
            if remainder > step * 1e-9 && remainder * px_per_value >= font::height(g.scale) as f64 {
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

/// The ticks of a plain linear **value axis** over `[lo, hi]` — any range, not
/// tied to an amplitude convention (no margin, no full-scale): the `plot`'s
/// vertical ruler for arbitrary numeric sequences. The step is the smallest
/// 1-2-5 rung whose labels (one line of text each) keep clear space over
/// `height_px`; majors are labeled with [`fmt_decimal`], minors (a fifth of
/// the step) appear when they clear the minimum tick gap. `frac` is 0 at `lo`
/// (the bottom), 1 at `hi`.
pub(crate) fn value_ticks(lo: f64, hi: f64, height_px: f64, metrics: &Metrics) -> Vec<Tick> {
    let span = hi - lo;
    if span <= 0.0 || height_px <= 0.0 {
        return Vec::new();
    }
    let g = Gaps::of(metrics);
    let px_per_value = height_px / span;
    let step = snap_125(g.label_v() / px_per_value);
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

/// Display coordinate `d` (0 = axis bottom, 1 = Nyquist) → frequency in Hz,
/// the exact mapping the spectrogram shader applies per scale — the single
/// inversion the ruler ticks and the cursor readout both use. `f_lo_norm` is
/// the shader's normalized log-axis floor (~20 Hz / Nyquist).
pub(crate) fn display_to_hz(d: f64, nyquist: f64, scale: FreqScale, f_lo_norm: f64) -> f64 {
    match scale {
        FreqScale::Linear => nyquist * d,
        FreqScale::Log => nyquist * f_lo_norm.clamp(1e-5, 0.5).powf(1.0 - d),
        FreqScale::Mel => mel_to_hz(d * hz_to_mel(nyquist)),
        FreqScale::Bark => {
            let z0 = hz_to_bark(0.0);
            bark_to_hz(z0 + d * (hz_to_bark(nyquist) - z0))
        }
    }
}

/// Frequency in Hz → display coordinate under `scale`, the inverse of
/// [`display_to_hz`] (the tick-placement direction).
pub(crate) fn hz_to_display(f: f64, nyquist: f64, scale: FreqScale, f_lo_norm: f64) -> f64 {
    match scale {
        FreqScale::Linear => f / nyquist,
        FreqScale::Log => {
            let f_lo = f_lo_norm.clamp(1e-5, 0.5);
            1.0 - (f / nyquist).ln() / f_lo.ln()
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
/// Nyquist at no zoom), matching the spectrogram shader's display→bin mapping
/// for `scale`. A **wide** window (a decade or more) uses the classic decade
/// scheme — 1/2/5 multiples labeled, the remaining integer multiples as
/// minors, thinned by the measured label height (the log and perceptual
/// scales compress unevenly). A **narrow** (zoomed) window, and the linear
/// axis at any zoom, fits a plain 1-2-5 ladder in hertz against its measured
/// labels, so zooming in keeps revealing finer round frequencies.
pub(crate) fn hz_ticks(
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
        // positions — the worst local gap decides.
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
/// device pixels — the spectral x ruler. The horizontal twin of [`hz_ticks`],
/// down to the two schemes it picks between: a **wide** window (a decade or
/// more, on a non-linear scale) walks the decades with 1/2/5 labeled, and a
/// **narrow** (zoomed) one — like the linear axis at any zoom — fits a plain
/// 1-2-5 ladder in hertz over what is actually visible, so zooming keeps
/// revealing finer round frequencies. What differs is only how a label is
/// measured: by its *width*, edge-clamped exactly as the renderer draws it,
/// since these sit side by side. `frac` is 0 at the window's left edge, 1 at
/// its right; `x_start = 0, x_len = 1` is the whole axis.
pub(crate) fn hz_ticks_h(
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
/// read-out tags it — three letters each, so the tag's footprint is constant
/// across scales.
pub(crate) fn scale_tag(scale: FreqScale) -> &'static str {
    match scale {
        FreqScale::Linear => "LIN",
        FreqScale::Log => "LOG",
        FreqScale::Mel => "MEL",
        FreqScale::Bark => "BRK",
    }
}

/// Draws the ticks of a horizontal ruler `strip` sitting under a view body:
/// a mark up against the body's bottom edge (taller when labeled), the label
/// centered under it and edge-clamped into the strip. The one drawing of the
/// x-ruler strip — the editor frames and the plot both call it.
pub(crate) fn draw_ticks_h(d: &mut Draw, strip: Rect, ticks: &[Tick]) {
    let (mesh, metrics, theme) = d.parts();
    let scale = metrics.caption_scale;
    for tick in ticks {
        let x = strip.x + strip.w * tick.frac as f32;
        let h = if tick.label.is_some() { 6.0 } else { 3.0 };
        mesh.rect(
            Rect::new(x, strip.y, metrics.divider_w, h),
            theme.ruler_line,
        );
        if let Some(label) = &tick.label {
            let w = font::width(label, scale);
            let lx = (x - w * 0.5).clamp(strip.x, (strip.x + strip.w - w).max(strip.x));
            font::text(mesh, label, lx, strip.y + 7.0, scale, theme.ruler_text);
        }
    }
}

/// Draws one lane's worth of vertical-ruler ticks into the strip left of the
/// body: tick marks against the body's left edge at `body_x` (longer when
/// labeled), labels right-aligned beside them and kept inside the strip
/// starting at `strip_x`. `frac` 0 is the lane's bottom. The one drawing of
/// the y-ruler strip, whatever the unit (amplitude, frequency, plain value).
/// The clear space `draw_ticks_v` leaves between a label's right edge and the
/// body it labels — so the width a strip must reserve is its widest label plus
/// this.
const LABEL_GAP: f32 = 10.0;

/// **The width a value strip has to be to draw `ticks` in full**: its widest
/// label plus [`LABEL_GAP`], or `0` when nothing is labelled.
///
/// This is what a fixed size role could not express. A label's width is a
/// property of the *data*: an axis over `[-1, 1]` formats `-1.0` and asks for
/// no more room than the role always gave it, while one over `[-0.1, 0.1]`
/// formats `-0.0625` and is clamped against the strip's edge unless it can ask
/// for the room to draw it. Callers take the role as the **floor**, so an axis
/// whose labels are narrow looks exactly as it always did.
pub(crate) fn ticks_width(ticks: &[Tick], metrics: &Metrics) -> f32 {
    let scale = metrics.caption_scale;
    let widest = ticks
        .iter()
        .filter_map(|t| t.label.as_deref())
        .map(|l| font::width(l, scale))
        .fold(0.0f32, f32::max);
    if widest > 0.0 {
        widest + LABEL_GAP
    } else {
        0.0
    }
}

/// The width a **value** strip over `[lo, hi]` asks for, never below the
/// `ruler_w` role: [`ticks_width`] of the ticks [`value_ticks`] will draw in a
/// lane `lane_h` px tall.
pub(crate) fn value_strip_w(lo: f64, hi: f64, lane_h: f32, metrics: &Metrics) -> f32 {
    let ticks = value_ticks(lo, hi, lane_h as f64, metrics);
    metrics.ruler_w.max(ticks_width(&ticks, metrics))
}

/// The width an **amplitude** strip asks for, never below the `ruler_w` role:
/// the same question as [`value_strip_w`] for the axis conventions
/// [`amp_ticks`] draws (decibels, normalized, percent, sample values).
pub(crate) fn amp_strip_w(
    unit: RulerY,
    lane_h: f32,
    bit_depth: u32,
    y_start: f64,
    y_len: f64,
    metrics: &Metrics,
) -> f32 {
    let ticks = amp_ticks(unit, lane_h as f64, bit_depth, y_start, y_len, metrics);
    metrics.ruler_w.max(ticks_width(&ticks, metrics))
}

pub(crate) fn draw_ticks_v(d: &mut Draw, body_x: f32, strip_x: f32, lane: Rect, ticks: &[Tick]) {
    if lane.h <= 4.0 {
        return;
    }
    let (mesh, metrics, theme) = d.parts();
    let scale = metrics.caption_scale;
    for tick in ticks {
        let y = lane.y + lane.h * (1.0 - tick.frac as f32);
        let w = if tick.label.is_some() { 8.0 } else { 4.0 };
        mesh.rect(
            Rect::new(body_x - w, y, w, metrics.divider_w),
            theme.ruler_line,
        );
        if let Some(label) = &tick.label {
            let lw = font::width(label, scale);
            let lx = (body_x - LABEL_GAP - lw).max(strip_x);
            let ty = (y - 3.0).clamp(lane.y, lane.y + lane.h - font::height(scale));
            font::text(mesh, label, lx, ty, scale, theme.ruler_text);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(ticks: &[Tick]) -> Vec<&str> {
        ticks.iter().filter_map(|t| t.label.as_deref()).collect()
    }

    /// A strip asks for what its own ticks need, floored by the role: an axis
    /// over `[-1, 1]` labels `-1.0` and is exactly as wide as it always was,
    /// while one over `[-0.1, 0.1]` labels `-0.0625` and asks for the room.
    #[test]
    fn a_value_strip_is_as_wide_as_its_own_labels() {
        let m = Metrics::default();
        let plain = value_strip_w(-1.0, 1.0, 200.0, &m);
        assert_eq!(plain, m.ruler_w, "the role holds an ordinary axis");

        let narrow = value_strip_w(-0.1, 0.1, 200.0, &m);
        assert!(
            narrow > m.ruler_w,
            "{narrow} did not grow past {}",
            m.ruler_w
        );
        // ...and it grew by exactly what the widest label needs to be drawn.
        let ticks = value_ticks(-0.1, 0.1, 200.0, &m);
        assert_eq!(narrow, ticks_width(&ticks, &m));
        let widest = ticks
            .iter()
            .filter_map(|t| t.label.as_deref())
            .max_by_key(|l| l.len())
            .unwrap();
        assert!(
            font::width(widest, m.caption_scale) + LABEL_GAP <= narrow,
            "'{widest}' still does not fit"
        );

        // Unlabelled ticks ask for nothing, so the role stands.
        assert_eq!(ticks_width(&[], &m), 0.0);
        assert_eq!(value_strip_w(0.0, 0.0, 200.0, &m), m.ruler_w);
    }

    /// The same rule on the amplitude conventions: an unzoomed axis is the
    /// role, a zoomed one is its labels.
    #[test]
    fn an_amp_strip_follows_its_window() {
        let m = Metrics::default();
        assert_eq!(
            amp_strip_w(RulerY::Norm, 200.0, 16, 0.0, 1.0, &m),
            m.ruler_w
        );
        assert!(amp_strip_w(RulerY::Norm, 200.0, 16, 0.4995, 0.001, &m) > m.ruler_w);
        // An axis that draws nothing keeps the role.
        assert_eq!(amp_strip_w(RulerY::Off, 200.0, 16, 0.0, 1.0, &m), m.ruler_w);
    }

    #[test]
    fn scale_tags_are_three_letters() {
        // The corner read-out slot is sized once: every tag is exactly three
        // characters, so switching scales never moves the chrome.
        for scale in [
            FreqScale::Linear,
            FreqScale::Log,
            FreqScale::Mel,
            FreqScale::Bark,
        ] {
            assert_eq!(scale_tag(scale).len(), 3, "{scale:?}");
        }
    }

    /// Recomputes the drawn label intervals of a horizontal ruler the way the
    /// frame renderer draws them (centered, edge-clamped) and asserts none
    /// overlap — the acceptance property.
    fn assert_no_h_collisions(ticks: &[Tick], width_px: f64, ctx: &str) {
        let mut spans: Vec<(f64, f64)> = ticks
            .iter()
            .filter_map(|t| {
                let label = t.label.as_deref()?;
                let w = font::width(label, Metrics::default().caption_scale) as f64;
                let x = t.frac * width_px;
                let lx = (x - w * 0.5).clamp(0.0, (width_px - w).max(0.0));
                Some((lx, lx + w))
            })
            .collect();
        spans.sort_by(|a, b| a.0.total_cmp(&b.0));
        for w in spans.windows(2) {
            assert!(
                w[1].0 >= w[0].1,
                "{ctx}: labels overlap at {:?} vs {:?}",
                w[0],
                w[1]
            );
        }
    }

    /// Asserts no two labels of a vertical ruler sit closer than one line of
    /// text (they are drawn at the tick's height).
    fn assert_no_v_collisions(ticks: &[Tick], height_px: f64, ctx: &str) {
        let mut ys: Vec<f64> = ticks
            .iter()
            .filter(|t| t.label.is_some())
            .map(|t| t.frac * height_px)
            .collect();
        ys.sort_by(f64::total_cmp);
        let line = font::height(Metrics::default().caption_scale) as f64;
        for w in ys.windows(2) {
            assert!(w[1] - w[0] >= line - 1e-9, "{ctx}: {w:?}");
        }
    }

    #[test]
    fn snap_follows_the_125_progression() {
        assert_eq!(snap_125(0.9), 1.0);
        assert_eq!(snap_125(1.2), 2.0);
        assert_eq!(snap_125(3.0), 5.0);
        assert_eq!(snap_125(7.0), 10.0);
        assert_eq!(snap_125(120.0), 200.0);
        assert_eq!(snap_125(0.03), 0.05);
    }

    #[test]
    fn time_ticks_pick_the_smallest_fitting_step() {
        // 10 s visible over 800 px at 48 kHz: one-second labels ("0".."10")
        // fit comfortably; half-second ones ("0.5", ...) collide.
        let ticks = time_ticks(
            0.0,
            480_000.0,
            800.0,
            48_000.0,
            TimeUnit::Seconds,
            &Metrics::default(),
        );
        assert!(!ticks.is_empty());
        for t in &ticks {
            assert!((0.0..=1.0).contains(&t.frac));
        }
        let l = labels(&ticks);
        assert!(
            l.contains(&"0") && l.contains(&"1") && l.contains(&"10"),
            "{l:?}"
        );
        assert_no_h_collisions(&ticks, 800.0, "10s/800px");
        // The same window on a narrow strip climbs the ladder.
        let narrow = time_ticks(
            0.0,
            480_000.0,
            160.0,
            48_000.0,
            TimeUnit::Seconds,
            &Metrics::default(),
        );
        assert!(labels(&narrow).len() < l.len());
        assert_no_h_collisions(&narrow, 160.0, "10s/160px");
    }

    #[test]
    fn time_ticks_fall_back_to_sample_counts() {
        let ticks = time_ticks(
            0.0,
            100_000.0,
            500.0,
            0.0,
            TimeUnit::Seconds,
            &Metrics::default(),
        );
        // No rate: labels are sample counts (compacted), on the 1-2-5 ladder.
        let l = labels(&ticks);
        assert!(l.contains(&"0") && l.contains(&"20K"), "{l:?}");
        let explicit = time_ticks(
            0.0,
            100_000.0,
            500.0,
            48_000.0,
            TimeUnit::Samples,
            &Metrics::default(),
        );
        assert_eq!(
            labels(&explicit),
            labels(&ticks),
            "samples mode ignores the rate"
        );
    }

    #[test]
    fn sample_steps_stay_integral() {
        // Zoomed to 30 samples over a wide strip: the step cannot go below 1,
        // so every label is a whole sample number.
        let ticks = time_ticks(
            100.0,
            30.0,
            1200.0,
            0.0,
            TimeUnit::Samples,
            &Metrics::default(),
        );
        let l = labels(&ticks);
        assert!(l.contains(&"100") && l.contains(&"130"), "{l:?}");
        for label in &l {
            assert!(label.parse::<i64>().is_ok(), "integral labels: {l:?}");
        }
    }

    #[test]
    fn time_labels_use_clock_form_and_millis() {
        assert_eq!(fmt_time(0.0, 1.0), "0");
        assert_eq!(fmt_time(75.0, 5.0), "1:15");
        assert_eq!(fmt_time(3723.0, 60.0), "1:02:03");
        assert_eq!(fmt_time(1.25, 0.05), "1.25");
        assert_eq!(fmt_time(0.005, 0.005), "0.005");
        assert_eq!(fmt_time(61.5, 0.5), "1:01.5");
    }

    #[test]
    fn zoomed_view_offsets_and_subdivides() {
        // A 1 s window starting at 2.5 s: sub-second majors from 2.5.
        let ticks = time_ticks(
            120_000.0,
            48_000.0,
            800.0,
            48_000.0,
            TimeUnit::Seconds,
            &Metrics::default(),
        );
        let l = labels(&ticks);
        assert!(l.contains(&"2.6") && l.contains(&"3.4"), "{l:?}");
        // Minors exist between majors (more ticks than labels).
        assert!(ticks.len() > l.len());
        assert_no_h_collisions(&ticks, 800.0, "zoomed 1s/800px");
    }

    #[test]
    fn beat_ticks_label_bars_and_beats_on_the_quant_grid() {
        // 8 beats visible over 800 px at 48 kHz, 2 beats/s (tempo 2.0):
        // one-beat labels fit. quant 4 -> bar:beat.
        let unit = TimeUnit::Beats {
            tempo: 2.0,
            beat_at: 0.0,
            quant: 4.0,
        };
        let ticks = time_ticks(0.0, 192_000.0, 800.0, 48_000.0, unit, &Metrics::default());
        let l = labels(&ticks);
        assert_eq!(l[0], "1:1");
        assert!(l.contains(&"1:3") && l.contains(&"2:1"), "{l:?}");
        // Minors (the binary subdivision) exist between majors.
        assert!(ticks.len() > l.len());
        for t in &ticks {
            assert!((0.0..=1.0).contains(&t.frac));
        }
        assert_no_h_collisions(&ticks, 800.0, "8 beats/800px");
    }

    #[test]
    fn beat_ladder_narrow_strip_climbs_to_bars() {
        // The same 8 beats on a strip too narrow for per-beat labels: the
        // ladder climbs to a bar-aligned step, so bar lines stay majors.
        let unit = TimeUnit::Beats {
            tempo: 2.0,
            beat_at: 0.0,
            quant: 4.0,
        };
        let ticks = time_ticks(0.0, 192_000.0, 120.0, 48_000.0, unit, &Metrics::default());
        let l = labels(&ticks);
        assert!(!l.is_empty());
        for label in &l {
            assert!(label.ends_with(":1"), "bar-aligned majors, got {l:?}");
        }
        assert_no_h_collisions(&ticks, 120.0, "8 beats/120px");
    }

    #[test]
    fn beat_steps_climb_the_musical_ladder() {
        // Binary fractions below a beat, then beats, then bar multiples;
        // every rung divides the next (a 3-beat bar skips the 2-beat rung).
        let steps = beat_steps(16.0, 800.0, 4.0, Gaps::of(&Metrics::default()));
        for w in steps.windows(2) {
            let ratio = w[1] / w[0];
            assert!(
                (ratio - ratio.round()).abs() < 1e-9,
                "every rung divides the next: {steps:?}"
            );
        }
        assert!(steps.contains(&1.0) && steps.contains(&2.0) && steps.contains(&4.0));
        let three = beat_steps(16.0, 800.0, 3.0, Gaps::of(&Metrics::default()));
        assert!(!three.contains(&2.0), "{three:?}");
        assert!(three.contains(&3.0), "{three:?}");
    }

    #[test]
    fn beats_without_rate_or_tempo_fall_back_to_samples() {
        let unit = TimeUnit::Beats {
            tempo: 0.0,
            beat_at: 0.0,
            quant: 4.0,
        };
        let ticks = time_ticks(0.0, 100_000.0, 500.0, 48_000.0, unit, &Metrics::default());
        assert_eq!(labels(&ticks)[0], "0");
        assert!(labels(&ticks).contains(&"20K"));
    }

    #[test]
    fn beat_offset_shifts_the_grid() {
        // beat_at 2 with quant 4: sample 0 is beat 2 (0-based) = bar 1 beat 3.
        let unit = TimeUnit::Beats {
            tempo: 2.0,
            beat_at: 2.0,
            quant: 4.0,
        };
        let ticks = time_ticks(0.0, 192_000.0, 800.0, 48_000.0, unit, &Metrics::default());
        let l = labels(&ticks);
        assert_eq!(l[0], "1:3", "{l:?}");
        assert!(l.contains(&"2:1"), "{l:?}");
    }

    #[test]
    fn readout_beats_reads_bar_beat() {
        // 48k samples at 2 beats/s = 2 beats in; quant 4 -> bar 1, beat 3.
        assert_eq!(
            readout_beats(48_000.0, 48_000.0, 2.0, 0.0, 4.0, 0.01),
            "1:3.00"
        );
        assert_eq!(
            readout_beats(48_000.0, 48_000.0, 2.0, 0.0, 0.0, 0.01),
            "2.00"
        );
        assert_eq!(readout_beats(48_000.0, 0.0, 2.0, 0.0, 4.0, 0.01), "48000");
        // Deep zoom (a pixel spans well under a hundredth of a beat): the
        // decimals refine with the view, up to the ruler's four-decimal cap.
        assert_eq!(
            readout_beats(48_000.0, 48_000.0, 2.0, 0.0, 4.0, 1e-3),
            "1:3.000"
        );
        assert_eq!(
            readout_beats(48_000.0, 48_000.0, 2.0, 0.0, 4.0, 1e-6),
            "1:3.0000"
        );
        // No pixel resolution known: the two-decimal floor.
        assert_eq!(
            readout_beats(48_000.0, 48_000.0, 2.0, 0.0, 4.0, 0.0),
            "1:3.00"
        );
    }

    #[test]
    fn readout_time_refines_with_the_pixel_resolution() {
        // Normal zoom (a pixel spans >= 1 ms): millisecond precision.
        assert_eq!(readout_time(24_000.0, 48_000.0, 0.01), "0.500");
        // Deep zoom (one 440 Hz cycle over ~600 px): finer than the ruler's
        // 4-decimal labels, never coarser.
        let secs_per_px = (1.0 / 440.0) / 600.0;
        assert_eq!(readout_time(48.0, 48_000.0, secs_per_px), "0.001000");
        // No pixel resolution known: the millisecond floor.
        assert_eq!(readout_time(24_000.0, 48_000.0, 0.0), "0.500");
        // No rate known: the sample count.
        assert_eq!(readout_time(24_000.0, 0.0, 0.01), "24000");
    }

    #[test]
    fn amp_ticks_norm_and_percent_share_the_geometry() {
        let norm = amp_ticks(RulerY::Norm, 400.0, 16, 0.0, 1.0, &Metrics::default());
        let pct = amp_ticks(RulerY::Percent, 400.0, 16, 0.0, 1.0, &Metrics::default());
        assert_eq!(norm.len(), pct.len());
        for (n, p) in norm.iter().zip(&pct) {
            assert!((n.frac - p.frac).abs() < 1e-12, "same positions");
        }
        // The extremes respect the waveform's vertical margin.
        let top = norm.iter().map(|t| t.frac).fold(0.0, f64::max);
        assert!((top - (1.0 + AMP_MARGIN as f64) / 2.0).abs() < 1e-9);
        let l = labels(&norm);
        assert!(
            l.contains(&"1") && l.contains(&"0") && l.contains(&"-1"),
            "{l:?}"
        );
        // Percent is signed: 0..100% above the zero line, 0..-100% below it.
        let p = labels(&pct);
        assert!(p.contains(&"100%") && p.contains(&"-100%"), "{p:?}");
        assert_no_v_collisions(&norm, 400.0, "norm/400px");
    }

    #[test]
    fn amp_ticks_bits_step_in_sample_values_and_keep_the_endpoints() {
        let bits = amp_ticks(RulerY::Bits, 400.0, 16, 0.0, 1.0, &Metrics::default());
        let l = labels(&bits);
        // 1-2-5 steps in integer sample values, compacted...
        assert!(l.contains(&"0") && l.contains(&"10K"), "{l:?}");
        // ...plus the full-scale endpoints the decimal step misses.
        assert!(l.contains(&"-32768") && l.contains(&"32768"), "{l:?}");
        // A different bit depth rescales the axis.
        let eight = amp_ticks(RulerY::Bits, 400.0, 8, 0.0, 1.0, &Metrics::default());
        assert!(labels(&eight).contains(&"-128"), "{:?}", labels(&eight));
    }

    #[test]
    fn amp_ticks_db_ladder_is_mirrored_and_placed_by_amplitude() {
        let ticks = amp_ticks(RulerY::Db, 600.0, 16, 0.0, 1.0, &Metrics::default());
        // The center line is the -inf mark.
        assert_eq!(ticks[0].frac, 0.5);
        assert_eq!(ticks[0].label.as_deref(), Some("-INF"));
        // 0 dB sits at full scale (frac = (1 + margin)/2), mirrored below.
        let full = (1.0 + AMP_MARGIN as f64) / 2.0;
        let zero: Vec<&Tick> = ticks
            .iter()
            .filter(|t| t.label.as_deref() == Some("0"))
            .collect();
        assert_eq!(zero.len(), 2);
        assert!((zero[0].frac - full).abs() < 1e-9);
        assert!((zero[1].frac - (1.0 - full)).abs() < 1e-9);
        // -6 dB sits at amplitude 10^(-6/20) of the margin, not halfway.
        let m6 = ticks
            .iter()
            .find(|t| t.label.as_deref() == Some("-6"))
            .unwrap();
        let expected = (10f64.powf(-6.0 / 20.0) * AMP_MARGIN as f64 + 1.0) / 2.0;
        assert!((m6.frac - expected).abs() < 1e-9);
        assert_no_v_collisions(&ticks, 600.0, "db/600px");
    }

    #[test]
    fn amp_zoom_window_reveals_finer_steps_and_names_whats_on_screen() {
        // Zoom into the top of the norm axis: the visible amplitudes span
        // roughly [0.6, 1.09]; the step refines below the full-view 0.1 and
        // every tick names an amplitude inside the window.
        let full_view = amp_ticks(RulerY::Norm, 400.0, 16, 0.0, 1.0, &Metrics::default());
        let zoomed = amp_ticks(RulerY::Norm, 400.0, 16, 0.8, 0.2, &Metrics::default());
        let parse = |t: &Tick| t.label.as_deref().unwrap().parse::<f64>().unwrap();
        let step_full = parse(&full_view[1]) - parse(&full_view[0]);
        let z: Vec<f64> = zoomed.iter().map(parse).collect();
        assert!(!z.is_empty());
        let step_zoom = z[1] - z[0];
        assert!(step_zoom < step_full, "{step_zoom} vs {step_full}");
        let margin = AMP_MARGIN as f64;
        for (tick, v) in zoomed.iter().zip(&z) {
            // Each tick sits exactly where its amplitude maps in the window.
            let expected = ((v * margin + 1.0) / 2.0 - 0.8) / 0.2;
            assert!((tick.frac - expected).abs() < 1e-9, "{v} at {}", tick.frac);
        }
        assert_no_v_collisions(&zoomed, 400.0, "norm zoomed");
        // The dB axis zoomed into the top reveals the fine (-1, -2) rungs the
        // full view drops on a short lane.
        let db_short = amp_ticks(RulerY::Db, 150.0, 16, 0.0, 1.0, &Metrics::default());
        let db_zoom = amp_ticks(RulerY::Db, 150.0, 16, 0.85, 0.15, &Metrics::default());
        assert!(
            !labels(&db_short).contains(&"-1"),
            "{:?}",
            labels(&db_short)
        );
        assert!(labels(&db_zoom).contains(&"-1"), "{:?}", labels(&db_zoom));
        assert_no_v_collisions(&db_zoom, 150.0, "db zoomed");
    }

    #[test]
    fn amp_readout_formats_per_unit() {
        assert_eq!(readout_amp(0.5, RulerY::Norm, 16, 0.01), "+0.50");
        assert_eq!(readout_amp(0.5, RulerY::Db, 16, 0.01), "-6.0 DB");
        assert_eq!(readout_amp(0.0, RulerY::Db, 16, 0.01), "-INF DB");
        assert_eq!(readout_amp(-0.5, RulerY::Bits, 16, 0.01), "-16384");
        assert_eq!(readout_amp(0.5, RulerY::Percent, 16, 0.01), "50%");
        assert_eq!(readout_amp(-0.5, RulerY::Percent, 16, 0.01), "-50%");
    }

    #[test]
    fn amp_readout_refines_with_the_pixel_resolution() {
        // Deep vertical zoom: the linear units refine to what a pixel
        // resolves, so the readout never shows fewer decimals than the
        // zoomed ruler labels.
        assert_eq!(readout_amp(0.5, RulerY::Norm, 16, 1e-4), "+0.5000");
        assert_eq!(readout_amp(0.5, RulerY::Percent, 16, 1e-4), "50.00%");
        // No pixel resolution known: the units' base precision.
        assert_eq!(readout_amp(0.5, RulerY::Norm, 16, 0.0), "+0.50");
        assert_eq!(readout_amp(0.5, RulerY::Percent, 16, 0.0), "50%");
        // dB keeps its tenth regardless (its ruler labels whole rungs).
        assert_eq!(readout_amp(0.5, RulerY::Db, 16, 1e-4), "-6.0 DB");
    }

    #[test]
    fn log_hz_ticks_match_the_shader_geometry() {
        // 48 kHz => Nyquist 24 kHz, f_lo = 20/24000.
        let nyq = 24_000.0;
        let f_lo = 20.0 / nyq;
        let ticks = hz_ticks(
            nyq,
            FreqScale::Log,
            f_lo,
            600.0,
            0.0,
            1.0,
            &Metrics::default(),
        );
        let l = labels(&ticks);
        for expected in ["100", "1K", "10K", "2K", "50"] {
            assert!(l.contains(&expected), "missing {expected} in {l:?}");
        }
        // The 1 kHz tick sits where the shader puts 1 kHz: d solves
        // f_lo^(1 - d) = f/nyq.
        let one_k = ticks
            .iter()
            .find(|t| t.label.as_deref() == Some("1K"))
            .unwrap();
        let expected_d = 1.0 - (1000.0f64 / nyq).ln() / f_lo.ln();
        assert!((one_k.frac - expected_d).abs() < 1e-9);
        // Everything stays inside the axis.
        for t in &ticks {
            assert!((0.0..=1.0).contains(&t.frac));
        }
        assert_no_v_collisions(&ticks, 600.0, "log/600px");
    }

    #[test]
    fn linear_hz_ticks_are_evenly_spaced() {
        let ticks = hz_ticks(
            24_000.0,
            FreqScale::Linear,
            20.0 / 24_000.0,
            600.0,
            0.0,
            1.0,
            &Metrics::default(),
        );
        let l = labels(&ticks);
        assert_eq!(l.first(), Some(&"0"));
        let majors: Vec<&Tick> = ticks.iter().filter(|t| t.label.is_some()).collect();
        let d01 = majors[1].frac - majors[0].frac;
        let d12 = majors[2].frac - majors[1].frac;
        assert!((d01 - d12).abs() < 1e-9, "even spacing on the linear axis");
        assert_no_v_collisions(&ticks, 600.0, "linear/600px");
    }

    #[test]
    fn mel_and_bark_ticks_sit_on_the_perceptual_mapping() {
        let nyq = 24_000.0;
        for scale in [FreqScale::Mel, FreqScale::Bark] {
            let ticks = hz_ticks(nyq, scale, 20.0 / nyq, 600.0, 0.0, 1.0, &Metrics::default());
            let l = labels(&ticks);
            assert_eq!(l.first(), Some(&"0"));
            assert!(l.contains(&"1K"), "{scale:?}: {l:?}");
            // The 1 kHz tick inverts back to 1 kHz through the shared
            // display mapping (the shader's geometry).
            let one_k = ticks
                .iter()
                .find(|t| t.label.as_deref() == Some("1K"))
                .unwrap();
            let f = display_to_hz(one_k.frac, nyq, scale, 20.0 / nyq);
            assert!((f - 1000.0).abs() < 1e-6, "{scale:?}: {f}");
            for t in &ticks {
                assert!((0.0..=1.0).contains(&t.frac));
            }
            assert_no_v_collisions(&ticks, 600.0, &format!("{scale:?}/600px"));
        }
    }

    #[test]
    fn hz_zoom_window_reveals_sub_decade_ticks() {
        // Zoom the log axis into the octave around 1-2 kHz: the window is
        // narrower than a decade, so the ladder switches to round hertz steps
        // finer than the decade scheme's 1/2/5.
        let nyq = 24_000.0;
        let f_lo = 20.0 / nyq;
        let d1 = hz_to_display(1_000.0, nyq, FreqScale::Log, f_lo);
        let d2 = hz_to_display(2_000.0, nyq, FreqScale::Log, f_lo);
        let ticks = hz_ticks(
            nyq,
            FreqScale::Log,
            f_lo,
            600.0,
            d1,
            d2 - d1,
            &Metrics::default(),
        );
        let l = labels(&ticks);
        assert!(l.contains(&"1.1K") || l.contains(&"1.2K"), "{l:?}");
        // Every label names a frequency inside the window, exactly on the
        // shader mapping.
        for t in ticks.iter().filter(|t| t.label.is_some()) {
            let f = display_to_hz(d1 + t.frac * (d2 - d1), nyq, FreqScale::Log, f_lo);
            assert!((999.0..=2001.0).contains(&f), "{f}");
        }
        assert_no_v_collisions(&ticks, 600.0, "log zoomed");
    }

    #[test]
    fn mel_display_mapping_round_trips() {
        // display_to_hz and hz_to_display invert each other on every scale.
        let nyq = 22_050.0;
        for scale in [
            FreqScale::Linear,
            FreqScale::Log,
            FreqScale::Mel,
            FreqScale::Bark,
        ] {
            for d in [0.1, 0.5, 0.9] {
                let f = display_to_hz(d, nyq, scale, 20.0 / nyq);
                let back = hz_to_display(f, nyq, scale, 20.0 / nyq);
                assert!((back - d).abs() < 1e-9, "{scale:?} at {d}");
            }
        }
    }

    #[test]
    fn no_labels_collide_at_any_window_zoom_unit_or_strip_size() {
        // The acceptance property, swept: every unit, at several strip sizes,
        // windows and zooms (including degenerate slivers), never lays out
        // two overlapping labels.
        let widths = [40.0, 90.0, 160.0, 320.0, 800.0, 2000.0];
        let windows = [
            (0.0, 480_000.0),
            (120_000.0, 48_000.0),
            (999.0, 77.0),
            (1_234_567.0, 3_456_789.0),
            (47_999.0, 2.0),
        ];
        for &w in &widths {
            for &(start, len) in &windows {
                for unit in [
                    TimeUnit::Seconds,
                    TimeUnit::Samples,
                    TimeUnit::Beats {
                        tempo: 2.5,
                        beat_at: 1.0,
                        quant: 3.0,
                    },
                ] {
                    let ticks = time_ticks(start, len, w, 48_000.0, unit, &Metrics::default());
                    assert_no_h_collisions(&ticks, w, &format!("{unit:?} {start}+{len} @{w}px"));
                }
            }
        }
        let heights = [30.0, 80.0, 150.0, 400.0, 1200.0];
        let y_windows = [(0.0, 1.0), (0.5, 0.5), (0.8, 0.2), (0.45, 0.02), (0.0, 0.3)];
        for &h in &heights {
            for &(y0, ylen) in &y_windows {
                for unit in [RulerY::Norm, RulerY::Db, RulerY::Bits, RulerY::Percent] {
                    let ticks = amp_ticks(unit, h, 16, y0, ylen, &Metrics::default());
                    assert_no_v_collisions(&ticks, h, &format!("{unit:?} y{y0}+{ylen} @{h}px"));
                }
                for scale in [
                    FreqScale::Linear,
                    FreqScale::Log,
                    FreqScale::Mel,
                    FreqScale::Bark,
                ] {
                    let ticks = hz_ticks(
                        24_000.0,
                        scale,
                        20.0 / 24_000.0,
                        h,
                        y0,
                        ylen,
                        &Metrics::default(),
                    );
                    assert_no_v_collisions(&ticks, h, &format!("{scale:?} y{y0}+{ylen} @{h}px"));
                    for t in &ticks {
                        assert!((0.0..=1.0).contains(&t.frac));
                    }
                }
            }
        }
    }

    #[test]
    fn value_ticks_fit_any_range() {
        // A non-normalized sequence range (say pwhite over [40, 4700]): round
        // 1-2-5 steps, labels inside the range, no collisions.
        let ticks = value_ticks(40.0, 4700.0, 400.0, &Metrics::default());
        let l = labels(&ticks);
        assert!(!l.is_empty());
        assert!(l.contains(&"1000") || l.contains(&"500"), "{l:?}");
        for t in &ticks {
            assert!((0.0..=1.0).contains(&t.frac));
        }
        assert_no_v_collisions(&ticks, 400.0, "value 40..4700");
        // A tiny fractional range refines below 1.
        let fine = value_ticks(-0.02, 0.03, 400.0, &Metrics::default());
        let fl = labels(&fine);
        assert!(
            fl.contains(&"0") && fl.iter().any(|s| s.contains('.')),
            "{fl:?}"
        );
        assert_no_v_collisions(&fine, 400.0, "value -0.02..0.03");
        // Degenerate span: nothing.
        assert!(value_ticks(1.0, 1.0, 400.0, &Metrics::default()).is_empty());
        assert!(value_ticks(0.0, 1.0, 0.0, &Metrics::default()).is_empty());
    }

    #[test]
    fn readout_value_resolves_a_thousandth_of_the_span() {
        assert_eq!(readout_value(0.5, 2.0), "0.5");
        assert_eq!(readout_value(1234.0, 5000.0), "1234");
        assert_eq!(readout_value(0.1234567, 0.001), "0.123457");
        assert_eq!(readout_value(-0.25, 2.0), "-0.25");
    }

    #[test]
    fn horizontal_hz_ticks_fit_by_label_width() {
        let nyq = 24_000.0;
        let f_lo = 20.0 / nyq;
        for scale in [
            FreqScale::Linear,
            FreqScale::Log,
            FreqScale::Mel,
            FreqScale::Bark,
        ] {
            let ticks = hz_ticks_h(nyq, scale, f_lo, 800.0, 0.0, 1.0, &Metrics::default());
            let l = labels(&ticks);
            assert!(l.contains(&"1K") || l.contains(&"2K"), "{scale:?}: {l:?}");
            // Every label sits exactly on the shared display mapping.
            for t in ticks.iter().filter(|t| t.label.is_some()) {
                let f = display_to_hz(t.frac, nyq, scale, f_lo);
                let named = t.label.as_deref().unwrap();
                let parsed = if let Some(k) = named.strip_suffix('K') {
                    k.parse::<f64>().unwrap() * 1000.0
                } else {
                    named.parse::<f64>().unwrap()
                };
                assert!(
                    (f - parsed).abs() <= parsed.max(1.0) * 1e-3 + 0.5,
                    "{scale:?}: tick {named} at {f} Hz"
                );
            }
            assert_no_h_collisions(&ticks, 800.0, &format!("{scale:?} horizontal"));
            // A narrow strip keeps fewer labels, still collision-free.
            let narrow = hz_ticks_h(nyq, scale, f_lo, 120.0, 0.0, 1.0, &Metrics::default());
            assert!(labels(&narrow).len() <= l.len());
            assert_no_h_collisions(&narrow, 120.0, &format!("{scale:?} narrow"));
        }
        assert!(
            hz_ticks_h(
                0.0,
                FreqScale::Log,
                0.001,
                800.0,
                0.0,
                1.0,
                &Metrics::default()
            )
            .is_empty()
        );
    }

    /// Zooming a frequency x axis reveals **finer** round frequencies inside
    /// the window and drops everything outside it — the property L8 fixed for
    /// every other axis, now that this one navigates too. The vertical twin
    /// has had it since the spectrogram's frequency window; this is the same
    /// rule read across the strip instead of up it.
    #[test]
    fn a_zoomed_horizontal_hz_ruler_reveals_finer_rungs() {
        let (nyq, f_lo, m) = (24_000.0, 20.0 / 24_000.0, Metrics::default());
        for scale in [
            FreqScale::Linear,
            FreqScale::Log,
            FreqScale::Mel,
            FreqScale::Bark,
        ] {
            let full = hz_ticks_h(nyq, scale, f_lo, 800.0, 0.0, 1.0, &m);
            // A tenth of the axis around the middle: whatever hertz that is
            // per scale, the ruler must name frequencies inside it and only
            // inside it, without colliding.
            let (x0, x_len) = (0.45, 0.1);
            let zoomed = hz_ticks_h(nyq, scale, f_lo, 800.0, x0, x_len, &m);
            assert!(!zoomed.is_empty(), "{scale:?}: a zoomed window has ticks");
            assert_no_h_collisions(&zoomed, 800.0, &format!("{scale:?} zoomed"));
            let lo = display_to_hz(x0, nyq, scale, f_lo);
            let hi = display_to_hz(x0 + x_len, nyq, scale, f_lo);
            for t in &zoomed {
                let f = display_to_hz(x0 + t.frac * x_len, nyq, scale, f_lo);
                assert!(
                    f >= lo - (hi - lo) * 1e-6 && f <= hi + (hi - lo) * 1e-6,
                    "{scale:?}: {f} Hz drawn outside [{lo}, {hi}]"
                );
            }
            // Finer, measured where the comparison is fair — inside the
            // window, which is the only place both rulers describe. (A
            // nonlinear axis packs its bottom decade tightly, so the smallest
            // gap over the *whole* ruler says nothing about the zoom.)
            let named = |ticks: &[Tick], start: f64, len: f64| -> Vec<f64> {
                ticks
                    .iter()
                    .filter(|t| t.label.is_some())
                    .map(|t| display_to_hz(start + t.frac * len, nyq, scale, f_lo))
                    .filter(|f| *f >= lo && *f <= hi)
                    .collect()
            };
            let before = named(&full, 0.0, 1.0);
            let after = named(&zoomed, x0, x_len);
            assert!(
                after.len() > before.len(),
                "{scale:?}: the window names {} frequencies zoomed in, {} zoomed out",
                after.len(),
                before.len()
            );
        }
        // An empty window draws nothing rather than dividing by zero.
        assert!(hz_ticks_h(24_000.0, FreqScale::Log, 1e-3, 800.0, 0.0, 0.0, &m).is_empty());
    }

    #[test]
    fn degenerate_inputs_yield_no_ticks() {
        assert!(
            time_ticks(
                0.0,
                0.0,
                800.0,
                48_000.0,
                TimeUnit::Seconds,
                &Metrics::default()
            )
            .is_empty()
        );
        assert!(
            time_ticks(
                0.0,
                100.0,
                0.0,
                48_000.0,
                TimeUnit::Seconds,
                &Metrics::default()
            )
            .is_empty()
        );
        assert!(
            hz_ticks(
                0.0,
                FreqScale::Log,
                0.001,
                600.0,
                0.0,
                1.0,
                &Metrics::default()
            )
            .is_empty()
        );
        assert!(
            hz_ticks(
                24_000.0,
                FreqScale::Linear,
                0.001,
                0.0,
                0.0,
                1.0,
                &Metrics::default()
            )
            .is_empty()
        );
        assert!(
            hz_ticks(
                24_000.0,
                FreqScale::Log,
                0.001,
                600.0,
                0.5,
                0.0,
                &Metrics::default()
            )
            .is_empty()
        );
        assert!(amp_ticks(RulerY::Norm, 0.0, 16, 0.0, 1.0, &Metrics::default()).is_empty());
        assert!(amp_ticks(RulerY::Norm, 400.0, 16, 0.5, 0.0, &Metrics::default()).is_empty());
        assert!(amp_ticks(RulerY::Off, 400.0, 16, 0.0, 1.0, &Metrics::default()).is_empty());
    }
}
