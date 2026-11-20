//! Drawing and pointer-math for the standard control widgets.
//!
//! Each control is built from the painter's flat primitives plus bitmap text, so
//! it needs no GPU code of its own: a `slider` is a track with a handle, a `knob`
//! a disc with a pointer, a `button`/`toggle`/`choice`/`number` a labelled box. The
//! drawing lives here; the *value* math a drag turns into (which fraction of a
//! slider's track the cursor is at, how far a vertical drag moves a knob) is
//! exposed as pure functions so it is unit-testable without a window. The
//! interaction *state* (what is pressed, drag anchors) lives in the windowed
//! front, which calls these.

use super::textedit;
use crate::host::widget::size;
use crate::host::widget::{Align, Range};
use clausters_gui_draw::draw::Draw;
use clausters_gui_draw::font::{self, Family};
use clausters_gui_draw::metrics::{Elevation, Metrics};
use clausters_gui_draw::paint::Mesh;
use clausters_gui_draw::rect::Rect;
use clausters_gui_draw::theme::Color;

/// The label strip height when a control carries a label, else 0 -- **and 0 when
/// the cell cannot hold both**.
///
/// A squeezed control drops its label before it squeezes its body: a field box
/// shorter than the text inside it is a drawing that lies (the glyphs stand
/// clear of their own background), while a value with no caption is merely
/// terser. Every geometry here goes through this, so the strip that is not
/// reserved is also not drawn.
pub fn label_height(rect_h: f32, has_label: bool, text_size: f32, m: &Metrics) -> f32 {
    // The pad is above the line and the descent below it: a caption clears what
    // it sits over by the same distance whether or not it happens to contain a
    // `p` ([`font::descent`]).
    let strip = font::height(text_size) + font::descent(text_size) + m.pad;
    if has_label && rect_h - strip - 2.0 * m.pad >= font::height(text_size) {
        strip
    } else {
        0.0
    }
}

/// The control body at the host's own text scale (the views that keep the fixed
/// label scale). Shared by drawing and hit-math so they agree on the track.
pub fn body_rect(rect: Rect, has_label: bool, m: &Metrics) -> Rect {
    body_rect_at(rect, has_label, m.text_scale, m)
}

/// The control body (the rect minus its label strip -- sized by the widget's
/// `text_size` -- and a small inset).
///
/// **In a short cell the inset gives way before the body does**, for the same
/// reason the label goes first ([`label_height`]): the inset is air, and a box
/// shorter than its text is a drawing that lies. So a cell with room for the
/// text and not for the air around it gets a body as tall as the text or as
/// the cell, and the sides keep their inset.
pub fn body_rect_at(rect: Rect, has_label: bool, text_size: f32, m: &Metrics) -> Rect {
    let top = rect.y + label_height(rect.h, has_label, text_size, m);
    let room = (rect.y + rect.h - top).max(0.0);
    let inset = ((room - font::height(text_size)) * 0.5).clamp(0.0, m.pad);
    Rect::new(
        rect.x + m.pad,
        top + inset,
        (rect.w - 2.0 * m.pad).max(0.0),
        room - 2.0 * inset,
    )
}

/// The strip a control reserves at the **bottom** of its body for its value
/// read-out: one row of text and the gap above it. The number lives there and
/// nowhere else, so it never lands on the thing it is reading -- a knob's disc,
/// a slider's groove and handle.
pub fn readout_h(text_size: f32, m: &Metrics) -> f32 {
    font::height(text_size) + m.pad
}

/// A slider's **track area**: its body minus the read-out strip
/// ([`readout_h`]) when it has one, the rect the groove and the handle live in.
///
/// Drawing and hit math both go through here, so a grab lands where the groove
/// is drawn. That matters most on a *vertical* slider, whose fraction runs along
/// the very axis the strip shortens: a body-wide hit would read the bottom of
/// the number row as the minimum, below where the track visibly ends.
pub fn slider_track(
    rect: Rect,
    has_label: bool,
    readout: bool,
    text_size: f32,
    m: &Metrics,
) -> Rect {
    let body = body_rect_at(rect, has_label, text_size, m);
    let strip = if readout {
        readout_h(text_size, m)
    } else {
        0.0
    };
    Rect::new(body.x, body.y, body.w, (body.h - strip).max(0.0))
}

/// **The band a slider is actually drawn in**, inside its track area -- the
/// groove down the middle plus the handle's grip across it, which is as thick
/// as this control ever gets.
///
/// The track area is the whole cell minus the label and the read-out, and the
/// slider fills a fraction of it: a groove `track_thick` across with a grip
/// `handle_grip` long over it, centred. So the area answers for a control that
/// is drawn several times thinner than it, which is why the press reads this
/// instead -- the hit is the drawing, the same rule the dial and the notehead
/// follow (`graphics::shape`).
pub fn slider_groove(body: Rect, vertical: bool, m: &Metrics) -> Rect {
    if vertical {
        let t = m.track_thick.max(m.handle_grip.min(body.w));
        Rect::new(body.x + (body.w - t) * 0.5, body.y, t, body.h)
    } else {
        let t = m.track_thick.max(m.handle_grip.min(body.h));
        Rect::new(body.x, body.y + (body.h - t) * 0.5, body.w, t)
    }
}

/// The 0..1 fraction along a horizontal track at pixel `px` (for a slider).
pub fn slider_fraction(body: Rect, px: f64) -> f32 {
    if body.w <= 0.0 {
        return 0.0;
    }
    (((px as f32) - body.x) / body.w).clamp(0.0, 1.0)
}

/// The 0..1 fraction along a vertical track at pixel `py` -- bottom is 0, top is
/// 1, so dragging up raises the value (for a `vertical` slider).
pub fn slider_fraction_v(body: Rect, py: f64) -> f32 {
    if body.h <= 0.0 {
        return 0.0;
    }
    (1.0 - ((py as f32) - body.y) / body.h).clamp(0.0, 1.0)
}

/// The fraction change from a vertical drag of `dy` device pixels over a body of
/// height `body_h`: dragging up (negative `dy`) increases the value. A full body
/// height is one full range; the factor keeps fine control on tall bodies.
pub fn drag_fraction_delta(dy: f64, body_h: f32) -> f32 {
    let span = body_h.max(40.0);
    (-(dy as f32)) / span
}

/// Draws the label strip above a control body, if it has a label (clipped to
/// the cell with an ellipsis).
pub(crate) fn label_strip(d: &mut Draw, label: Option<&str>, rect: Rect, size: f32) {
    let pad = d.m.pad;
    label_strip_at(d, label, rect, size, pad);
}

/// **A control's label strip**, as the design system's label: the medium
/// weight, its start `inset` in from the cell's -- where the control's body
/// starts, so the caption stands flush with what it names (a field's box
/// is flush with its cell, a slider's track a pad in).
pub(crate) fn label_strip_at(d: &mut Draw, label: Option<&str>, rect: Rect, size: f32, inset: f32) {
    let (mesh, m, theme) = d.parts();
    if label_height(rect.h, label.is_some(), size, m) <= 0.0 {
        return; // no room for both: the body keeps the cell
    }
    if let Some(text) = label {
        font::text_ellipsis_in(
            mesh,
            Family::UiMedium,
            text,
            rect.x + inset,
            rect.y + m.pad,
            (rect.w - inset - m.pad).max(0.0),
            size,
            theme.text,
        );
    }
}

pub fn slider(d: &mut Draw, r: &Range, rect: Rect, vertical: bool, readout: bool, size: f32) {
    label_strip(d, r.label.as_deref(), rect, size);
    // The groove lives in the track area, the number in the strip under it (the
    // knob's posture): the read-out is beside what it reads, never over it.
    let body = slider_track(rect, r.label.is_some(), readout, size, d.m);
    let (mesh, m, theme) = d.parts();
    // The track's groove, the value riding it and the handle's grip across the
    // axis: the handle is a short grip, **not** the full body span.
    let (track_thick, handle_thick) = (m.track_thick, m.handle_thick);
    let f = r.fraction();
    if vertical {
        // Track down the centre; min at the bottom, max at the top.
        let cx = body.x + body.w * 0.5;
        mesh.rect(
            Rect::new(cx - track_thick * 0.5, body.y, track_thick, body.h),
            theme.track,
        );
        let hy = body.y + body.h * (1.0 - f);
        mesh.rect(
            Rect::new(
                cx - track_thick * 0.5,
                hy,
                track_thick,
                (body.y + body.h - hy).max(0.0),
            ),
            theme.accent_dim,
        );
        let grip = m.handle_grip.min(body.w);
        mesh.rect(
            Rect::new(cx - grip * 0.5, hy - handle_thick * 0.5, grip, handle_thick),
            theme.accent,
        );
    } else {
        let mid = body.y + body.h * 0.5;
        mesh.rect(
            Rect::new(body.x, mid - track_thick * 0.5, body.w, track_thick),
            theme.track,
        );
        let hx = body.x + body.w * f;
        mesh.rect(
            Rect::new(
                body.x,
                mid - track_thick * 0.5,
                (hx - body.x).max(0.0),
                track_thick,
            ),
            theme.accent_dim,
        );
        let grip = m.handle_grip.min(body.h);
        mesh.rect(
            Rect::new(
                hx - handle_thick * 0.5,
                mid - grip * 0.5,
                handle_thick,
                grip,
            ),
            theme.accent,
        );
    }
    if readout {
        let strip = Rect::new(body.x, body.y + body.h, body.w, readout_h(size, m));
        value_text(d, &fmt(r.value), strip, size);
    }
}

