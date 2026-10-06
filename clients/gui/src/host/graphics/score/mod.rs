//! The `score` widget's renderer: a verovio display list -> triangle mesh.
//!
//! Music notation is vector art -- SMuFL glyph outlines (noteheads, clefs,
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
//! renderer by sending the same display list -- no engraving logic is
//! duplicated per language.
//!
//! Curves (glyph outlines, slurs, ties) are filled with lyon's
//! `FillTessellator`; strokes (staff/stems/ledger) are the painter's own
//! thick-line quads. Everything is baked into screen coordinates *before*
//! tessellation so the curve-flattening tolerance is expressed in pixels.

//! **Module layout.** The element's growth is *semantic* rather than graphic --
//! the timemap, the identity of the element under the cursor, transposition in
//! diatonic steps, the edit-back payloads -- so it is a submodule split by what
//! each part knows: [`list`] decodes the client's page off the wire, [`glyphs`]
//! turns an outline string into a path, [`tess`] paints, and [`cursor`] holds
//! the two indexes and the mappings a gesture measures against. This file is
//! the page itself: the geometry types, the primitive, and the [`ScoreData`]
//! every one of them is a method of.

mod cursor;
mod glyphs;
mod hit;
mod list;
mod tess;

#[cfg(test)]
mod tests;

use std::collections::HashMap;

use crate::host::paint::Color;

/// An affine map restricted to translate + non-uniform scale -- the only
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

    /// `self` applied after `inner` (i.e. `self` after `inner`).
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
    /// The middle of the box, `(x, y)`.
    fn middle(self) -> (f32, f32) {
        (0.5 * (self.x0 + self.x1), 0.5 * (self.y0 + self.y1))
    }

    /// The box `xf` maps this one onto -- still axis-aligned, since `xf` only
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

pub use hit::{HitBox, HitGrid, HitShape};
pub use list::{edit_cursor, selection};
pub use tess::{edges, triangles};

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
    /// element's **local** coordinates; `xf` maps it to page units -- mapped in
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

/// The height of a line of capitals to the em, for the text of a page: what
/// turns the font size an engraver names into the body box the host's own
/// scale is set by. A text face's is between 0.66 (a Times) and 0.73 (the
/// sans a host usually loads); the page is laid out for the first.
pub(crate) const CAP_PER_EM: f32 = 0.7;

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

/// What a kept fill was tessellated from: a glyph of the page's table, by its
/// codepoint, or a path of the page's own, by the primitive that holds it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum FillOf {
    Glyph(u32),
    Prim(usize),
}

/// **The fills a page has tessellated, kept**: triangle corners in the path's
/// own coordinates, by what was filled and how finely.
///
/// A page is drawn again on every frame its cursor moves, and it is the same
/// page: a few dozen glyphs, placed hundreds of times. Reading each outline's
/// path and tessellating it for every placement, every frame, was most of
/// what drawing a page cost. A fill is made once per **level** -- the
/// tolerance asked for, rounded down to a power of two -- so a zoom that
/// stays within an octave reuses it and one that leaves it flattens finer,
/// never coarser than was asked. The model is replaced when the page is, and
/// the fills go with it.
#[derive(Clone, Debug, Default)]
pub(crate) struct Fills(std::cell::RefCell<HashMap<(FillOf, i16), Fill>>);

/// One kept fill: triangle corners, three to a triangle.
type Fill = std::rc::Rc<[[f32; 2]]>;

impl Fills {
    /// More levels than a page is ever zoomed through: past it everything is
    /// dropped, so a long sitting cannot grow the table without bound.
    const MOST: usize = 4096;

    /// The fill of the path `d`, which is what `of` names, at a tolerance no
    /// coarser than `tol` (in the path's units).
    pub(crate) fn of(&self, of: FillOf, d: &str, tol: f32) -> Fill {
        let level = tol.max(f32::MIN_POSITIVE).log2().floor().clamp(-64.0, 64.0) as i16;
        let mut kept = self.0.borrow_mut();
        if let Some(fill) = kept.get(&(of, level)) {
            return fill.clone();
        }
        if kept.len() >= Self::MOST {
            kept.clear();
        }
        let fill: std::rc::Rc<[[f32; 2]]> =
            tess::triangles(d, 2.0f32.powi(i32::from(level))).into();
        kept.insert((of, level), fill.clone());
        fill
    }

    /// How many fills are kept.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.0.borrow().len()
    }
}

