//! Adaptive ruler tick math for the editor-grade views — pure, display-only.
//!
//! A time axis under the waveform/spectrogram, an amplitude axis beside the
//! waveform and a frequency axis beside the spectrogram need tick positions
//! and labels that stay legible at any zoom. The math is classic editor
//! chrome: a 1-2-5 progression snapped to the decade matching the visible
//! span (time in seconds or samples; amplitude in normalized/percent/integer
//! sample units), a binary/bar ladder on the musical `beats` axis (labels
//! `bar:beat` off the client's quant grid, via the shared
//! `clausters_core::tempoclock::bar`/`beat_in_bar`), a fixed mirrored dB
//! ladder on the dBFS amplitude axis, and decade ticks on the frequency axis
//! — placed with the **identical** display→bin geometry the spectrogram
//! shader uses (linear, log, mel or bark; the perceptual forms from
//! `clausters_core::scale`), so a tick labeled 1 kHz sits exactly on the
//! 1 kHz row of pixels. No GPU, no widget types: positions come out as
//! fractions of the visible span, and the frame renderer turns them into
//! painter geometry.

use clausters_core::scale::{bark_to_hz, hz_to_bark, hz_to_mel, mel_to_hz};
use clausters_core::tempoclock;

use crate::spectrogram::FreqScale;
use crate::waveform::AMP_MARGIN;

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

/// Target device pixels between labeled (major) time ticks.
const MAJOR_PX: f64 = 90.0;
/// Minimum device pixels between minor ticks for them to be drawn at all.
const MINOR_PX: f64 = 9.0;
/// Target device pixels between labeled amplitude ticks (vertical axes are
/// shorter, especially per lane, so they aim denser than the time axis).
const AMP_MAJOR_PX: f64 = 36.0;
/// Minimum device pixels between labels on the greedy (dB, mel/bark) axes.
const LABEL_MIN_PX: f64 = 22.0;

/// The ticks of a time ruler spanning samples `[start, start + len)` over
/// `width_px` device pixels. With `TimeUnit::Seconds` (and a positive
/// `sample_rate`) the steps and labels are in clock time; with
/// `TimeUnit::Beats` on the musical grid (labels `bar:beat`); otherwise in
/// sample counts. Major ticks carry labels; minors appear only when they are
/// at least a few pixels apart.
pub(crate) fn time_ticks(
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
    } = unit
        && sample_rate > 0.0
        && tempo > 0.0
    {
        return beat_ticks(start, len, width_px, sample_rate, tempo, beat_at, quant);
    }
    let seconds = unit == TimeUnit::Seconds && sample_rate > 0.0;
    let (axis_start, axis_len) = if seconds {
        (start / sample_rate, len / sample_rate)
    } else {
        (start, len)
    };
    let mut step = snap_125(axis_len * MAJOR_PX / width_px);
    if !seconds {
        step = step.max(1.0); // samples are integral
    }
    let minor = step / 5.0;
    let draw_minors = minor / axis_len * width_px >= MINOR_PX;
    let fine = if draw_minors { minor } else { step };

    let mut out = Vec::new();
    let mut k = (axis_start / fine).ceil() as i64;
    loop {
        let t = k as f64 * fine;
        if t > axis_start + axis_len + fine * 1e-9 {
            break;
        }
        let major = (t / step - (t / step).round()).abs() < 1e-6;
        let label = major.then(|| {
            if seconds {
                fmt_time(t, step)
            } else {
                fmt_samples(t)
            }
        });
        out.push(Tick {
            frac: ((t - axis_start) / axis_len).clamp(0.0, 1.0),
            label,
        });
        k += 1;
    }
    out
}