/// **Where a knob's dial is**, as `(cx, cy, radius)` inside its `body` -- one
/// function so the drawing and the hit-test cannot disagree about the disc.
///
/// The read-out takes a strip at the bottom of the body and the disc is sized
/// in the area above it, so the number stays inside the body: it never overlaps
/// the disc nor spills past the cell into the row below.
pub fn knob_disc(body: Rect, size: f32, m: &Metrics) -> (f32, f32, f32) {
    let disc_h = (body.h - readout_h(size, m)).max(0.0);
    (
        body.x + body.w * 0.5,
        body.y + disc_h * 0.5,
        (body.w.min(disc_h) * 0.5 - 2.0).max(2.0),
    )
}

/// **The diameter a knob's disc keeps at least**: the `knob_d` role, grown
/// with the knob's own `text_size` where that is larger than the default one
/// (both logical, so a display's scale moves neither), so a disc is never
/// small beside its own caption and read-out -- a disc a hand cannot aim at is
/// not a control. It is what the knob's floor keeps, on both axes.
pub fn knob_disc_d(r: &Range, m: &Metrics) -> f32 {
    m.knob_d * (r.text_size / font::DEFAULT_SIZE).max(1.0)
}

/// **Whether a knob in a cell `rect_h` tall draws its label**: only where the
/// disc and the read-out still get their whole height under it. The label is
/// what a knob gives up first ([`label_give`]), so a knob placed at its floor
/// draws its disc at full size and no caption -- never a caption over a disc
/// shrunk to make room for it.
pub fn knob_labelled(r: &Range, rect_h: f32, size: f32, m: &Metrics) -> bool {
    r.label.is_some()
        && rect_h + 0.5
            >= size::label_strip(true, size, m)
                + size::body_inset(m)
                + knob_disc_d(r, m)
                + readout_h(size, m)
}

/// The body a knob draws its disc and read-out in: the cell under its label
/// where it draws one ([`knob_labelled`]), the cell whole where it does not.
pub fn knob_body(r: &Range, rect: Rect, size: f32, m: &Metrics) -> Rect {
    body_rect_at(rect, knob_labelled(r, rect.h, size, m), size, m)
}

pub fn knob(d: &mut Draw, r: &Range, rect: Rect, size: f32) {
    let labelled = knob_labelled(r, rect.h, size, d.m);
    if labelled {
        label_strip(d, r.label.as_deref(), rect, size);
    }
    let body = body_rect_at(rect, labelled, size, d.m);
    let (cx, cy, radius) = knob_disc(body, size, d.m);
    let text_h = readout_h(size, d.m);
    knob_dial(d, cx, cy, radius, r.fraction());
    let readout = Rect::new(body.x, body.y + body.h - text_h, body.w, text_h);
    value_text(d, &fmt(r.value), readout, size);
}

/// **The dial itself** -- the ring, the face and the pointer at `fraction` of a
/// 270-degree sweep, min at lower-left and max at lower-right.
///
/// Apart from the widget so that a *header's* level knob is the same dial as a
/// `knob` widget's rather than a second drawing of the same idea: a control
/// that reads one way in one place and another way in another is two controls
/// to a reader. What the widget adds around it is its own -- a label strip, a
/// read-out -- and a header cell has room for neither.
pub fn knob_dial(d: &mut Draw, cx: f32, cy: f32, radius: f32, fraction: f32) {
    let (mesh, _, theme) = d.parts();
    mesh.disc(cx, cy, radius, theme.track);
    mesh.disc(cx, cy, radius - 3.0, theme.field);
    let angle = (135.0 + 270.0 * fraction.clamp(0.0, 1.0)).to_radians();
    let tip = [cx + radius * angle.cos(), cy + radius * angle.sin()];
    mesh.line([cx, cy], tip, 3.0, theme.accent);
}

/// The two cells of a number's **stepper**: the arrow that raises it, over the
/// one that lowers it, at the right edge of its body. One function for the
/// drawing and the press.
pub fn stepper_cells(body: Rect, size: f32, m: &Metrics) -> (Rect, Rect) {
    let w = (font::height(size) + m.pad).min(body.w * 0.5);
    let x = body.x + body.w - w;
    let half = body.h * 0.5;
    (
        Rect::new(x, body.y, w, half),
        Rect::new(x, body.y + half, w, body.h - half),
    )
}

pub fn number(d: &mut Draw, r: &Range, rect: Rect, size: f32, stepper: bool, typed: Option<&str>) {
    label_strip(d, r.label.as_deref(), rect, size);
    let whole = body_rect_at(rect, r.label.is_some(), size, d.m);
    // The arrows take their cells off the field, so the value is centred in
    // what is left of it rather than under them.
    let cells = stepper.then(|| stepper_cells(whole, size, d.m));
    let body = match cells {
        Some((up, _)) => Rect::new(whole.x, whole.y, (whole.w - up.w).max(0.0), whole.h),
        None => whole,
    };
    if let Some((up, down)) = cells {
        let (mesh, m, theme) = d.parts();
        mesh.rect(Rect::new(up.x, up.y, up.w, up.h + down.h), theme.track);
        // The arrows are half a field tall each, so they draw a size down.
        let small = font::quantize_size(size * 0.5).max(1.0);
        for (cell, glyph) in [
            (up, font::symbol::POINT_UP),
            (down, font::symbol::POINT_DOWN),
        ] {
            font::text_centered(mesh, &glyph.to_string(), cell, small, theme.accent);
        }
        mesh.rect(
            Rect::new(up.x, up.y + up.h, up.w, m.divider_w),
            theme.separator,
        );
    }
    let (mesh, m, theme) = d.parts();
    // The figure alone in a framed field: a number is read, so nothing
    // behind it draws a level the way a meter or a slider would.
    mesh.rect(body, theme.field);
    border(mesh, body, m.divider_w, theme.accent);
    match typed {
        // what is being typed, with the caret after it
        Some(typed) => {
            let w = font::width(typed, size);
            let x = body.x + (body.w - w) * 0.5;
            let ty = body.y + (body.h - font::height(size)) * 0.5;
            font::text(mesh, typed, x, ty, size, theme.text);
            mesh.rect(
                Rect::new(x + w, ty, m.divider_w.max(1.0), font::height(size)),
                theme.text,
            );
        }
        None => font::text_centered(mesh, &fmt(r.value), body, size, theme.text),
    }
}

/// Draws a `label` into its rect: the line block vertically centered, each
/// line placed by `align`. With `wrap` the text word-wraps on the font's
/// fixed advance and lines past the rect's bottom are dropped; without it the
/// single line clips with an ellipsis instead of bleeding into a neighbor.
pub fn draw_label(d: &mut Draw, text: &str, rect: Rect, size: f32, wrap: bool, align: Align) {
    draw_label_as(d, text, rect, size, wrap, align, LabelLook::default());
}

/// **How a label reads besides its text**: a dot before it in a role of the
/// theme (a state: `success`, `warning`, `destructive`), and the text
/// muted, as a status line's is.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LabelLook {
    pub dot: Option<clausters_gui_draw::theme::Color>,
    pub muted: bool,
    pub variant: LabelVariant,
}

/// **What a label is**, the design system's: plain text, a field's caption
/// (the medium weight, set solid, flush with its field), the help under a
/// field (the small size, quiet), a badge (a pill on the `secondary`
/// fill, in the small size and the medium weight), a path (a value a
/// person reads and does not type: a field's box with the `border` edge on
/// `muted` at two fifths, the code face at the type scale's size), or code
/// (a path or a command in a line of a list: the code face, small and
/// quiet, as a help is).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LabelVariant {
    #[default]
    Text,
    Caption,
    Help,
    Badge,
    Path,
    Code,
}

impl LabelVariant {
    pub(crate) fn from_str(s: &str) -> Option<LabelVariant> {
        Some(match s {
            "text" => LabelVariant::Text,
            "caption" => LabelVariant::Caption,
            "help" => LabelVariant::Help,
            "badge" => LabelVariant::Badge,
            "path" => LabelVariant::Path,
            "code" => LabelVariant::Code,
            _ => return None,
        })
    }

    /// The face it is drawn in.
    pub fn family(self) -> Family {
        match self {
            LabelVariant::Caption | LabelVariant::Badge => Family::UiMedium,
            LabelVariant::Path | LabelVariant::Code => Family::Mono,
            _ => Family::Ui,
        }
    }

