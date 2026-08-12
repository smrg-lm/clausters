//! Drawing the shared-memory-backed views: the level `meter` and the `scope`.
//!
//! These are the cheap counterparts of the heavy GPU views: their *data* is a
//! single control bus read straight from the shared-memory segment each frame
//! (see [`super::shm`]), so they need no buffer, no analysis and no dedicated
//! pipeline — just the flat-geometry painter ([`super::paint`]) plus bitmap text,
//! exactly like the standard controls. The drawing lives here as pure functions
//! over a [`Draw`]; the windowed front supplies the live value(s) read from
//! shared memory and keeps the scope's rolling history. Keeping it GPU- and
//! shm-free makes it unit-testable without a window.

use crate::spectrogram::FreqScale;

use super::controls::body_rect;
use super::font;
use super::frame::lane_rect;
use super::graphics::signal::trace;
use super::layout::Rect;
use super::live::TapWindow;
use super::paint::{Color, Draw};
use super::ruler;
use crate::viewport::{Axis, Unit};

/// The 0..1 position of `value` in `[min, max]`, clamped. A degenerate range
/// (min == max) maps to 0.
///
/// The value axis this expresses is a [`Axis::ranged`]; this stays as the
/// `f32` door the drawing code calls, so a paint site keeps naming a range
/// rather than building an axis per mark.
pub fn fraction(value: f32, min: f32, max: f32) -> f32 {
    Axis::ranged(min as f64, max as f64, Unit::Norm).fraction_clamped(value as f64) as f32
}

/// Draws a vertical level meter: a framed field with a green column rising from
/// the bottom to `fraction` of the body height, plus the raw value as text.
pub fn draw_meter(d: &mut Draw, rect: Rect, value: f32, fraction: f32, label: Option<&str>) {
    label_strip(d, label, rect);
    let (mesh, m, theme) = d.parts();
    let body = body_rect(rect, label.is_some(), m);
    if body.w <= 0.0 || body.h <= 0.0 {
        return;
    }
    mesh.rect(body, theme.field);
    let fill_h = body.h * fraction.clamp(0.0, 1.0);
    mesh.rect(
        Rect::new(body.x, body.y + body.h - fill_h, body.w, fill_h),
        theme.accent,
    );
    mesh.border(body, m.divider_w, theme.accent);
    value_text(d, &fmt(value), body);
}

/// Draws a time-domain scope: a framed field with a polyline through `history`
/// (oldest sample at the left, newest at the right), each sample normalized into
/// `[min, max]`. Fewer than two samples draw just the frame.
pub fn draw_scope(
    d: &mut Draw,
    rect: Rect,
    history: &[f32],
    min: f32,
    max: f32,
    label: Option<&str>,
) {
    label_strip(d, label, rect);
    let (mesh, m, theme) = d.parts();
    let body = body_rect(rect, label.is_some(), m);
    if body.w <= 0.0 || body.h <= 0.0 {
        return;
    }
    mesh.rect(body, theme.field);
    mesh.border(body, m.divider_w, theme.accent);
    let color = theme.trace;
    // A control bus's history is one channel of a live source: the same
    // renderer, so a history longer than the body's pixels summarizes instead
    // of aliasing — which a polyline of its own never did.
    trace_lane(d, body, history, 1, 0, (min, max), color);
}

/// The display parameters of one audio-rate oscilloscope draw, alongside its
/// aligned [`TapWindow`].
pub(crate) struct WaveParams<'a> {
    pub window: &'a TapWindow,
    pub min: f32,
    pub max: f32,
    /// The display window in ms (places the x ruler's ticks).
    pub window_ms: f32,
    pub trigger: f32,
    pub overlay: bool,
    pub ruler: bool,
    pub ruler_y: bool,
    pub label: Option<&'a str>,
}

