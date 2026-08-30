//! The `score` widget's renderer: a verovio display list -> triangle mesh.
//!
//! Music notation is vector art — SMuFL glyph outlines (noteheads, clefs,
//! rests, accidentals, flags) plus engraving strokes and fills (staff lines,
//! stems, ledger lines, beams, slurs, ties). None of it is data-viz, so it does
//! not get its own GPU pipeline: every primitive is tessellated into the same
//! flat-colored [`Mesh`](crate::host::paint::Mesh) the rest of the chrome uses, which
//! keeps it one upload, one draw, and WebGL2-safe by construction.
//!
//! The host is the *renderer*; it never depends on verovio. A client (the
//! Python `clausters.gui` submodule, driving verovio) engraves the score and
//! sends a **semantic display list**: a table of glyph outlines keyed by SMuFL
//! codepoint, plus placed primitives in verovio page units. The host fits that
//! page into the widget rect and tessellates. The web client reuses this same
//! renderer by sending the same display list — no engraving logic is
//! duplicated per language.
//!
//! Curves (glyph outlines, slurs, ties) are filled with lyon's
//! `FillTessellator`; strokes (staff/stems/ledger) are the painter's own
//! thick-line quads. Everything is baked into screen coordinates *before*
//! tessellation so the curve-flattening tolerance is expressed in pixels.

//! **Module layout.** The element's growth is *semantic* rather than graphic —
//! the timemap, the identity of the element under the cursor, transposition in
//! diatonic steps, the edit-back payloads — so it is a submodule split by what
//! each part knows: [`list`] decodes the client's page off the wire, [`glyphs`]
//! turns an outline string into a path, [`tess`] paints, and [`cursor`] holds
//! the two indexes and the mappings a gesture measures against. This file is
//! the page itself: the geometry types, the primitive, and the [`ScoreData`]
//! every one of them is a method of.

mod cursor;
mod glyphs;
mod list;
mod tess;

#[cfg(test)]
mod tests;

use std::collections::HashMap;

use crate::host::paint::Color;

/// An affine map restricted to translate + non-uniform scale — the only
/// transforms verovio emits (`translate(...)` and `scale(...)`, the glyph's
/// inner `scale(1,-1)` folded into a negative `sy`). Composing two of these
/// stays in the family, so a full matrix is unnecessary.
#[derive(Clone, Copy, Debug)]
pub struct Affine {
    pub tx: f32,
    pub ty: f32,
    pub sx: f32,
    pub sy: f32,
}

impl Affine {
    pub const IDENTITY: Affine = Affine {
        tx: 0.0,
        ty: 0.0,
        sx: 1.0,
        sy: 1.0,
    };

    /// `self` applied after `inner` (i.e. `self ∘ inner`).
    pub fn then(self, inner: Affine) -> Affine {
        Affine {
            tx: self.tx + self.sx * inner.tx,
            ty: self.ty + self.sy * inner.ty,
            sx: self.sx * inner.sx,
            sy: self.sy * inner.sy,
        }
    }

    #[inline]
    pub fn apply(self, x: f32, y: f32) -> [f32; 2] {
        [self.tx + self.sx * x, self.ty + self.sy * y]
    }

    /// The inverse map, or `None` when a scale collapsed to zero (a degenerate
    /// transform has no point to map back to).
    pub fn invert(self) -> Option<Affine> {
        if self.sx == 0.0 || self.sy == 0.0 {
            return None;
        }
        Some(Affine {
            tx: -self.tx / self.sx,
            ty: -self.ty / self.sy,
            sx: 1.0 / self.sx,
            sy: 1.0 / self.sy,
        })
    }
}