    /// The scale its text is drawn at when its `text_size` is the default:
    /// the type scale's size for it.
    pub fn size(self, m: &Metrics) -> Option<f32> {
        match self {
            LabelVariant::Text => None,
            LabelVariant::Caption => Some(font::em_scale(Family::UiMedium, m.type_sm)),
            LabelVariant::Help => Some(font::em_scale(Family::Ui, m.type_xs)),
            LabelVariant::Badge => Some(font::em_scale(Family::UiMedium, m.type_xs)),
            LabelVariant::Path => Some(font::em_scale(Family::Mono, m.type_sm)),
            LabelVariant::Code => Some(font::em_scale(Family::Mono, m.type_xs)),
        }
    }

    /// The line its text is set on, for a variant of the type scale.
    pub fn line(self, m: &Metrics) -> Option<f32> {
        match self {
            LabelVariant::Text => None,
            LabelVariant::Caption => Some(m.type_sm),
            LabelVariant::Help => Some(m.line_xs),
            LabelVariant::Badge => Some(m.badge_h),
            LabelVariant::Path => Some(m.field_h),
            LabelVariant::Code => Some(m.line_xs),
        }
    }

    /// The inset its text stands in across: a plain label's pad, nothing
    /// for a caption or a help, which line up with their field.
    pub fn inset(self, m: &Metrics) -> f32 {
        match self {
            LabelVariant::Caption | LabelVariant::Help | LabelVariant::Code => 0.0,
            LabelVariant::Badge => m.badge_x,
            LabelVariant::Path => m.field_x,
            LabelVariant::Text => m.pad,
        }
    }
}

/// [`draw_label`], with its [`LabelLook`]: the dot, the size of a lower
/// case letter, stands at the line's start and the text after it.
pub fn draw_label_as(
    d: &mut Draw,
    text: &str,
    rect: Rect,
    size: f32,
    wrap: bool,
    align: Align,
    look: LabelLook,
) {
    let (mesh, m, theme) = d.parts();
    let family = look.variant.family();
    if look.variant == LabelVariant::Badge {
        // a pill as wide as what it says, at the start of its cell
        let h = m.badge_h.min(rect.h);
        let w = (font::width_in(family, text, size) + 2.0 * m.badge_x).min(rect.w);
        let x = align_x(align, rect.x, rect.w, w);
        let pill = Rect::new(x, rect.y + (rect.h - h) * 0.5, w, h);
        mesh.round_rect(pill, h * 0.5, theme.secondary);
        let ty = pill.y + (pill.h - font::height(size)) * 0.5;
        font::text_ellipsis_in(
            mesh,
            family,
            text,
            pill.x + m.badge_x,
            ty,
            (pill.w - 2.0 * m.badge_x).max(0.0),
            size,
            theme.secondary_text,
        );
        return;
    }
    if look.variant == LabelVariant::Path {
        // the box across its cell, one field high, in the middle of it
        let h = m.field_h.min(rect.h);
        let body = Rect::new(rect.x, rect.y + (rect.h - h) * 0.5, rect.w, h);
        mesh.round_rect(body, m.radius_md, with_alpha(theme.muted, 0.4));
        mesh.round_border(body, m.divider_w, m.radius_md, theme.separator);
    }
    let inset = look.variant.inset(m);
    let mut left = rect.x + inset;
    let mut avail = (rect.w - 2.0 * inset).max(0.0);
    if let Some(color) = look.dot {
        let r = (font::height(size) * 0.4).max(1.0);
        let cy = rect.y + rect.h * 0.5;
        mesh.disc(left + r, cy, r, color);
        let taken = 2.0 * r + m.pad;
        left += taken;
        avail = (avail - taken).max(0.0);
    }
    let ink = if look.muted || matches!(look.variant, LabelVariant::Help | LabelVariant::Code) {
        theme.text_dim
    } else {
        theme.text
    };
    if wrap {
        let lines = font::wrap_in(family, text, avail, size);
        let advance = look
            .variant
            .line(m)
            .unwrap_or_else(|| font::line_advance(size));
        let block_h = lines.len() as f32 * advance;
        let mut y = (rect.y + (rect.h - block_h) * 0.5).max(rect.y);
        for (i, line) in lines.iter().enumerate() {
            // The first line always draws; later lines drop once they overflow.
            if i > 0 && y + font::height(size) > rect.y + rect.h {
                break;
            }
            let x = align_x(align, left, avail, font::width_in(family, line, size));
            let ty = match look.variant.line(m) {
                Some(_) => y + (advance - font::height(size)).max(0.0) * 0.5,
                None => y,
            };
            font::text_in(mesh, family, line, x, ty, size, ink);
            y += advance;
        }
    } else {
        let y = (rect.y + (rect.h - font::height(size)) * 0.5).max(rect.y);
        let x = align_x(
            align,
            left,
            avail,
            font::width_in(family, text, size).min(avail),
        );
        font::text_ellipsis_in(mesh, family, text, x, y, avail, size, ink);
    }
}

/// The x a line of width `tw` starts at inside `[left, left + avail]`.
fn align_x(align: Align, left: f32, avail: f32, tw: f32) -> f32 {
    match align {
        Align::Start => left,
        Align::Center => left + (avail - tw) * 0.5,
        Align::End => left + avail - tw,
    }
    .max(left)
}

/// A 1-color outline of `rect`, `w` pixels thick (four thin rects).
fn border(mesh: &mut Mesh, rect: Rect, w: f32, color: Color) {
    mesh.rect(Rect::new(rect.x, rect.y, rect.w, w), color);
    mesh.rect(Rect::new(rect.x, rect.y + rect.h - w, rect.w, w), color);
    mesh.rect(Rect::new(rect.x, rect.y, w, rect.h), color);
    mesh.rect(Rect::new(rect.x + rect.w - w, rect.y, w, rect.h), color);
}

/// **A button's variant**, the design system's: what it is for, read in
/// its fill, its edge and its ink.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Variant {
    /// The button that does what a form is for: the `primary` fill.
    #[default]
    Default,
    /// An edge and the ground: a second action beside the first.
    Outline,
    /// The `secondary` fill: a quieter action.
    Secondary,
    /// No fill until the pointer is over it: a tool in a row of tools.
    Ghost,
    /// An action that destroys, in the `destructive` ink on its faint fill.
    Destructive,
    /// Words, underlined under the pointer.
    Link,
}

impl Variant {
    pub(crate) fn from_str(s: &str) -> Option<Variant> {
        Some(match s {
            "default" => Variant::Default,
            "outline" => Variant::Outline,
            "secondary" => Variant::Secondary,
            "ghost" => Variant::Ghost,
            "destructive" => Variant::Destructive,
            "link" => Variant::Link,
            _ => return None,
        })
    }
}

/// **A button's size**, the design system's: the height of its box, which
/// an icon alone fills as a square.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ButtonSize {
    #[default]
    Default,
    Sm,
    Xs,
    Lg,
}

impl ButtonSize {
    pub(crate) fn from_str(s: &str) -> Option<ButtonSize> {
        Some(match s {
            "default" => ButtonSize::Default,
            "sm" => ButtonSize::Sm,
            "xs" => ButtonSize::Xs,
            "lg" => ButtonSize::Lg,
            _ => return None,
        })
    }

    /// The height of its box.
    pub fn height(self, m: &Metrics) -> f32 {
        match self {
            ButtonSize::Default => m.button_h,
            ButtonSize::Sm => m.button_sm_h,
            ButtonSize::Xs => m.button_xs_h,
            ButtonSize::Lg => m.button_lg_h,
        }
    }
}

/// How a button stands right now, and which look it wears.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ButtonLook {
    /// On: a tool whose switch is on, a toggle drawn as a button.
    pub active: bool,
    /// The pointer is over it.
    pub hovered: bool,
    /// The `flat` look: the ghost variant, whatever `variant` says -- what a
    /// row of tools wants, where a wall of filled boxes would outweigh the
    /// work.
    pub flat: bool,
    /// Its variant.
    pub variant: Variant,
    /// Held down by the hand: what it says stands a little lower.
    pub held: bool,
}

/// Whether `theme` is a dark look: its ground darker than its middle grey.
pub(crate) fn dark(theme: &clausters_gui_draw::theme::Theme) -> bool {
    let [r, g, b, _] = theme.background;
    0.2126 * r + 0.7152 * g + 0.0722 * b < 0.5
}

/// The scale a control's words are drawn at when its `text_size` is the
/// default: the interface's size of the type scale, in `family`.
pub fn ui_size(family: Family, m: &Metrics) -> f32 {
    font::em_scale(family, m.type_sm)
}

/// **The character of the icon `name`** the host carries, or `fallback`
/// where it carries none (a build that strokes no path): what a tool of the
/// host's own chrome is drawn with.
pub fn icon(name: &str, fallback: char) -> char {
    font::outline::icon(name).unwrap_or(fallback)
}

