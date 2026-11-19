//! The metrics: every chrome size as a named **role**, in one table.
//!
//! The twin of [`crate::theme`], one step behind it: no layout or paint site
//! names a number, it names a role of this struct, exactly as no paint site
//! names an RGBA. The roles are named by **function** -- the spacing family
//! ([`Metrics::pad`], `gap`, `margin`, `indent`), the control family
//! (`control_h`, `row_h`, `track_thick`, `handle_thick`, `handle_grip`,
//! `box_side`, `knob_d`), the chrome family (`ruler_h`, `status_h`, `ruler_w`, `header_w`,
//! `divider_w`, `focus_ring`, `trace_w`, `point_radius`, `hit_slop`, `label_gap`,
//! `tick_gap`) and the text family (`text_scale`, `label_scale`,
//! `caption_scale`, `micro_scale`) -- never by the widget that happens to read
//! them, so one number serves every widget that means the same thing by it.
//!
//! **The specification's tokens** join them: the design system the host's
//! look is specified from gives a type scale (`type_xs` to `type_lg`, pixels
//! to the em, each with its `line_`), four radii (`radius_sm` to
//! `radius_xl`), three lifts of elevation (`shadow_xs` to `shadow_lg`, whose
//! layers [`Metrics::shadow`] gives) and the insets and extents of each of
//! its components (`bar_h`, `menu_item_x`, `button_h`, `dialog_inset`...),
//! each the specification's number at the host's own density.
//!
//! **The defaults are generated, not invented.** [`Metrics::generated`] is one
//! quantized modular scale over the font cell ([`CELL`], 14 logical px -- not
//! a round decimal, because every readable
//! widget is text plus padding and the cell is what makes a button, a number
//! field and a menu line up unaided). Spacings and extents land on a 2-px
//! grid, hairlines on whole pixels, text scales on half-steps of the bitmap
//! cell. It is a **generator for the table, not arithmetic on a frame**: the
//! resolved table is constant data every draw call reads.
//!
//! One `scale` multiplier -- the reserved key of the `[gui.metrics]` config
//! table -- regenerates the whole table at another density, which is
//! deliberately the *whole* density surface: a host has one density the way it
//! has one look, so there are no size tokens on the wire and no per-widget
//! density. It covers this table (the chrome and the control sizing); a
//! widget's own `text_size` prop is the wire's number and stays untouched.
//!
//! **The table is logical; a window resolves it.** The roles the config
//! declares are **logical** pixels, the same units the wire's own `w`/`h`/`x`/
//! `y`/`margin`/`gap` carry. [`Metrics::resolved`] turns one into the physical
//! table a window paints with, at that window's `ui_scale` -- written by the
//! shell (winit's `scale_factor` natively, the page's `devicePixelRatio` in the
//! browser), never read by this core. It runs **once per scale change**, not
//! per frame: layout and painting stay the code they were, and the per-frame
//! cost of HiDPI is zero.
//!
//! **What the table does not hold** is a widget's own structural geometry --
//! the patcher's box/port series, the piano roll's key gutter and velocity
//! lane, the score's staff step, a knob's internal insets. Those interlock
//! inside one widget rather than forming shared vocabulary, so they stay in
//! their module; a role applies where the meaning is shared, which is what
//! keeps the table small enough to be read.

use crate::font;

/// The base unit of every size role: the height of a line of text at the
/// default scale, in logical pixels.
///
/// **Declared, not derived** -- and that is the whole of its documentation.
/// It used to be `font::GLYPH_H * font::DEFAULT_SIZE`, which made the layout of
/// every window a consequence of how the *face* was drawn: a glyph box that
/// grew two rows to hold a descender would have taken this from 14 to 18 and
/// moved every padding, control height and ruler in the host with it, for a
/// reason that has nothing to do with how big anything should be. The
/// dependency runs the other way now -- the cell is declared and the face is
/// drawn to fit it -- so a typeface may change without relaying out a window,
/// which is also the property [`crate::font`]'s successor will need.
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

/// Quantizes a hairline weight to whole pixels, at least one -- a divider, a
/// track edge and a glyph pixel are all one unit, and a fractional position
/// turns a crisp line into a two-pixel grey smear.
fn hairline(v: f32) -> f32 {
    v.round().max(1.0)
}

