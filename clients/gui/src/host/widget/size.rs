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
//! **What a size may read, stated once.** The line is not "never the data", it
//! is **where the value is resolved**: a prop that settles at a *mutation
//! point* — a `/gui_def`, a `/gui_set` — may size, exactly where the theme
//! already resolves, while a value that *streams* (a scope's samples, a meter's
//! level, a bound number's turn) never can. So a `/gui_set` of a value still
//! cannot relayout a window. The two are kept apart by which function answers:
//! [`WidgetKind::natural_size`] reads no content at all, and only
//! [`Element::hug`](super::Element::hug) — asked of a leaf inside a container
//! that carries `hug`, and nowhere else — reads the content props a widget
//! draws (a label's text, a menu's options). A container that hugs has *asked*
//! for its size to follow what it holds; nothing else pays for it.
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
//! share of the leftover. The cross axis keeps filling. It asks
//! [`Widget::natural_size`], not the kind's, because a **container** may be
//! sized by what it holds — see [`Widget::hug_size`], which is still one
//! bottom-up walk over these same pure functions and not a measurement pass.

use super::super::font;
use super::super::metrics::{Metrics, snap_px};
use super::{Flow, Layout, Widget, WidgetKind};

/// A widget's wanted extent per axis, `None` on an axis meaning elastic.
pub type Natural = (Option<f32>, Option<f32>);

/// The height of one row of text plus the padding above and below it — the box
/// a line of content needs.
pub(crate) fn line_box(size: f32, m: &Metrics) -> f32 {
    font::height(size) + 2.0 * m.pad
}

/// The height of one line of *control*: a row of text and its padding, never
/// under the shared `control_h`, so a button, a number field and a menu line up
/// in a row unaided.
pub(crate) fn control_box(size: f32, m: &Metrics) -> f32 {
    line_box(size, m).max(m.control_h)
}

/// The strip a labelled control reserves above its body (zero when unlabelled)
/// — [`crate::host::graphics::controls::body_rect_at`]'s own reservation.
pub(crate) fn label_strip(has_label: bool, size: f32, m: &Metrics) -> f32 {
    if has_label {
        font::height(size) + m.pad
    } else {
        0.0
    }
}

/// The width a run of text needs in a **cell** it is drawn into with the
/// ordinary padding on both sides — a label's line, a button's caption.
pub(crate) fn text_box(text: &str, size: f32, m: &Metrics) -> f32 {
    font::width(text, size) + 2.0 * m.pad
}

/// The width a control's cell needs to show `text` inside its **body**: the
/// body is inset from the cell and the text from the body
/// ([`crate::host::graphics::controls::field`]), so a field pays the padding twice.
pub(crate) fn field_w(text: &str, size: f32, m: &Metrics) -> f32 {
    font::width(text, size) + 4.0 * m.pad
}

/// The vertical inset a control's body takes inside its cell (top and bottom).
pub(crate) fn body_inset(m: &Metrics) -> f32 {
    2.0 * m.pad
}

impl WidgetKind {
    /// How big this widget wants to be, per axis — `None` meaning elastic (the
    /// layout decides). Pure over the metrics, the widget's presentation props
    /// and the placement `scale` its text draws at (1.0 outside a workspace);
    /// see the module documentation for the content/surface split.
    pub fn natural_size(&self, m: &Metrics, scale: f32) -> Natural {
        match self {
            // -- Mixed: a thickness across the control's axis, elastic along it --
            // A ruler is a strip: it spans its axis and knows its thickness.
            WidgetKind::TimeRuler { .. } => (None, Some(m.ruler_h)),

            // A registered element declares its own, under the same rule the
            // arms above follow: pure over the metrics, its presentation props
            // and the scale — never over its data.
            WidgetKind::Custom(el) => el.natural(m, scale),

            // -- Surfaces: the extent is the caller's --
            _ => (None, None),
        }
    }
}

impl Widget {
    /// How big this widget wants to be in the layout that places it — what
    /// [`super::super::layout`] resolves after an explicit size and an explicit
    /// weight.
    ///
    /// It is its kind's own [`natural_size`](WidgetKind::natural_size), except
    /// for a container that carries **`hug`**: that one wants its content, so
    /// it answers the composition of what its children want
    /// ([`Widget::hug_size`]). A container is elastic without the prop, so no
    /// existing def moves.
    pub fn natural_size(&self, m: &Metrics, scale: f32) -> Natural {
        if self.hugs() {
            self.hug_size(m, scale)
        } else {
            self.kind.natural_size(m, scale)
        }
    }

