//! A widget's **natural size**: how big it wants to be, per axis.
//!
//! The layout is one pass with no measurement and no constraint solver, so a
//! natural size is a **pure function of the metrics table** ([`Metrics`]) and
//! of the widget's own *presentation* props (its `text_size`, whether it
//! carries a label, whether it wraps) — never of its **data**. A scope's
//! height must not follow its sample count and a label's width must not follow
//! its string: a size that reads the data turns a `/gui_set` into a relayout of
//! the window, which is both a visible jump and a per-message cost. A widget
//! with more content than room clips or scrolls; it does not grow.
//!
//! The split is **content vs surface**. Content whose extent the widget itself
//! knows is intrinsic: a label's line, a button's box, a toggle, a number, a
//! menu, a single-line field, a control's label strip. A surface whose extent
//! is the caller's stays elastic — a panel, a scroll, a patch canvas, a track,
//! the heavy views, a plot, a node tree, a canvas — and takes what the layout
//! gives it. Mixed is the ordinary case rather than the exception: a slider has
//! a natural thickness across its track and is elastic along it; a ruler strip
//! has a natural height and spans its axis.
//!
//! It is also a pure function of the placement's **scale** — the zoom a widget
//! is seen through inside a `scroll` workspace, which is what its own text draws
//! at (`text_size * scale`). A natural size that ignored it would promise a box
//! for a 14-pixel line and then draw a 28-pixel one: the text spills into the
//! next row, and a control that reserves strips for its label and its read-out
//! (a knob) has nothing left for the part that matters — its disc collapses to a
//! dot. Outside a workspace the scale is 1 and this is the identity.
//!
//! [`super::super::layout`] consumes this on a `row`/`col` main axis, in one
//! resolution order: explicit `w`/`h` → explicit `weight` → natural size → a
//! share of the leftover. The cross axis keeps filling.

use super::super::controls;
use super::super::font;
use super::super::metrics::Metrics;
use super::{Range, WidgetKind};

/// A widget's wanted extent per axis, `None` on an axis meaning elastic.
pub type Natural = (Option<f32>, Option<f32>);

/// The height of one row of text plus the padding above and below it — the box
/// a line of content needs.
fn line_box(size: f32, m: &Metrics) -> f32 {
    font::height(size) + 2.0 * m.pad
}

/// The height of one line of *control*: a row of text and its padding, never
/// under the shared `control_h`, so a button, a number field and a menu line up
/// in a row unaided.
fn control_box(size: f32, m: &Metrics) -> f32 {
    line_box(size, m).max(m.control_h)
}

/// The strip a labelled control reserves above its body (zero when unlabelled)
/// — [`super::super::controls::body_rect_at`]'s own reservation.
fn label_strip(has_label: bool, size: f32, m: &Metrics) -> f32 {
    if has_label {
        font::height(size) + m.pad
    } else {
        0.0
    }
}

/// The vertical inset a control's body takes inside its cell (top and bottom).
fn body_inset(m: &Metrics) -> f32 {
    2.0 * m.pad
}