/// The `beats` form of the time ruler: the axis converted to beat positions,
/// the step snapped to the musical ladder (binary fractions of a beat, whole
/// beats, bars and powers-of-two bars), majors labeled `bar:beat` (1-based)
/// off the quant grid, minors on the binary subdivision.
fn beat_ticks(
    start: f64,
    len: f64,
    width_px: f64,
    rate: f64,
    tempo: f64,
    beat_at: f64,
    quant: f64,
) -> Vec<Tick> {
    let b0 = beat_at + start / rate * tempo;
    let blen = len / rate * tempo;
    if blen <= 0.0 {
        return Vec::new();
    }
    let step = beat_step(blen * MAJOR_PX / width_px, quant);
    let minor = step / 2.0;
    let draw_minors = minor / blen * width_px >= MINOR_PX;
    let fine = if draw_minors { minor } else { step };

    let mut out = Vec::new();
    let mut k = (b0 / fine).ceil() as i64;
    loop {
        let b = k as f64 * fine;
        if b > b0 + blen + fine * 1e-9 {
            break;
        }
        let major = (b / step - (b / step).round()).abs() < 1e-6;
        out.push(Tick {
            frac: ((b - b0) / blen).clamp(0.0, 1.0),
            label: major.then(|| fmt_bar_beat(b, quant, step)),
        });
        k += 1;
    }
    out
}

