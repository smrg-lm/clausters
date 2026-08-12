//! The metrics: every chrome size as a named **role**, in one table.
//!
//! The twin of [`super::theme`], one step behind it: no layout or paint site
//! names a number, it names a role of this struct, exactly as no paint site
//! names an RGBA. The roles are named by **function** — the spacing family
//! ([`Metrics::pad`], `gap`, `margin`, `indent`), the control family
//! (`control_h`, `row_h`, `track_thick`, `handle_thick`, `handle_grip`,
//! `box_side`, `knob_d`), the chrome family (`ruler_h`, `ruler_w`, `header_w`,
//! `divider_w`, `focus_ring`, `trace_w`, `point_radius`, `hit_slop`, `label_gap`,
//! `tick_gap`) and the text family (`text_scale`, `label_scale`,
//! `caption_scale`, `micro_scale`) — never by the widget that happens to read
//! them, so one number serves every widget that means the same thing by it.
//!
//! **The defaults are generated, not invented.** [`Metrics::generated`] is one
//! quantized modular scale over the font cell ([`CELL`], 14 logical px — not
//! a round decimal, because every readable
//! widget is text plus padding and the cell is what makes a button, a number
//! field and a menu line up unaided). Spacings and extents land on a 2-px
//! grid, hairlines on whole pixels, text scales on half-steps of the bitmap
//! cell. It is a **generator for the table, not arithmetic on a frame**: the
//! resolved table is constant data every draw call reads.
//!
//! One `scale` multiplier — the reserved key of the `[gui.metrics]` config
//! table — regenerates the whole table at another density, which is
//! deliberately the *whole* density surface: a host has one density the way it
//! has one look, so there are no size tokens on the wire and no per-widget
//! density. It covers this table (the chrome and the control sizing); a
//! widget's own `text_size` prop is the wire's number and stays untouched.
//!
//! **The table is logical; a window resolves it.** The roles the config
//! declares are **logical** pixels, the same units the wire's own `w`/`h`/`x`/
//! `y`/`margin`/`gap` carry. [`Metrics::resolved`] turns one into the physical
//! table a window paints with, at that window's `ui_scale` — written by the
//! shell (winit's `scale_factor` natively, the page's `devicePixelRatio` in the
//! browser), never read by this core. It runs **once per scale change**, not
//! per frame: layout and painting stay the code they were, and the per-frame
//! cost of HiDPI is zero.
//!
//! **What the table does not hold** is a widget's own structural geometry —
//! the patcher's box/port series, the piano roll's key gutter and velocity
//! lane, the score's staff step, a knob's internal insets. Those interlock
//! inside one widget rather than forming shared vocabulary, so they stay in
//! their module; a role applies where the meaning is shared, which is what
//! keeps the table small enough to be read.

use super::font;

/// The base unit of every size role: the height of a line of text at the
/// default scale, in logical pixels.
///
/// **Declared, not derived** — and that is the whole of its documentation.
/// It used to be `font::GLYPH_H * font::DEFAULT_SIZE`, which made the layout of
/// every window a consequence of how the *face* was drawn: a glyph box that
/// grew two rows to hold a descender would have taken this from 14 to 18 and
/// moved every padding, control height and ruler in the host with it, for a
/// reason that has nothing to do with how big anything should be. The
/// dependency runs the other way now — the cell is declared and the face is
/// drawn to fit it — so a typeface may change without relaying out a window,
/// which is also the property [`super::font`]'s successor will need.
pub const CELL: f32 = 14.0;

/// The grid the spacing and extent roles land on, logical pixels. Two rather
/// than four (which the roadmap sketched): the shipped spacing pair is 6 and
/// every shipped chrome extent is even, so a 4-px grid would have moved them
/// and cost this refactor its zero-visual-change guarantee.
const GRID: f32 = 2.0;

/// Quantizes an extent onto the [`GRID`], at least one grid step.
fn grid(v: f32) -> f32 {
    ((v / GRID).round() * GRID).max(GRID)
}

/// Quantizes a hairline weight to whole pixels, at least one — a divider, a
/// track edge and a glyph pixel are all one unit, and a fractional position
/// turns a crisp line into a two-pixel grey smear.
fn hairline(v: f32) -> f32 {
    v.round().max(1.0)
}

