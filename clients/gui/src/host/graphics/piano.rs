//! The virtual piano keyboard's graphic primitives: a playable keyboard laid
//! out with **real piano proportions** (relative units, so it resizes freely),
//! and an overview strip spanning the full MIDI range for zoom/pan navigation
//! — all pure over a [`Draw`] (the flat-geometry [`crate::host::paint`]
//! painter), so everything is unit-testable without a window.
//!
//! The key geometry follows the real instrument, expressed only as ratios of
//! the white-key width (the one derived unit): every white key is 1 unit wide,
//! a black key [`BLACK_W`] units wide and [`BLACK_LEN`] of the white key's
//! length, and the black keys are **not** centered on the white-key boundaries
//! — within the C–E group the rear (upper) segments of C, D and E are equal
//! (`(3 − 2b)/3` each), within F–B those of F, G, A and B are equal
//! (`(4 − 3b)/4`), which is how the physical keyboard distributes them. All
//! layout derives from the widget rect, so the drawing scales with it.
//!
//! Everything here is **display logic plus message shaping**; the one piece of
//! general musical knowledge — note spelling and the MIDI↔hertz map — lives in
//! `clausters_core::scale` per the placement rule.

use clausters_core::scale;

use crate::host::font;
use crate::host::layout::Rect;
use crate::host::metrics::Metrics;
use crate::host::paint::Draw;
use crate::host::theme::with_alpha;

// --- Proportions (white-key units) -----------------------------------------

/// Black-key width as a fraction of the white-key width — the real
/// instrument's 13.7 mm over 23.5 mm.
pub const BLACK_W: f32 = 13.7 / 23.5;
/// Black-key length as a fraction of the white-key length.
pub const BLACK_LEN: f32 = 0.6;
/// The full MIDI range the overview spans and pan/zoom clamp to.
pub const MIDI_MAX: i32 = 127;
/// The smallest visible span zoom can reach (one octave), in semitones.
pub const MIN_SPAN: i32 = 12;

/// The rear (upper) segment width of the C–E group's white keys, so C#/D# and
/// the three segments share the 3-unit span exactly: `3·cr + 2·b = 3`.
const C_REAR: f32 = (3.0 - 2.0 * BLACK_W) / 3.0;
/// The rear segment width of the F–B group's white keys: `4·fr + 3·b = 4`.
const F_REAR: f32 = (4.0 - 3.0 * BLACK_W) / 4.0;

/// White keys below each pitch class within one octave (cumulative count).
const WHITES_BEFORE: [i32; 12] = [0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6];
/// The pitch class of each white key index within one octave (C D E F G A B).
const WHITE_PCS: [i32; 7] = [0, 2, 4, 5, 7, 9, 11];

/// A black key's left offset within its octave, in white-key units from the
/// octave's C. `None` for a white pitch class.
pub fn black_offset(pc: usize) -> Option<f32> {
    match pc {
        1 => Some(C_REAR),                              // C#
        3 => Some(2.0 * C_REAR + BLACK_W),              // D#
        6 => Some(3.0 + F_REAR),                        // F#
        8 => Some(3.0 + 2.0 * F_REAR + BLACK_W),        // G#
        10 => Some(3.0 + 3.0 * F_REAR + 2.0 * BLACK_W), // A#
        _ => None,
    }
}

// --- Layout -----------------------------------------------------------------

/// The regions and derived units of a `piano` widget rect: the optional label
/// strip is already consumed; `keys` is the playable keyboard, `overview` the
/// full-range navigation strip above it (when on). `min`/`max` are the visible
/// range (min snapped down to a white key), `white_w` the derived white-key
/// width in device pixels. The renderer and the hit-test both build this, so a
/// key occupies the same pixels either way.
#[derive(Clone, Copy, Debug)]
pub struct Layout {
    pub keys: Rect,
    pub overview: Option<Rect>,
    pub min: i32,
    pub max: i32,
    pub white_w: f32,
}

/// Snap a MIDI note down to a white key (identity for whites).
pub fn snap_white_down(midi: i32) -> i32 {
    let mut m = midi.clamp(0, MIDI_MAX);
    while scale::is_black_key(m) {
        m -= 1;
    }
    m
}

/// Snap a MIDI note up to a white key (identity for whites).
pub fn snap_white_up(midi: i32) -> i32 {
    let mut m = midi.clamp(0, MIDI_MAX);
    while scale::is_black_key(m) {
        m += 1;
    }
    m
}

