//! The phasescope (goniometer): drawing a stereo pair as a Lissajous figure.
//!
//! A phasescope reads two audio taps (left and right) and plots their recent
//! sample pairs in the 45°-rotated **mid/side** plane — the audio-engineering
//! goniometer, where a mono signal draws a vertical line, an anti-phase one a
//! horizontal line, and a wide stereo field fills the lozenge. The coordinate
//! transform itself is general audio geometry, so it lives once in
//! `clausters_core::measure` (the [`lissajous_point`]); this module is only the
//! **display** — persistence trail, the framed field, and a correlation readout
//! ([`correlation`]) as companion chrome. Pure (no GPU, no shm), so both fronts
//! share it and it is unit-testable.
//!
//! The tick stores the two taps' windows interleaved `[l0, r0, l1, r1, …]` under
//! the element itself (its `live` state, filled by the tick); the render draws
//! that verbatim, so a `hold` phasescope simply keeps its last window.

use clausters_core::measure::{correlation, lissajous_point};

use crate::host::controls::body_rect;
use crate::host::font;
use crate::host::layout::Rect;
use crate::host::paint::Draw;

/// The furthest a sample reaches from the origin: a full-scale mono signal sits
/// at mid `(1+1)/√2 = √2`. The field is scaled so that extent just fits, with a
/// little margin.
const MAX_EXTENT: f32 = std::f32::consts::SQRT_2;
/// Cap on drawn trail segments, so a long window stays a bounded mesh; a denser
/// window is strided down to roughly this many points.
const MAX_SEGMENTS: usize = 2000;

/// Draws a phasescope from an interleaved `[l, r, l, r, …]` window: the framed
/// goniometer field with an age-faded Lissajous trail (oldest faint, newest
/// bright), a faint mid/side center cross, and a correlation bar beneath. An
/// empty or odd-length window draws just the field and an empty readout.
pub fn draw_phasescope(d: &mut Draw, rect: Rect, interleaved: &[f32], label: Option<&str>) {
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
    let outer = body_rect(rect, label.is_some(), m);
    if outer.w <= 0.0 || outer.h <= 0.0 {
        return;
    }
    // Split off a strip at the bottom for the correlation readout; the square
    // goniometer field takes the rest, centered.
    // The readout is one line of control: the `control_h` role.
    let corr_h = m.control_h.min(outer.h * 0.4);
    let field_area = Rect::new(outer.x, outer.y, outer.w, (outer.h - corr_h).max(0.0));
    let side = field_area.w.min(field_area.h);
    let field = Rect::new(
        field_area.x + (field_area.w - side) * 0.5,
        field_area.y + (field_area.h - side) * 0.5,
        side,
        side,
    );
    mesh.rect(field, theme.field);
    mesh.border(field, m.divider_w, theme.accent);

    let (cx, cy) = (field.x + side * 0.5, field.y + side * 0.5);
    // Mid is vertical, side horizontal: a faint center cross reads the axes.
    mesh.line([cx, field.y], [cx, field.y + side], m.divider_w, theme.grid);
    mesh.line([field.x, cy], [field.x + side, cy], m.divider_w, theme.grid);

    let scale = (side * 0.5 / MAX_EXTENT) * 0.95;
    let n = interleaved.len() / 2;
    if n >= 2 {
        let step = n.div_ceil(MAX_SEGMENTS).max(1);
        let point = |i: usize| -> [f32; 2] {
            let [x, y] = lissajous_point(interleaved[2 * i], interleaved[2 * i + 1]);
            [cx + x * scale, cy - y * scale] // screen y grows downward
        };
        let mut prev = point(0);
        let mut i = step;
        while i < n {
            let p = point(i);
            // Age fade: newest segments brightest.
            let age = i as f32 / n as f32;
            let color = [
                theme.trace_bright[0],
                theme.trace_bright[1],
                theme.trace_bright[2],
                (0.15 + 0.85 * age) * theme.trace_bright[3],
            ];
            mesh.line(prev, p, 1.2, color);
            prev = p;
            i += step;
        }
    }

    // Correlation readout under the field.
    let strip = Rect::new(outer.x, field_area.y + field_area.h, outer.w, corr_h);
    draw_correlation(&mut Draw::new(mesh, m, theme), strip, interleaved);
}