impl WidgetKind {
    /// How big this widget wants to be, per axis — `None` meaning elastic (the
    /// layout decides). Pure over the metrics, the widget's presentation props
    /// and the placement `scale` its text draws at (1.0 outside a workspace);
    /// see the module documentation for the content/surface split.
    pub fn natural_size(&self, m: &Metrics, scale: f32) -> Natural {
        // Every text-derived extent is measured at the size the text will
        // actually be drawn at, which is the widget's `text_size` through the
        // placement's zoom.
        let text = |size: &f32| *size * scale;
        match self {
            // -- Content: the widget knows its own extent --
            WidgetKind::Label {
                text_size, wrap, ..
            } => (
                None,
                // A wrapped label's line count follows its string, which is
                // data: it stays elastic and clips what does not fit.
                (!wrap).then(|| line_box(text(text_size), m)),
            ),
            WidgetKind::Button { text_size, .. } => (None, Some(control_box(text(text_size), m))),
            // A toggle owns its cell: the box and its label sit on one row, so
            // the box's own side is the floor its height cannot go under.
            WidgetKind::Toggle { text_size, .. } => {
                (None, Some(control_box(text(text_size), m).max(m.box_side)))
            }
            WidgetKind::Number(r) => (None, Some(field_h(r, m, scale))),
            WidgetKind::Menu {
                label, text_size, ..
            } => (
                None,
                Some(
                    label_strip(label.is_some(), text(text_size), m)
                        + body_inset(m)
                        + control_box(text(text_size), m),
                ),
            ),
            WidgetKind::Text {
                label,
                text_size,
                multiline,
                ..
            } => (
                None,
                // A multiline field is a text *surface*: its height is the
                // caller's, and it scrolls its rows inside it.
                (!multiline).then(|| {
                    label_strip(label.is_some(), text(text_size), m)
                        + body_inset(m)
                        + control_box(text(text_size), m)
                }),
            ),

            // -- Mixed: a thickness across the control's axis, elastic along it --
            WidgetKind::Slider { range, vertical } => {
                if *vertical {
                    (Some(slider_across(m)), None)
                } else {
                    (None, Some(slider_thick(range, m, scale)))
                }
            }
            // A knob knows its height, not its width: the disc sizes itself to
            // the shorter side of its body and centres there, so extra width is
            // slack it absorbs, while extra height would stack it under dead
            // space. Elastic across, so a row of knobs still spreads.
            WidgetKind::Knob(r) => (None, Some(knob_h(r, m, scale))),
            // A ruler is a strip: it spans its axis and knows its thickness.
            WidgetKind::TimeRuler { .. } => (None, Some(m.ruler_h)),

            // -- Surfaces: the extent is the caller's --
            _ => (None, None),
        }
    }
}

/// A labelled field's height: its label strip, its body inset and one control
/// line (the read-out row).
fn field_h(r: &Range, m: &Metrics, scale: f32) -> f32 {
    let size = r.text_size * scale;
    label_strip(r.label.is_some(), size, m) + body_inset(m) + control_box(size, m)
}

/// A horizontal slider's thickness: the label strip, the body inset, the
/// handle's grip across the track and the read-out strip under it — the same
/// reservation the drawing makes ([`controls::slider_track`]), so the groove
/// gets the grip it asked for and the number gets its own row.
fn slider_thick(r: &Range, m: &Metrics, scale: f32) -> f32 {
    let size = r.text_size * scale;
    label_strip(r.label.is_some(), size, m)
        + body_inset(m)
        + m.handle_grip.max(m.handle_thick)
        + controls::readout_h(size, m)
}

/// A vertical slider's width: the grip across the track, inset in the body.
/// The value read-out shares that width and ellipsizes — a number's own length
/// is data, and no size here may follow it.
fn slider_across(m: &Metrics) -> f32 {
    body_inset(m) + m.handle_grip.max(m.box_side)
}