/// Snap a raw beat step up the musical ladder: binary fractions of a beat
/// below 1, whole-beat divisors of the bar inside it, then bars and
/// powers-of-two bars. Every rung divides the next, so bar boundaries are
/// always majors.
fn beat_step(raw: f64, quant: f64) -> f64 {
    let raw = raw.max(f64::MIN_POSITIVE);
    if raw <= 1.0 {
        // 1, 1/2, 1/4, ... — the binary subdivision grid.
        return 2f64.powf(raw.log2().ceil()).min(1.0);
    }
    if quant > 1.0 {
        if raw <= quant {
            // Whole beats inside the bar: 2 beats only when it divides the
            // bar (keeps bar lines on majors), else jump straight to the bar.
            if raw <= 2.0 && quant % 2.0 == 0.0 {
                return 2.0;
            }
            return quant;
        }
        // Whole bars, doubling.
        return quant * 2f64.powf((raw / quant).log2().ceil());
    }
    // No bar grid: plain powers of two of a beat.
    2f64.powf(raw.log2().ceil())
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
/// seconds below, with just enough decimals for `step` (up to milliseconds).
fn fmt_time(secs: f64, step: f64) -> String {
    let decimals = if step >= 1.0 {
        0
    } else {
        (-step.log10()).ceil().clamp(1.0, 3.0) as usize
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

/// The cursor-readout form of a sample position in clock time (millisecond
/// precision), falling back to a sample count when no rate is known.
pub(crate) fn readout_time(sample: f64, sample_rate: f64) -> String {
    if sample_rate > 0.0 {
        fmt_time(sample.max(0.0) / sample_rate, 0.001)
    } else {
        readout_samples(sample)
    }
}

/// The cursor-readout form of a sample position as a plain sample count.
pub(crate) fn readout_samples(sample: f64) -> String {
    format!("{}", sample.max(0.0).round() as i64)
}

/// The cursor-readout form of a sample position on the beat grid:
/// `bar:beat` with two decimals (plain beats without a grid), falling back
/// to the sample count when the rate or tempo is unknown.
pub(crate) fn readout_beats(
    sample: f64,
    sample_rate: f64,
    tempo: f64,
    beat_at: f64,
    quant: f64,
) -> String {
    if sample_rate <= 0.0 || tempo <= 0.0 {
        return readout_samples(sample);
    }
    let beats = beat_at + sample.max(0.0) / sample_rate * tempo;
    if quant <= 0.0 {
        return format!("{beats:.2}");
    }
    let bar = tempoclock::bar(beats, quant) + 1.0;
    let beat = tempoclock::beat_in_bar(beats, quant) + 1.0;
    format!("{}:{beat:.2}", bar as i64)
}

/// The cursor-readout form of a normalized amplitude in the vertical-ruler
/// unit (`Norm` for the unit-less kinds).
pub(crate) fn readout_amp(amp: f64, unit: RulerY, bit_depth: u32) -> String {
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
        RulerY::Percent => format!("{:.0}%", amp * 100.0),
        _ => format!("{amp:+.2}"),
    }
}

/// The ticks of an amplitude ruler beside a waveform lane of `height_px`
/// device pixels, in the vertical unit `unit` (`Off`/`Hz` yield none). The
/// positions respect the same `AMP_MARGIN` the waveform geometry applies, so
/// a tick sits exactly on the amplitude it names. The linear units (`Norm`,
/// `Percent`, `Bits`) share the 1-2-5 geometry and differ only in labels;
/// `Db` places the fixed dBFS ladder mirrored about the (silence) center
/// line, dropping rungs that crowd.
pub(crate) fn amp_ticks(unit: RulerY, height_px: f64, bit_depth: u32) -> Vec<Tick> {
    if height_px <= 0.0 {
        return Vec::new();
    }
    let margin = AMP_MARGIN as f64;
    // frac 0 = bottom of the lane, 1 = top; amplitude in [-1, 1].
    let frac_of = |amp: f64| (amp * margin + 1.0) / 2.0;
    match unit {
        RulerY::Off | RulerY::Hz => Vec::new(),
        RulerY::Db => {
            let mut out = vec![Tick {
                frac: 0.5,
                label: Some("-INF".to_string()),
            }];
            let mut last = 0.5; // distance filter, walking outward from center
            for db in [-60.0, -48.0, -36.0, -24.0, -18.0, -12.0, -6.0, 0.0] {
                let amp = 10f64.powf(db / 20.0);
                let f = frac_of(amp);
                if (f - last) * height_px < MINOR_PX {
                    continue;
                }
                let label = ((f - last) * height_px >= LABEL_MIN_PX || db == 0.0)
                    .then(|| format!("{}", db as i64));
                for frac in [f, 1.0 - f] {
                    out.push(Tick {
                        frac,
                        label: label.clone(),
                    });
                }
                last = f;
            }
            out
        }
        _ => {
            let full = if unit == RulerY::Bits {
                2f64.powi(bit_depth.saturating_sub(1) as i32)
            } else {
                1.0
            };
            let label_of = |v: f64| match unit {
                // Signed: 0..100% above the zero line, 0..-100% below it.
                RulerY::Percent => format!("{:.0}%", v / full * 100.0),
                RulerY::Bits => fmt_samples(v),
                _ => trim_decimal(v),
            };
            let step = snap_125(2.0 * full * AMP_MAJOR_PX / height_px).min(full);
            let mut out = Vec::new();
            let mut k = (-full / step).ceil() as i64;
            while k as f64 * step <= full + step * 1e-9 {
                let v = k as f64 * step;
                out.push(Tick {
                    frac: frac_of((v / full).clamp(-1.0, 1.0)),
                    label: Some(label_of(v)),
                });
                k += 1;
            }
            // The full-scale endpoints when the decimal step misses them (a
            // 16-bit axis stepping 10000 still labels ±32768) and there is
            // room left over the last stepped tick.
            let remainder = full - (full / step).floor() * step;
            if remainder > step * 1e-9 && remainder / (2.0 * full) * margin * height_px >= MINOR_PX
            {
                for sign in [-1.0, 1.0] {
                    out.push(Tick {
                        frac: frac_of(sign),
                        label: Some(label_of(sign * full)),
                    });
                }
            }
            out
        }
    }
}

/// A short decimal label with trailing zeros trimmed (`0.5`, `-0.25`, `1`).
fn trim_decimal(v: f64) -> String {
    let s = format!("{v:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" {
        "0".to_string()
    } else {
        s.to_string()
    }
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
fn hz_to_display(f: f64, nyquist: f64, scale: FreqScale, f_lo_norm: f64) -> f64 {
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

/// The ticks of a frequency ruler over the full display axis (bottom = the
/// axis floor, top = Nyquist), matching the spectrogram shader's display→bin
/// mapping for `scale`. On the **log** axis (`f_lo_norm` = the shader's
/// normalized ~20 Hz floor) ticks sit on the 1/2/5 multiples of each decade,
/// labeled, with the remaining integer multiples as minors. On the **linear**
/// axis it is the same 1-2-5 progression as the time ruler, over `height_px`.
/// The **mel**/**bark** axes place the same decade candidates through the
/// perceptual mapping, greedily dropping ticks that crowd (the scales
/// compress both ends unevenly).
pub(crate) fn hz_ticks(
    nyquist: f64,
    scale: FreqScale,
    f_lo_norm: f64,
    height_px: f64,
) -> Vec<Tick> {
    if nyquist <= 0.0 || height_px <= 0.0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    match scale {
        FreqScale::Log => {
            let f_lo = (f_lo_norm.clamp(1e-5, 0.5)) * nyquist;
            let mut decade = 10f64.powf(f_lo.log10().floor());
            while decade <= nyquist {
                for mult in 1..10 {
                    let f = decade * mult as f64;
                    if f < f_lo || f > nyquist {
                        continue;
                    }
                    let d = hz_to_display(f, nyquist, scale, f_lo_norm);
                    let major = matches!(mult, 1 | 2 | 5);
                    out.push(Tick {
                        frac: d.clamp(0.0, 1.0),
                        label: major.then(|| fmt_hz(f)),
                    });
                }
                decade *= 10.0;
            }
        }
        FreqScale::Linear => {
            let step = snap_125(nyquist * MAJOR_PX / height_px);
            let mut k = 0i64;
            loop {
                let f = k as f64 * step;
                if f > nyquist {
                    break;
                }
                out.push(Tick {
                    frac: (f / nyquist).clamp(0.0, 1.0),
                    label: Some(fmt_hz(f)),
                });
                k += 1;
            }
        }
        FreqScale::Mel | FreqScale::Bark => {
            out.push(Tick {
                frac: 0.0,
                label: Some("0".to_string()),
            });
            let (mut last_any, mut last_label) = (0.0f64, 0.0f64);
            let mut decade = 10.0;
            while decade <= nyquist {
                for mult in 1..10 {
                    let f = decade * mult as f64;
                    if f > nyquist {
                        continue;
                    }
                    let d = hz_to_display(f, nyquist, scale, f_lo_norm).clamp(0.0, 1.0);
                    let y = d * height_px;
                    let major = matches!(mult, 1 | 2 | 5);
                    if major && y - last_label >= LABEL_MIN_PX {
                        out.push(Tick {
                            frac: d,
                            label: Some(fmt_hz(f)),
                        });
                        last_label = y;
                        last_any = y;
                    } else if y - last_any >= MINOR_PX {
                        out.push(Tick {
                            frac: d,
                            label: None,
                        });
                        last_any = y;
                    }
                }
                decade *= 10.0;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(ticks: &[Tick]) -> Vec<&str> {
        ticks.iter().filter_map(|t| t.label.as_deref()).collect()
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
    fn time_ticks_step_in_125_seconds_and_stay_in_range() {
        // 10 s visible over 800 px at 48 kHz: majors ~90 px apart => 2 s step.
        let ticks = time_ticks(0.0, 480_000.0, 800.0, 48_000.0, TimeUnit::Seconds);
        assert!(!ticks.is_empty());
        for t in &ticks {
            assert!((0.0..=1.0).contains(&t.frac));
        }
        assert_eq!(labels(&ticks), vec!["0", "2", "4", "6", "8", "10"]);
    }

    #[test]
    fn time_ticks_fall_back_to_sample_counts() {
        let ticks = time_ticks(0.0, 100_000.0, 500.0, 0.0, TimeUnit::Seconds);
        // No rate: labels are sample counts (compacted), stepping 1-2-5.
        assert_eq!(
            labels(&ticks),
            vec!["0", "20K", "40K", "60K", "80K", "100K"]
        );
        let explicit = time_ticks(0.0, 100_000.0, 500.0, 48_000.0, TimeUnit::Samples);
        assert_eq!(
            labels(&explicit),
            labels(&ticks),
            "samples mode ignores the rate"
        );
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
        // A 1 s window starting at 2.5 s: majors every 0.2 s from 2.6.
        let ticks = time_ticks(120_000.0, 48_000.0, 800.0, 48_000.0, TimeUnit::Seconds);
        let l = labels(&ticks);
        assert!(l.contains(&"2.6") && l.contains(&"3.4"), "{l:?}");
        // Minors exist between majors (more ticks than labels).
        assert!(ticks.len() > l.len());
    }

    #[test]
    fn beat_ticks_label_bars_and_beats_on_the_quant_grid() {
        // 8 beats visible over 800 px at 48 kHz, 2 beats/s (tempo 2.0): raw
        // step = 8 * 90 / 800 = 0.9 beats -> 1 beat. quant 4 -> bar:beat.
        let unit = TimeUnit::Beats {
            tempo: 2.0,
            beat_at: 0.0,
            quant: 4.0,
        };
        let ticks = time_ticks(0.0, 192_000.0, 800.0, 48_000.0, unit);
        let l = labels(&ticks);
        assert_eq!(l[0], "1:1");
        assert!(l.contains(&"1:3") && l.contains(&"2:1"), "{l:?}");
        // Minors (the half-beat subdivision) exist between majors.
        assert!(ticks.len() > l.len());
        for t in &ticks {
            assert!((0.0..=1.0).contains(&t.frac));
        }
    }

    #[test]
    fn beat_step_climbs_the_musical_ladder() {
        // Binary fractions below a beat, bar-aligned steps above it.
        assert_eq!(beat_step(0.3, 4.0), 0.5);
        assert_eq!(beat_step(0.9, 4.0), 1.0);
        assert_eq!(beat_step(1.5, 4.0), 2.0);
        assert_eq!(beat_step(3.0, 4.0), 4.0); // straight to the bar
        assert_eq!(beat_step(5.0, 4.0), 8.0); // 2 bars
        // A 3-beat bar has no 2-beat rung (it would miss the bar line).
        assert_eq!(beat_step(1.5, 3.0), 3.0);
        // No grid: powers of two of a beat.
        assert_eq!(beat_step(3.0, 0.0), 4.0);
    }

    #[test]
    fn beats_without_rate_or_tempo_fall_back_to_samples() {
        let unit = TimeUnit::Beats {
            tempo: 0.0,
            beat_at: 0.0,
            quant: 4.0,
        };
        let ticks = time_ticks(0.0, 100_000.0, 500.0, 48_000.0, unit);
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
        let ticks = time_ticks(0.0, 192_000.0, 800.0, 48_000.0, unit);
        let l = labels(&ticks);
        assert_eq!(l[0], "1:3", "{l:?}");
        assert!(l.contains(&"2:1"), "{l:?}");
    }

    #[test]
    fn readout_beats_reads_bar_beat() {
        // 48k samples at 2 beats/s = 2 beats in; quant 4 -> bar 1, beat 3.
        assert_eq!(readout_beats(48_000.0, 48_000.0, 2.0, 0.0, 4.0), "1:3.00");
        assert_eq!(readout_beats(48_000.0, 48_000.0, 2.0, 0.0, 0.0), "2.00");
        assert_eq!(readout_beats(48_000.0, 0.0, 2.0, 0.0, 4.0), "48000");
    }

    #[test]
    fn amp_ticks_norm_and_percent_share_the_geometry() {
        let norm = amp_ticks(RulerY::Norm, 400.0, 16);
        let pct = amp_ticks(RulerY::Percent, 400.0, 16);
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
    }

    #[test]
    fn amp_ticks_bits_step_in_sample_values_and_keep_the_endpoints() {
        let bits = amp_ticks(RulerY::Bits, 400.0, 16);
        let l = labels(&bits);
        // 1-2-5 steps in integer sample values, compacted...
        assert!(
            l.contains(&"0") && l.contains(&"10K") && l.contains(&"-10K"),
            "{l:?}"
        );
        // ...plus the full-scale endpoints the decimal step misses.
        assert!(l.contains(&"-32768") && l.contains(&"32768"), "{l:?}");
        // A different bit depth rescales the axis.
        let eight = amp_ticks(RulerY::Bits, 400.0, 8);
        assert!(labels(&eight).contains(&"-128"), "{:?}", labels(&eight));
    }

    #[test]
    fn amp_ticks_db_ladder_is_mirrored_and_placed_by_amplitude() {
        let ticks = amp_ticks(RulerY::Db, 600.0, 16);
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
    }

    #[test]
    fn amp_readout_formats_per_unit() {
        assert_eq!(readout_amp(0.5, RulerY::Norm, 16), "+0.50");
        assert_eq!(readout_amp(0.5, RulerY::Db, 16), "-6.0 DB");
        assert_eq!(readout_amp(0.0, RulerY::Db, 16), "-INF DB");
        assert_eq!(readout_amp(-0.5, RulerY::Bits, 16), "-16384");
        assert_eq!(readout_amp(0.5, RulerY::Percent, 16), "50%");
        assert_eq!(readout_amp(-0.5, RulerY::Percent, 16), "-50%");
    }

    #[test]
    fn log_hz_ticks_match_the_shader_geometry() {
        // 48 kHz => Nyquist 24 kHz, f_lo = 20/24000.
        let nyq = 24_000.0;
        let f_lo = 20.0 / nyq;
        let ticks = hz_ticks(nyq, FreqScale::Log, f_lo, 600.0);
        let l = labels(&ticks);
        for expected in ["100", "1K", "10K", "20K", "2K", "50"] {
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
    }

    #[test]
    fn linear_hz_ticks_are_evenly_spaced() {
        let ticks = hz_ticks(24_000.0, FreqScale::Linear, 20.0 / 24_000.0, 600.0);
        let l = labels(&ticks);
        assert_eq!(l.first(), Some(&"0"));
        assert_eq!(l.last(), Some(&"20K"));
        let d01 = ticks[1].frac - ticks[0].frac;
        let d12 = ticks[2].frac - ticks[1].frac;
        assert!((d01 - d12).abs() < 1e-9, "even spacing on the linear axis");
    }

    #[test]
    fn mel_and_bark_ticks_sit_on_the_perceptual_mapping() {
        let nyq = 24_000.0;
        for scale in [FreqScale::Mel, FreqScale::Bark] {
            let ticks = hz_ticks(nyq, scale, 20.0 / nyq, 600.0);
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
            // Ticks never crowd closer than the label spacing allows.
            let mut labeled: Vec<f64> = ticks
                .iter()
                .filter(|t| t.label.is_some())
                .map(|t| t.frac * 600.0)
                .collect();
            labeled.sort_by(f64::total_cmp);
            for w in labeled.windows(2) {
                assert!(w[1] - w[0] >= LABEL_MIN_PX - 1e-9, "{scale:?}: {w:?}");
            }
        }
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
    fn degenerate_inputs_yield_no_ticks() {
        assert!(time_ticks(0.0, 0.0, 800.0, 48_000.0, TimeUnit::Seconds).is_empty());
        assert!(time_ticks(0.0, 100.0, 0.0, 48_000.0, TimeUnit::Seconds).is_empty());
        assert!(hz_ticks(0.0, FreqScale::Log, 0.001, 600.0).is_empty());
        assert!(hz_ticks(24_000.0, FreqScale::Linear, 0.001, 0.0).is_empty());
        assert!(amp_ticks(RulerY::Norm, 0.0, 16).is_empty());
        assert!(amp_ticks(RulerY::Off, 400.0, 16).is_empty());
    }
}