/// An axis-aligned page-unit box, with `x0 <= x1` and `y0 <= y1`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Bounds {
    /// The box `xf` maps this one onto — still axis-aligned, since `xf` only
    /// translates and scales; a negative scale flips it, so the corners are
    /// re-ordered.
    fn transformed(self, xf: Affine) -> Bounds {
        let [ax, ay] = xf.apply(self.x0, self.y0);
        let [bx, by] = xf.apply(self.x1, self.y1);
        Bounds {
            x0: ax.min(bx),
            y0: ay.min(by),
            x1: ax.max(bx),
            y1: ay.max(by),
        }
    }

    fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x0 && x <= self.x1 && y >= self.y0 && y <= self.y1
    }

    /// Whether `(x, y)` is on the primitive this box was measured around —
    /// which is the box itself for everything engraved straight, and the
    /// **ellipse inscribed in it** for a notehead.
    fn holds(&self, shape: HitShape, x: f32, y: f32) -> bool {
        match shape {
            HitShape::Rect => self.contains(x, y),
            HitShape::Ellipse => crate::host::graphics::shape::in_ellipse(
                x as f64,
                y as f64,
                crate::host::layout::Rect::new(
                    self.x0,
                    self.y0,
                    self.x1 - self.x0,
                    self.y1 - self.y0,
                ),
            ),
        }
    }

    fn area(&self) -> f32 {
        (self.x1 - self.x0) * (self.y1 - self.y0)
    }

    fn grown(self, m: f32) -> Bounds {
        Bounds {
            x0: self.x0 - m,
            y0: self.y0 - m,
            x1: self.x1 + m,
            y1: self.y1 + m,
        }
    }
}

/// **What an entry's extent means**: the box, or the ellipse inside it.
///
/// A hit index is measured as boxes because that is what a path's extent is,
/// but the box is not always the shape: a notehead is an oval lying in a
/// rectangle whose corners are paper, and on a dense page those corners belong
/// to the beam, the stem or the note on the next line. The distinction is
/// carried per entry rather than decided at the test, because only the indexer
/// knows what it measured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HitShape {
    /// The box itself — a line, a stem, a beam, a text run, any glyph whose
    /// outline fills what was measured around it.
    Rect,
    /// The ellipse inscribed in the box: a notehead.
    Ellipse,
}

/// The SMuFL **Noteheads** range (U+E0A0–U+E0FF): the glyphs whose shape is an
/// oval and whose box therefore over-answers for them. Whether a codepoint is a
/// notehead is a fact about the font's layout, not about this page, so it is
/// read straight off the range rather than configured.
fn is_notehead(cp: u32) -> bool {
    (0xE0A0..=0xE0FF).contains(&cp)
}

/// One entry of the hit-testing index: the page-unit extent of an identified
/// primitive and the shape it stands for, paired with the MEI `xml:id` it was
/// engraved from.
#[derive(Clone, Debug)]
pub struct HitBox {
    pub id: String,
    pub bounds: Bounds,
    pub shape: HitShape,
}

/// One placed element of the engraved page, in verovio page units.
#[derive(Clone, Debug)]
pub enum Prim {
    /// A SMuFL glyph: its outline (looked up by `cp` in [`ScoreData::glyphs`])
    /// mapped from font units to page units by `xf` (with the y-flip folded in).
    Glyph {
        cp: u32,
        xf: Affine,
        id: Option<String>,
    },
    /// A stroked polyline: staff lines, stems, ledger lines, bar lines.
    Line {
        pts: Vec<[f32; 2]>,
        width: f32,
        id: Option<String>,
    },
    /// A filled region: beams (polygons), slurs and ties (filled cubic
    /// outlines), augmentation dots (ellipses). `d` is the outline in the
    /// element's **local** coordinates; `xf` maps it to page units — mapped in
    /// the host (not baked into `d` on the client) so comma/space coordinate
    /// separators never confuse a numeric rewrite.
    Fill {
        d: String,
        xf: Affine,
        id: Option<String>,
    },
    /// Verbatim text (not SMuFL): volta numbers, tempo, lyrics, titles. `x, y`
    /// is the baseline and `size` the em height, both in page units; the host
    /// draws it in its own font.
    Text {
        s: String,
        x: f32,
        y: f32,
        size: f32,
        /// How the string sits against `x`: `Middle` centres it, `End` puts its
        /// right edge there. A title is centred on the page and a composer
        /// flush to its right margin, and both say so this way rather than with
        /// a pre-measured x -- the width is the renderer's, and only the
        /// renderer knows it.
        anchor: Anchor,
        id: Option<String>,
    },
}

