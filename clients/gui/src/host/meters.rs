//! Drawing the shared-memory-backed views: the level `meter` and the `scope`.
//!
//! These are the cheap counterparts of the heavy GPU views: their *data* is a
//! single control bus read straight from the shared-memory segment each frame
//! (see [`super::shm`]), so they need no buffer, no analysis and no dedicated
//! pipeline — just the flat-geometry painter ([`super::paint`]) plus bitmap text,
//! exactly like the standard controls. The drawing lives here as pure functions
//! over a [`Mesh`]; the windowed front supplies the live value(s) read from
//! shared memory and keeps the scope's rolling history. Keeping it GPU- and
//! shm-free makes it unit-testable without a window.

use crate::spectrogram::FreqScale;

use super::controls::body_rect;
use super::font;
use super::frame::lane_rect;
use super::layout::Rect;
use super::live::TapWindow;
use super::metrics::Metrics;
use super::paint::{Color, Mesh};
use super::ruler;
use super::theme::Theme;
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
#[allow(clippy::too_many_arguments)] // one view's draw: its value, box, look
pub fn draw_meter(
    mesh: &mut Mesh,
    rect: Rect,
    value: f32,
    fraction: f32,
    label: Option<&str>,
    m: &Metrics,
    theme: &Theme,
) {
    label_strip(mesh, label, rect, m, theme);
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
    value_text(mesh, &fmt(value), body, m, theme);
}