/// The text a button shows: its icon, its label, or the two with a space
/// between them. A button given neither says `BUTTON`, as it always has.
pub fn button_text(label: Option<&str>, icon: Option<char>) -> String {
    match (icon, label) {
        (Some(i), Some(l)) => format!("{i} {l}"),
        (Some(i), None) => i.to_string(),
        (None, Some(l)) => l.to_string(),
        (None, None) => "BUTTON".to_string(),
    }
}

/// **The lone symbol a caption is**, when it is one character a window
/// brought the outline of: a shape that scales to any size, unlike the
/// bitmap face's whole steps.
fn lone_symbol(caption: &str) -> Option<char> {
    let mut chars = caption.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => font::outline::advance(c, 1.0).map(|_| c),
        _ => None,
    }
}

/// **The smallest size a lone symbol is drawn at**: the host's text size, or
/// the caption's own where that is smaller -- a symbol shrunk to fit its
/// box never goes under the text beside it.
fn symbol_floor_size(size: f32, m: &Metrics) -> f32 {
    size.min(m.text_scale)
}

/// **The size a lone symbol is drawn at**: its own `size` times `fit`, the
/// factor its container found for every lone symbol it holds
/// ([`Ctx::text_fit`](crate::host::widget::element::Ctx::text_fit)), never
/// under [`symbol_floor_size`] -- and any other caption at its own size. So
/// the symbols of a palette shrink together, keeping their sizes against each
/// other, where a word is cut to an ellipsis instead.
pub fn symbol_size(caption: &str, size: f32, fit: f32, m: &Metrics) -> f32 {
    if lone_symbol(caption).is_none() || fit >= 1.0 {
        return size;
    }
    (size * fit).max(symbol_floor_size(size, m)).min(size)
}

/// **The caption an element draws, when it is a lone symbol**, and the size
/// it is drawn at: what a container shares one fit among
/// ([`crate::host::widget::Element::lone_symbol`]).
pub fn as_lone_symbol(caption: String, size: f32) -> Option<(String, f32)> {
    lone_symbol(&caption).map(|_| (caption, size))
}

/// **The narrowest a button's caption is given**: a lone symbol at its
/// smallest size ([`symbol_floor_size`]), any other caption cut to an
/// ellipsis (`size::caption_floor`). No padding: a squeezed control gives
/// that up first.
pub fn caption_floor(caption: &str, size: f32, m: &Metrics) -> f32 {
    match lone_symbol(caption) {
        Some(_) => font::width(caption, symbol_floor_size(size, m)),
        None => size::caption_floor(caption, size),
    }
}

/// **A button**, as the design system draws one: its box with the medium
/// corner in its variant's fill and edge, and centred in it the icon, then
/// the label in the medium weight, its variant's ink; a little lower while it
/// is held. Its states are its variant's: under the pointer or on, a ghost
/// or an outlined button takes the `muted` fill and a filled one fades to
/// four fifths.
pub fn button(
    d: &mut Draw,
    label: Option<&str>,
    icon: Option<char>,
    rect: Rect,
    look: ButtonLook,
    size: f32,
) {
    let variant = if look.flat {
        Variant::Ghost
    } else {
        look.variant
    };
    let lit = look.hovered || look.active;
    let radius = d.m.radius_md;
    if variant == Variant::Outline {
        d.shadow(rect, radius, Elevation::Xs);
    }
    let (mesh, m, theme) = d.parts();
    let dark = dark(theme);
    let fade = |c: Color| if lit { with_alpha(c, 0.8) } else { c };
    let (fill, edge, ink) = match variant {
        Variant::Default => (Some(fade(theme.primary)), None, theme.primary_text),
        Variant::Secondary => (Some(fade(theme.secondary)), None, theme.secondary_text),
        Variant::Outline => {
            let ground = if lit {
                theme.muted
            } else if dark {
                with_alpha(theme.input, 0.3)
            } else {
                theme.background
            };
            let edge = if dark { theme.input } else { theme.separator };
            (Some(ground), Some(edge), theme.text)
        }
        Variant::Ghost => (lit.then_some(theme.muted), None, theme.text),
        Variant::Destructive => (
            Some(with_alpha(theme.destructive, if lit { 0.2 } else { 0.1 })),
            None,
            theme.destructive,
        ),
        Variant::Link => (None, None, theme.primary),
    };
    if let Some(fill) = fill {
        mesh.round_rect(rect, radius, fill);
    }
    if let Some(edge) = edge {
        mesh.round_border(rect, m.divider_w, radius, edge);
    }
    let dy = if look.held { m.press_shift } else { 0.0 };
    let face = Rect::new(rect.x, rect.y + dy, rect.w, rect.h);
    let (left, right) = button_face(mesh, label, icon, face, size, m, ink);
    if variant == Variant::Link && look.hovered && right > left {
        let y = face.y + (face.h + font::height(size)) * 0.5 + m.divider_w;
        mesh.rect(Rect::new(left, y, right - left, m.divider_w), ink);
    }
}

/// **What a button says, centred in `face`**: its icon, the button's gap,
/// its label in the medium weight -- cut short where the box is narrower.
/// The span the label took, for an underline.
fn button_face(
    mesh: &mut Mesh,
    label: Option<&str>,
    icon: Option<char>,
    face: Rect,
    size: f32,
    m: &Metrics,
    ink: Color,
) -> (f32, f32) {
    let glyph = font::height(size);
    let ty = face.y + (face.h - glyph) * 0.5;
    let icon_s = icon.map(|c| c.to_string());
    let icon_w = icon_s.as_deref().map_or(0.0, |c| font::width(c, size));
    let label = match (label, icon) {
        (Some(l), _) => Some(l),
        (None, Some(_)) => None,
        (None, None) => Some("BUTTON"),
    };
    let gap = if icon.is_some() && label.is_some() {
        m.button_gap
    } else {
        0.0
    };
    let label_w = label.map_or(0.0, |l| font::width_in(Family::UiMedium, l, size));
    let room = (face.w - 2.0 * m.pad.min(m.button_x)).max(0.0);
    let full = icon_w + gap + label_w;
    // **An icon alone is centred whole**, as any lone symbol is, even where
    // it is wider than the room inside the box: cut to the room it stood
    // off centre, a few pixels from where the same icon is drawn when it
    // cannot be pressed. Words are cut to the room.
    let total = if label.is_none() {
        full
    } else {
        full.min(room)
    };
    let mut x = face.x + (face.w - total) * 0.5;
    if let Some(c) = &icon_s {
        font::text(mesh, c, x, ty, size, ink);
        x += icon_w + gap;
    }
    let start = x;
    if let Some(l) = label {
        let avail = (face.x + face.w - x)
            .max(0.0)
            .min(label_w.max(room - icon_w - gap));
        font::text_ellipsis_in(mesh, Family::UiMedium, l, x, ty, avail, size, ink);
        x += label_w.min(avail);
    }
    (start, x)
}

/// **A scroll bar's thumb**, as the design system draws one: no track,
/// the thumb `thumb` (as thick as `scrollbar_w`) kept `scrollbar_inset`
/// off its edges, a pill of the quiet text at two fifths. Every scroll bar
/// the host draws is this one.
pub fn scroll_thumb(
    mesh: &mut Mesh,
    thumb: Rect,
    m: &Metrics,
    theme: &clausters_gui_draw::theme::Theme,
) {
    let r = thumb.inset(m.scrollbar_inset);
    if r.w <= 0.0 || r.h <= 0.0 {
        return;
    }
    mesh.round_rect(r, r.w.min(r.h) * 0.5, with_alpha(theme.text_dim, 0.4));
}

/// **A scroll bar over content an element scrolls itself** (the code, the
/// documentation): where its thumb stands along its track, and the hand on
/// it. A press on the thumb holds it where it was taken; a press on the
/// track off it brings the thumb's middle to the pointer first, as a
/// plane's bar does; a drag moves the view as far as the thumb moves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollBar {
    /// Whether it runs down the element's right edge (and scrolls down).
    pub vertical: bool,
    /// The groove the thumb travels in.
    pub track: Rect,
    /// The thumb: where the view stands, and how much of it is seen.
    pub thumb: Rect,
    /// The most the view's offset goes to.
    span: f32,
}

/// What a press on a [`ScrollBar`] holds: where along the track it was and
/// the offset the view had then.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ThumbGrip {
    vertical: bool,
    from: f32,
    offset: f32,
}