/// The cumulative white-key x of a MIDI note from MIDI 0, in white-key units:
/// a white key's own left edge, or (for a black key) the left edge of the
/// white key below it plus the in-octave black offset fraction.
pub fn white_units(midi: i32) -> f32 {
    let octave = midi.div_euclid(12);
    let pc = scale::pitch_class(midi);
    match black_offset(pc) {
        Some(off) => octave as f32 * 7.0 + off,
        None => (octave * 7 + WHITES_BEFORE[pc]) as f32,
    }
}

/// How many white keys `[min, max]` spans (inclusive).
pub fn n_white(min: i32, max: i32) -> usize {
    (min..=max).filter(|&p| !scale::is_black_key(p)).count()
}

/// Compute the widget's layout: an optional label strip at the top, the
/// overview strip (when `overview`), and the keyboard filling the rest. The
/// visible range is normalized so both edges are white keys (min down, max up)
/// — the keyboard always starts and ends on a full white key.
pub fn layout(
    rect: Rect,
    min: i32,
    max: i32,
    overview: bool,
    has_label: bool,
    m: &Metrics,
) -> Layout {
    let label_h = if has_label {
        font::height(m.label_scale) + m.pad
    } else {
        0.0
    };
    let min = snap_white_down(min.min(max));
    let max = snap_white_up(max.max(min)).max(min);
    let top = rect.y + label_h;
    let inner_h = (rect.h - label_h).max(0.0);
    // The navigation strip is a ruler-thin band over the keys.
    let ov_h = if overview {
        m.ruler_h.min(inner_h)
    } else {
        0.0
    };
    let ov = (ov_h > 0.0).then(|| Rect::new(rect.x, top, rect.w, ov_h));
    let keys = Rect::new(rect.x, top + ov_h, rect.w, (inner_h - ov_h).max(0.0));
    let white_w = keys.w / n_white(min, max).max(1) as f32;
    Layout {
        keys,
        overview: ov,
        min,
        max,
        white_w,
    }
}

/// The pixel rect of one key in the visible range (`None` outside it): a white
/// key spans the full keyboard height, a black key the top [`BLACK_LEN`].
pub fn key_rect(l: &Layout, midi: i32) -> Option<Rect> {
    if midi < l.min || midi > l.max {
        return None;
    }
    let x = l.keys.x + (white_units(midi) - white_units(l.min)) * l.white_w;
    if scale::is_black_key(midi) {
        Some(Rect::new(
            x,
            l.keys.y,
            BLACK_W * l.white_w,
            BLACK_LEN * l.keys.h,
        ))
    } else {
        Some(Rect::new(x, l.keys.y, l.white_w, l.keys.h))
    }
}

/// The key under `(x, y)` in the keyboard area, if any — black keys win over
/// the white keys they overlap (they are drawn on top).
pub fn hit(l: &Layout, x: f32, y: f32) -> Option<i32> {
    if !l.keys.contains(x as f64, y as f64) {
        return None;
    }
    // The absolute white-unit coordinate from MIDI 0.
    let u = (x - l.keys.x) / l.white_w.max(1e-6) + white_units(l.min);
    // Black keys first, but only within their shorter length.
    if y < l.keys.y + BLACK_LEN * l.keys.h {
        let octave = (u / 7.0).floor() as i32;
        for oct in [octave - 1, octave, octave + 1] {
            for pc in [1, 3, 6, 8, 10] {
                let p = oct * 12 + pc;
                if !(l.min..=l.max).contains(&p) || !(0..=MIDI_MAX).contains(&p) {
                    continue;
                }
                let left = white_units(p);
                if u >= left && u < left + BLACK_W {
                    return Some(p);
                }
            }
        }
    }
    // Then the white column under the cursor.
    let w = u.floor() as i32;
    let octave = w.div_euclid(7);
    let p = octave * 12 + WHITE_PCS[w.rem_euclid(7) as usize];
    ((l.min..=l.max).contains(&p) && (0..=MIDI_MAX).contains(&p)).then_some(p)
}

/// The MIDI velocity a press height maps to within a key: 1 at the rear (top)
/// edge, 127 at the front (bottom) edge — striking closer to the front plays
/// louder.
pub fn velocity_at(l: &Layout, midi: i32, y: f32) -> i32 {
    let Some(r) = key_rect(l, midi) else {
        return 100;
    };
    let frac = ((y - r.y) / r.h.max(1.0)).clamp(0.0, 1.0);
    1 + (frac * 126.0).round() as i32
}