/// Draws the correlation strip: a `[-1, +1]` bar filled from center toward the
/// measured coefficient (green toward mono/+1, red toward anti-phase/−1), with a
/// numeric readout. A silent/DC window (undefined correlation) shows a dash.
fn draw_correlation(d: &mut Draw, strip: Rect, interleaved: &[f32]) {
    let (mesh, m, theme) = d.parts();
    if strip.w <= 0.0 || strip.h <= 0.0 {
        return;
    }
    let n = interleaved.len() / 2;
    let mut left = Vec::with_capacity(n);
    let mut right = Vec::with_capacity(n);
    for i in 0..n {
        left.push(interleaved[2 * i]);
        right.push(interleaved[2 * i + 1]);
    }
    let r = correlation(&left, &right);

    // The bar track, centered vertically with a small inset.
    let bar_h = (strip.h - 2.0 * m.pad).max(2.0);
    let bar = Rect::new(strip.x, strip.y + (strip.h - bar_h) * 0.5, strip.w, bar_h);
    mesh.rect(bar, theme.field);
    let cx = bar.x + bar.w * 0.5;
    mesh.line([cx, bar.y], [cx, bar.y + bar.h], m.divider_w, theme.grid); // zero tick
    if let Some(r) = r {
        let half = bar.w * 0.5;
        let fill = half * r.abs().clamp(0.0, 1.0);
        let color = if r >= 0.0 { theme.accent } else { theme.warn };
        let x = if r >= 0.0 { cx } else { cx - fill };
        mesh.rect(Rect::new(x, bar.y, fill, bar.h), color);
    }
    mesh.border(bar, m.divider_w, theme.grid);
    let text = match r {
        Some(r) => format!("r {r:+.2}"),
        None => "r  --".to_string(),
    };
    let w = font::width(&text, m.text_scale);
    font::text(
        mesh,
        &text,
        (cx - w * 0.5).max(bar.x),
        bar.y + (bar.h - font::height(m.text_scale)) * 0.5,
        m.text_scale,
        theme.text,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::metrics::Metrics;
    use crate::host::paint::Mesh;
    use crate::host::theme::Theme;

    /// An interleaved window of `n` pairs from two channel functions.
    fn window(n: usize, l: impl Fn(usize) -> f32, r: impl Fn(usize) -> f32) -> Vec<f32> {
        (0..n).flat_map(|i| [l(i), r(i)]).collect()
    }

    #[test]
    fn draws_trail_and_readout_for_a_stereo_window() {
        let w = window(
            256,
            |i| (i as f32 * 0.2).sin(),
            |i| (i as f32 * 0.2 + 1.0).sin(),
        );
        let mut mesh = Mesh::new();
        draw_phasescope(
            &mut Draw::new(&mut mesh, &Metrics::default(), &Theme::default()),
            Rect::new(0.0, 0.0, 200.0, 240.0),
            &w,
            Some("phase"),
        );
        assert!(!mesh.is_empty(), "a stereo window draws geometry");
    }

    #[test]
    fn an_empty_window_draws_only_chrome() {
        let mut mesh = Mesh::new();
        draw_phasescope(
            &mut Draw::new(&mut mesh, &Metrics::default(), &Theme::default()),
            Rect::new(0.0, 0.0, 200.0, 240.0),
            &[],
            None,
        );
        // The field, cross and correlation track still draw; it does not panic
        // and produces some geometry (the frame), just no trail.
        assert!(!mesh.is_empty());
    }

    #[test]
    fn odd_length_window_is_safe() {
        let mut mesh = Mesh::new();
        // Three floats = one full pair plus a stray; must not read out of range.
        draw_phasescope(
            &mut Draw::new(&mut mesh, &Metrics::default(), &Theme::default()),
            Rect::new(0.0, 0.0, 120.0, 160.0),
            &[0.1, 0.2, 0.3],
            None,
        );
    }
}