/// Quantizes a glyph scale. The embedded font is a bitmap, so a scale that
/// does not divide its cell evenly makes a glyph's own pixels unequal — ragged
/// rather than soft. Whole numbers are the rule; the half-step is the one
/// concession, because the ruler and clip captions ship at 1.5 and the reduced
/// caption has no other rung between 1 and 2.
fn glyph(v: f32) -> f32 {
    (v * 2.0).round().max(2.0) / 2.0
}

macro_rules! metrics_roles {
    ($( $(#[$doc:meta])* $name:ident ),+ $(,)?) => {
        /// The host's size roles, in pixels (the text family in glyph scales)
        /// — logical as the config declares them, physical once a window has
        /// [`resolved`](Metrics::resolved) them. One logical instance per host
        /// plus one resolved per window; every layout and paint site reads one
        /// role of the resolved table.
        #[derive(Debug, Clone, Copy, PartialEq)]
        pub struct Metrics {
            $( $(#[$doc])* pub $name: f32, )+
            /// The logical -> physical multiplier this table was resolved at:
            /// `1.0` for the logical table the config declares, the window's
            /// scale factor for the table a window paints with (see
            /// [`Metrics::resolved`]). It is **not** a role — the config cannot
            /// set it, only a shell can, and one host has as many resolved
            /// tables as it has windows.
            pub ui_scale: f32,
        }

        impl Metrics {
            /// Every role name, in declaration order.
            pub const NAMES: &'static [&'static str] = &[$(stringify!($name),)+];

            /// Sets one role by name. `false` if no such role exists.
            pub fn set(&mut self, name: &str, value: f32) -> bool {
                match name {
                    $( stringify!($name) => { self.$name = value; true } )+
                    _ => false,
                }
            }

            /// Reads one role by name.
            pub fn get(&self, name: &str) -> Option<f32> {
                match name {
                    $( stringify!($name) => Some(self.$name), )+
                    _ => None,
                }
            }
        }
    };
}

metrics_roles! {
    // -- Spacing --
    /// Inside a widget, between its frame and its content.
    pad,
    /// Between two siblings of a container.
    gap,
    /// Inside a container, before its children.
    margin,
    /// One nesting level of an indented list (the node tree's children).
    indent,

    // -- Controls --
    /// The height of one line of control: a cell of text and its padding —
    /// what makes a button, a number field and a menu line up in a row.
    control_h,
    /// The pitch of one row in a list of controls (a control plus a gap).
    row_h,
    /// The thickness of a control's groove across its axis (a slider's track).
    track_thick,
    /// The thickness of the value riding that groove (a slider's handle).
    handle_thick,
    /// The length of a handle across the control's axis (its grip).
    handle_grip,
    /// The side of a square marker (a toggle's box).
    box_side,
    /// The diameter of a round control (a knob): **two lines of control**, not
    /// a box-sized marker. A dial is read by its angle, so it needs the sweep
    /// to be legible — and a disc reads smaller than a box of the same
    /// bounding rect, which is why it is its own role rather than `control_h`
    /// twice over.
    knob_d,

    // -- Chrome --
    /// The height of a ruler strip along a horizontal axis.
    ruler_h,
    /// The **floor** width of a ruler strip beside a vertical axis. A strip
    /// asks for what its own labels need (`ruler::ticks_width`) and takes this
    /// when they are narrower, which every fixed-vocabulary axis is; a value
    /// axis over small numbers formats far longer ones and asks for more.
    ruler_w,
    /// The width of a row's header column (a lane's name and controls).
    header_w,
    /// A hairline: a divider between lanes, a box edge.
    divider_w,
    /// The weight of the ring around the widget holding the keyboard focus.
    /// Heavier than a hairline on purpose: it has to read *over* the edge a
    /// control already draws, or focus looks like a rendering artifact.
    focus_ring,
    /// The weight of a drawn signal trace.
    trace_w,
    /// The radius of a placed point (a break-point, an automation node).
    point_radius,
    /// The corner radius of a **text plate** — the ground a caption drawn over
    /// a picture sits on, so the glyphs read against whatever is under them.
    plate_radius,
    /// The slack around a small target's geometry, so it stays clickable.
    hit_slop,
    /// The width of an **edge grip**: the strip at each end of a placed object
    /// that resizes it (a clip's ends), drawn where it can be grabbed.
    grip_w,
    /// The smallest gap between two ruler labels before the ladder steps up.
    label_gap,
    /// The smallest gap between two ruler ticks before the ladder steps up.
    tick_gap,

    // -- Text (glyph scales over the embedded bitmap font) --
    /// Primary text: control labels, values, readouts.
    text_scale,
    /// In-view labels drawn over a surface (a lane's name tag).
    label_scale,
    /// Reduced text: ruler labels, a clip's caption.
    caption_scale,
    /// The densest legible mark (a key's octave label).
    micro_scale,
}

impl Default for Metrics {
    /// The table at the host's own density — the sizes the widgets always had.
    fn default() -> Self {
        Self::generated(1.0)
    }
}

impl Metrics {
    /// The table generated by the modular scale at `density` (1.0 = the host's
    /// own density; below it compact, above it comfortable). Every role is
    /// derived from the font cell and quantized, so a density is one
    /// multiplier applied to one table rather than a pass over the tree.
    pub fn generated(density: f32) -> Self {
        let k = density.max(0.1);
        let cell = CELL * k;
        // Spacing: quarter and (nearly) half the cell.
        let pad = grid(cell * 0.25);
        let gap = grid(cell * 0.4);
        // Text first, since the roles sized to hold text read its advance.
        let text_scale = glyph(font::DEFAULT_SIZE * k);
        let caption_scale = glyph(1.5 * k);
        // Controls: a cell of text plus its padding, and the parts riding it.
        let control_h = grid(cell + 2.0 * pad);
        let box_side = grid(control_h * 1.1);
        let track_thick = pad;
        Self {
            pad,
            gap,
            margin: gap,
            indent: grid(cell),

            control_h,
            row_h: grid(control_h + gap),
            track_thick,
            handle_thick: grid(2.0 * track_thick),
            handle_grip: grid(cell * 1.25),
            box_side,
            knob_d: grid(2.0 * control_h * 1.08),

            ruler_h: grid(cell * 1.25),
            // Five captions wide: the widest labels of the *fixed* vertical
            // vocabularies, `-32768`, `20K` and `-INF`. A data-dependent axis
            // measures its own and asks for more when it needs it.
            ruler_w: grid(5.0 * font::advance(caption_scale)),
            // Eight characters of header text wide.
            header_w: grid(8.0 * font::advance(text_scale)),
            divider_w: hairline(k),
            focus_ring: hairline(2.0 * k),
            trace_w: (1.5 * k).max(0.5),
            point_radius: pad,
            plate_radius: pad,
            hit_slop: pad,
            grip_w: grid(3.0 * pad),
            label_gap: grid(cell),
            tick_gap: hairline(cell * 0.5),

            text_scale,
            label_scale: text_scale,
            caption_scale,
            micro_scale: glyph(k),

            ui_scale: 1.0,
        }
    }

    /// This logical table resolved to the **physical** pixels of a window at
    /// `ui_scale` — the one resolution HiDPI costs, run on a scale change and
    /// never per frame.
    ///
    /// Every role is scaled and re-quantized by its own family (extents onto
    /// the [`GRID`], hairlines onto whole pixels, text onto half-steps of the
    /// bitmap cell), because the chrome *is* hairlines: a divider, a track edge
    /// and a glyph pixel are one unit each, and a fractional position turns a
    /// crisp line into a two-pixel grey smear.
    ///
    /// At `ui_scale == 1.0` this is the **identity** — the table the config
    /// declared, number for number, quantizers included: a host on an ordinary
    /// display paints exactly what it always did, and a role set to an odd 5 px
    /// by hand stays 5.
    pub fn resolved(&self, ui_scale: f32) -> Self {
        let k = if ui_scale.is_finite() {
            ui_scale.max(0.1)
        } else {
            1.0
        };
        if k == 1.0 {
            return *self;
        }
        Self {
            pad: grid(self.pad * k),
            gap: grid(self.gap * k),
            margin: grid(self.margin * k),
            indent: grid(self.indent * k),

            control_h: grid(self.control_h * k),
            row_h: grid(self.row_h * k),
            track_thick: grid(self.track_thick * k),
            handle_thick: grid(self.handle_thick * k),
            handle_grip: grid(self.handle_grip * k),
            box_side: grid(self.box_side * k),
            knob_d: grid(self.knob_d * k),

            ruler_h: grid(self.ruler_h * k),
            ruler_w: grid(self.ruler_w * k),
            header_w: grid(self.header_w * k),
            divider_w: hairline(self.divider_w * k),
            focus_ring: hairline(self.focus_ring * k),
            trace_w: (self.trace_w * k).max(0.5),
            point_radius: grid(self.point_radius * k),
            plate_radius: grid(self.plate_radius * k),
            hit_slop: grid(self.hit_slop * k),
            grip_w: grid(self.grip_w * k),
            label_gap: grid(self.label_gap * k),
            tick_gap: hairline(self.tick_gap * k),

            text_scale: glyph(self.text_scale * k),
            label_scale: glyph(self.label_scale * k),
            caption_scale: glyph(self.caption_scale * k),
            micro_scale: glyph(self.micro_scale * k),

            // Relative, so resolutions compose: a table already at 2 resolved by
            // another 2 is a table at 4, which is what a zoom inside a scaled
            // window is.
            ui_scale: self.ui_scale * k,
        }
    }

    /// This table's own logical base, resolved at `scale` — the table a widget
    /// seen through a zoom is sized and drawn with.
    ///
    /// A `scroll` workspace multiplies everything inside it by its zoom, and a
    /// zoom is an *enlargement*: the text, the padding, a disc's diameter and the
    /// gaps between rows all have to move together, or a zoomed box comes out as
    /// a box with oversized text jammed into it. So a placement's scale picks the
    /// table, exactly as a window's does — this is that same resolution, taken
    /// relative to whatever scale this table is already at (so it is the identity
    /// at the window's own scale).
    pub fn at(&self, scale: f32) -> Self {
        self.resolved(scale / self.ui_scale.max(0.01))
    }

    /// One of the **wire's own** lengths (`w`/`h`/`x`/`y`/`margin`/`gap`) in
    /// physical pixels, at this table's scale (see [`snap_px`]).
    pub fn px(&self, logical: f32) -> f32 {
        snap_px(logical, self.ui_scale)
    }

    /// Overlays `(role, value)` pairs — the `[gui.metrics]` config table.
    /// Unknown roles and unusable values are skipped and reported back as
    /// warnings, so a stale style file degrades to the default sizes, never to
    /// an error.
    ///
    /// The reserved key `scale` is the density multiplier: it **regenerates**
    /// the whole table at that density before any explicit role applies,
    /// whatever order the entries arrive in.
    pub fn overlay<'a, I>(&mut self, entries: I) -> Vec<String>
    where
        I: IntoIterator<Item = (&'a str, f64)>,
    {
        let entries: Vec<(&str, f64)> = entries.into_iter().collect();
        let mut warnings = Vec::new();
        for (_, value) in entries.iter().filter(|(role, _)| *role == "scale") {
            if usable(*value) {
                *self = Self::generated(*value as f32);
            } else {
                warnings.push(format!("metrics: bad density 'scale = {value}'"));
            }
        }
        for (role, value) in entries.iter().filter(|(role, _)| *role != "scale") {
            if !usable(*value) {
                warnings.push(format!("metrics: bad size '{value}' for role '{role}'"));
            } else if !self.set(role, *value as f32) {
                warnings.push(format!("metrics: unknown role '{role}'"));
            }
        }
        warnings
    }
}

/// A **logical** length the wire declared, in physical pixels at `scale`,
/// snapped to a whole pixel: a declared strip lands on the same grid the
/// chrome's hairlines are drawn against, at any scale. The one door the wire's
/// own lengths pass through (the layout reads it per placement space, so a
/// navigable plane can keep its own units).
pub fn snap_px(logical: f32, scale: f32) -> f32 {
    (logical * scale).round()
}

/// Whether a configured number can be a size: finite and positive (a zero
/// spacing would be legitimate, but a zero thickness or scale is a role that
/// vanishes, and the config cannot say which role it is setting).
fn usable(v: f64) -> bool {
    v.is_finite() && v > 0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The refactor's guard: the generated table is the sizes the widgets had
    /// before any role existed. Every number here was a `const` in some
    /// module; if one moves, the change is visible and deliberate.
    #[test]
    fn defaults_are_the_shipped_sizes() {
        let m = Metrics::default();
        assert_eq!(CELL, 14.0);
        assert_eq!(m.pad, 4.0);
        assert_eq!(m.gap, 6.0);
        assert_eq!(m.margin, 6.0);
        assert_eq!(m.indent, 14.0);
        assert_eq!(m.control_h, 22.0);
        // The roles a widget's natural size resolves from: the scale is what
        // fixes them, since no drawing site named them before.
        assert_eq!(m.row_h, 28.0);
        // Two lines of control (plus the disc's own 8%): a dial's sweep has to
        // be readable, which a box-sized disc is not.
        assert_eq!(m.knob_d, 48.0);
        assert_eq!(m.track_thick, 4.0);
        assert_eq!(m.handle_thick, 8.0);
        assert_eq!(m.handle_grip, 18.0);
        assert_eq!(m.box_side, 24.0);
        assert_eq!(m.ruler_h, 18.0);
        assert_eq!(m.ruler_w, 46.0);
        assert_eq!(m.header_w, 96.0);
        assert_eq!(m.divider_w, 1.0);
        assert_eq!(m.focus_ring, 2.0);
        assert_eq!(m.trace_w, 1.5);
        assert_eq!(m.point_radius, 4.0);
        assert_eq!(m.plate_radius, 4.0);
        assert_eq!(m.hit_slop, 4.0);
        assert_eq!(m.grip_w, 12.0);
        assert_eq!(m.label_gap, 14.0);
        assert_eq!(m.tick_gap, 7.0);
        assert_eq!(m.text_scale, 2.0);
        assert_eq!(m.label_scale, 2.0);
        assert_eq!(m.caption_scale, 1.5);
        assert_eq!(m.micro_scale, 1.0);
    }

    #[test]
    fn quantization_holds_at_every_density() {
        for density in [0.5, 0.75, 1.0, 1.25, 1.5, 2.0, 3.0] {
            let m = Metrics::generated(density);
            for role in [
                "pad",
                "gap",
                "margin",
                "indent",
                "control_h",
                "row_h",
                "track_thick",
                "handle_thick",
                "handle_grip",
                "box_side",
                "knob_d",
                "ruler_h",
                "ruler_w",
                "header_w",
            ] {
                let v = m.get(role).unwrap();
                assert_eq!(v % GRID, 0.0, "{role} at {density} is off the grid: {v}");
                assert!(v >= GRID, "{role} at {density} vanished: {v}");
            }
            for role in ["divider_w", "focus_ring", "tick_gap"] {
                let v = m.get(role).unwrap();
                assert_eq!(v.fract(), 0.0, "{role} at {density} is not whole: {v}");
                assert!(v >= 1.0);
            }
            for role in ["text_scale", "label_scale", "caption_scale", "micro_scale"] {
                let v = m.get(role).unwrap();
                assert_eq!(
                    (v * 2.0).fract(),
                    0.0,
                    "{role} at {density} is not a half-step: {v}"
                );
                assert!(v >= 1.0, "{role} at {density} is illegible: {v}");
            }
        }
    }

    #[test]
    fn density_grows_the_table_monotonically() {
        let (small, big) = (Metrics::generated(1.0), Metrics::generated(2.0));
        assert!(big.control_h > small.control_h);
        assert!(big.header_w > small.header_w);
        assert!(big.text_scale > small.text_scale);
        let compact = Metrics::generated(0.5);
        assert!(compact.control_h < small.control_h);
    }

    #[test]
    fn partial_overlay_keeps_the_rest() {
        let mut m = Metrics::default();
        let warnings = m.overlay([("pad", 8.0)]);
        assert!(warnings.is_empty());
        assert_eq!(m.pad, 8.0);
        assert_eq!(
            m.gap,
            Metrics::default().gap,
            "unlisted roles keep the default"
        );
    }

    #[test]
    fn scale_regenerates_before_the_roles() {
        let mut m = Metrics::default();
        // The role comes first in the table; the density still applies first.
        let warnings = m.overlay([("pad", 3.0), ("scale", 2.0)]);
        assert!(warnings.is_empty());
        assert_eq!(m.pad, 3.0, "the explicit role wins over the density");
        assert_eq!(m.control_h, Metrics::generated(2.0).control_h);
    }

    #[test]
    fn overlay_warns_and_continues() {
        let mut m = Metrics::default();
        let warnings = m.overlay([
            ("no_such_role", 4.0),
            ("gap", f64::NAN),
            ("scale", 0.0),
            ("pad", 10.0),
        ]);
        assert_eq!(warnings.len(), 3);
        assert_eq!(m.gap, Metrics::default().gap, "a bad size leaves the role");
        assert_eq!(m.pad, 10.0, "later entries still apply");
    }

    #[test]
    fn resolving_at_scale_one_is_the_identity() {
        // Including a role the config set to a number the generator would never
        // pick: an ordinary display paints what the config declared.
        let mut m = Metrics::default();
        assert!(m.overlay([("pad", 5.0), ("text_scale", 2.0)]).is_empty());
        assert_eq!(m.resolved(1.0), m);
        assert_eq!(m.ui_scale, 1.0);
    }

    #[test]
    fn resolving_scales_the_table_and_keeps_its_quantization() {
        let m = Metrics::default().resolved(2.0);
        assert_eq!(m.ui_scale, 2.0);
        assert_eq!(m.pad, 8.0);
        assert_eq!(m.control_h, 44.0);
        assert_eq!(m.divider_w, 2.0, "a hairline is whole pixels, doubled");
        assert_eq!(m.text_scale, 4.0);
        assert_eq!(m.caption_scale, 3.0, "1.5 doubles onto a whole step");
        for role in Metrics::NAMES {
            let v = m.get(role).unwrap();
            assert!(v.is_finite() && v > 0.0, "{role} vanished: {v}");
        }
    }

    #[test]
    fn a_fractional_scale_still_lands_on_the_grid() {
        // The usual HiDPI factors, including the fractional ones a desktop
        // scales by: every extent stays on the grid and every glyph scale on a
        // half-step, so nothing draws a smeared hairline.
        for scale in [1.25, 1.5, 1.75, 2.0, 3.0] {
            let m = Metrics::default().resolved(scale);
            for role in ["pad", "gap", "control_h", "knob_d", "header_w"] {
                let v = m.get(role).unwrap();
                assert_eq!(v % GRID, 0.0, "{role} at {scale} is off the grid: {v}");
            }
            for role in ["divider_w", "focus_ring", "tick_gap"] {
                assert_eq!(m.get(role).unwrap().fract(), 0.0);
            }
            for role in ["text_scale", "caption_scale"] {
                let v = m.get(role).unwrap();
                assert_eq!((v * 2.0).fract(), 0.0, "{role} at {scale}: {v}");
                assert!(v >= 1.0);
            }
        }
    }

    #[test]
    fn the_wire_s_own_lengths_scale_and_snap() {
        let m = Metrics::default().resolved(2.0);
        assert_eq!(m.px(48.0), 96.0);
        assert_eq!(m.px(10.5), 21.0);
        // A logical number that lands between pixels takes the nearer one.
        assert_eq!(Metrics::default().resolved(1.5).px(15.0), 23.0);
        assert_eq!(Metrics::default().px(15.0), 15.0);
    }

    #[test]
    fn every_name_sets_and_gets() {
        let mut m = Metrics::default();
        for name in Metrics::NAMES {
            assert!(m.get(name).is_some());
            assert!(m.set(name, 3.0));
        }
        assert!(m.get("scale").is_none(), "the density is not a role");
    }
}