    /// Whether this container carries the `hug` prop.
    fn hugs(&self) -> bool {
        matches!(
            self.kind,
            WidgetKind::Window { hug: true, .. }
                | WidgetKind::Panel { hug: true, .. }
                | WidgetKind::Stack { hug: true, .. }
        )
    }

    /// How big this widget wants to be **when it is being fitted to its
    /// content** — the question a hugging container asks, and the one it asks
    /// its own children in turn, so `hug` on the outermost container fits the
    /// whole subtree under it rather than one level of it.
    ///
    /// Two things separate it from [`Widget::natural_size`], and both are the
    /// point of the milestone. A **container** composes here instead of being
    /// the elastic surface it is by default: a `row` sums along its axis and
    /// takes the largest across it, a `col` the other way round, a `grid`
    /// counts its cells, a `free` container reaches its children's placements,
    /// and a `stack` takes the largest of **every** page (not the shown one, so
    /// flipping a pager does not resize it). And a **leaf** may answer from the
    /// props that describe its content ([`Element::hug`](super::Element::hug))
    /// — a label's text, a menu's options — which its ordinary natural size may
    /// not.
    ///
    /// `None` on an axis still means elastic, and it **propagates**: a
    /// container holding one surface whose extent is the caller's (a plane, a
    /// lane, a heavy view) cannot know its own, so it hands the axis back to
    /// the layout rather than guessing at it.
    pub fn hug_size(&self, m: &Metrics, scale: f32) -> Natural {
        match &self.kind {
            WidgetKind::Window { layout, flow, .. } | WidgetKind::Panel { layout, flow, .. } => {
                compose(&self.children, *layout, *flow, m, scale)
            }
            // A stack arranges nothing: every page fills it, so its content is
            // the largest of them, inset by its own margin.
            WidgetKind::Stack { margin, .. } => {
                let pad = 2.0 * margin.map_or(m.margin, |v| snap_px(v, scale)).max(0.0);
                let (w, h) = self
                    .children
                    .iter()
                    .map(|c| c.hug_size(m, scale))
                    .fold((Some(0.0f32), Some(0.0f32)), |(aw, ah), (w, h)| {
                        (largest(aw, w), largest(ah, h))
                    });
                (w.map(|w| w + pad), h.map(|h| h + pad))
            }
            WidgetKind::Custom(el) => el.hug(m, scale),
            // Everything else wants what it always wanted: a `scroll` is a
            // viewport onto a content area of its own (`content_w`/`content_h`
            // is where a plane's extent is named), a time container's span is
            // its axis', and a built-in leaf has no content prop to read.
            _ => self.kind.natural_size(m, scale),
        }
    }
}

/// The composition of `children`'s content sizes under one arrangement — the
/// whole of what a hugging container adds, and one bottom-up walk over
/// functions that were already pure.
fn compose(children: &[Widget], layout: Layout, flow: Flow, m: &Metrics, scale: f32) -> Natural {
    let pad = 2.0 * flow.margin.map_or(m.margin, |v| snap_px(v, scale)).max(0.0);
    let gap = flow.gap.map_or(m.gap, |v| snap_px(v, scale)).max(0.0);
    let gaps = gap * (children.len().max(1) - 1) as f32;
    let sizes: Vec<Natural> = children.iter().map(|c| c.hug_size(m, scale)).collect();
    let (w, h) = match layout {
        // Along the axis the children are strung on, the container is their
        // sum plus the gaps between them; across it, the largest of them.
        Layout::Row => (
            total(sizes.iter().map(|s| s.0)).map(|w| w + gaps),
            sizes.iter().map(|s| s.1).fold(Some(0.0), largest),
        ),
        Layout::Col => (
            sizes.iter().map(|s| s.0).fold(Some(0.0), largest),
            total(sizes.iter().map(|s| s.1)).map(|h| h + gaps),
        ),
        // Equal cells, so the grid is its widest cell times its column count.
        Layout::Grid => {
            let n = sizes.len();
            let cols = match flow.cols {
                Some(c) => (c as usize).clamp(1, n.max(1)),
                None => (n as f64).sqrt().ceil() as usize,
            };
            let rows = n.div_ceil(cols.max(1));
            let cell_w = sizes.iter().map(|s| s.0).fold(Some(0.0), largest);
            let cell_h = sizes.iter().map(|s| s.1).fold(Some(0.0), largest);
            (
                cell_w.map(|w| w * cols as f32 + gap * (cols.max(1) - 1) as f32),
                cell_h.map(|h| h * rows as f32 + gap * (rows.max(1) - 1) as f32),
            )
        }
        // Absolute placement: the container reaches as far as its children are
        // put, each one's declared position plus the size it declared or wants.
        Layout::Free => {
            let reach = |pos: fn(&Widget) -> Option<f32>,
                         size: fn(&Widget) -> Option<f32>,
                         want: fn(&Natural) -> Option<f32>| {
                children
                    .iter()
                    .zip(&sizes)
                    .map(|(c, natural)| {
                        let extent = size(c)
                            .map(|s| snap_px(s, scale))
                            .or_else(|| want(natural))?;
                        Some(pos(c).map_or(0.0, |p| snap_px(p, scale)) + extent)
                    })
                    .fold(Some(0.0f32), largest)
            };
            (
                reach(|c| c.place.x, |c| c.place.w, |n| n.0),
                reach(|c| c.place.y, |c| c.place.h, |n| n.1),
            )
        }
    };
    (w.map(|w| w + pad), h.map(|h| h + pad))
}