impl ScrollBar {
    /// The bar along `track` of a view `seen` long over content it scrolls
    /// by up to `span`, at `offset`, its thumb no shorter than `least`; `None`
    /// when there is nothing to scroll.
    pub fn new(
        vertical: bool,
        track: Rect,
        seen: f32,
        span: f32,
        offset: f32,
        least: f32,
    ) -> Option<Self> {
        if span <= 0.5 {
            return None;
        }
        let len = if vertical { track.h } else { track.w };
        let size = (len * seen / (seen + span)).max(least.min(len)).min(len);
        let at = (len - size) * (offset / span).clamp(0.0, 1.0);
        let thumb = if vertical {
            Rect::new(track.x, track.y + at, track.w, size)
        } else {
            Rect::new(track.x + at, track.y, size, track.h)
        };
        Some(Self {
            vertical,
            track,
            thumb,
            span,
        })
    }

    fn along(&self, (x, y): (f64, f64)) -> f32 {
        if self.vertical { y as f32 } else { x as f32 }
    }

    /// The view's offset per pixel of the thumb's travel.
    fn per_px(&self) -> f32 {
        let (len, size) = if self.vertical {
            (self.track.h, self.thumb.h)
        } else {
            (self.track.w, self.thumb.w)
        };
        self.span / (len - size).max(1.0)
    }

    /// **A press at `at`**, the view at `offset`: on the track, the hold and
    /// the offset the view takes now (the thumb's middle at the pointer when
    /// the press was off it); elsewhere, `None`.
    pub fn press(&self, at: (f64, f64), offset: f32) -> Option<(ThumbGrip, f32)> {
        if !self.track.contains(at.0, at.1) {
            return None;
        }
        let along = self.along(at);
        let (start, size) = if self.vertical {
            (self.thumb.y, self.thumb.h)
        } else {
            (self.thumb.x, self.thumb.w)
        };
        let offset = if along < start || along > start + size {
            (offset + (along - (start + size * 0.5)) * self.per_px()).clamp(0.0, self.span)
        } else {
            offset
        };
        let grip = ThumbGrip {
            vertical: self.vertical,
            from: along,
            offset,
        };
        Some((grip, offset))
    }

    /// The offset the view takes with the thumb held by `grip` dragged to
    /// `at`.
    pub fn drag(&self, grip: ThumbGrip, at: (f64, f64)) -> f32 {
        (grip.offset + (self.along(at) - grip.from) * self.per_px()).clamp(0.0, self.span)
    }
}

impl ThumbGrip {
    /// Whether it holds the vertical bar's thumb.
    pub fn vertical(&self) -> bool {
        self.vertical
    }
}

/// `c` at `alpha` of its own strength.
fn with_alpha(c: Color, alpha: f32) -> Color {
    [c[0], c[1], c[2], c[3] * alpha]
}

/// How a toggle is drawn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ToggleView {
    /// A box that is filled or empty, with a word beside it.
    #[default]
    Check,
    /// A track with a knob that sits at one end or the other.
    Switch,
    /// A button that stays pressed.
    Button,
}

impl ToggleView {
    pub(crate) fn from_str(s: &str) -> Option<ToggleView> {
        Some(match s {
            "check" => ToggleView::Check,
            "switch" => ToggleView::Switch,
            "button" => ToggleView::Button,
            _ => return None,
        })
    }
}

/// **The box a toggle is drawn as**, inside its cell: the design system's
/// checkbox (`check_w` square) or switch (`switch_w` by `switch_h`),
/// centred vertically at the left of the rect.
pub fn toggle_box(rect: Rect, view: ToggleView, m: &Metrics) -> Rect {
    let (w, h) = match view {
        ToggleView::Switch => (m.switch_w, m.switch_h),
        _ => (m.check_w, m.check_w),
    };
    let h = h.min(rect.h);
    let w = w.min(rect.w);
    Rect::new(rect.x, rect.y + (rect.h - h) * 0.5, w, h)
}

/// **What a toggle answers the pointer on**: the box, plus the label beside it
/// when there is one -- the two things drawn, and nothing of the cell the
/// layout stretched around them.
///
/// A checkbox is a small square with a word next to it, and a cell it does not
/// fill on **either** axis: a row of controls is as tall as the tallest of
/// them, so a toggle beside a slider gets a column of air over and under its
/// box as well as the run of it after the label. Both are the layout's, not the
/// control's, so a click landing on them goes back to the chain -- the first
/// pass here bounded only the width, which is exactly half a fix and reads as
/// none at all in the panel that showed it.
///
/// The label counts because it is drawn as part of the affordance (clicking the
/// word toggles, as everywhere else), and it counts for **what fits**: a label
/// ellipsized to the cell is hit over the part that is on screen. Its line is
/// centred on the box, so the band is the taller of the two.
pub fn toggle_hit(
    rect: Rect,
    view: ToggleView,
    label: Option<&str>,
    size: f32,
    m: &Metrics,
) -> Rect {
    // Drawn as a button it *is* its cell, like a button.
    if view == ToggleView::Button {
        return rect;
    }
    let b = toggle_box(rect, view, m);
    let Some(text) = label else { return b };
    let tx = b.x + b.w + m.check_gap;
    let w = font::width(text, size).min((rect.x + rect.w - tx).max(0.0));
    let h = b.h.max(font::height(size)).min(rect.h);
    Rect::new(
        rect.x,
        rect.y + (rect.h - h) * 0.5,
        (tx + w - rect.x).max(b.w),
        h,
    )
}

/// **A toggle**, as the design system draws its three: a checkbox (a
/// rounded square on the field's edge, filled `primary` with a check in its
/// text when on), a switch (a round track, `primary` when on and the field's
/// edge when off, its thumb on the ground crossing to the far end), or a
/// toggle button (a ghost button whose on state is the `muted` fill). The
/// words stand after the box in the interface's size.
#[allow(clippy::too_many_arguments)] // one control: its state, its place, its look
pub fn toggle(
    d: &mut Draw,
    on: bool,
    label: Option<&str>,
    icon: Option<char>,
    rect: Rect,
    view: ToggleView,
    hovered: bool,
    size: f32,
) {
    if view == ToggleView::Button {
        let (mesh, m, theme) = d.parts();
        let radius = m.radius_md;
        let lit = on || hovered;
        if lit {
            mesh.round_rect(rect, radius, theme.muted);
        }
        let ink = if hovered && !on {
            theme.text_dim
        } else {
            theme.text
        };
        button_face(mesh, label, icon, rect, size, m, ink);
        return;
    }
    let b = toggle_box(rect, view, d.m);
    let (mesh, m, theme) = d.parts();
    let dark = dark(theme);
    match view {
        ToggleView::Switch => {
            let radius = b.h * 0.5;
            let track = if on {
                theme.primary
            } else if dark {
                with_alpha(theme.input, 0.8)
            } else {
                theme.input
            };
            mesh.round_rect(b, radius, track);
            let knob = (b.h - 2.0 * m.divider_w).max(1.0);
            let x = if on {
                b.x + b.w - m.divider_w - knob
            } else {
                b.x + m.divider_w
            };
            let thumb = match (dark, on) {
                (true, true) => theme.primary_text,
                (true, false) => theme.text,
                _ => theme.background,
            };
            mesh.round_rect(
                Rect::new(x, b.y + m.divider_w, knob, knob),
                knob * 0.5,
                thumb,
            );
        }
        _ => {
            let radius = m.check_radius;
            if on {
                mesh.round_rect(b, radius, theme.primary);
                let check = self::icon("check", font::symbol::CHECK);
                let scale = font::em_scale(Family::Ui, b.h * 0.875);
                font::text_centered(mesh, &check.to_string(), b, scale, theme.primary_text);
            } else {
                if dark {
                    mesh.round_rect(b, radius, with_alpha(theme.input, 0.3));
                }
                mesh.round_border(b, m.divider_w, radius, theme.input);
            }
        }
    }
    let text = match (icon, label) {
        (None, None) => return,
        (icon, label) => button_text(label.or(Some("")), icon),
    };
    let tx = b.x + b.w + m.check_gap;
    let ty = rect.y + (rect.h - font::height(size)) * 0.5;
    let avail = (rect.x + rect.w - tx).max(0.0);
    font::text_ellipsis(mesh, text.trim_end(), tx, ty, avail, size, theme.text);
}

/// Draws a `choice` as the design system's select: the chosen option in a
/// field, and a chevron at its right pointing down, since a press opens the
/// option list over the window. The chevron's room is taken out of the
/// text's width, so a long option is cut before it reaches it.
pub fn choice(d: &mut Draw, current: &str, label: Option<&str>, rect: Rect, size: f32) {
    let m = d.m;
    let body = field_box(rect, label.is_some(), size, m);
    let chevron_w = m.icon + m.field_x;
    let look = FieldLook {
        end: chevron_w,
        ..FieldLook::default()
    };
    field_with(d, current, label, rect, size, false, None, look);
    let (mesh, m, theme) = d.parts();
    let chevron = icon("chevron-down", font::symbol::POINT_DOWN);
    let cell = Rect::new(body.x + body.w - chevron_w, body.y, m.icon, body.h);
    let scale = font::em_scale(Family::Ui, m.type_sm);
    font::text_centered(
        mesh,
        &chevron.to_string(),
        cell,
        scale,
        with_alpha(theme.text_dim, 0.5),
    );
}