// --- Navigation (pan / zoom over the MIDI range) ----------------------------

/// Pan the visible range by `delta` semitones, keeping the span and clamping
/// to the MIDI range; the result's min snaps down to a white key.
pub fn pan_range(min: i32, max: i32, delta: i32) -> (i32, i32) {
    let span = (max - min).max(1);
    let new_min = (min + delta).clamp(0, MIDI_MAX - span);
    let new_min = snap_white_down(new_min);
    (new_min, (new_min + span).min(MIDI_MAX))
}

/// Pan by whole white keys (the wheel-over-the-keys step): `steps` white keys
/// right (positive) or left (negative).
pub fn pan_white(min: i32, max: i32, steps: i32) -> (i32, i32) {
    let mut m = snap_white_down(min);
    let span = (max - min).max(1);
    for _ in 0..steps.abs() {
        let next = if steps > 0 { m + 1 } else { m - 1 };
        let next = if steps > 0 {
            snap_white_up(next)
        } else {
            snap_white_down(next)
        };
        if next < 0 || next + span > MIDI_MAX {
            break;
        }
        m = next;
    }
    (m, (m + span).min(MIDI_MAX))
}

/// Zoom the visible range by `factor` (>1 widens, <1 narrows) anchored at
/// `anchor` (a MIDI note that keeps its on-screen position): the span clamps
/// to `[MIN_SPAN, 127]`, the range to the MIDI axis, and min snaps to a white
/// key.
pub fn zoom_range(min: i32, max: i32, factor: f64, anchor: f64) -> (i32, i32) {
    let span = (max - min).max(1) as f64;
    let new_span = (span * factor)
        .round()
        .clamp(MIN_SPAN as f64, MIDI_MAX as f64) as i32;
    let frac = ((anchor - min as f64) / span).clamp(0.0, 1.0);
    let new_min = (anchor - frac * new_span as f64).round() as i32;
    let new_min = snap_white_down(new_min.clamp(0, MIDI_MAX - new_span));
    (new_min, (new_min + new_span).min(MIDI_MAX))
}

// --- Overview strip ---------------------------------------------------------

/// The x pixel of a MIDI note on the overview strip (which always spans the
/// full `0..=127` range).
pub fn overview_key_x(strip: Rect, midi: i32) -> f32 {
    strip.x + midi as f32 / (MIDI_MAX + 1) as f32 * strip.w
}

/// The MIDI note under an overview-strip x pixel (clamped into range).
pub fn overview_hit(strip: Rect, x: f32) -> i32 {
    (((x - strip.x) / strip.w.max(1.0)) * (MIDI_MAX + 1) as f32).floor() as i32
}

/// Center the visible range on `midi` (the overview click/drag), keeping the
/// span; min snaps to a white key.
pub fn center_range(min: i32, max: i32, midi: i32) -> (i32, i32) {
    let span = (max - min).max(1);
    pan_range(min, max, midi - (min + span / 2))
}

// --- Drawing ----------------------------------------------------------------

/// What [`draw`] paints beyond the geometry: the pressed keys, the active
/// (non-grayed) range and the optional label.
pub struct PianoDraw<'a> {
    pub pressed: &'a [i32],
    pub active_min: i32,
    pub active_max: i32,
    pub label: Option<&'a str>,
}