/// **A text of the page being typed over where it is drawn**: the element, the
/// string as it stands, and the caret in it. The page draws this in the place
/// of the text it engraved under that id, at the same anchor, so a centred
/// title stays centred while it grows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextEditing {
    pub id: String,
    pub value: String,
    pub caret: crate::host::graphics::textedit::Caret,
}

/// One position of the playback cursor: at musical time `t` (ms) the sounding
/// event sits at page-x `x`, spanning its system's staff from `y0` to `y1`. The
/// track is the bridge from the timemap (onset ms per MEI id, from the client)
/// to geometry (the id's placed x) -- precomputed on the client, sorted by `t`.
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

/// Where a press on a staff landed during note entry: the staff it belongs to
/// (its rank in its system, from zero), how far up that staff in whole
/// diatonic steps, and the element whose column it fell in -- the nearest
/// sounding element of that staff, across.
///
/// It carries no pitch and no duration on purpose. A staff position becomes a
/// pitch only once something knows the clef and the key, and a duration is a
/// choice nobody made by clicking -- both are the client's, which is the same
/// line every other score gesture draws.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub staff: usize,
    pub position: i32,
    pub at: Option<String>,
}

/// **Where note entry writes next**, as its owner names it: the column of the
/// element `at`, on the `staff`-th staff of the system that element is in --
/// or, with `end`, just past it, where a voice that has run out goes on. The
/// host draws it and knows nothing else about it.
#[derive(Clone, Debug, PartialEq)]
pub struct EditCursor {
    pub at: String,
    pub staff: usize,
    pub end: bool,
}

/// The default page units per diatonic step: verovio's default `unit` (9) times
/// its definition factor (10). Used when a display list names no `step` -- every
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
    /// The fills this page has tessellated, kept ([`Fills`]).
    pub(crate) fills: Fills,
    /// The playback-cursor track (sorted by `t`), empty when the client sent no
    /// timemap.
    pub cursors: Vec<Cursor>,
    /// A **static** playback time in ms, set with `/gui_set playhead`; negative
    /// means none. It stands still -- a stopped transport located on a note keeps
    /// its cursor there, and it must not drift with the engine clock.
    pub playhead: f32,
    /// The playhead origin: the engine sample-clock value at score time 0
    /// (negative = not playing, fall back to the static `playhead`). Set once at
    /// the start of a pass and the cursor then *sweeps* on its own -- the host
    /// reads the clock every frame, so playback needs zero messages.
    pub playhead_at: f64,
    /// The sweep's **loop region** in musical ms -- the score's own unit, as
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
    /// The spatial index in front of `hits`, built with them.
    pub grid: HitGrid,
    /// **Each id's entries of `hits`**, in their order -- an element's first
    /// is its notehead -- so what is selected, dragged or under the edit
    /// cursor is found without walking the page.
    pub by_id: HashMap<String, Vec<u32>>,
    /// **Each staff's entries of `hits`, across**: for every staff of
    /// `staves`, the entries whose middle is nearer it than any other, as
    /// `(x of the middle, entry)` in order of `x`. What a press in note entry
    /// searches for the element it fell beside.
    pub rows: Vec<Vec<(f32, u32)>>,
    /// **The ids of the primitives that draw the staff lines**, filled by the
    /// same pass that clusters them into staves ([`ScoreData::staves`]).
    ///
    /// The engraver labels a staff line with the staff's own `xml:id`, and a
    /// line is a hairline the width of the system -- the tightest box on the
    /// page. So on a page taking note entry a press aimed at a line rather than
    /// a space was answered with the staff and spent on a selection. This is
    /// what tells the *staff's own drawing* from everything else a press can
    /// land on, which is the whole of what that fix is allowed to reach: a
    /// slur, a hairpin, a dynamic and a beam are elements of the score and are
    /// selected by pointing at them, whether or not they sound.
    ///
    /// Derived here rather than declared, because it is the same geometric rule
    /// the staves themselves come from -- a line long relative to the page's
    /// other horizontal strokes -- and a second source for one fact is a second
    /// answer to it.
    pub staff_ids: std::collections::HashSet<String>,
    /// The engraved staves, top to bottom -- derived with the hit index, and
    /// what tells a dragged pitch when it has left the staff.
    pub staves: Vec<Staff>,
    /// The selected elements' MEI `xml:id`s, drawn highlighted; empty =
    /// nothing selected. Set by a press on the page (Ctrl adds or removes one,
    /// Shift extends to one) and by `/gui_set selected`, which takes one id or
    /// a list of them.
    pub selected: Vec<String>,
    /// Page units per **diatonic step** -- half the staff-line spacing, the
    /// quantum a pitch drag counts in. It comes from the client with the page
    /// (it depends on verovio's `unit` option, not on the staff scale), so the
    /// host quantizes exactly what the engraver drew.
    pub step: f32,
    /// The pitch drag in flight, drawn as a displacement of its element. It
    /// stands after the release until the client sends the re-engraved page:
    /// the answer is one message away, and snapping back first would show the
    /// old pitch for a frame.
    pub drag: Option<ScoreDrag>,
    /// Whether a drag on an element **edits** it (a pitch drag -> `"transpose"`).
    /// Off by default: a score is a view, and the host holds no score, so an
    /// edit the client will not apply is a gesture that cannot be fulfilled -- an
    /// editor opts in (`editable: true`). Selection and the `"element"` click are
    /// not gated by this: inspecting a read-only page is not editing it.
    pub editable: bool,
    /// Whether the page is in **note entry**: a press on a staff reports
    /// where it landed (`"enter"`) instead of selecting or dragging.
    ///
    /// Its own flag rather than a second meaning for `editable`, because it
    /// takes over the page's every press: outside the mode a press selects
    /// and a drag moves, and inside it a press is an entry and never a drag.
    pub entry: bool,
    /// The text of the page being typed over, when one is: host state, like
    /// the selection, that no display list carries.
    pub editing: Option<TextEditing>,
    /// **The edit cursor** of note entry, when its owner put one on the page
    /// (`edit_cursor`); `None` outside the mode.
    pub edit_cursor: Option<EditCursor>,
    /// The ids that name a **sounding element** -- a note, a rest -- as against
    /// the staff and layer furniture that also carries one. Sent by the client,
    /// because the walk that engraved the page is what knows, and to a renderer
    /// an id is an id.
    pub elements: std::collections::HashSet<String>,
    /// The page's systems, each the `[y_top, y_bottom]` its staves span.
    ///
    /// The client reads them and the host does not re-derive them, for the
    /// reason the client's own walk gives: a **gap cannot tell a grand staff
    /// from two systems**, and what settles it -- a barline drawn through the
    /// brace -- is a notation fact rather than a measurement. Without it a press
    /// on the third system's upper staff named staff 4 of a two-staff score,
    /// which no model has.
    pub systems: Vec<[f32; 2]>,
    /// **What each id is**: the engraver's class for the element that carries
    /// it (`note`, `rest`, `slur`, `clef`, ...), sent by the client because the
    /// walk that engraved the page is what knows. What a kind admits is
    /// `clausters_core::notation::admits`, read through [`ScoreData::admits`].
    pub kinds: HashMap<String, String>,
}