/// **The box a field is drawn as**: its whole cell under the label strip,
/// as wide as the cell -- the design system's input, flush with the caption
/// over it.
pub fn field_box(rect: Rect, has_label: bool, size: f32, m: &Metrics) -> Rect {
    let top = rect.y + label_height(rect.h, has_label, size, m);
    Rect::new(rect.x, top, rect.w, (rect.y + rect.h - top).max(0.0))
}

/// Where a field's text starts across its box, and how wide it may run.
fn field_text(body: Rect, m: &Metrics, end: f32) -> (f32, f32) {
    let x = body.x + m.field_x;
    (x, (body.w - 2.0 * m.field_x - end).max(0.0))
}

/// The top inset of a text area's lines.
fn area_top(m: &Metrics) -> f32 {
    2.0 * m.field_y
}

/// The editable text field, as the design system's input: a box with the
/// medium corner on the field's edge (the ring's while it has the keys, the
/// destructive's while it is `invalid`), lifted a little, its text inset.
/// `caret` is `Some` only while the field is focused (it then draws the
/// selection and the caret). The **layout does not depend on focus**: a
/// multiline field always lays its lines top-aligned like a text editor (not a
/// centered label), and a single-line field always sits on one
/// vertically-centered row -- an unfocused field uses a caret at the start
/// (scroll offset 0), so the pre-written text reads exactly as it will once
/// clicked into. A single-line field clips overflow with an ellipsis when
/// unfocused, and scrolls to the caret when focused. (A `choice`'s read-out
/// reuses this as an unfocused single-line field.)
pub fn field(
    d: &mut Draw,
    value: &str,
    label: Option<&str>,
    rect: Rect,
    size: f32,
    multiline: bool,
    caret: Option<textedit::Caret>,
) {
    field_with(
        d,
        value,
        label,
        rect,
        size,
        multiline,
        caret,
        FieldLook::default(),
    );
}

/// **What a field's owner adds to its look**: room kept free at its right
/// (a select's chevron), its edge in the destructive role (`invalid`), and
/// the words it shows quieter while it holds none (`placeholder`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FieldLook<'a> {
    pub end: f32,
    pub invalid: bool,
    pub placeholder: Option<&'a str>,
}

/// [`field`], with what its owner adds to it ([`FieldLook`]).
#[allow(clippy::too_many_arguments)] // the field, and what its owner adds to it
pub fn field_with(
    d: &mut Draw,
    value: &str,
    label: Option<&str>,
    rect: Rect,
    size: f32,
    multiline: bool,
    caret: Option<textedit::Caret>,
    look: FieldLook,
) {
    let (end, invalid) = (look.end, look.invalid);
    label_strip_at(d, label, rect, size, 0.0);
    let m = d.m;
    let body = field_box(rect, label.is_some(), size, m);
    let radius = m.radius_md;
    d.shadow(body, radius, Elevation::Xs);
    let dark = dark(d.theme);
    let ground = if dark {
        with_alpha(d.theme.input, 0.3)
    } else {
        d.theme.background
    };
    d.mesh.round_rect(body, radius, ground);
    let edge = if invalid {
        d.theme.destructive
    } else if caret.is_some() {
        d.theme.focus
    } else {
        d.theme.input
    };
    d.mesh.round_border(body, m.divider_w, radius, edge);

    let (text_x, text_w) = field_text(body, m, end);
    // The scroll window is counted in clusters -- a caret moves by clusters,
    // so what has to stay visible is one -- while every *position* inside the
    // line is measured as drawn, which is what a proportional face and a line
    // that changes direction need (see `visible_line`).
    let cols = font::fit_chars(text_w, size);
    // Unfocused: lay out around a caret at the start (no scroll, no caret drawn).
    let lay = caret.unwrap_or_default();

    if !multiline {
        // One row, vertically centered. Unfocused text that overflows clips with
        // an ellipsis (the label/choice look); focused, it scrolls to the caret.
        let ty = (body.y + (body.h - font::height(size)) * 0.5).max(body.y);
        let first = value.split('\n').next().unwrap_or("");
        if let (true, Some(words)) = (value.is_empty(), look.placeholder) {
            font::text_ellipsis(d.mesh, words, text_x, ty, text_w, size, d.theme.text_dim);
        }
        if caret.is_none() {
            font::text_ellipsis(d.mesh, first, text_x, ty, text_w, size, d.theme.text);
            return;
        }
        let hstart = textedit::h_scroll(textedit::line_col(value, lay.pos).1, cols);
        draw_line(d, value, 0, text_x, ty, hstart, cols, text_w, size, caret);
        return;
    }

    // Multiline: a top-aligned block of rows, scrolled (when focused) to the
    // caret's line/column so it stays visible; from the top-left otherwise.
    let hstart = textedit::h_scroll(textedit::line_col(value, lay.pos).1, cols);
    let row_h = font::line_advance(size);
    let rows = (((body.h - 2.0 * area_top(m)) / row_h) as usize).max(1);
    let row_start = textedit::h_scroll(textedit::line_col(value, lay.pos).0, rows);
    let mut byte = 0usize; // running byte offset of each line's start
    for (i, line) in value.split('\n').enumerate() {
        if i >= row_start && i < row_start + rows {
            let ty = body.y + area_top(m) + (i - row_start) as f32 * row_h;
            draw_line(
                d, value, byte, text_x, ty, hstart, cols, text_w, size, caret,
            );
        }
        byte += line.len() + 1; // + the '\n'
    }
}

/// **The part of `line` a field shows**, as the byte range of `line` it
/// holds: from cluster `hstart`, at most `cols` clusters and never wider than
/// `text_w` pixels as drawn.
///
/// The two limits are one under the fixed-pitch bitmap (`cols` *is* `text_w`
/// divided by the cell) and two under a proportional face, where a window of N
/// clusters may be wider or narrower than the box. The width is the one that
/// binds: a field's text is not clipped by a scissor, so a line that overflowed
/// would bleed over the chrome beside it. A cluster -- a letter with its marks,
/// a syllable -- is shown whole or not at all.
fn visible_line(
    line: &str,
    hstart: usize,
    cols: usize,
    text_w: f32,
    size: f32,
) -> std::ops::Range<usize> {
    use unicode_segmentation::UnicodeSegmentation;
    let mut clusters = line.grapheme_indices(true).skip(hstart);
    let Some((start, first)) = clusters.next() else {
        return line.len()..line.len();
    };
    let mut end = start + first.len();
    for (at, cluster) in clusters.take(cols.saturating_sub(1)) {
        let next = at + cluster.len();
        if font::width(&line[start..next], size) > text_w {
            break;
        }
        end = next;
    }
    start..end
}

/// Draws one line of a field: its visible glyphs (scrolled by `hstart`
/// clusters, clipped to the body), and -- only when `caret` is `Some`
/// (focused) -- the selection highlight over the spans its selected part
/// covers on the line, and the caret, drawn where its logical position stands
/// when it falls on this line. `line_byte` is the byte offset of the line's
/// start in `value`.
#[allow(clippy::too_many_arguments)] // the line and its window, past the context
fn draw_line(
    d: &mut Draw,
    value: &str,
    line_byte: usize,
    x: f32,
    y: f32,
    hstart: usize,
    cols: usize,
    text_w: f32,
    size: f32,
    caret: Option<textedit::Caret>,
) {
    let (mesh, _m, theme) = d.parts();
    let end_byte = value[line_byte..]
        .find('\n')
        .map_or(value.len(), |i| line_byte + i);
    let line = &value[line_byte..end_byte];
    let shown = visible_line(line, hstart, cols, text_w, size);
    // the visible part, in bytes of `value`
    let (from, to) = (line_byte + shown.start, line_byte + shown.end);
    let visible = &value[from..to];

    // Selection highlight (drawn under the text): the part of the selection
    // this line shows, painted over every span it covers as drawn -- one, or
    // several where it crosses a change of direction.
    if let Some((s, e)) = caret.and_then(|c| c.selection()) {
        let (a, b) = (s.clamp(from, to), e.clamp(from, to));
        if b > a {
            for (x0, x1) in font::spans(visible, a - from, b - from, size) {
                mesh.rect(
                    Rect::new(x + x0, y, x1 - x0, font::height(size)),
                    theme.selection,
                );
            }
        }
    }

    // The visible glyphs.
    font::text(mesh, visible, x, y, size, theme.text);

    // The caret, when focused and sitting on this line within the visible
    // window, at the visual place of its logical position.
    if let Some(caret) = caret
        && crate::host::blink::shown()
        && (from..=to).contains(&caret.pos)
        && textedit::line_col(value, caret.pos).0
            == value[..line_byte].bytes().filter(|&b| b == b'\n').count()
    {
        let cx = x + font::caret_x(visible, caret.pos - from, size);
        mesh.rect(
            Rect::new(cx, y, size.max(1.0), font::height(size)),
            theme.text,
        );
    }
}