/// Draw the keyboard: white keys with a hairline gap, black keys on top,
/// pressed keys highlighted, keys outside the active range grayed, and a note
/// name on each C when the white keys are wide enough to carry it.
pub fn draw(d: &mut Draw, l: &Layout, state: &PianoDraw) {
    if l.keys.w <= 0.0 || l.keys.h <= 0.0 {
        return;
    }
    let (mesh, m, theme) = d.parts();
    mesh.rect(l.keys, theme.key_gap);
    let active = |p: i32| p >= state.active_min && p <= state.active_max;
    // White keys first (with a 1px gap between them), then the C labels, then
    // the black keys on top.
    for p in l.min..=l.max {
        if scale::is_black_key(p) {
            continue;
        }
        let Some(r) = key_rect(l, p) else { continue };
        let color = if state.pressed.contains(&p) {
            theme.key_pressed
        } else if active(p) {
            theme.key_white
        } else {
            theme.key_inactive
        };
        mesh.rect(Rect::new(r.x + 0.5, r.y, (r.w - 1.0).max(1.0), r.h), color);
        if scale::pitch_class(p) == 0 {
            let name = scale::note_name(p);
            if font::width(&name, m.micro_scale) + 2.0 <= l.white_w {
                font::text(
                    mesh,
                    &name,
                    r.x + 2.0,
                    r.y + r.h - font::height(m.micro_scale) - 2.0,
                    m.micro_scale,
                    theme.key_label,
                );
            }
        }
    }
    for p in l.min..=l.max {
        if !scale::is_black_key(p) {
            continue;
        }
        let Some(r) = key_rect(l, p) else { continue };
        let color = if state.pressed.contains(&p) {
            theme.key_pressed_black
        } else if active(p) {
            theme.key_black
        } else {
            theme.key_inactive_black
        };
        mesh.rect(r, color);
    }
    mesh.border(l.keys, m.divider_w, theme.frame);
}

/// Draw the overview strip: the full MIDI range compressed, the active range
/// lit against the gray outside, black-key positions shaded, the pressed keys
/// marked, and the visible window as a translucent box with bright edges — the
/// keyboard's "ruler", dragged to pan and wheeled to zoom.
pub fn draw_overview(
    d: &mut Draw,
    strip: Rect,
    min: i32,
    max: i32,
    active: (i32, i32),
    pressed: &[i32],
) {
    if strip.w <= 0.0 || strip.h <= 0.0 {
        return;
    }
    let (mesh, m, theme) = d.parts();
    mesh.rect(strip, theme.key_overview);
    // The active range reads as the lit band.
    let (a0, a1) = (active.0.clamp(0, MIDI_MAX), active.1.clamp(0, MIDI_MAX));
    if a1 >= a0 {
        let x0 = overview_key_x(strip, a0);
        let x1 = overview_key_x(strip, a1 + 1);
        mesh.rect(
            Rect::new(x0, strip.y, x1 - x0, strip.h),
            theme.key_overview_active,
        );
    }
    // Black-key shading in the strip's lower half sketches the octaves.
    let key_w = strip.w / (MIDI_MAX + 1) as f32;
    for p in 0..=MIDI_MAX {
        if scale::is_black_key(p) {
            let x = overview_key_x(strip, p);
            mesh.rect(
                Rect::new(x, strip.y, key_w.max(1.0), strip.h * 0.5),
                theme.key_overview_black,
            );
        }
    }
    for &p in pressed {
        if (0..=MIDI_MAX).contains(&p) {
            let x = overview_key_x(strip, p);
            mesh.rect(
                Rect::new(x, strip.y + strip.h * 0.6, key_w.max(1.5), strip.h * 0.4),
                theme.flag,
            );
        }
    }
    // The visible window box.
    let wx0 = overview_key_x(strip, min);
    let wx1 = overview_key_x(strip, max + 1);
    let window = Rect::new(wx0, strip.y, (wx1 - wx0).max(2.0), strip.h);
    mesh.rect(window, with_alpha(theme.accent, 0.35));
    mesh.border(window, m.divider_w, theme.hilite);
    mesh.border(strip, m.divider_w, theme.frame);
}