/// A knob's height: the label strip, the body inset, the disc and the read-out
/// strip the drawing reserves under it.
fn knob_h(r: &Range, m: &Metrics, scale: f32) -> f32 {
    let size = r.text_size * scale;
    label_strip(r.label.is_some(), size, m)
        + body_inset(m)
        + m.knob_d
        + controls::readout_h(size, m)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::controls;
    use crate::host::guidef::GuiNode;
    use crate::host::layout::Rect;
    use crate::host::widget::Widget;

    fn kind(json: &str) -> WidgetKind {
        Widget::from_node(1, &GuiNode::parse(json.as_bytes()).unwrap(), &[])
            .unwrap()
            .kind
    }

    #[test]
    fn a_surface_is_elastic_on_both_axes() {
        let m = Metrics::default();
        for json in [
            r#"{"type":"panel"}"#,
            r#"{"type":"scroll"}"#,
            r#"{"type":"waveform","data":[]}"#,
            r#"{"type":"plot","data":[]}"#,
            r#"{"type":"nodetree"}"#,
            r#"{"type":"track"}"#,
            r#"{"type":"patch"}"#,
        ] {
            assert_eq!(kind(json).natural_size(&m, 1.0), (None, None), "{json}");
        }
    }

    /// The regression a natural size made visible: a slider's read-out used to
    /// be drawn at the bottom-right of its whole body, which was fine while the
    /// body was as tall as the window and put the number **on the groove** once
    /// the body was only as tall as the control asked for. The reservation is
    /// now one thing, shared by the drawing, the hit math and this size.
    #[test]
    fn a_slider_keeps_its_groove_and_its_number_apart() {
        let m = Metrics::default();
        let size = crate::host::font::DEFAULT_SIZE;
        for (json, labelled) in [
            (r#"{"type":"slider","label":"amp"}"#, true),
            (r#"{"type":"slider"}"#, false),
        ] {
            let k = kind(json);
            let h = k
                .natural_size(&m, 1.0)
                .1
                .expect("a horizontal slider knows it");
            let cell = Rect::new(0.0, 0.0, 200.0, h);
            let body = controls::body_rect_at(cell, labelled, size, &m);
            let track = controls::slider_track(cell, labelled, size, &m);
            assert!(
                track.h >= m.handle_grip,
                "{json}: the groove lost its grip: {} < {}",
                track.h,
                m.handle_grip
            );
            assert!(
                (track.y + track.h + controls::readout_h(size, &m) - (body.y + body.h)).abs()
                    < 1e-3,
                "{json}: the read-out strip is not the rest of the body"
            );
        }
    }

    /// A widget seen through a workspace's zoom draws its text at
    /// `text_size * scale`, so its natural box has to be measured there too. The
    /// failure this pins is not subtle: a knob whose strips were measured at 1.0
    /// and drawn at 2.0 had nothing left for its disc, which collapsed to a dot.
    #[test]
    fn a_natural_size_is_measured_at_the_scale_its_text_draws_at() {
        let m = Metrics::default().resolved(2.0);
        let size = crate::host::font::DEFAULT_SIZE;
        for scale in [1.0, 2.0, 4.0] {
            let drawn = size * scale;
            let label = kind(r#"{"type":"label","text":"a"}"#)
                .natural_size(&m, scale)
                .1
                .unwrap();
            assert!(
                label >= crate::host::font::height(drawn),
                "at {scale} a label's box is smaller than its own line"
            );

            let cell = Rect::new(
                0.0,
                0.0,
                400.0,
                kind(r#"{"type":"knob","label":"amount"}"#)
                    .natural_size(&m, scale)
                    .1
                    .unwrap(),
            );
            // What the knob's drawing does with that height: its body, minus the
            // strip it reserves for the read-out, must still be the disc.
            let body = controls::body_rect_at(cell, true, drawn, &m);
            let disc = body.h - controls::readout_h(drawn, &m);
            assert!(
                (disc - m.knob_d).abs() < 1e-3,
                "at {scale} the disc got {disc} of the {} it asked for",
                m.knob_d
            );
        }
    }

    #[test]
    fn content_widgets_know_their_height_and_not_their_width() {
        let m = Metrics::default();
        for json in [
            r#"{"type":"label","text":"hello"}"#,
            r#"{"type":"button","label":"go"}"#,
            r#"{"type":"toggle"}"#,
            r#"{"type":"number"}"#,
            r#"{"type":"menu","options":["a","b"]}"#,
            r#"{"type":"text","value":"x"}"#,
        ] {
            let (w, h) = kind(json).natural_size(&m, 1.0);
            assert_eq!(w, None, "{json} spans its axis");
            let h = h.unwrap_or_else(|| panic!("{json} knows its height"));
            assert!(h >= m.control_h, "{json} fits a line of control: {h}");
        }
    }

    #[test]
    fn a_natural_size_never_reads_the_data() {
        // The same widget with a longer string, more options, more samples: the
        // size is the same, so a `/gui_set` never relayouts the window.
        let m = Metrics::default();
        let short = kind(r#"{"type":"label","text":"a"}"#).natural_size(&m, 1.0);
        let long = kind(r#"{"type":"label","text":"a much longer caption"}"#).natural_size(&m, 1.0);
        assert_eq!(short, long);
        let one = kind(r#"{"type":"menu","options":["a"]}"#).natural_size(&m, 1.0);
        let many = kind(r#"{"type":"menu","options":["a","b","c","d"]}"#).natural_size(&m, 1.0);
        assert_eq!(one, many);
    }

    #[test]
    fn a_wrapped_label_and_a_multiline_field_stay_elastic() {
        let m = Metrics::default();
        assert_eq!(
            kind(r#"{"type":"label","text":"a b c","wrap":1}"#).natural_size(&m, 1.0),
            (None, None)
        );
        assert_eq!(
            kind(r#"{"type":"text","value":"a","multiline":1}"#).natural_size(&m, 1.0),
            (None, None)
        );
    }

    #[test]
    fn a_label_strip_makes_a_control_taller() {
        let m = Metrics::default();
        let bare = kind(r#"{"type":"number"}"#)
            .natural_size(&m, 1.0)
            .1
            .unwrap();
        let titled = kind(r#"{"type":"number","label":"cutoff"}"#)
            .natural_size(&m, 1.0)
            .1
            .unwrap();
        assert!(
            titled > bare,
            "the label strip is reserved: {titled} > {bare}"
        );
        assert_eq!(titled - bare, font::height(font::DEFAULT_SIZE) + m.pad);
    }

    #[test]
    fn a_bigger_text_size_makes_a_control_taller() {
        let m = Metrics::default();
        let small = kind(r#"{"type":"button","label":"go"}"#)
            .natural_size(&m, 1.0)
            .1
            .unwrap();
        let big = kind(r#"{"type":"button","label":"go","text_size":4}"#)
            .natural_size(&m, 1.0)
            .1
            .unwrap();
        assert!(big > small);
    }

    #[test]
    fn a_slider_is_thick_across_its_axis_and_elastic_along_it() {
        let m = Metrics::default();
        let (w, h) = kind(r#"{"type":"slider"}"#).natural_size(&m, 1.0);
        assert_eq!(w, None, "a horizontal slider spans its track");
        assert!(h.unwrap() >= m.handle_grip);
        let (w, h) = kind(r#"{"type":"slider","vertical":1}"#).natural_size(&m, 1.0);
        assert!(w.unwrap() >= m.handle_grip);
        assert_eq!(h, None, "a vertical slider spans its track");
    }

    #[test]
    fn a_knob_is_as_tall_as_its_disc_and_spreads_across() {
        let m = Metrics::default();
        let (w, h) = kind(r#"{"type":"knob"}"#).natural_size(&m, 1.0);
        assert_eq!(w, None, "a row of knobs still spreads");
        assert!(h.unwrap() > m.knob_d, "the disc plus its read-out");
    }

    #[test]
    fn a_ruler_strip_is_as_thick_as_the_role() {
        let m = Metrics::default();
        assert_eq!(
            kind(r#"{"type":"timeruler"}"#).natural_size(&m, 1.0),
            (None, Some(m.ruler_h))
        );
    }

    #[test]
    fn density_scales_every_natural_size() {
        let compact = Metrics::generated(0.75);
        let comfortable = Metrics::generated(1.5);
        for json in [
            r#"{"type":"label","text":"a"}"#,
            r#"{"type":"button","label":"go"}"#,
            r#"{"type":"knob","label":"amp"}"#,
            r#"{"type":"slider"}"#,
        ] {
            let a = kind(json).natural_size(&compact, 1.0);
            let b = kind(json).natural_size(&comfortable, 1.0);
            assert!(
                b.1.unwrap() > a.1.unwrap(),
                "{json} follows the density: {a:?} {b:?}"
            );
        }
    }
}