/// A size that is not quantized, only kept visible: a trace's weight, a
/// size of the type scale (an outline face is drawn at any size), a lift.
fn fine(v: f32) -> f32 {
    v.max(0.5)
}

/// Quantizes a glyph scale. The embedded font is a bitmap, so a scale that
/// does not divide its cell evenly makes a glyph's own pixels unequal -- ragged
/// rather than soft. Whole numbers are the rule; the half-step is the one
/// concession, because the ruler and clip captions ship at 1.5 and the reduced
/// caption has no other rung between 1 and 2.
fn glyph(v: f32) -> f32 {
    glyph_scale(v)
}

/// The same quantization, for a scale computed from the room a drawing has
/// rather than read from the table -- a lamp's number sized to the strip it is
/// written across. The rule is the font's and not the table's, so it holds
/// wherever a scale comes from.
pub fn glyph_scale(v: f32) -> f32 {
    (v * 2.0).round().max(2.0) / 2.0
}

macro_rules! metrics_roles {
    ($( $(#[$doc:meta])* $name:ident: $quant:ident ),+ $(,)?) => {
        /// The host's size roles, in pixels (the text family in glyph scales)
        /// -- logical as the config declares them, physical once a window has
        /// [`resolved`](Metrics::resolved) them. One logical instance per host
        /// plus one resolved per window; every layout and paint site reads one
        /// role of the resolved table.
        #[derive(Debug, Clone, Copy, PartialEq)]
        pub struct Metrics {
            $( $(#[$doc])* pub $name: f32, )+
            /// The logical -> physical multiplier this table was resolved at:
            /// `1.0` for the logical table the config declares, the window's
            /// scale factor for the table a window paints with (see
            /// [`Metrics::resolved`]). It is **not** a role -- the config cannot
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

            /// Every role scaled by `k` and quantized by its own family (see
            /// [`resolved`](Metrics::resolved)).
            fn scaled(&self, k: f32) -> Self {
                Self {
                    $( $name: $quant(self.$name * k), )+
                    // Relative, so resolutions compose: a table already at 2
                    // resolved by another 2 is a table at 4, which is what a
                    // zoom inside a scaled window is.
                    ui_scale: self.ui_scale * k,
                }
            }
        }
    };
}

metrics_roles! {
    // -- Spacing --
    /// Inside a widget, between its frame and its content.
    pad: grid,
    /// Between two siblings of a container.
    gap: grid,
    /// Inside a container, before its children.
    margin: grid,
    /// One nesting level of an indented list (the node tree's children).
    indent: grid,

    // -- Controls --
    /// The height of one line of control: a cell of text and its padding --
    /// what makes a button, a number field and a menu line up in a row.
    control_h: grid,
    /// The pitch of one row in a list of controls (a control plus a gap).
    row_h: grid,
    /// The thickness of a control's groove across its axis (a slider's track).
    track_thick: grid,
    /// The thickness of the value riding that groove (a slider's handle).
    handle_thick: grid,
    /// The length of a handle across the control's axis (its grip).
    handle_grip: grid,
    /// The side of a square marker (a toggle's box).
    box_side: grid,
    /// The diameter of a round control (a knob): **two lines of control**, not
    /// a box-sized marker. A dial is read by its angle, so it needs the sweep
    /// to be legible -- and a disc reads smaller than a box of the same
    /// bounding rect, which is why it is its own role rather than `control_h`
    /// twice over.
    knob_d: grid,

    // -- Chrome --
    /// The height of a ruler strip along a horizontal axis.
    ruler_h: grid,
    /// The height of **one line of the status bar** along the window's bottom
    /// edge (`host::status`). A ruler's height rather than a control's: the bar
    /// is a strip of chrome with a line written on it, which is what a ruler is.
    status_h: grid,
    /// The **floor** width of a ruler strip beside a vertical axis. A strip
    /// asks for what its own labels need (`ruler::ticks_width`) and takes this
    /// when they are narrower, which every fixed-vocabulary axis is; a value
    /// axis over small numbers formats far longer ones and asks for more.
    ruler_w: grid,
    /// The width of a row's header column (a lane's name and controls).
    header_w: grid,
    /// The height of a **panel's header**: the one strip along a panel's
    /// top, which a dock's panel draws its grip, its tabs or its tools and
    /// its buttons in, and a window that names a `header` its tools in.
    panel_h: grid,
    /// A hairline: a divider between lanes, a box edge.
    divider_w: hairline,
    /// The weight of the ring around the widget holding the keyboard focus.
    /// Heavier than a hairline on purpose: it has to read *over* the edge a
    /// control already draws, or focus looks like a rendering artifact.
    focus_ring: hairline,
    /// The weight of a drawn signal trace.
    trace_w: fine,
    /// The radius of a placed point (a break-point, an automation node).
    point_radius: grid,
    /// The corner radius of a **text plate** -- the ground a caption drawn over
    /// a picture sits on, so the glyphs read against whatever is under them.
    plate_radius: grid,
    /// The slack around a small target's geometry, so it stays clickable.
    hit_slop: grid,
    /// The width of an **edge grip**: the strip at each end of a placed object
    /// that resizes it (a clip's ends), drawn where it can be grabbed.
    grip_w: grid,
    /// The smallest gap between two ruler labels before the ladder steps up.
    label_gap: grid,
    /// The smallest gap between two ruler ticks before the ladder steps up.
    tick_gap: hairline,

    // -- Text (glyph scales over the embedded bitmap font) --
    /// Primary text: control labels, values, readouts.
    text_scale: glyph,
    /// In-view labels drawn over a surface (a lane's name tag).
    label_scale: glyph,
    /// Reduced text: ruler labels, a clip's caption.
    caption_scale: glyph,
    /// The densest legible mark (a key's octave label).
    micro_scale: glyph,

    // -- The type scale (pixels to the em, as the specification sizes text) --
    /// The smallest size: a key beside its entry, help under a field, the
    /// status footer, a count.
    type_xs: fine,
    /// The interface's size: menus, tabs, buttons, fields, a dialog's text.
    type_sm: fine,
    /// A page's body.
    type_base: fine,
    /// An alert's title.
    type_lg: fine,
    /// The line [`type_xs`](Metrics::type_xs) is set on.
    line_xs: grid,
    /// The line [`type_sm`](Metrics::type_sm) is set on.
    line_sm: grid,
    /// The line [`type_base`](Metrics::type_base) is set on.
    line_base: grid,
    /// The line [`type_lg`](Metrics::type_lg) is set on.
    line_lg: grid,

    // -- Radii --
    /// The small corner: a menu's entry, a menu bar's title, a tooltip.
    radius_sm: grid,
    /// The medium corner: a button, a field, a menu's list, a section.
    radius_md: grid,
    /// The large corner: the specification's base radius.
    radius_lg: grid,
    /// The extra large corner: a dialog's card.
    radius_xl: grid,

    // -- Elevation (how far a surface stands off the window) --
    /// The extra small lift: a field, an outlined button.
    shadow_xs: fine,
    /// The medium lift: a menu's list, a hover card.
    shadow_md: fine,
    /// The large lift: a submenu's list, a dialog.
    shadow_lg: fine,

    // -- States --
    /// How far a pressed button goes down.
    press_shift: hairline,

    // -- Spacing per component --
    /// The application bar's height.
    bar_h: grid,
    /// The application bar's inset at its ends.
    bar_inset: grid,
    /// The status footer's height.
    footer_h: grid,
    /// The status footer's inset at its ends.
    footer_inset: grid,
    /// The width of a panel's grip, at the start of its header.
    panel_grip: grid,
    /// The widest a tab grows.
    tab_w: grid,
    /// A tab's inset before its name.
    tab_inset: grid,
    /// A tab's inset after its close mark.
    tab_end: grid,
    /// The side of a tab's close mark, and of the small tools of a header.
    tab_close: grid,
    /// A menu bar title's inset across.
    menu_title_x: grid,
    /// A menu bar title's inset down.
    menu_title_y: grid,
    /// A menu's list, from its edge to its entries.
    menu_inset: grid,
    /// An entry's inset across.
    menu_item_x: grid,
    /// An entry's inset down.
    menu_item_y: grid,
    /// The narrowest a menu's list is.
    menu_w: grid,
    /// The narrowest a submenu's list is.
    submenu_w: grid,
    /// The indent a check or a radio entry gives its text, for the mark.
    menu_mark: grid,
    /// The widest a menu's list grows (a list of recent files).
    menu_max_w: grid,
    /// The gap between an entry's text and its aside (a recent file's
    /// folder).
    menu_aside: grid,
    /// A hover card's inset across.
    card_x: grid,
    /// A hover card's inset down.
    card_y: grid,
    /// The widest a hover card grows.
    card_w: grid,
    /// Between a hover card's head and the documentation under it.
    card_gap: grid,
    /// The tallest a hover card's documentation stands before it scrolls.
    card_doc_h: grid,
    /// A tooltip's inset across.
    tip_x: grid,
    /// A tooltip's inset down.
    tip_y: grid,
    /// A tooltip's inset after the key it ends in.
    tip_end: grid,
    /// How far a tooltip stands from what it is the tip of.
    tip_offset: grid,
    /// How far a tooltip's arrow reaches out of it, toward what it is the
    /// tip of.
    tip_arrow: grid,
    /// Between a tooltip's text and its key.
    tip_key_gap: grid,
    /// The height of a key drawn as a key cap (a tooltip's).
    kbd_h: grid,
    /// A key cap's inset across.
    kbd_x: grid,
    /// A dialog's inset.
    dialog_inset: grid,
    /// Between a dialog's parts: its header, its body, its footer.
    dialog_gap: grid,
    /// The width a dialog takes.
    dialog_w: grid,
    /// The width a dialog of settings takes.
    dialog_wide_w: grid,
    /// The width an alert dialog takes.
    alert_w: grid,
    /// Between a dialog's title and its description.
    dialog_title_gap: grid,
    /// Between an alert dialog's title and its description.
    alert_title_gap: grid,
    /// How far a dialog's close button stands from its top right corner.
    dialog_close: grid,
    /// A bordered section's inset, inside a dialog.
    section_inset: grid,
    /// A row of a list inside a dialog.
    list_row_h: grid,
    /// A button's height.
    button_h: grid,
    /// A small button's height.
    button_sm_h: grid,
    /// An extra small button's height.
    button_xs_h: grid,
    /// A large button's height.
    button_lg_h: grid,
    /// A button's inset across.
    button_x: grid,
    /// Between a button's icon and its text.
    button_gap: grid,
    /// A field's height.
    field_h: grid,
    /// A field's inset across.
    field_x: grid,
    /// A field's inset down.
    field_y: grid,
    /// The least a text area is high.
    text_area_h: grid,
    /// Between a label and the control it names.
    label_to_field: grid,
    /// A checkbox's side.
    check_w: grid,
    /// A checkbox's corner.
    check_radius: grid,
    /// Between a checkbox or a switch and the words beside it.
    check_gap: grid,
    /// A switch's track, along it.
    switch_w: grid,
    /// A switch's track, across it.
    switch_h: grid,
    /// A toggle button's inset across.
    toggle_x: grid,
    /// A badge's height.
    badge_h: grid,
    /// A badge's inset across.
    badge_x: grid,
    /// A scrollbar's width.
    scrollbar_w: grid,
    /// How far a scrollbar's thumb keeps off its edges.
    scrollbar_inset: grid,
    /// The band a split's line is grabbed in.
    split_grab: grid,
    /// An icon's side.
    icon: grid,
    /// A small icon's side.
    icon_sm: grid,
    /// The find bar's height.
    find_h: grid,
    /// The find bar's field's height.
    find_field_h: grid,
}

impl Default for Metrics {
    /// The table at the host's own density -- the sizes the widgets always had.
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
        // A size the specification gives, in its logical pixels, at this
        // density.
        let spec = |v: f32| grid(v * k);
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
            status_h: grid(cell * 1.25),
            // Five captions wide: the widest labels of the *fixed* vertical
            // vocabularies, `-32768`, `20K` and `-INF`. A data-dependent axis
            // measures its own and asks for more when it needs it.
            ruler_w: grid(5.0 * font::advance(caption_scale)),
            // Eight characters of header text wide.
            header_w: grid(8.0 * font::advance(text_scale)),
            panel_h: spec(36.0),
            divider_w: hairline(k),
            focus_ring: hairline(3.0 * k),
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

            type_xs: fine(12.0 * k),
            type_sm: fine(14.0 * k),
            type_base: fine(16.0 * k),
            type_lg: fine(18.0 * k),
            line_xs: spec(16.0),
            line_sm: spec(20.0),
            line_base: spec(24.0),
            line_lg: spec(28.0),

            radius_sm: spec(6.0),
            radius_md: spec(8.0),
            radius_lg: spec(10.0),
            radius_xl: spec(14.0),

            shadow_xs: fine(k),
            shadow_md: fine(4.0 * k),
            shadow_lg: fine(10.0 * k),
            press_shift: hairline(k),

            bar_h: spec(40.0),
            bar_inset: spec(6.0),
            footer_h: spec(24.0),
            footer_inset: spec(12.0),
            panel_grip: spec(20.0),
            tab_w: spec(224.0),
            tab_inset: spec(12.0),
            tab_end: spec(4.0),
            tab_close: spec(24.0),
            menu_title_x: spec(8.0),
            menu_title_y: spec(4.0),
            menu_inset: spec(4.0),
            menu_item_x: spec(8.0),
            menu_item_y: spec(6.0),
            menu_w: spec(144.0),
            submenu_w: spec(128.0),
            menu_mark: spec(32.0),
            menu_max_w: spec(576.0),
            menu_aside: spec(16.0),
            card_x: spec(12.0),
            card_y: spec(8.0),
            card_w: spec(768.0),
            card_gap: spec(6.0),
            card_doc_h: spec(240.0),
            tip_x: spec(12.0),
            tip_y: spec(6.0),
            tip_end: spec(6.0),
            tip_offset: spec(10.0),
            tip_arrow: spec(5.0),
            tip_key_gap: spec(8.0),
            kbd_h: spec(20.0),
            kbd_x: spec(4.0),
            dialog_inset: spec(24.0),
            dialog_gap: spec(24.0),
            dialog_w: spec(448.0),
            dialog_wide_w: spec(576.0),
            alert_w: spec(512.0),
            dialog_title_gap: spec(8.0),
            alert_title_gap: spec(6.0),
            dialog_close: spec(16.0),
            section_inset: spec(16.0),
            list_row_h: spec(44.0),
            button_h: spec(36.0),
            button_sm_h: spec(32.0),
            button_xs_h: spec(24.0),
            button_lg_h: spec(40.0),
            button_x: spec(10.0),
            button_gap: spec(6.0),
            field_h: spec(36.0),
            field_x: spec(10.0),
            field_y: spec(4.0),
            text_area_h: spec(64.0),
            label_to_field: spec(8.0),
            check_w: spec(16.0),
            check_radius: spec(4.0),
            check_gap: spec(8.0),
            switch_w: spec(32.0),
            switch_h: spec(18.0),
            toggle_x: spec(8.0),
            badge_h: spec(20.0),
            badge_x: spec(8.0),
            scrollbar_w: spec(10.0),
            scrollbar_inset: spec(2.0),
            split_grab: spec(4.0),
            icon: spec(16.0),
            icon_sm: spec(12.0),
            find_h: spec(36.0),
            find_field_h: spec(28.0),

            ui_scale: 1.0,
        }
    }

    /// This logical table resolved to the **physical** pixels of a window at
    /// `ui_scale` -- the one resolution HiDPI costs, run on a scale change and
    /// never per frame.
    ///
    /// Every role is scaled and re-quantized by its own family (extents onto
    /// the [`GRID`], hairlines onto whole pixels, text onto half-steps of the
    /// bitmap cell), because the chrome *is* hairlines: a divider, a track edge
    /// and a glyph pixel are one unit each, and a fractional position turns a
    /// crisp line into a two-pixel grey smear.
    ///
    /// At `ui_scale == 1.0` this is the **identity** -- the table the config
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
        self.scaled(k)
    }

    /// This table's own logical base, resolved at `scale` -- the table a widget
    /// seen through a zoom is sized and drawn with.
    ///
    /// A `scroll` workspace multiplies everything inside it by its zoom, and a
    /// zoom is an *enlargement*: the text, the padding, a disc's diameter and the
    /// gaps between rows all have to move together, or a zoomed box comes out as
    /// a box with oversized text jammed into it. So a placement's scale picks the
    /// table, exactly as a window's does -- this is that same resolution, taken
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

    /// Overlays `(role, value)` pairs -- the `[gui.metrics]` config table.
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

/// **How far a surface stands off the window**, which sets the shadow it
/// casts: the three lifts of the specification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Elevation {
    /// A field, an outlined button ([`Metrics::shadow_xs`]).
    Xs,
    /// A menu's list, a hover card ([`Metrics::shadow_md`]).
    Md,
    /// A submenu's list, a dialog ([`Metrics::shadow_lg`]).
    Lg,
}

/// **One layer of a shadow**: the surface's shape moved down by `y`, grown
/// by `spread` (shrunk when it is negative) and blurred over `blur`, drawn
/// at `alpha` of the theme's `shadow`. A shadow is one or two of them, as
/// the specification lays two shadows under one surface.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadowLayer {
    pub y: f32,
    pub blur: f32,
    pub spread: f32,
    pub alpha: f32,
}