/// Where a text primitive's `x` falls in the string it places.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Anchor {
    /// `x` is the left edge -- the SVG default, and what a measure number or a
    /// volta uses.
    #[default]
    Start,
    /// `x` is the middle.
    Middle,
    /// `x` is the right edge.
    End,
}

impl Anchor {
    /// The left edge of a `width`-wide string placed against `x`.
    pub fn left(self, x: f32, width: f32) -> f32 {
        match self {
            Anchor::Start => x,
            Anchor::Middle => x - 0.5 * width,
            Anchor::End => x - width,
        }
    }
}

impl Prim {
    /// The MEI xml:id this primitive was engraved from, if any (the hook for
    /// hit-testing and edit-back once the score view becomes interactive).
    pub fn id(&self) -> Option<&str> {
        match self {
            Prim::Glyph { id, .. }
            | Prim::Line { id, .. }
            | Prim::Fill { id, .. }
            | Prim::Text { id, .. } => id.as_deref(),
        }
    }
}

/// One position of the playback cursor: at musical time `t` (ms) the sounding
/// event sits at page-x `x`, spanning its system's staff from `y0` to `y1`. The
/// track is the bridge from the timemap (onset ms per MEI id, from the client)
/// to geometry (the id's placed x) — precomputed on the client, sorted by `t`.
#[derive(Clone, Copy, Debug)]
pub struct Cursor {
    pub t: f32,
    pub x: f32,
    pub y0: f32,
    pub y1: f32,
}

/// The pitch drag in flight: the element being dragged and how many diatonic
/// steps **up** the gesture has moved it so far (negative = down). The page is
/// drawn with that element displaced, so the drag reads as notation while it
/// happens.
///
/// The displacement is the *drawing's* quantity and stays one; what the release
/// sends is the **absolute** position it lands on ([`ScoreData::staff_position`]
/// plus these steps), because an edit that travels has to be idempotent. The
/// client owns the score and answers with a re-engraved page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScoreDrag {
    pub id: String,
    pub steps: i32,
}

/// One engraved staff: the page-y of its top and bottom lines and the width
/// they are stroked with. Derived from the drawing (the wide horizontal lines,
/// clustered by system), because a pitch dragged off the staff needs ledger
/// lines and only the staff says where they go.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Staff {
    pub y0: f32,
    pub y1: f32,
    pub width: f32,
}

/// Where a press on blank paper landed: the staff it belongs to (top down from
/// zero), how far up that staff in whole diatonic steps, and the element the
/// note would follow.
///
/// It carries no pitch and no duration on purpose. A staff position becomes a
/// pitch only once something knows the clef and the key, and a duration is a
/// choice nobody made by clicking — both are the client's, which is the same
/// line every other score gesture draws.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub staff: usize,
    pub position: i32,
    pub after: Option<String>,
}

/// The default page units per diatonic step: verovio's default `unit` (9) times
/// its definition factor (10). Used when a display list names no `step` — every
/// display list `clausters.gui.notation` builds does.
pub const STEP: f32 = 90.0;