/// Draws a time-domain scope: a framed field with a polyline through `history`
/// (oldest sample at the left, newest at the right), each sample normalized into
/// `[min, max]`. Fewer than two samples draw just the frame.
#[allow(clippy::too_many_arguments)] // one view's draw: its data, box, look
pub fn draw_scope(
    mesh: &mut Mesh,
    rect: Rect,
    history: &[f32],
    min: f32,
    max: f32,
    label: Option<&str>,
    m: &Metrics,
    theme: &Theme,
) {
    label_strip(mesh, label, rect, m, theme);
    let body = body_rect(rect, label.is_some(), m);
    if body.w <= 0.0 || body.h <= 0.0 {
        return;
    }
    mesh.rect(body, theme.field);
    mesh.border(body, m.divider_w, theme.accent);
    if history.len() >= 2 {
        let dx = body.w / (history.len() - 1) as f32;
        let y_at = |v: &f32| body.y + body.h * (1.0 - fraction(*v, min, max));
        let mut prev = [body.x, y_at(&history[0])];
        for (i, v) in history.iter().enumerate().skip(1) {
            let p = [body.x + i as f32 * dx, y_at(v)];
            mesh.line(prev, p, m.trace_w, theme.trace);
            prev = p;
        }
    }
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
pub(crate) fn draw_wave(mesh: &mut Mesh, rect: Rect, p: &WaveParams, m: &Metrics, theme: &Theme) {
    label_strip(mesh, p.label, rect, m, theme);
    let mut body = body_rect(rect, p.label.is_some(), m);
    let strip_x = (p.ruler_y && body.w > m.ruler_w * 2.0).then(|| {
        let x = body.x;
        body.x += m.ruler_w;
        body.w -= m.ruler_w;
        x
    });
    let x_strip = (p.ruler && body.h > m.ruler_h * 2.0).then(|| {
        body.h -= m.ruler_h;
        Rect::new(body.x, body.y + body.h, body.w, m.ruler_h)
    });
    if body.w <= 0.0 || body.h <= 0.0 {
        return;
    }
    mesh.rect(body, theme.field);
    mesh.border(body, m.divider_w, theme.accent);
    if let Some(strip) = x_strip {
        let ticks = ruler::hz_ticks_h(
            p.window_ms.max(0.1) as f64,
            FreqScale::Linear,
            1e-4,
            strip.w as f64,
            m,
        );
        ruler::draw_ticks_h(mesh, strip, &ticks, m, theme);
    }
    let channels = p.window.channels.max(1);
    let frames = p.window.frames();
    let lanes = if p.overlay { 1 } else { channels };
    for ch in 0..channels {
        let lane = lane_rect(body, lanes, if p.overlay { 0 } else { ch });
        if ch > 0 && !p.overlay {
            mesh.rect(
                Rect::new(body.x, lane.y, body.w, m.divider_w),
                theme.lane_divider,
            );
        }
        if (ch == 0 || !p.overlay)
            && let Some(strip_x) = strip_x
        {
            let ticks = ruler::value_ticks(p.min as f64, p.max as f64, lane.h as f64, m);
            ruler::draw_ticks_v(mesh, body.x, strip_x, lane, &ticks, m, theme);
        }
        if ch == 0 && frames > 0 {
            // The trigger level, in the channel the alignment is searched in.
            let y = lane.y + lane.h * (1.0 - fraction(p.trigger, p.min, p.max));
            mesh.rect(Rect::new(body.x, y, body.w, m.divider_w), theme.trigger);
        }
        let color = if channels > 1 {
            theme.series(ch)
        } else {
            theme.trace
        };
        let at = |f: usize| p.window.samples[f * channels + ch];
        trace_lane(mesh, lane, frames, &at, p.min, p.max, m, color);
    }
    if frames > 0 {
        value_text(
            mesh,
            if p.window.locked { "lock" } else { "free" },
            body,
            m,
            theme,
        );
    }
}

/// One channel's trace into `lane`: a per-column min/max envelope when the
/// frames outnumber the pixels, a polyline otherwise.
#[allow(clippy::too_many_arguments)] // one lane's trace: its data, box, look
fn trace_lane(
    mesh: &mut Mesh,
    lane: Rect,
    frames: usize,
    at: &impl Fn(usize) -> f32,
    min: f32,
    max: f32,
    m: &Metrics,
    color: Color,
) {
    let columns = lane.w.max(1.0) as usize;
    let y_at = |v: f32| lane.y + lane.h * (1.0 - fraction(v, min, max));
    if frames > columns * 2 {
        for c in 0..columns {
            let f0 = c * frames / columns;
            let f1 = ((c + 1) * frames / columns).max(f0 + 1);
            let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
            for f in f0..f1 {
                let s = at(f);
                lo = lo.min(s);
                hi = hi.max(s);
            }
            let (y0, y1) = (y_at(hi), y_at(lo));
            let x = lane.x + c as f32;
            mesh.rect(
                Rect::new(x, y0, m.divider_w, (y1 - y0).max(m.divider_w)),
                color,
            );
        }
    } else if frames >= 2 {
        let dx = lane.w / (frames - 1) as f32;
        let mut prev = [lane.x, y_at(at(0))];
        for f in 1..frames {
            let p = [lane.x + f as f32 * dx, y_at(at(f))];
            mesh.line(prev, p, m.trace_w, color);
            prev = p;
        }
    }
}

/// Draws the label strip above a view body, if it has a label.
fn label_strip(mesh: &mut Mesh, label: Option<&str>, rect: Rect, m: &Metrics, theme: &Theme) {
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
pub(crate) fn value_text(mesh: &mut Mesh, s: &str, body: Rect, m: &Metrics, theme: &Theme) {
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
            &mut m,
            Rect::new(0.0, 0.0, 40.0, 120.0),
            0.5,
            0.5,
            Some("out"),
            &Metrics::default(),
            &Theme::default(),
        );
        assert!(!m.is_empty(), "a meter with a positive fill draws geometry");
    }

    #[test]
    fn scope_draws_a_polyline_for_history() {
        let mut empty = Mesh::new();
        draw_scope(
            &mut empty,
            Rect::new(0.0, 0.0, 80.0, 60.0),
            &[0.0],
            -1.0,
            1.0,
            None,
            &Metrics::default(),
            &Theme::default(),
        );
        let with_one = empty.vertex_count();

        let mut many = Mesh::new();
        draw_scope(
            &mut many,
            Rect::new(0.0, 0.0, 80.0, 60.0),
            &[0.0, 0.5, -0.5, 1.0],
            -1.0,
            1.0,
            None,
            &Metrics::default(),
            &Theme::default(),
        );
        assert!(
            many.vertex_count() > with_one,
            "more history points add line segments"
        );
    }
}