/// Draws an audio-rate oscilloscope: the [`TapWindow`]'s channels as stacked
/// lanes (or color-coded `overlay` traces in one field), each an
/// already-aligned display window (see `clausters_core::oscil`) over `[min, max]` —
/// a polyline while the data fits the width, a per-column min/max envelope
/// when it does not (never resolving finer than the screen). The chrome names
/// what the trigger did: a faint line marks the `trigger` level in the first
/// channel's lane (where the alignment is searched) and a `lock`/`free`
/// read-out says whether it fired. `ruler` is the x strip in milliseconds of
/// the window, `ruler_y` the per-lane value strip. An empty window draws just
/// the framed field.
pub(crate) fn draw_wave(d: &mut Draw, rect: Rect, p: &WaveParams) {
    label_strip(d, p.label, rect);
    let m = d.m;
    let mut body = body_rect(rect, p.label.is_some(), m);
    let lanes = if p.overlay {
        1
    } else {
        p.window.channels.max(1)
    };
    // Height first: the x strip takes it, and it is what decides how finely the
    // value axis steps and therefore how wide the labels the y strip holds are.
    let takes_x = p.ruler && body.h > m.ruler_h * 2.0;
    let lane_h = (if takes_x { body.h - m.ruler_h } else { body.h }) / lanes as f32;
    let want_w = ruler::value_strip_w(p.min as f64, p.max as f64, lane_h, m);
    let strip_x = (p.ruler_y && body.w > want_w * 2.0).then(|| {
        let x = body.x;
        body.x += want_w;
        body.w -= want_w;
        x
    });
    let x_strip = takes_x.then(|| {
        body.h -= m.ruler_h;
        Rect::new(body.x, body.y + body.h, body.w, m.ruler_h)
    });
    if body.w <= 0.0 || body.h <= 0.0 {
        return;
    }
    d.mesh.rect(body, d.theme.field);
    d.mesh.border(body, m.divider_w, d.theme.accent);
    if let Some(strip) = x_strip {
        let ticks = ruler::hz_ticks_h(
            p.window_ms.max(0.1) as f64,
            FreqScale::Linear,
            1e-4,
            strip.w as f64,
            0.0,
            1.0,
            m,
        );
        ruler::draw_ticks_h(d, strip, &ticks);
    }
    let channels = p.window.channels.max(1);
    let frames = p.window.frames();
    for ch in 0..channels {
        let lane = lane_rect(body, lanes, if p.overlay { 0 } else { ch });
        if ch > 0 && !p.overlay {
            d.mesh.rect(
                Rect::new(body.x, lane.y, body.w, m.divider_w),
                d.theme.lane_divider,
            );
        }
        if (ch == 0 || !p.overlay)
            && let Some(strip_x) = strip_x
        {
            let ticks = ruler::value_ticks(p.min as f64, p.max as f64, lane.h as f64, m);
            ruler::draw_ticks_v(d, body.x, strip_x, lane, &ticks);
        }
        if ch == 0 && frames > 0 {
            // The trigger level, in the channel the alignment is searched in.
            let y = lane.y + lane.h * (1.0 - fraction(p.trigger, p.min, p.max));
            d.mesh
                .rect(Rect::new(body.x, y, body.w, m.divider_w), d.theme.trigger);
        }
        let color = if channels > 1 {
            d.theme.series(ch)
        } else {
            d.theme.trace
        };
        trace_lane(
            d,
            lane,
            &p.window.samples,
            channels,
            ch,
            (p.min, p.max),
            color,
        );
    }
    if frames > 0 {
        value_text(d, if p.window.locked { "lock" } else { "free" }, body);
    }
}

/// One channel of a live source into `lane`, through the **one** column source
/// and mesh renderer every view of a signal against time reads
/// ([`trace::draw_channel`]): a per-column min/max envelope while the frames
/// outnumber the pixels, a polyline once they do not.
///
/// It used to be this module's own loop — the copy the signal element's
/// collapse left outside, with a regime rule of its own (`frames > columns *
/// 2`), a column inked one hairline wide however wide the pixel column was,
/// and no baseline. A live view is the same drawing of the same signal as a
/// stored one; only where the samples come from differs.
fn trace_lane(
    d: &mut Draw,
    lane: Rect,
    samples: &[f32],
    channels: usize,
    ch: usize,
    domain: (f32, f32),
    color: Color,
) {
    let (min, max) = domain;
    let (mesh, m, _theme) = d.parts();
    let frames = samples.len() / channels.max(1);
    if frames < 2 {
        return;
    }
    // A live window is drawn whole: its span is the lane, end to end.
    let span = (frames - 1) as f64;
    trace::draw_channel(
        mesh,
        lane,
        &trace::Trace::samples(samples, channels),
        ch,
        |x| (x - lane.x) as f64 / lane.w.max(1.0) as f64 * span,
        |s| lane.x + (s / span) as f32 * lane.w,
        |v| lane.y + lane.h * (1.0 - fraction(v, min, max)),
        trace::TraceStyle::new(color, m.trace_w).with_dots(m.point_radius),
    );
}

/// Draws the label strip above a view body, if it has a label.
fn label_strip(d: &mut Draw, label: Option<&str>, rect: Rect) {
    let (mesh, m, theme) = d.parts();
    if let Some(text) = label {
        font::text(
            mesh,
            text,
            rect.x + m.pad,
            rect.y + m.pad,
            m.text_scale,
            theme.text,
        );
    }
}

/// A value read-out at the top-right of a body — the corner slot the scope's
/// `lock`/`free` state and the spectral views' scale tag share.
pub(crate) fn value_text(d: &mut Draw, s: &str, body: Rect) {
    let (mesh, m, theme) = d.parts();
    let w = font::width(s, m.text_scale);
    let x = (body.x + body.w - w - m.pad).max(body.x);
    font::text(mesh, s, x, body.y + m.pad, m.text_scale, theme.text);
}

