//! Adaptive ruler tick math for the editor-grade views — pure, display-only.
//!
//! A time axis under the waveform/spectrogram and a frequency axis beside the
//! spectrogram need tick positions and labels that stay legible at any zoom.
//! The math is classic editor chrome: a 1-2-5 progression snapped to the
//! decade matching the visible span (time, in seconds or samples), and decade
//! ticks with 2×/5× subdivisions on the log frequency axis — placed with the
//! **identical** display→bin geometry the spectrogram shader uses, so a tick
//! labeled 1 kHz sits exactly on the 1 kHz row of pixels. No GPU, no widget
//! types: positions come out as fractions of the visible span, and the frame
//! renderer turns them into painter geometry.

/// One ruler tick: its position as a fraction of the visible axis span
/// (0 = start/bottom, 1 = end/top) and its label (`None` for a minor tick).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Tick {
    pub frac: f64,
    pub label: Option<String>,
}

/// How the time ruler labels its ticks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TimeUnit {
    /// `h:mm:ss.mmm`-style clock time (needs the sample rate).
    Seconds,
    /// Plain sample counts (the fallback when no rate is known).
    Samples,
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

/// The ticks of a time ruler spanning samples `[start, start + len)` over
/// `width_px` device pixels. With `TimeUnit::Seconds` (and a positive
/// `sample_rate`) the steps and labels are in clock time; otherwise in sample
/// counts. Major ticks carry labels; minors (a fifth of the major step)
/// appear only when they are at least a few pixels apart.
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

/// The ticks of a frequency ruler over the full display axis (bottom = the
/// axis floor, top = Nyquist), matching the spectrogram shader's display→bin
/// mapping. On the **log** axis (`f_lo_norm` = the shader's normalized ~20 Hz
/// floor) ticks sit on the 1/2/5 multiples of each decade, labeled, with the
/// remaining integer multiples as minors; the display position inverts the
/// shader's `bin_norm = f_lo^(1 - d)`. On the **linear** axis it is the same
/// 1-2-5 progression as the time ruler, over `height_px`.
pub(crate) fn hz_ticks(nyquist: f64, log: bool, f_lo_norm: f64, height_px: f64) -> Vec<Tick> {
    if nyquist <= 0.0 || height_px <= 0.0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    if log {
        let f_lo = (f_lo_norm.clamp(1e-5, 0.5)) * nyquist;
        let ln_flo = f_lo_norm.clamp(1e-5, 0.5).ln();
        let mut decade = 10f64.powf(f_lo.log10().floor());
        while decade <= nyquist {
            for mult in 1..10 {
                let f = decade * mult as f64;
                if f < f_lo || f > nyquist {
                    continue;
                }
                // d from the shader's bin_norm = f_lo_norm^(1 - d).
                let d = 1.0 - (f / nyquist).ln() / ln_flo;
                let major = matches!(mult, 1 | 2 | 5);
                out.push(Tick {
                    frac: d.clamp(0.0, 1.0),
                    label: major.then(|| fmt_hz(f)),
                });
            }
            decade *= 10.0;
        }
    } else {
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
    fn log_hz_ticks_match_the_shader_geometry() {
        // 48 kHz => Nyquist 24 kHz, f_lo = 20/24000.
        let nyq = 24_000.0;
        let f_lo = 20.0 / nyq;
        let ticks = hz_ticks(nyq, true, f_lo, 600.0);
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
        let ticks = hz_ticks(24_000.0, false, 20.0 / 24_000.0, 600.0);
        let l = labels(&ticks);
        assert_eq!(l.first(), Some(&"0"));
        assert_eq!(l.last(), Some(&"20K"));
        let d01 = ticks[1].frac - ticks[0].frac;
        let d12 = ticks[2].frac - ticks[1].frac;
        assert!((d01 - d12).abs() < 1e-9, "even spacing on the linear axis");
    }

    #[test]
    fn degenerate_inputs_yield_no_ticks() {
        assert!(time_ticks(0.0, 0.0, 800.0, 48_000.0, TimeUnit::Seconds).is_empty());
        assert!(time_ticks(0.0, 100.0, 0.0, 48_000.0, TimeUnit::Seconds).is_empty());
        assert!(hz_ticks(0.0, true, 0.001, 600.0).is_empty());
        assert!(hz_ticks(24_000.0, false, 0.001, 0.0).is_empty());
    }
}