/// The caret byte offset a click at `(x, y)` lands on in a text `field`,
/// reconstructing the same layout [`field`] draws (label strip, body inset,
/// horizontal/vertical scroll from `current`) so a click lands on the glyph it
/// points at. `has_label` and `size` are the widget's; `current` is the caret
/// before the click (its scroll offset is what the field is showing).
#[allow(clippy::too_many_arguments)] // a hit-test needs the field's full layout
pub fn caret_at(
    rect: Rect,
    value: &str,
    has_label: bool,
    size: f32,
    multiline: bool,
    current: textedit::Caret,
    x: f64,
    y: f64,
    m: &Metrics,
) -> usize {
    let body = field_box(rect, has_label, size, m);
    let (text_x, text_w) = field_text(body, m, 0.0);
    let cols = font::fit_chars(text_w, size);
    let caret_col = textedit::line_col(value, current.pos).1;
    let hstart = textedit::h_scroll(caret_col, cols);
    // The position a click lands on is **measured against the glyphs actually
    // shown**, the same part of the line `draw_line` drew, as drawn -- so a
    // click lands on the letter it points at, in a right-to-left run too.
    let at_in = |line_byte: usize| {
        let end = value[line_byte..]
            .find('\n')
            .map_or(value.len(), |i| line_byte + i);
        let line = &value[line_byte..end];
        let shown = visible_line(line, hstart, cols, text_w, size);
        let visible = &line[shown.clone()];
        line_byte + shown.start + font::byte_at(visible, x as f32 - text_x, size)
    };

    if !multiline {
        return at_in(0);
    }

    let caret_line = textedit::line_col(value, current.pos).0;
    let row_h = font::line_advance(size);
    let rows = (((body.h - 2.0 * area_top(m)) / row_h) as usize).max(1);
    let row_start = textedit::h_scroll(caret_line, rows);
    let n_lines = value.split('\n').count();
    let rel = ((y as f32 - (body.y + area_top(m))) / row_h).max(0.0) as usize;
    let line = (row_start + rel).min(n_lines.saturating_sub(1));
    at_in(textedit::offset_of(value, line, 0))
}

/// A value read-out at the bottom-right of a body (clipped with an ellipsis
/// when the body is narrower than the number).
fn value_text(d: &mut Draw, s: &str, body: Rect, size: f32) {
    let (mesh, m, theme) = d.parts();
    let avail = (body.w - m.pad).max(0.0);
    let w = font::width(s, size).min(avail);
    let x = (body.x + body.w - w - m.pad).max(body.x);
    let y = (body.y + body.h - font::height(size)).max(body.y);
    font::text_ellipsis(mesh, s, x, y, avail, size, theme.text);
}

/// The cell width a control's value **read-out** needs: the widest number this
/// range can ever show, plus the insets [`value_text`] draws it inside (the
/// body's on both sides and its own on the right).
///
/// It measures the **bounds**, never the current value -- `min` and `max` are
/// props and the value is not, so a control fitted to its content keeps one
/// width while it is turned instead of resizing under the hand turning it.
pub(crate) fn readout_w(r: &Range, size: f32, m: &Metrics) -> f32 {
    let widest = font::width(&fmt(r.min), size).max(font::width(&fmt(r.max), size));
    widest + 3.0 * m.pad
}

/// Formats a control value compactly (drops trailing zeros within 2 decimals).
pub(crate) fn fmt(v: f32) -> String {
    if v.fract() == 0.0 && v.abs() < 1e6 {
        format!("{v:.0}")
    } else {
        format!("{v:.2}")
    }
}

// --- Natural sizes ----------------------------------------------------------
//
// How tall a control wants to be is the same fact as how it is drawn: the
// read-out strip a slider reserves under its groove is one number, read here by
// the drawing and by the size. They lived apart, which is how the size pass came
// to import this module; they are one section now, and an element's `natural`
// calls straight into it.

/// What a labelled control gives up first when the strip holding it is short:
/// its **label strip**, and nothing else. Subtracted from a natural height it
/// is that control's floor
/// ([`Element::floor`](crate::host::widget::element::Element::floor)).
///
/// It is the same term [`label_height`] stops reserving once the cell cannot
/// hold both, read from one place so the floor a widget offers and the drawing
/// that follows cannot name different numbers -- offer a strip the drawing
/// keeps and the body is cut instead; keep one the drawing drops and the
/// control floats in dead space. Unlabelled, there is nothing to give and this
/// is zero.
pub fn label_give(r: &Range, m: &Metrics, scale: f32) -> f32 {
    size::label_strip(r.label.is_some(), r.text_size * scale, m)
}

/// A labelled field's height: its label strip, its body inset and one control
/// line (the read-out row).
pub fn field_h(r: &Range, m: &Metrics, scale: f32) -> f32 {
    let size = r.text_size * scale;
    size::label_strip(r.label.is_some(), size, m) + size::body_inset(m) + size::control_box(size, m)
}

/// A horizontal slider's thickness: the label strip, the body inset, the
/// handle's grip across the track and the read-out strip under it when it has
/// one -- the same reservation the drawing makes ([`slider_track`]), so the
/// groove gets the grip it asked for and the number gets its own row.
pub fn slider_thick(r: &Range, readout: bool, m: &Metrics, scale: f32) -> f32 {
    let size = r.text_size * scale;
    let strip = if readout { readout_h(size, m) } else { 0.0 };
    size::label_strip(r.label.is_some(), size, m)
        + size::body_inset(m)
        + m.handle_grip.max(m.handle_thick)
        + strip
}

/// **How many control lines of travel a slider's track keeps at least**:
/// short enough for a dense panel, long enough that a hand still sets a value
/// along it rather than only jumping between its ends.
pub const SLIDER_TRAVEL_LINES: f32 = 3.0;

/// **The least travel a slider's track keeps**, beside its grip
/// ([`SLIDER_TRAVEL_LINES`] control lines).
pub fn slider_travel(m: &Metrics) -> f32 {
    SLIDER_TRAVEL_LINES * m.control_h
}

/// A vertical slider's width: the grip across the track, inset in the body.
/// The value read-out shares that width and ellipsizes -- a number's own length
/// is data, and no size here may follow it.
pub fn slider_across(m: &Metrics) -> f32 {
    size::body_inset(m) + m.handle_grip.max(m.box_side)
}