impl ScoreData {
    /// What a hand may do to the element `id`: the core's table, read for the
    /// kind the page names. An id the page names no kind for admits nothing
    /// beyond being selected -- a gesture this host cannot be sure of is one it
    /// does not offer.
    /// The string the page draws under `id`, when that element is a text --
    /// a title, a name, a footnote.
    pub fn text_of(&self, id: &str) -> Option<&str> {
        self.prims.iter().find_map(|p| match p {
            Prim::Text {
                s, id: Some(own), ..
            } if own == id => Some(s.as_str()),
            _ => None,
        })
    }

    pub fn admits(&self, id: &str) -> clausters_core::notation::Admits {
        self.kinds
            .get(id)
            .map(|kind| clausters_core::notation::admits(kind))
            .unwrap_or_default()
    }
}

impl Default for ScoreData {
    fn default() -> ScoreData {
        ScoreData {
            vb_w: 0.0,
            vb_h: 0.0,
            glyphs: HashMap::new(),
            prims: Vec::new(),
            fills: Fills::default(),
            cursors: Vec::new(),
            playhead: -1.0,
            playhead_at: -1.0,
            playhead_loop_start: 0.0,
            playhead_loop_len: 0.0,
            sample_rate: 0.0,
            hits: Vec::new(),
            grid: HitGrid::default(),
            by_id: HashMap::new(),
            rows: Vec::new(),
            staff_ids: std::collections::HashSet::new(),
            staves: Vec::new(),
            selected: Vec::new(),
            step: STEP,
            drag: None,
            editable: false,
            entry: false,
            editing: None,
            edit_cursor: None,
            elements: std::collections::HashSet::new(),
            systems: Vec::new(),
            kinds: HashMap::new(),
        }
    }
}

/// The three roles a score paints in: the engraving ink, the playback cursor
/// and the selection highlight -- bundled so the theme travels as one argument.
#[derive(Clone, Copy, Debug)]
pub struct ScoreColors {
    pub ink: Color,
    pub playhead: Color,
    pub selection: Color,
}