/// A fully engraved page ready to render: the definition viewBox (for fitting),
/// the glyph-outline table (deduplicated by codepoint), and the placed
/// primitives.
#[derive(Clone, Debug)]
pub struct ScoreData {
    /// The verovio `definition-scale` viewBox, `(width, height)` in page units.
    pub vb_w: f32,
    pub vb_h: f32,
    /// SMuFL codepoint -> outline path `d` (font units, y-up before the flip).
    pub glyphs: HashMap<u32, String>,
    pub prims: Vec<Prim>,
    /// The playback-cursor track (sorted by `t`), empty when the client sent no
    /// timemap.
    pub cursors: Vec<Cursor>,
    /// A **static** playback time in ms, set with `/gui_set playhead`; negative
    /// means none. It stands still — a stopped transport located on a note keeps
    /// its cursor there, and it must not drift with the engine clock.
    pub playhead: f32,
    /// The playhead origin: the engine sample-clock value at score time 0
    /// (negative = not playing, fall back to the static `playhead`). Set once at
    /// the start of a pass and the cursor then *sweeps* on its own — the host
    /// reads the clock every frame, so playback needs zero messages.
    pub playhead_at: f64,
    /// The sweep's **loop region** in musical ms — the score's own unit, as
    /// `playhead` is: with `playhead_loop_len > 0` the swept cursor wraps
    /// inside `[playhead_loop_start, + len)` instead of running off the page,
    /// so a repeated passage is followed on the same one anchor and still
    /// costs no message per frame. A non-positive length is the straight pass.
    pub playhead_loop_start: f32,
    pub playhead_loop_len: f32,
    /// The sample rate converting the clock to musical ms (0 = unknown, use the
    /// server's own rate).
    pub sample_rate: f64,
    /// The hit-testing index: the page-unit extent of every identified
    /// primitive, derived from `prims` and `glyphs` when the display list is
    /// parsed (see [`ScoreData::index`]).
    pub hits: Vec<HitBox>,
    /// The engraved staves, top to bottom — derived with the hit index, and
    /// what tells a dragged pitch when it has left the staff.
    pub staves: Vec<Staff>,
    /// The selected element's MEI `xml:id`, drawn highlighted; `None` = nothing
    /// selected. Set by a click on the page and by `/gui_set selected`.
    pub selected: Option<String>,
    /// Page units per **diatonic step** — half the staff-line spacing, the
    /// quantum a pitch drag counts in. It comes from the client with the page
    /// (it depends on verovio's `unit` option, not on the staff scale), so the
    /// host quantizes exactly what the engraver drew.
    pub step: f32,
    /// The pitch drag in flight, drawn as a displacement of its element. It
    /// stands after the release until the client sends the re-engraved page:
    /// the answer is one message away, and snapping back first would show the
    /// old pitch for a frame.
    pub drag: Option<ScoreDrag>,
    /// Whether a drag on an element **edits** it (a pitch drag → `"transpose"`).
    /// Off by default: a score is a view, and the host holds no score, so an
    /// edit the client will not apply is a gesture that cannot be fulfilled — an
    /// editor opts in (`editable: true`). Selection and the `"element"` click are
    /// not gated by this: inspecting a read-only page is not editing it.
    pub editable: bool,
    /// Whether a press on **blank paper** inside a staff reports where it
    /// landed (`"insert"`), for a page that takes note entry.
    ///
    /// Its own flag rather than a second meaning for `editable`, because it
    /// takes over a gesture that already does something useful: on any other
    /// page, pressing blank paper clears the selection, and a page that had not
    /// asked for note entry would start reporting an insertion every time a
    /// user dismissed one.
    pub entry: bool,
    /// The ids that name a **sounding element** — a note, a rest — as against
    /// the staff and layer furniture that also carries one. Sent by the client,
    /// because the walk that engraved the page is what knows, and to a renderer
    /// an id is an id.
    pub elements: std::collections::HashSet<String>,
    /// The page's systems, each the `[y_top, y_bottom]` its staves span.
    ///
    /// The client reads them and the host does not re-derive them, for the
    /// reason the client's own walk gives: a **gap cannot tell a grand staff
    /// from two systems**, and what settles it — a barline drawn through the
    /// brace — is a notation fact rather than a measurement. Without it a press
    /// on the third system's upper staff named staff 4 of a two-staff score,
    /// which no model has.
    pub systems: Vec<[f32; 2]>,
}

impl Default for ScoreData {
    fn default() -> ScoreData {
        ScoreData {
            vb_w: 0.0,
            vb_h: 0.0,
            glyphs: HashMap::new(),
            prims: Vec::new(),
            cursors: Vec::new(),
            playhead: -1.0,
            playhead_at: -1.0,
            playhead_loop_start: 0.0,
            playhead_loop_len: 0.0,
            sample_rate: 0.0,
            hits: Vec::new(),
            staves: Vec::new(),
            selected: None,
            step: STEP,
            drag: None,
            editable: false,
            entry: false,
            elements: std::collections::HashSet::new(),
            systems: Vec::new(),
        }
    }
}

/// The three roles a score paints in: the engraving ink, the playback cursor
/// and the selection highlight — bundled so the theme travels as one argument.
#[derive(Clone, Copy, Debug)]
pub struct ScoreColors {
    pub ink: Color,
    pub playhead: Color,
    pub selection: Color,
}