/// Draw the whole widget: the label strip (when labelled), the overview and
/// the keyboard — the one entry the frame renderer calls.
#[allow(clippy::too_many_arguments)] // one widget's flat prop set, all scalars
pub fn draw_widget(
    d: &mut Draw,
    rect: Rect,
    min: i32,
    max: i32,
    overview: bool,
    active_min: i32,
    active_max: i32,
    pressed: &[i32],
    label: Option<&str>,
) {
    let m = d.m;
    if let Some(text) = label {
        font::text(
            d.mesh,
            text,
            rect.x + m.pad,
            rect.y + 2.0,
            m.label_scale,
            d.theme.text,
        );
    }
    let l = layout(rect, min, max, overview, label.is_some(), m);
    if let Some(strip) = l.overview {
        draw_overview(d, strip, l.min, l.max, (active_min, active_max), pressed);
    }
    draw(
        d,
        &l,
        &PianoDraw {
            pressed,
            active_min,
            active_max,
            label,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::paint::Mesh;
    use crate::host::theme::Theme;

    fn l(min: i32, max: i32) -> Layout {
        layout(
            Rect::new(0.0, 0.0, 700.0, 120.0),
            min,
            max,
            false,
            false,
            &Metrics::default(),
        )
    }

    #[test]
    fn an_octave_spans_exactly_seven_white_units() {
        assert_eq!(white_units(72) - white_units(60), 7.0);
        assert_eq!(white_units(12) - white_units(0), 7.0);
        // Consecutive white keys are 1 unit apart.
        assert_eq!(white_units(62) - white_units(60), 1.0); // C→D
        assert_eq!(white_units(65) - white_units(64), 1.0); // E→F (no black between)
    }

    #[test]
    fn rear_segments_are_equal_within_each_group() {
        // C–E: the segments left of C#, between C# and D#, and right of D#.
        let c_sharp = black_offset(1).unwrap();
        let d_sharp = black_offset(3).unwrap();
        let seg1 = c_sharp; // C's rear
        let seg2 = d_sharp - (c_sharp + BLACK_W); // D's rear
        let seg3 = 3.0 - (d_sharp + BLACK_W); // E's rear
        assert!((seg1 - C_REAR).abs() < 1e-6);
        assert!((seg2 - C_REAR).abs() < 1e-6);
        assert!((seg3 - C_REAR).abs() < 1e-6);
        // F–B: four equal rear segments around F#, G#, A#.
        let f_sharp = black_offset(6).unwrap();
        let g_sharp = black_offset(8).unwrap();
        let a_sharp = black_offset(10).unwrap();
        assert!((f_sharp - 3.0 - F_REAR).abs() < 1e-6);
        assert!((g_sharp - (f_sharp + BLACK_W) - F_REAR).abs() < 1e-6);
        assert!((a_sharp - (g_sharp + BLACK_W) - F_REAR).abs() < 1e-6);
        assert!((7.0 - (a_sharp + BLACK_W) - F_REAR).abs() < 1e-6);
        // The published ratios hold.
        assert!((BLACK_W - 13.7 / 23.5).abs() < 1e-6);
        assert!((BLACK_LEN - 0.6).abs() < 1e-6);
    }

    #[test]
    fn layout_normalizes_the_range_to_white_edges() {
        // A black min snaps down, a black max snaps up.
        let lay = l(61, 70); // C#4..A#4
        assert_eq!(lay.min, 60); // C4
        assert_eq!(lay.max, 71); // B4
        // One octave C..B = 7 white keys over 700 px → 100 px per white key.
        assert_eq!(n_white(60, 71), 7);
        assert!((lay.white_w - 100.0).abs() < 1e-3);
    }

    #[test]
    fn key_rects_follow_the_proportions() {
        let lay = l(60, 71);
        let c = key_rect(&lay, 60).unwrap();
        assert_eq!((c.x, c.w), (0.0, lay.white_w));
        assert_eq!(c.h, 120.0);
        let c_sharp = key_rect(&lay, 61).unwrap();
        assert!((c_sharp.x - C_REAR * lay.white_w).abs() < 1e-3);
        assert!((c_sharp.w - BLACK_W * lay.white_w).abs() < 1e-3);
        assert!((c_sharp.h - BLACK_LEN * 120.0).abs() < 1e-3);
        // Out of the visible range: no rect.
        assert!(key_rect(&lay, 59).is_none());
        assert!(key_rect(&lay, 72).is_none());
    }

    #[test]
    fn hit_prefers_black_keys_and_respects_their_length() {
        let lay = l(60, 71);
        let cs = key_rect(&lay, 61).unwrap();
        // In the black key's zone: the black key wins.
        assert_eq!(hit(&lay, cs.x + cs.w * 0.5, cs.y + cs.h * 0.5), Some(61));
        // Same x below the black key's length: the white key underneath (C#'s
        // left half overlaps C).
        assert_eq!(hit(&lay, cs.x + 1.0, 119.0), Some(60));
        // A plain white press.
        let d = key_rect(&lay, 62).unwrap();
        assert_eq!(hit(&lay, d.x + d.w * 0.5, 119.0), Some(62));
        // Outside the keyboard: no key.
        assert_eq!(hit(&lay, -5.0, 60.0), None);
        assert_eq!(hit(&lay, 5.0, 200.0), None);
    }

    #[test]
    fn hit_works_from_an_arbitrary_snapped_min() {
        // A range starting mid-octave (F#3 snaps down to F3 = 53).
        let lay = layout(
            Rect::new(50.0, 10.0, 400.0, 100.0),
            54,
            65,
            false,
            false,
            &Metrics::default(),
        );
        assert_eq!(lay.min, 53);
        let f = key_rect(&lay, 53).unwrap();
        assert_eq!(hit(&lay, f.x + 2.0, f.y + f.h - 1.0), Some(53));
        let fs = key_rect(&lay, 54).unwrap();
        assert_eq!(hit(&lay, fs.x + fs.w * 0.5, fs.y + fs.h * 0.5), Some(54));
    }

    #[test]
    fn velocity_runs_from_rear_1_to_front_127() {
        let lay = l(60, 71);
        let c = key_rect(&lay, 60).unwrap();
        assert_eq!(velocity_at(&lay, 60, c.y), 1);
        assert_eq!(velocity_at(&lay, 60, c.y + c.h), 127);
        let mid = velocity_at(&lay, 60, c.y + c.h * 0.5);
        assert!((63..=65).contains(&mid), "got {mid}");
        // A black key maps over its own (shorter) length.
        let cs = key_rect(&lay, 61).unwrap();
        assert_eq!(velocity_at(&lay, 61, cs.y + cs.h), 127);
    }

    #[test]
    fn pan_clamps_and_keeps_the_span() {
        assert_eq!(pan_range(60, 72, 12), (72, 84));
        // Clamped at the top: the span holds against 127.
        let (min, max) = pan_range(100, 124, 12);
        assert_eq!(max - min, 24);
        assert!(max <= MIDI_MAX);
        // Clamped at the bottom.
        assert_eq!(pan_range(0, 24, -12), (0, 24));
        // The result's min is always a white key.
        let (min, _) = pan_range(60, 72, 1);
        assert!(!scale::is_black_key(min));
    }

    #[test]
    fn pan_white_steps_whole_white_keys() {
        let (min, max) = pan_white(60, 72, 1);
        assert_eq!((min, max), (62, 74)); // C→D
        let (min, _) = pan_white(60, 72, -1);
        assert_eq!(min, 59); // C→B below
        // At the edge it stays put.
        assert_eq!(pan_white(0, 12, -1), (0, 12));
    }

    #[test]
    fn zoom_is_anchored_clamped_and_white_snapped() {
        // Zooming in around the middle keeps the anchor inside.
        let (min, max) = zoom_range(36, 96, 0.5, 66.0);
        assert!(min <= 66 && 66 <= max);
        assert!(max - min < 60);
        assert!(!scale::is_black_key(min));
        // The span never shrinks below one octave…
        let (min, max) = zoom_range(60, 72, 0.1, 66.0);
        assert_eq!(max - min, MIN_SPAN);
        // …and never grows past the MIDI range.
        let (min, max) = zoom_range(0, 120, 4.0, 60.0);
        assert!(min >= 0 && max <= MIDI_MAX);
        assert!(max - min >= 120);
    }

    #[test]
    fn overview_maps_round_trip() {
        let strip = Rect::new(10.0, 0.0, 256.0, 18.0);
        for p in [0, 21, 60, 108, 127] {
            let x = overview_key_x(strip, p);
            assert_eq!(overview_hit(strip, x + 0.5), p);
        }
        // Clamping happens at the caller (gestures) — the raw map is linear.
        let (min, max) = center_range(60, 72, 100);
        assert_eq!(max - min, 12);
        assert!((min..=max).contains(&100) || max == MIDI_MAX);
    }

    #[test]
    fn drawing_covers_keys_and_overview() {
        let mut mesh = Mesh::new();
        draw_widget(
            &mut Draw::new(&mut mesh, &Metrics::default(), &Theme::default()),
            Rect::new(0.0, 0.0, 700.0, 140.0),
            60,
            72,
            true,
            0,
            127,
            &[60, 61],
            Some("piano"),
        );
        assert!(mesh.vertex_count() > 0);
        // Grayed keys still draw (they are visible, just inactive).
        let mut gray = Mesh::new();
        draw_widget(
            &mut Draw::new(&mut gray, &Metrics::default(), &Theme::default()),
            Rect::new(0.0, 0.0, 700.0, 140.0),
            60,
            72,
            false,
            64,
            67,
            &[],
            None,
        );
        assert!(gray.vertex_count() > 0);
    }
}
