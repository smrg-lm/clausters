//! The 2D workspace's navigation math — pure, window-free.
//!
//! A `scroll` container shows its virtual content area through a 2D window:
//! per axis a pan offset in content units ([`ScrollView::view_x`]/`view_y`,
//! the content coordinate at the widget's top-left edge) plus one **uniform
//! scale** ([`ScrollView::zoom`], physical pixels per content unit, defaulting
//! to the window's UI scale) shared
//! by both axes so the plane never distorts. The zoom keeps the content point
//! under the cursor fixed — the same pivot math as [`View::zoom`], expressed
//! on the scale factor because the two axes share it (a per-axis [`View`]
//! window would let the clamp de-couple the scales). Pan clamps each axis to
//! `[0, content - visible]`; when the window shows more than the content
//! (zoomed out past it) the axis pins to `0` and the slack stays empty, so a
//! wide-but-short plane zooms out freely.
//!
//! [`View`]: crate::viewport::View
//! [`View::zoom`]: crate::viewport::View::zoom
//! [`ScrollView`]: super::widget::ScrollView
//! [`ScrollView::view_x`]: super::widget::ScrollView::view_x
//! [`ScrollView::zoom`]: super::widget::ScrollView::zoom

use super::layout::Rect;

/// The zoom bounds: 1/8 of natural size out to 8x in — generous for a patch
/// canvas or an arrangement plane while keeping the mesh geometry sane.
pub const MIN_ZOOM: f64 = 0.125;
pub const MAX_ZOOM: f64 = 8.0;

/// Device pixels one wheel step pans when zoom is disabled (a plain scroll
/// view's wheel), before the zoom divides it back into content units.
pub const WHEEL_PAN_PX: f64 = 48.0;

/// How far past its content edges the **free plane** may pan, as a fraction of
/// the visible size — see [`clamp_pan`]. Half a viewport all around: enough
/// that every drag direction does something wherever you are (a plane pinned
/// exactly at its content corner reads as broken), and little enough that the
/// content can never be lost off-screen.
pub const SLACK: f64 = 0.5;

/// Clamps a zoom factor into the workspace's bounds (and away from 0/NaN).
pub fn clamp_zoom(zoom: f64) -> f64 {
    if zoom.is_finite() {
        zoom.clamp(MIN_ZOOM, MAX_ZOOM)
    } else {
        1.0
    }
}

/// Clamps one axis' pan offset against the content, with `slack` content units
/// of overscroll allowed past each edge: the window may start anywhere in
/// `[-slack, content - visible + slack]`.
///
/// **The two `scroll` shapes want different bounds, and that is the point.** A
/// *scroll view* is a bounded document — you cannot scroll above its first row
/// — so the constrained axes pass `slack = 0` and get the strict
/// `[0, content - visible]`. The *free plane* is conceptually unbounded and
/// `content_w`/`content_h` merely say where its contents happen to sit, so it
/// passes [`SLACK`] and can always be dragged in any direction. That split is
/// the milestone's "general first, constrained by configuration" rule applied
/// to the bounds themselves: the strict clamp is the *constrained* behavior,
/// and applying it to the plane too (as this first did) makes the general case
/// inherit a restriction that only the special case means.
///
/// When the window already shows more than the content, the axis pins to `0`
/// (plus its slack) and the leftover stays empty.
pub fn clamp_pan(start: f64, viewport_px: f32, zoom: f64, content: f32, slack: f64) -> f64 {
    let visible = viewport_px.max(0.0) as f64 / clamp_zoom(zoom);
    let slack = (visible * slack).max(0.0);
    let far = (content as f64 - visible).max(0.0);
    start.clamp(-slack, far + slack)
}

/// Zooms by `factor` (>1 zooms in) keeping the content point under the cursor
/// `(cx, cy)` (device pixels inside `area`) fixed. The pivot math of
/// [`View::zoom`](crate::viewport::View::zoom) on the shared scale: the
/// content coordinate under the cursor before and after the scale change is
/// equated and the pan absorbs the difference. The zoom is clamped here; the
/// pan offsets come back raw and clamp at the one door every scroll write
/// goes through (`interact::scroll_set_view`, which knows the content size).
pub fn zoom_at(
    (view_x, view_y, zoom): (f64, f64, f64),
    area: Rect,
    (cx, cy): (f64, f64),
    factor: f64,
) -> (f64, f64, f64) {
    let old = clamp_zoom(zoom);
    let new = clamp_zoom(old * factor);
    let (px, py) = (cx - area.x as f64, cy - area.y as f64);
    (
        view_x + px / old - px / new,
        view_y + py / old - py / new,
        new,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_keeps_the_content_point_under_the_cursor_fixed() {
        let area = Rect::new(10.0, 10.0, 400.0, 300.0);
        let (vx, vy, z) = (100.0, 50.0, 1.0);
        let (cx, cy) = (210.0, 160.0); // 200, 150 inside the area
        let before = (vx + 200.0 / z, vy + 150.0 / z);
        let (nx, ny, nz) = zoom_at((vx, vy, z), area, (cx, cy), 2.0);
        assert_eq!(nz, 2.0);
        let after = (nx + 200.0 / nz, ny + 150.0 / nz);
        assert!((before.0 - after.0).abs() < 1e-9);
        assert!((before.1 - after.1).abs() < 1e-9);
    }

    #[test]
    fn zoom_clamps_to_the_bounds() {
        let area = Rect::new(0.0, 0.0, 400.0, 300.0);
        let (_, _, z) = zoom_at((0.0, 0.0, 1.0), area, (0.0, 0.0), 1e9);
        assert_eq!(z, MAX_ZOOM);
        let (_, _, z) = zoom_at((0.0, 0.0, 1.0), area, (0.0, 0.0), 1e-9);
        assert_eq!(z, MIN_ZOOM);
        assert_eq!(clamp_zoom(f64::NAN), 1.0);
    }

    #[test]
    fn a_bounded_axis_clamps_hard_to_the_content() {
        // A scroll view (slack 0): 400 px at zoom 2 shows 200 content units of
        // 1000, so the start lives in [0, 800] — no scrolling above the first
        // row, none past the last.
        assert_eq!(clamp_pan(1e6, 400.0, 2.0, 1000.0, 0.0), 800.0);
        assert_eq!(clamp_pan(-5.0, 400.0, 2.0, 1000.0, 0.0), 0.0);
        // The window shows more than the content: pinned to 0, leftover empty.
        assert_eq!(clamp_pan(50.0, 400.0, 0.25, 1000.0, 0.0), 0.0);
    }

    #[test]
    fn the_free_plane_may_overscroll_by_its_slack() {
        // The plane: 400 px at zoom 1 = 400 visible of 1000, slack half a
        // viewport, so the window lives in [-200, 800]. The point is that at
        // the content's origin the plane still moves *every* way — pinning it
        // exactly at the corner is what reads as a broken drag.
        assert_eq!(clamp_pan(-1e6, 400.0, 1.0, 1000.0, SLACK), -200.0);
        assert_eq!(clamp_pan(1e6, 400.0, 1.0, 1000.0, SLACK), 800.0);
        assert_eq!(clamp_pan(-50.0, 400.0, 1.0, 1000.0, SLACK), -50.0);
        // Slack scales with the *visible* size, so it survives a zoom: at
        // zoom 2 only 200 units are visible and the slack is 100.
        assert_eq!(clamp_pan(-1e6, 400.0, 2.0, 1000.0, SLACK), -100.0);
    }
}