/// A knob's height: the label strip, the body inset, the disc and the read-out
/// strip the drawing reserves under it.
pub fn knob_h(r: &Range, m: &Metrics, scale: f32) -> f32 {
    let size = r.text_size * scale;
    size::label_strip(r.label.is_some(), size, m)
        + size::body_inset(m)
        + knob_disc_d(r, m)
        + readout_h(size, m)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clausters_gui_draw::theme::Theme;

    /// **A knob at its floor gives up its label, not its disc**: placed at the
    /// height its floor promises, it draws no caption and its disc at full
    /// size; at its whole height it draws both.
    #[test]
    fn a_knob_at_its_floor_keeps_its_disc_whole() {
        let m = Metrics::default();
        let r = Range::parse(
            &serde_json::json!({"label": "cutoff"})
                .as_object()
                .unwrap()
                .clone(),
        );
        let size = r.text_size;
        let whole = knob_h(&r, &m, 1.0);
        let floor = whole - label_give(&r, &m, 1.0);
        assert!(knob_labelled(&r, whole, size, &m));
        assert!(!knob_labelled(&r, floor, size, &m));
        let disc = |h: f32| {
            let rect = Rect::new(0.0, 0.0, 200.0, h);
            knob_disc(knob_body(&r, rect, size, &m), size, &m).2
        };
        assert_eq!(disc(floor), disc(whole), "the same disc either way");
        assert!(disc(floor) * 2.0 >= m.knob_d - 4.0, "and a whole one");
        // **text twice the default's makes a disc twice the default's**
        let big = Range::parse(
            serde_json::json!({"label": "cutoff", "text_size": 2.0 * clausters_gui_draw::font::DEFAULT_SIZE})
                .as_object()
                .unwrap(),
        );
        assert_eq!(knob_disc_d(&big, &m), 2.0 * m.knob_d);
        assert_eq!(knob_disc_d(&r, &m), m.knob_d);
    }

    /// A cell too short for the inset keeps its text inside its box; one with
    /// room keeps the inset, and one shorter than the text is all box.
    #[test]
    fn a_short_cell_gives_up_the_inset_before_the_box_is_under_its_text() {
        let m = Metrics::default();
        let text = font::height(m.text_scale);
        let cell = |h: f32| Rect::new(0.0, 10.0, 100.0, h);

        let roomy = body_rect_at(cell(m.control_h + 2.0 * m.pad), false, m.text_scale, &m);
        assert_eq!((roomy.y, roomy.h), (10.0 + m.pad, m.control_h));

        let short = body_rect_at(cell(text + m.pad), false, m.text_scale, &m);
        assert_eq!(short.h, text, "as tall as the text, centred in the cell");
        assert_eq!(short.y, 10.0 + m.pad * 0.5);
        assert_eq!((short.x, short.w), (m.pad, 100.0 - 2.0 * m.pad));

        let shorter = body_rect_at(cell(text - 2.0), false, m.text_scale, &m);
        assert_eq!((shorter.y, shorter.h), (10.0, text - 2.0));
    }

    #[test]
    fn slider_fraction_spans_the_body() {
        let body = Rect::new(10.0, 0.0, 100.0, 20.0);
        assert_eq!(slider_fraction(body, 10.0), 0.0);
        assert_eq!(slider_fraction(body, 60.0), 0.5);
        assert_eq!(slider_fraction(body, 110.0), 1.0);
        assert_eq!(slider_fraction(body, 200.0), 1.0, "clamped past the end");
    }

    #[test]
    fn vertical_slider_fraction_runs_bottom_to_top() {
        let body = Rect::new(0.0, 10.0, 20.0, 100.0);
        assert_eq!(slider_fraction_v(body, 110.0), 0.0, "bottom edge is min");
        assert_eq!(slider_fraction_v(body, 60.0), 0.5, "centre is half");
        assert_eq!(slider_fraction_v(body, 10.0), 1.0, "top edge is max");
        assert_eq!(slider_fraction_v(body, -50.0), 1.0, "clamped above the top");
    }

    #[test]
    fn drag_up_increases_value() {
        // Dragging up (dy negative) yields a positive fraction delta.
        assert!(drag_fraction_delta(-50.0, 100.0) > 0.0);
        assert!(drag_fraction_delta(50.0, 100.0) < 0.0);
    }

    #[test]
    fn body_sits_below_the_label_strip() {
        let rect = Rect::new(0.0, 0.0, 100.0, 80.0);
        let m = Metrics::default();
        let with = body_rect(rect, true, &m);
        let without = body_rect(rect, false, &m);
        assert!(with.y > without.y, "a label pushes the body down");
    }

    #[test]
    fn text_size_scales_the_label_strip() {
        let rect = Rect::new(0.0, 0.0, 100.0, 80.0);
        let m = Metrics::default();
        let small = body_rect_at(rect, true, 1.0, &m);
        let big = body_rect_at(rect, true, 4.0, &m);
        assert!(big.y > small.y, "a bigger label strip pushes the body down");
        assert_eq!(
            body_rect(rect, true, &m),
            body_rect_at(rect, true, m.text_scale, &m)
        );
    }

    #[test]
    fn label_alignment_places_the_line() {
        let rect = Rect::new(0.0, 0.0, 200.0, 40.0);
        let mut xs = Vec::new();
        for align in [Align::Start, Align::Center, Align::End] {
            let mut mesh = Mesh::new();
            draw_label(
                &mut Draw::new(&mut mesh, &Metrics::default(), &Theme::default()),
                "HI",
                rect,
                2.0,
                false,
                align,
            );
            xs.push(mesh.positions().map(|(x, _)| x).fold(f32::MAX, f32::min));
        }
        assert!(xs[0] < xs[1] && xs[1] < xs[2], "start < center < end");
    }

    #[test]
    fn wrapped_label_stays_inside_its_rect() {
        let rect = Rect::new(10.0, 10.0, 90.0, 60.0);
        let mut m = Mesh::new();
        draw_label(
            &mut Draw::new(&mut m, &Metrics::default(), &Theme::default()),
            "a rather long label that must wrap over several lines",
            rect,
            2.0,
            true,
            Align::Start,
        );
        assert!(!m.is_empty());
        let max_x = m.positions().map(|(x, _)| x).fold(f32::MIN, f32::max);
        let max_y = m.positions().map(|(_, y)| y).fold(f32::MIN, f32::max);
        assert!(max_x <= rect.x + rect.w, "wrap bleeds right");
        assert!(max_y <= rect.y + rect.h, "overflowing lines must drop");
    }

    #[test]
    fn unwrapped_label_clips_with_an_ellipsis() {
        let rect = Rect::new(0.0, 0.0, 60.0, 30.0);
        let mut m = Mesh::new();
        draw_label(
            &mut Draw::new(&mut m, &Metrics::default(), &Theme::default()),
            "far too long to fit here",
            rect,
            2.0,
            false,
            Align::Start,
        );
        let max_x = m.positions().map(|(x, _)| x).fold(f32::MIN, f32::max);
        assert!(max_x <= rect.x + rect.w, "single line bleeds past the rect");
    }

    /// **An icon alone stands where it stands unpressable**: a button's
    /// lone icon wider than the room inside its box is centred whole, as
    /// the same glyph drawn as text is, so a way back that becomes possible
    /// does not move.
    #[test]
    fn a_lone_icon_is_centred_whole_like_its_text() {
        let theme = Theme::default();
        let m = Metrics::default();
        let r = Rect::new(10.0, 0.0, 14.0, 24.0);
        let left = |mesh: &Mesh| mesh.positions().map(|(x, _)| x).fold(f32::MAX, f32::min);
        let mut pressed = Mesh::new();
        button(
            &mut Draw::new(&mut pressed, &m, &theme),
            None,
            Some('W'),
            r,
            ButtonLook {
                flat: true,
                ..ButtonLook::default()
            },
            2.0,
        );
        let mut text = Mesh::new();
        font::text_centered(&mut text, "W", r, 2.0, theme.text);
        assert!((left(&pressed) - left(&text)).abs() < 0.01);
    }

    /// **A latch is the design system's toggle**: off, no fill; on, the
    /// `muted` fill a ghost button lights with.
    #[test]
    fn a_latch_is_drawn_as_a_toggle_on_or_off() {
        let theme = Theme::default();
        let fill = |on: bool| {
            let mut mesh = Mesh::new();
            toggle(
                &mut Draw::new(&mut mesh, &Metrics::default(), &theme),
                on,
                Some("x"),
                None,
                Rect::new(0.0, 0.0, 40.0, 30.0),
                ToggleView::Button,
                false,
                2.0,
            );
            mesh.colors_by_y().next().map(|(_, color)| color)
        };
        assert_ne!(fill(false), Some(theme.muted));
        assert_eq!(fill(true), Some(theme.muted));
    }

    #[test]
    fn knob_geometry_stays_within_its_cell() {
        // A wide, short labelled cell like gui_panel's knob row: the disc is
        // bounded by the short side, leaving little vertical room. The value
        // read-out used to be placed past the body and spilled below the cell
        // into the next row; it must now stay within the cell.
        let cell = Rect::new(12.0, 34.0, 175.0, 103.0);
        let r = Range {
            value: 800.0,
            min: 20.0,
            max: 20000.0,
            curve: 0.0,
            exponential: false,
            step: 0.0,
            label: Some("cutoff".into()),
            text_size: Metrics::default().text_scale,
        };
        let mut mesh = Mesh::new();
        knob(
            &mut Draw::new(&mut mesh, &Metrics::default(), &Theme::default()),
            &r,
            cell,
            r.text_size,
        );
        let bottom = cell.y + cell.h;
        let max_y = mesh.positions().map(|(_, y)| y).fold(f32::MIN, f32::max);
        assert!(
            max_y <= bottom,
            "knob geometry spills {:.1}px below the cell (max_y {max_y} > bottom {bottom})",
            max_y - bottom
        );
    }

    /// **A scroll bar is held where it is taken**: a press on the thumb
    /// keeps the view, a press on the track off it brings the thumb's middle
    /// there, and a drag moves the view as far as the thumb moves, between
    /// the top and the end.
    #[test]
    fn a_scroll_bar_is_dragged_by_its_thumb() {
        let track = Rect::new(90.0, 0.0, 10.0, 100.0);
        // a quarter seen: the thumb is 25 long, 75 of travel over 300
        let bar = ScrollBar::new(true, track, 100.0, 300.0, 0.0, 4.0).expect("a bar");
        assert_eq!((bar.thumb.y, bar.thumb.h), (0.0, 25.0));
        assert!(ScrollBar::new(true, track, 100.0, 0.0, 0.0, 4.0).is_none());
        let (grip, offset) = bar.press((95.0, 10.0), 0.0).expect("on the thumb");
        assert_eq!(offset, 0.0, "a press on the thumb keeps the view");
        assert!((bar.drag(grip, (95.0, 35.0)) - 100.0).abs() < 1e-3);
        assert_eq!(
            bar.drag(grip, (95.0, 500.0)),
            300.0,
            "no further than the end"
        );
        let (_, offset) = bar.press((95.0, 50.0), 0.0).expect("on the track");
        assert!(
            (offset - 150.0).abs() < 1e-3,
            "the thumb's middle at the press"
        );
        assert!(bar.press((50.0, 50.0), 0.0).is_none(), "off the track");
    }
}