/// The sum of the children's extents along one axis — elastic as a whole the
/// moment one of them is, since a container cannot know what a surface next to
/// it will take.
fn total(mut sizes: impl Iterator<Item = Option<f32>>) -> Option<f32> {
    sizes.try_fold(0.0, |acc, s| Some(acc + s?.max(0.0)))
}

/// The larger of two extents, elastic if either is.
fn largest(acc: Option<f32>, s: Option<f32>) -> Option<f32> {
    Some(acc?.max(s?.max(0.0)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::graphics::controls;
    use crate::host::guidef::GuiNode;
    use crate::host::layout::Rect;
    use crate::host::widget::Widget;

    fn tree(json: &str) -> Widget {
        Widget::from_node(1, &GuiNode::parse(json.as_bytes()).unwrap(), &[]).unwrap()
    }

    fn kind(json: &str) -> WidgetKind {
        tree(json).kind
    }

    /// The size a widget wants in the layout that places it, at the default
    /// density and outside any workspace.
    fn wants(json: &str) -> Natural {
        tree(json).natural_size(&Metrics::default(), 1.0)
    }

    /// The size a widget wants when a container is being fitted to it.
    fn content(json: &str) -> Natural {
        tree(json).hug_size(&Metrics::default(), 1.0)
    }

    /// A container is the elastic surface it always was until it asks not to
    /// be: this is what "no existing def moves" means, tested rather than
    /// promised.
    #[test]
    fn a_container_is_elastic_until_it_hugs() {
        let of = |hug: &str| {
            format!(
                r#"{{"type":"layout","flow":"row"{hug},"children":[{{"type":"button","label":"go"}}]}}"#
            )
        };
        assert_eq!(wants(&of("")), (None, None));
        let (w, h) = wants(&of(r#","hug":1"#));
        assert!(w.is_some() && h.is_some(), "a hugging row wants its button");
    }

    /// The composition itself: along the axis the children are strung on, the
    /// container is their sum plus the gaps; across it, the largest of them.
    #[test]
    fn a_hugging_row_sums_along_its_axis_and_takes_the_largest_across() {
        let m = Metrics::default();
        let children = r#"{"type":"button","label":"one"},{"type":"toggle","label":"two"}"#;
        let (row_w, row_h) = wants(&format!(
            r#"{{"type":"layout","flow":"row","hug":1,"children":[{children}]}}"#
        ));
        let (col_w, col_h) = wants(&format!(
            r#"{{"type":"layout","flow":"col","hug":1,"children":[{children}]}}"#
        ));

        let button = content(r#"{"type":"button","label":"one"}"#);
        let toggle = content(r#"{"type":"toggle","label":"two"}"#);
        let pad = 2.0 * m.margin;
        assert_eq!(
            row_w,
            Some(button.0.unwrap() + toggle.0.unwrap() + m.gap + pad)
        );
        assert_eq!(row_h, Some(button.1.unwrap().max(toggle.1.unwrap()) + pad));
        // A column says the same thing the other way round.
        assert_eq!(col_w, Some(button.0.unwrap().max(toggle.0.unwrap()) + pad));
        assert_eq!(
            col_h,
            Some(button.1.unwrap() + toggle.1.unwrap() + m.gap + pad)
        );
    }

    /// Elastic propagates: a container holding a surface whose extent is the
    /// caller's cannot know its own, and hands the axis back rather than
    /// guessing. A `plane` is elastic on both, a horizontal `slider` only
    /// along its track.
    #[test]
    fn a_surface_inside_a_hug_hands_the_axis_back() {
        assert_eq!(
            wants(r#"{"type":"layout","flow":"row","hug":1,"children":[{"type":"plane"}]}"#),
            (None, None)
        );
        let (w, h) =
            wants(r#"{"type":"layout","flow":"row","hug":1,"children":[{"type":"slider"}]}"#);
        assert_eq!(w, None, "the slider spans its track, so the row does too");
        assert!(h.is_some(), "and it still knows how thick it is");
    }

    /// `hug` is asked of the **subtree**, not of one level: a plain panel
    /// nested in a hugging one is measured too, so a toolbar of grouped
    /// buttons needs the prop once.
    #[test]
    fn a_hug_reaches_through_a_plain_container() {
        let nested = wants(
            r#"{"type":"layout","flow":"row","hug":1,"children":[
                 {"type":"layout","flow":"row","children":[{"type":"button","label":"go"}]}]}"#,
        );
        assert!(nested.0.is_some() && nested.1.is_some());
    }

    /// A stack shows one page at a time and must not resize when it flips, so
    /// it hugs the largest of **every** page.
    #[test]
    fn a_stack_hugs_its_largest_page_and_not_the_shown_one() {
        let of = |index: i32| {
            format!(
                r#"{{"type":"layout","flow":"stack","hug":1,"index":{index},"children":[
                     {{"type":"button","label":"x"}},
                     {{"type":"button","label":"a much longer caption"}}]}}"#
            )
        };
        assert_eq!(wants(&of(0)), wants(&of(1)));
        assert_eq!(
            wants(&of(0)).0,
            wants(
                r#"{"type":"layout","flow":"stack","hug":1,"children":[
                     {"type":"button","label":"a much longer caption"}]}"#
            )
            .0
        );
    }

    /// Equal cells, so a hugged grid is its widest cell times its column count.
    #[test]
    fn a_hugging_grid_counts_its_cells() {
        let m = Metrics::default();
        let (w, h) = wants(
            r#"{"type":"layout","flow":"grid","cols":2,"hug":1,"children":[
                 {"type":"button","label":"a"},{"type":"button","label":"b"},
                 {"type":"button","label":"c"}]}"#,
        );
        let cell = content(r#"{"type":"button","label":"a"}"#);
        assert_eq!(w, Some(2.0 * cell.0.unwrap() + m.gap + 2.0 * m.margin));
        // Three children in two columns is two rows.
        assert_eq!(h, Some(2.0 * cell.1.unwrap() + m.gap + 2.0 * m.margin));
    }

    /// A `free` container reaches as far as its children are placed: each one's
    /// declared position plus the size it declared, or the size it wants.
    #[test]
    fn a_hugging_free_container_reaches_its_placements() {
        let m = Metrics::default();
        let (w, h) = wants(
            r#"{"type":"layout","flow":"free","hug":1,"children":[
                 {"type":"button","label":"a","x":10,"y":20,"w":100,"h":30},
                 {"type":"button","label":"b","x":5,"y":5}]}"#,
        );
        assert_eq!(w, Some(110.0 + 2.0 * m.margin));
        assert_eq!(h, Some(50.0 + 2.0 * m.margin));
    }

    /// The rule the milestone states out loud: a prop that settles at a
    /// mutation point may size, a **value** may not — or a widget would resize
    /// under the gesture writing it. A menu is the case that has both.
    #[test]
    fn a_prop_may_size_a_hug_and_a_value_may_not() {
        let hug = |json: &str| {
            wants(&format!(
                r#"{{"type":"layout","flow":"row","hug":1,"children":[{json}]}}"#
            ))
            .0
        };

        // The options are a prop: a longer one makes the menu wider.
        let short = hug(r#"{"type":"menu","options":["a"]}"#);
        let long = hug(r#"{"type":"menu","options":["a much longer option"]}"#);
        assert!(long > short, "{long:?} > {short:?}");
        // The choice among them is a value: picking one changes nothing.
        assert_eq!(
            hug(r#"{"type":"menu","options":["a","bbbbbbbbbb"],"index":0}"#),
            hug(r#"{"type":"menu","options":["a","bbbbbbbbbb"],"index":1}"#)
        );
        // And neither does what a field holds, or what a number reads.
        assert_eq!(
            hug(r#"{"type":"text","value":"x"}"#),
            hug(r#"{"type":"text","value":"a much longer string"}"#)
        );
        assert_eq!(
            hug(r#"{"type":"number","value":1}"#),
            hug(r#"{"type":"number","value":123456789}"#)
        );
    }

    /// A hugged control is fitted to **its whole composition**, not to the one
    /// part that suggested a number: a knob is a label strip over a disc over a
    /// value read-out, one element drawing three things into one cell, and a
    /// width that fitted the disc alone ellipsized the name and clipped the
    /// number — parts of the widget cut to fit the widget. Found by eye on
    /// `gui_standalone`, which is why the assertion is over the *drawing's* own
    /// available widths and not over the terms of the sum.
    #[test]
    fn a_hugged_control_fits_every_part_it_draws() {
        use crate::host::font;
        for scale in [1.0f32, 1.5, 2.0] {
            let m = Metrics::default().resolved(scale);
            let json = r#"{"type":"knob","label":"freq","min":80.0,"max":400.0,"value":160.0}"#;
            let el = tree(json);
            let (w, _) = el.hug_size(&m, scale);
            let w = w.expect("a hugged knob knows its width");
            let size = font::DEFAULT_SIZE * scale;
            // The label strip draws at `x + pad` with the rest of the cell.
            assert!(
                w - 2.0 * m.pad >= font::width("freq", size),
                "at {scale} the label is cut: {w}"
            );
            // The read-out is right-aligned inside the body, one pad in.
            assert!(
                w - 3.0 * m.pad >= font::width("400", size),
                "at {scale} the read-out is cut: {w}"
            );
            // And the disc still gets its whole diameter.
            assert!(w - 2.0 * m.pad >= m.knob_d, "at {scale} the disc shrank");
        }
    }

    /// The bounds are props and the value is not: a control fitted to its
    /// content keeps one width while it is turned, which is the same rule the
    /// menu states and the one that keeps a hug from resizing under the hand.
    #[test]
    fn a_read_out_is_measured_from_the_range_and_not_from_the_value() {
        let m = Metrics::default();
        let of = |v: f32| {
            tree(&format!(
                r#"{{"type":"knob","min":0.0,"max":20000.0,"value":{v}}}"#
            ))
            .hug_size(&m, 1.0)
            .0
        };
        assert_eq!(of(0.0), of(19999.5));
        // ...and it is wide enough for the widest number it can reach.
        assert!(
            of(0.0).unwrap()
                > tree(r#"{"type":"knob","min":0.0,"max":1.0}"#)
                    .hug_size(&m, 1.0)
                    .0
                    .unwrap()
        );
    }

    /// A widget's own natural size never reads its content, hug or no hug: the
    /// two questions are separate functions, and only the hugging container
    /// asks the second one.
    #[test]
    fn the_ordinary_pass_still_reads_nothing() {
        let m = Metrics::default();
        let short = kind(r#"{"type":"button","label":"a"}"#).natural_size(&m, 1.0);
        let long =
            kind(r#"{"type":"button","label":"a much longer caption"}"#).natural_size(&m, 1.0);
        assert_eq!(short, long);
        assert_eq!(short.0, None, "and it spans its axis, as it always did");
    }

    #[test]
    fn a_surface_is_elastic_on_both_axes() {
        let m = Metrics::default();
        for json in [
            r#"{"type":"layout"}"#,
            r#"{"type":"plane"}"#,
            r#"{"type":"signal","view":"trace","data":[]}"#,
            r#"{"type":"signal","navigable":0,"data":[]}"#,
            r#"{"type":"nodes"}"#,
            r#"{"type":"field"}"#,
            r#"{"type":"plane"}"#,
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
            kind(r#"{"type":"field","h":20.0}"#).natural_size(&m, 1.0),
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