/// Each elevation's layers per pixel of its lift, as `(y, blur, spread,
/// alpha)`: at the default lifts (1, 4 and 10) they are the
/// specification's shadows to the pixel, and a lift set by hand deepens or
/// flattens its level whole.
const SHADOWS: [&[(f32, f32, f32, f32)]; 3] = [
    &[(1.0, 2.0, 0.0, 0.5)],
    &[(1.0, 1.5, -0.25, 1.0), (0.5, 1.0, -0.5, 1.0)],
    &[(1.0, 1.5, -0.3, 1.0), (0.4, 0.6, -0.4, 1.0)],
];

impl Metrics {
    /// **The layers of the shadow `level` casts**, in this table's pixels.
    pub fn shadow(&self, level: Elevation) -> impl Iterator<Item = ShadowLayer> + use<> {
        let (lift, layers) = match level {
            Elevation::Xs => (self.shadow_xs, SHADOWS[0]),
            Elevation::Md => (self.shadow_md, SHADOWS[1]),
            Elevation::Lg => (self.shadow_lg, SHADOWS[2]),
        };
        layers
            .iter()
            .map(move |&(y, blur, spread, alpha)| ShadowLayer {
                y: y * lift,
                blur: blur * lift,
                spread: spread * lift,
                alpha,
            })
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
        assert_eq!(m.header_w, 72.0);
        assert_eq!(m.divider_w, 1.0);
        assert_eq!(m.focus_ring, 3.0);
        assert_eq!(m.trace_w, 1.5);
        assert_eq!(m.point_radius, 4.0);
        assert_eq!(m.plate_radius, 4.0);
        assert_eq!(m.hit_slop, 4.0);
        assert_eq!(m.grip_w, 12.0);
        assert_eq!(m.label_gap, 14.0);
        assert_eq!(m.tick_gap, 7.0);
        assert_eq!(m.text_scale, 1.5);
        assert_eq!(m.label_scale, 1.5);
        assert_eq!(m.caption_scale, 1.5);
        assert_eq!(m.micro_scale, 1.0);
        // the specification's, number for number
        assert_eq!(
            (m.type_xs, m.type_sm, m.type_base, m.type_lg),
            (12.0, 14.0, 16.0, 18.0)
        );
        assert_eq!(
            (m.line_xs, m.line_sm, m.line_base, m.line_lg),
            (16.0, 20.0, 24.0, 28.0)
        );
        assert_eq!(
            (m.radius_sm, m.radius_md, m.radius_lg, m.radius_xl),
            (6.0, 8.0, 10.0, 14.0)
        );
        assert_eq!(
            (m.bar_h, m.footer_h, m.panel_h, m.button_h),
            (40.0, 24.0, 36.0, 36.0)
        );
    }

    /// **The shadows are the specification's at the default lifts**, and
    /// follow a lift set by hand and a window's scale.
    #[test]
    fn each_elevation_casts_the_specification_s_layers() {
        let m = Metrics::default();
        let layers = |m: &Metrics, e| {
            m.shadow(e)
                .map(|l| (l.y, l.blur, l.spread, l.alpha))
                .collect::<Vec<_>>()
        };
        assert_eq!(layers(&m, Elevation::Xs), [(1.0, 2.0, 0.0, 0.5)]);
        assert_eq!(
            layers(&m, Elevation::Md),
            [(4.0, 6.0, -1.0, 1.0), (2.0, 4.0, -2.0, 1.0)]
        );
        assert_eq!(
            layers(&m, Elevation::Lg),
            [(10.0, 15.0, -3.0, 1.0), (4.0, 6.0, -4.0, 1.0)]
        );
        let twice = m.resolved(2.0);
        assert_eq!(layers(&twice, Elevation::Md)[0], (8.0, 12.0, -2.0, 1.0));
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
                "status_h",
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
        assert_eq!(m.text_scale, 3.0, "1.5 doubles onto a whole step");
        assert_eq!(m.caption_scale, 3.0);
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