/// Formats a value compactly (drops trailing zeros within 2 decimals).
fn fmt(v: f32) -> String {
    if v.fract() == 0.0 && v.abs() < 1e6 {
        format!("{v:.0}")
    } else {
        format!("{v:.2}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::metrics::Metrics;
    use crate::host::paint::Mesh;
    use crate::host::theme::Theme;

    #[test]
    fn fraction_clamps_and_handles_degenerate_range() {
        assert_eq!(fraction(0.5, 0.0, 1.0), 0.5);
        assert_eq!(fraction(-1.0, 0.0, 1.0), 0.0, "below min clamps to 0");
        assert_eq!(fraction(2.0, 0.0, 1.0), 1.0, "above max clamps to 1");
        assert_eq!(fraction(0.0, 0.0, 2.0), 0.0);
        assert_eq!(fraction(5.0, 3.0, 3.0), 0.0, "min == max maps to 0");
    }

    #[test]
    fn meter_emits_fill_geometry() {
        let mut m = Mesh::new();
        draw_meter(
            &mut Draw::new(&mut m, &Metrics::default(), &Theme::default()),
            Rect::new(0.0, 0.0, 40.0, 120.0),
            0.5,
            0.5,
            Some("out"),
        );
        assert!(!m.is_empty(), "a meter with a positive fill draws geometry");
    }

    #[test]
    fn scope_draws_a_polyline_for_history() {
        let mut empty = Mesh::new();
        draw_scope(
            &mut Draw::new(&mut empty, &Metrics::default(), &Theme::default()),
            Rect::new(0.0, 0.0, 80.0, 60.0),
            &[0.0],
            -1.0,
            1.0,
            None,
        );
        let with_one = empty.vertex_count();

        let mut many = Mesh::new();
        draw_scope(
            &mut Draw::new(&mut many, &Metrics::default(), &Theme::default()),
            Rect::new(0.0, 0.0, 80.0, 60.0),
            &[0.0, 0.5, -0.5, 1.0],
            -1.0,
            1.0,
            None,
        );
        assert!(
            many.vertex_count() > with_one,
            "more history points add line segments"
        );
    }

    /// The live views read the **one** trace renderer now, so a history longer
    /// than the body's pixels summarizes into min/max columns instead of
    /// drawing a segment per sample. This module used to have a polyline of its
    /// own that stepped `lane.w / (frames - 1)` however many frames there were,
    /// which aliases and costs the data rather than the screen — the rule every
    /// other view of a signal has always followed.
    #[test]
    fn a_long_history_costs_the_body_not_its_samples() {
        let body = Rect::new(0.0, 0.0, 80.0, 60.0);
        let history: Vec<f32> = (0..20_000).map(|i| (i as f32 * 0.01).sin()).collect();
        let mut mesh = Mesh::new();
        draw_scope(
            &mut Draw::new(&mut mesh, &Metrics::default(), &Theme::default()),
            body,
            &history,
            -1.0,
            1.0,
            None,
        );
        // The field, its border and at most one six-vertex column per pixel.
        let columns = (mesh.vertex_count() as f32 - 60.0) / 6.0;
        assert!(
            columns <= body.w + 2.0,
            "a screenful of columns, not 20000 segments: {columns}"
        );
        assert!(!mesh.is_empty());
    }

    /// A live lane is drawn by the shared renderer, so it inks what the other
    /// two ink: an offset signal is a band at its own level (never a fill from
    /// the baseline) and a signal that swings across zero is the solid body.
    /// Before the fold this module drew a hairline envelope of its own, so a
    /// live view never quite matched the stored view beside it.
    #[test]
    fn a_live_lane_inks_what_every_other_trace_inks() {
        let lane = Rect::new(0.0, 0.0, 100.0, 100.0);
        let draw = |samples: &[f32], min: f32, max: f32| {
            let mut mesh = Mesh::new();
            trace_lane(
                &mut Draw::new(&mut mesh, &Metrics::default(), &Theme::default()),
                lane,
                samples,
                1,
                0,
                (min, max),
                [1.0, 1.0, 1.0, 1.0],
            );
            mesh.extent().expect("the lane drew").h
        };
        let offset = vec![0.8f32; 4_000];
        assert!(
            draw(&offset, -1.0, 1.0) < lane.h * 0.05,
            "an offset signal is a band where the samples are"
        );
        let swinging: Vec<f32> = (0..4_000)
            .map(|i| if i % 2 == 0 { 0.9 } else { -0.9 })
            .collect();
        assert!(
            draw(&swinging, -1.0, 1.0) > lane.h * 0.8,
            "and a swinging one is the body its own data fills"
        );
    }
}
