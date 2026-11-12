//! Rendering one window's widget tree into its wgpu surface -- the shared frame
//! path, agnostic of platform and of how the host is driven.
//!
//! This is the code the milestone calls "isolate the surface/GPU/loop port":
//! both fronts feed the **same** [`render`] one tree plus its per-window GPU
//! resources, so the browser is pixel-faithful to the desktop by construction,
//! not by a parallel renderer. The native windowed front ([`super::gui`]) calls
//! it with live inputs (the shared-memory bus source, scope histories, the node
//! tree, the held-button highlight); the browser entry point (`super::web`)
//! calls it with the streamed equivalents. It builds the flat-geometry [`Mesh`]
//! from the placed widgets ([`super::layout`] + [`clausters_gui_draw::paint`]/
//! [`clausters_gui_draw::font`]), uploads the heavy `waveform`/`spectrogram`/`canvas` views,
//! and draws the whole frame in one pass -- the editor chrome (rulers,
//! selection, playhead, cursor readout) as a second, *overlay* mesh drawn
//! after the heavy views so it reads on top of them, and a third over a
//! dialog's own heavy views and under nothing ([`Batches`]).
//!
//! **Module layout.** This file is the frame's spine: the [`FrameInputs`] a
//! front fills, the batches a window keeps, and [`render`] itself -- lay out,
//! collect, draw, upload, one pass. What a window keeps of a view's data is
//! [`slots`], and what a view has not got yet for the zoom it is shown at is
//! [`owed`](mod@owed). The two long halves `render` calls are children too,
//! one per direction of the frame. [`items`] is the *read* half:
//! the per-widget snapshots and the single tree walk that fills them, kept
//! together because a new item type is a struct plus an arm of that walk.
//! [`draw`] is the *write* half: the three mesh passes over those snapshots,
//! which by then hold no borrow of the host tree.

pub(crate) mod draw;
mod items;
mod owed;
mod slots;
mod underlay;

use crate::host::diag;
use clausters_core::spectra;
use draw::*;
pub(crate) use draw::{draw_time_ruler, marker_at, ruler_strip, ruler_strip_body};
// The arrow's width is the drawing's own business; a gesture asks `marker_at`
// what was hit. The test that pins the reach to it is the one exception.
use items::*;
pub(crate) use owed::*;
pub(crate) use slots::*;
use underlay::Underlay;

use std::collections::HashMap;
use std::sync::Arc;

use crate::gpu::Gpu;
use crate::spectrogram::{FreqScale, SpectrogramView, Stft};
use crate::view::{Framing, Renderers, TimelineView};
use crate::viewport::View;
use crate::waveform::{WaveformData, WaveformView};

use super::bands::Bands;
use super::chrome;
use super::layout;
use crate::canvas::{self, CanvasView};
use clausters_gui_draw::metrics::Metrics;
use clausters_gui_draw::rect::Rect;

use super::ruler::{self, TimeUnit};
use super::status;
use super::timeline::{GroupState, group_key};
use super::widget::element::{Ctx, Loaded, SlotFill, SlotFrame};
use super::widget::{EditorProps, Ruler, RulerY, Widget, WidgetKind};
use super::world::World;
use crate::host::graphics::signal::layers::Paint;
use crate::painter::Painter;
use clausters_gui_draw::draw::Draw;
use clausters_gui_draw::font;
use clausters_gui_draw::paint::{Ink, Mesh};
use clausters_gui_draw::theme::{Theme, with_alpha};

/// The window clear color: the theme's `background` role as a `wgpu::Color`.
pub(crate) fn clear_color(theme: &Theme) -> wgpu::Color {
    wgpu::Color {
        r: theme.background[0] as f64,
        g: theme.background[1] as f64,
        b: theme.background[2] as f64,
        a: theme.background[3] as f64,
    }
}

/// The body a timeline view draws into: its rect minus the time-ruler strip
/// under it (when the x ruler is on) and the gutter band to its left -- each
/// ruler gets its own space instead of overlaying the view.
/// `indent` is the **group's** gutter, not this view's own `ruler_w`: a
/// waveform sharing an axis with a lane or a roll starts its trace where they
/// start their body, and draws its value ruler into the whole band.
pub(crate) fn timeline_body(
    rect: Rect,
    editor: &EditorProps,
    has_label: bool,
    indent: f32,
    metrics: &Metrics,
) -> Rect {
    let (mut x, mut y, mut w, mut h) = (rect.x, rect.y, rect.w, rect.h);
    // The label strip comes off the top, the same one every other view
    // reserves ([`crate::host::widget::size::label_strip`]) -- a heavy view is
    // not a different kind of widget just because its picture is a texture.
    //
    // **Plus the gap under the caption**, which is the one part of the strip a
    // heavy view has to reserve for itself: everywhere else it arrives with
    // [`crate::host::graphics::controls::body_rect`]'s inset, and a timeline
    // body has none on purpose -- the picture runs edge to edge, so a take's
    // samples fill the box it is given. Without it the caption sits on the
    // samples. The formula is `body_rect`'s vertical half, spelled out.
    let strip = crate::host::widget::size::label_strip(has_label, metrics.text_scale, metrics);
    let strip = if has_label {
        strip + metrics.pad
    } else {
        strip
    };
    let strip = strip.min(h);
    y += strip;
    h -= strip;
    if editor.strip() {
        h = (h - metrics.ruler_h).max(0.0);
    }
    let indent = indent.min(w);
    x += indent;
    w = (w - indent).max(0.0);
    Rect::new(x, y, w, h)
}

/// What a drag is holding while this frame is drawn -- the frame's answer to
/// *whose* affordances may light up.
///
/// A grip is a promise about the next press, so during a drag it belongs to the
/// clip already held and to nothing else: another clip lighting up would offer
/// a grab that is not on the table, and the held clip's own grip must not blink
/// out every time the pointer wanders between two snap steps (the clip moves in
/// steps; the pointer does not).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) enum Grab {
    /// Nothing is held: affordances follow the pointer.
    #[default]
    None,
    /// Something else is held (a control, a curve, the axis): no clip lights up.
    Other,
    /// A **marquee** is being swept over the element `id`, and this is the
    /// rectangle: the frame draws it, through the routine every swept selection
    /// in the window is drawn with. It is here rather than in the element
    /// because the drag is the machine's -- one marquee, wherever a hand sweeps
    /// one.
    Marquee(i32, Rect),
}

/// The live inputs the frame needs beyond the tree and the GPU resources. The
/// native front fills them from its state; the browser front passes the
/// streamed equivalents.
///
/// Two kinds of thing, deliberately separated. [`world`](Self::world) is what
/// nobody owns -- the outside, identical for every element of the frame. The
/// fields beside it are **one widget's own interaction state**, fed back down
/// so that widget can draw itself mid-gesture; each of them is a widget that
/// cannot yet hold its own state, and each disappears as its leaf moves behind
/// [`Element`](super::widget::Element).
pub(crate) struct FrameInputs<'a> {
    /// The host's size roles: every layout and paint site of this frame reads
    /// its spacing, control and text sizes from here (see
    /// [`clausters_gui_draw::metrics`]).
    pub(crate) metrics: &'a Metrics,
    /// The read-only per-frame facts no widget owns (see [`World`]).
    pub(crate) world: World<'a>,
    /// The id of the widget holding the keyboard focus in this window, if any:
    /// the frame rings it, and the element draws whatever else being focused
    /// means to it (a field's caret and selection).
    pub(crate) focused: Option<i32>,
    /// What a drag is holding right now (see [`Grab`]).
    pub(crate) grab: Grab,
    /// **This window's status bar**: what it has said and whether it is open
    /// (see [`super::status`]). `None` for a window that has said nothing yet
    /// -- the band is still carved, because a window that carries a bar carries
    /// it before it has anything to put in it.
    pub(crate) status: Option<&'a status::Status>,
    /// **What is open over this window**: the lists and the tip of the popup
    /// layer (see [`super::popup`]). The host's, like the status bar, and
    /// drawn last so it covers whatever it opened over.
    pub(crate) popups: Option<&'a super::popup::Popups>,
    /// **The host's clipboard**, which a tool's paste is offered against
    /// ([`super::actions`]). `None` draws every tool as its owner's reading
    /// alone would.
    pub(crate) clipboard: Option<&'a super::clipboard::Clip>,
    /// Whether the view this window's actions address holds a selection
    /// ([`super::Host::window_selected`]), for the tools that need one.
    pub(crate) selected: Option<bool>,
}

impl Default for FrameInputs<'_> {
    fn default() -> Self {
        // A 'static empty table for the no-transport case (the world brings its
        // own empties).
        static METRICS: std::sync::OnceLock<Metrics> = std::sync::OnceLock::new();
        Self {
            metrics: METRICS.get_or_init(Metrics::default),
            world: World::default(),
            focused: None,
            grab: Grab::None,
            status: None,
            popups: None,
            clipboard: None,
            selected: None,
        }
    }
}

/// The shared state a placed timeline widget draws with -- the window, the
/// selection and the playhead of its navigation group, which is where all
/// three live. A widget in no group yet (nothing registered its data) falls
/// back to its own def-time props over `fallback`, the window it would have
/// seeded: the same values the group is about to take.
fn chrome_for(
    inputs: &FrameInputs,
    id: i32,
    editor: &EditorProps,
    fallback: impl FnOnce() -> View,
) -> GroupState {
    match inputs.world.timelines.state(group_key(id, editor.link)) {
        Some(state) => state,
        None => GroupState::seed(editor, fallback()),
    }
}

/// The **placed** navigation window a member's own data is drawn through: the
/// group window shifted so the member's data sample 0 lands at timeline
/// position `offset`. The GPU body upload uses this (its data is in local
/// sample units); the time ruler and the selection/playhead overlay keep the
/// timeline-unit window. At `offset = 0` (the un-placed default) it is the
/// identity.
fn placed_nav(nav: &View, offset: f64) -> View {
    View {
        start: nav.start - offset,
        len: nav.len,
    }
}

/// Maps sample position `s` into `body`'s x range through `nav`.
fn sample_to_x(s: f64, nav: &View, body: Rect) -> f32 {
    (body.x as f64 + (s - nav.start) / nav.len * body.w as f64) as f32
}

/// **The ink a placed widget draws with**: the opacity its subtree resolved to
/// ([`Widget::alpha`]) and its declared corner radius in the pixels of the
/// space it was placed in -- the wire's number through the placement's own
/// table, exactly like every other declared length, so a widget inside a zoomed
/// workspace rounds by as much as it grew.
///
/// One function because both meshes and every draw pass ask the same question,
/// and because it is the only place the two props meet the frame at all: an
/// element is never told it is being faded.
///
/// [`Widget::alpha`]: super::widget::Widget::alpha
pub(crate) fn ink_of(p: &layout::Placed) -> Ink {
    Ink {
        alpha: p.widget.alpha,
        radius: p.widget.radius.map_or(0.0, |r| p.metrics.px(r)),
    }
}

/// The row channel `ch` of `channels` occupies inside `body` (stacked top to
/// bottom, no gap -- the divider line is overlay chrome).
///
/// The **third** row a view stacks, beside a roll's semitone and a
/// multitrack's lane, and the same structure: a band of the vertical axis. So
/// it is a [`Bands`] like the other two, on its uniform arm -- a channel stack
/// divides its body evenly because every channel is worth the same picture.
///
/// Same structure, **different word**: `lane` is the arrangement's, a track's
/// contents, and it is not this. The two were one name until the document
/// claimed the first.
pub(crate) fn channel_rect(body: Rect, channels: usize, ch: usize) -> Rect {
    let channels = channels.max(1);
    let (y, h) = Bands::uniform(channels, body.h / channels as f32).band(ch);
    Rect::new(body.x, body.y + y, body.w, h)
}

/// **What a scrolled stack of rows draws within**: the widget's clip, narrowed
/// to the body's height across the widget's width, so a row scrolled out of
/// the body is cut where the body ends while its y-ruler labels beside the
/// body still draw.
pub(crate) fn rows_clip(clip: Option<Rect>, rect: Rect, body: Rect) -> Rect {
    let band = Rect::new(rect.x, body.y, rect.w, body.h);
    clip.map_or(band, |c| c.intersect(band))
}

/// The time-ruler unit of `editor` (the beats grid rides its props).
pub(crate) fn time_unit(editor: &EditorProps) -> TimeUnit<'_> {
    match editor.ruler {
        Ruler::Samples => TimeUnit::Samples,
        Ruler::Beats => TimeUnit::Beats {
            tempo: editor.tempo,
            beat_at: editor.beat_at,
            quant: editor.quant,
            map: editor.tempo_map.as_deref(),
            bars: &editor.bars,
        },
        _ => TimeUnit::Seconds,
    }
}

/// The stacked-channel index under window y `cy` (clamped into range).
pub(crate) fn channel_at(body: Rect, channels: usize, cy: f64) -> usize {
    let rel = ((cy - body.y as f64) / body.h.max(1.0) as f64).clamp(0.0, 1.0);
    ((rel * channels as f64) as usize).min(channels.saturating_sub(1))
}

/// **A window's flat batches**, in the order the pass draws them, with the
/// heavy views' texture passes between them, and what the window keeps of its
/// last whole frame ([`Kept`]).
///
/// - `base`: the window's widgets. The window's own textures go over it.
/// - `over`: what the window's elements draw over their own textures (the
///   selection, the readout) and, with a dialog up, the scrim and the dialog's
///   flat widgets. The dialog's textures go over it.
/// - `live`: what moves -- the lines a clock sweeps, the bars a level fills,
///   a reading set many times a second ([`Element::draw_live`]). The one batch
///   a tick builds again.
/// - `top`: what the dialog's elements draw over their textures, then the
///   host's bands and lists, which cover everything.
///
/// Two batches were enough while nothing with a texture could stand over the
/// window: a spectrogram or a shader `canvas` inside a dialog was drawn with
/// the window's, and so under the dialog that held it.
///
/// [`Element::draw_live`]: super::widget::Element::draw_live
pub(crate) struct Batches {
    pub(crate) base: Painter,
    pub(crate) over: Painter,
    pub(crate) live: Painter,
    pub(crate) top: Painter,
    kept: Kept,
    /// What lies under the live layers, kept on the card ([`Underlay`]).
    underlay: Underlay,
    /// Every frame drawn in one pass, for a test to compare the kept
    /// picture's frames with.
    #[cfg(test)]
    one_pass: bool,
    asked: Asked,
    /// What the last frame cost, phase by phase ([`FrameCost`]).
    cost: FrameCost,
}

/// **What one frame cost, by phase** -- read by the front after each frame
/// and counted against the window (`/gui_activity`), so a profile says where
/// a slow window's frames go rather than only how long they took.
///
/// Timed with the same clock on both fronts (`web_time`), around work that is
/// the same code on both: the one thing that can differ is the platform under
/// `present`, which is why it is a phase of its own.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FrameCost {
    /// Placing the tree: the layout pass of a whole frame.
    pub layout: std::time::Duration,
    /// Drawing the picture into meshes, past the layout -- or, for a frame of
    /// what moves, the live layers alone.
    pub build: std::time::Duration,
    /// Writing the meshes and the heavy views' textures and uniforms to the
    /// card.
    pub upload: std::time::Duration,
    /// Recording the render pass and submitting it.
    pub encode: std::time::Duration,
    /// Taking a drawable from the surface and presenting it.
    pub present: std::time::Duration,
    /// Bytes of vertices written to the card.
    pub uploaded: u64,
    /// Widgets laid out (a whole frame's placements; `0` for one of what
    /// moves).
    pub placed: u64,
}

/// What the next frame of a window was asked for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Wanted {
    /// Nobody of this host asked -- the platform did, or a caller that named
    /// no reason -- so the frame is a whole one.
    #[default]
    Unsaid,
    /// Only what moves: a tick found a counter or a fed value moved, or a set
    /// wrote a live prop.
    Live,
    /// The picture: something it is drawn from changed.
    Whole,
}

/// **What the next frame of a window was asked for, and by whom**: the one
/// place the question is kept, so that a front draws the least a frame may
/// be and never less.
///
/// A redraw has two kinds of cause. Something the picture is drawn from
/// changed -- a set, a gesture, a reply, a resize -- or only what moves did.
/// A front says which as it asks ([`want_whole`](Self::want_whole),
/// [`want_live`](Self::want_live)), and the frame reads the answer once.
/// **Whole wins, and so does silence**: a frame somebody asked the whole of
/// is whole whatever a tick asked beside it, and one nobody of this host
/// asked for -- the platform's, or a caller that named no reason -- is whole
/// too, since a picture kept past a change nobody declared is a wrong one.
#[derive(Default)]
struct Asked(std::cell::Cell<Wanted>);

impl Asked {
    fn want_whole(&self) {
        self.0.set(Wanted::Whole);
    }

    fn want_live(&self) {
        if self.0.get() == Wanted::Unsaid {
            self.0.set(Wanted::Live);
        }
    }

    fn whole(&self) -> bool {
        self.0.get() != Wanted::Live
    }

    fn drawn(&self) {
        self.0.set(Wanted::Unsaid);
    }
}

impl Batches {
    /// The batches of a surface drawing into `target`.
    pub(crate) fn new(device: &wgpu::Device, target: crate::view::Target) -> Self {
        // One glyph sheet for the window: the batches share the first's.
        let base = Painter::new(device, target);
        Self {
            over: Painter::sharing(device, target, &base),
            live: Painter::sharing(device, target, &base),
            top: Painter::sharing(device, target, &base),
            base,
            kept: Kept::default(),
            underlay: Underlay::new(device, target),
            #[cfg(test)]
            one_pass: false,
            asked: Asked::default(),
            cost: FrameCost::default(),
        }
    }

    /// What the last frame cost.
    pub(crate) fn cost(&self) -> FrameCost {
        self.cost
    }

    /// **The picture changed**: the next frame lays the window out and draws
    /// all of it. What every redraw is unless it was asked for
    /// [`want_live`](Self::want_live) alone.
    pub(crate) fn want_whole(&self) {
        self.asked.want_whole();
    }

    /// **Only what moves moved**: the next frame may keep the window's
    /// picture and draw its live layers again -- unless something else asked
    /// for the whole of it before that frame is drawn.
    pub(crate) fn want_live(&self) {
        self.asked.want_live();
    }

    /// Whether the frame about to be drawn has to be a whole one, read once
    /// by the front that draws it. A frame nobody of this host asked for is.
    pub(crate) fn whole_wanted(&self) -> bool {
        self.asked.whole()
    }
}

/// What a call to [`render`] came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Drawn {
    /// The window laid out and drawn, all of it.
    Whole,
    /// The live layers drawn again over the picture the window kept.
    Live,
    /// Nothing: the live layers were where they are on screen.
    Still,
    /// Nothing, and the frame is still owed: the surface had no drawable.
    Owed,
}

/// **What a window keeps of its last whole frame**, so that a frame drawn
/// because a line moved builds that line and nothing else.
///
/// A window's picture is a function of its tree, of the host's state and of
/// the world; of the world, the clock and the fed values are what moves on
/// its own. Everything an element draws from those is on its live layer, so a
/// frame asked for by a tick alone finds the rest already in the batches --
/// the vertices on the card, the heavy views framed -- and has only to draw
/// the live layers again, in the placements kept here.
///
/// The card still draws the whole window: what is kept is the work of
/// describing it, which is where a frame's time went.
#[derive(Default)]
struct Kept {
    /// Whether the batches hold the picture of the tree as it stands.
    valid: bool,
    /// The framebuffer that picture was laid out for.
    size: (u32, u32),
    /// The color it was cleared to.
    clear: Option<wgpu::Color>,
    /// The glyph atlas' epoch it was drawn at: a sheet packed again since
    /// holds other glyphs where this picture's text points.
    epoch: u64,
    /// Where each element with a live layer was placed.
    places: Vec<LivePlace>,
    /// The heavy views of the window and, with a dialog up, of the dialog.
    collected: Option<Collected>,
    inside: Option<Collected>,
    /// The live layer as it was last drawn.
    live: Mesh,
    /// Where what is drawn over the live layers lies (the host's bands, the
    /// open menus), as triangle bounds joined: a live layer that meets it
    /// is drawn in the single pass ([`underlay::plan`]).
    over: Vec<[f32; 4]>,
}

impl Kept {
    /// Whether anything is drawn on the live layers: a placed element with
    /// one, or a timeline's playhead.
    fn has_live(&self) -> bool {
        !self.places.is_empty()
            || self
                .collected
                .as_ref()
                .is_some_and(|c| c.timeline_items.iter().any(|i| i.axis_owned))
    }
}

/// The glyph atlas' epoch (`font::atlas::epoch`): the same for ever in a
/// build with no rasterizer, where nothing is ever packed again.
fn atlas_epoch() -> u64 {
    #[cfg(feature = "font-atlas")]
    {
        font::atlas::epoch()
    }
    #[cfg(not(feature = "font-atlas"))]
    {
        0
    }
}

/// **How far a live layer has to move to be drawn again**, in device pixels.
/// A quarter: with four samples to the pixel that is the step a line's edge
/// can show, and without them a pixel is, so nothing under it is visible.
const LIVE_SLACK: f32 = 0.25;

/// One element's placement, kept without the tree it was laid out from: the
/// way down to its widget, and what a [`Ctx`] is made of.
struct LivePlace {
    /// The index of each child on the way from the root to the widget.
    path: Vec<usize>,
    rect: Rect,
    clip: Option<Rect>,
    scale: f32,
    metrics: Metrics,
    indent: f32,
    ink: Ink,
}

/// The placements of `placed[range]` that have a live layer, or `None` when
/// one of them cannot be found again from the root -- a frame that keeps
/// nothing, then, and is drawn whole the next time too.
fn live_places(
    placed: &[layout::Placed<'_>],
    range: std::ops::Range<usize>,
) -> Option<Vec<LivePlace>> {
    let mut out = Vec::new();
    for i in range {
        let p = &placed[i];
        let WidgetKind::Custom(el) = &p.widget.kind else {
            continue;
        };
        if !el.needs().live {
            continue;
        }
        let mut path = Vec::new();
        let mut at = p;
        while let Some(parent) = at.parent.and_then(|n| placed.get(n)) {
            let n = parent
                .widget
                .children
                .iter()
                .position(|child| std::ptr::eq(child, at.widget))?;
            path.push(n);
            at = parent;
        }
        path.reverse();
        out.push(LivePlace {
            path,
            rect: p.rect,
            clip: p.clip,
            scale: p.scale,
            metrics: p.metrics,
            indent: p.indent,
            ink: ink_of(p),
        });
    }
    Some(out)
}

/// **The live layers of the elements in `places`**, into `mesh`: each element
/// found again in `tree` and handed the placement it was laid out in, with
/// the world as it is now.
fn draw_live_places(
    mesh: &mut Mesh,
    tree: &Widget,
    places: &[LivePlace],
    inputs: &FrameInputs,
    theme: &Theme,
) {
    for place in places {
        let Some(widget) = place.path.iter().try_fold(tree, |w, &n| w.children.get(n)) else {
            continue;
        };
        let WidgetKind::Custom(el) = &widget.kind else {
            continue;
        };
        mesh.set_clip(place.clip);
        mesh.set_ink(place.ink);
        let th = widget.theme.as_deref().unwrap_or(theme);
        let m = &place.metrics;
        let ctx = Ctx {
            world: &inputs.world,
            metrics: m,
            rect: place.rect,
            indent: place.indent,
            scale: place.scale,
            clip: place.clip,
            time: widget.id.zip(widget.kind.editor()).and_then(|(id, e)| {
                inputs
                    .world
                    .timelines
                    .space_of(id, e.link, Some(inputs.world.clocks.at(Some(id))))
            }),
            focused: widget.id.is_some() && widget.id == inputs.focused,
            hovered: widget.id.is_some() && widget.id == inputs.popups.and_then(|o| o.hover),
            clock: inputs.world.clocks.at(widget.id),
            // the live layer draws what moves, never a caption
            text_fit: 1.0,
        };
        el.draw_live(&mut Draw::new(mesh, m, th), &ctx);
        // **The focus ring stays over what moves.** It is drawn with the
        // picture, under this layer, so a live layer that reaches its edge --
        // a multitrack's meters at rest, a column the height of its track --
        // painted over the ring wherever they met. Drawn again here, it is the
        // last thing of the element, as it is in the picture.
        if ctx.focused {
            mesh.set_clip(place.clip);
            mesh.set_ink(Ink::default());
            mesh.border(place.rect, m.focus_ring, th.focus);
        }
    }
    mesh.set_clip(None);
    mesh.set_ink(Ink::default());
}

/// Renders `tree` into `gpu`'s surface, using the window's `batches` (flat
/// geometry under, between and over the heavy views), the `waveforms`/
/// `spectrograms`/`canvases` GPU resources, plus `inputs` for the live values.
///
/// **A whole frame** is one immutable mesh-building pass over the placed
/// widgets, then the GPU uploads and the single render pass. **A frame asked
/// for what moves alone** (`whole` false, and a picture kept to draw it over)
/// builds the live layers again and nothing else -- and when they are where
/// they were on screen, it draws nothing at all, so how often a window is
/// drawn follows how far its lines moved.
///
/// Answers what the frame came to ([`Drawn`]).
#[allow(clippy::too_many_arguments)] // the per-window resource set, both fronts
pub(crate) fn render(
    gpu: &mut Gpu,
    renderers: &mut Renderers,
    batches: &mut Batches,
    waveforms: &mut HashMap<SlotAt, WaveformSlot>,
    spectrograms: &mut HashMap<SlotAt, SpectrogramSlot>,
    canvases: &mut HashMap<i32, CanvasView>,
    tree: &Widget,
    inputs: &FrameInputs,
    theme: &Theme,
    whole: bool,
) -> Drawn {
    let (fb_w, fb_h) = (gpu.config.width.max(1), gpu.config.height.max(1));
    batches.cost = FrameCost::default();
    // **The host's own chrome draws in the window's theme**: the status bar,
    // the menu bar, the lists and the tip that open over the tree, the scrim
    // behind a dialog. They belong to no widget, so they used to take the
    // host's theme and ignore a window that set one of its own.
    let theme = tree.theme.as_deref().unwrap_or(theme);
    // **Only what moves**: the picture is in the batches, so the live layers
    // are drawn again where they were placed -- built first, since a glyph
    // one of them needs may be the one the atlas has to be packed again for,
    // and a picture from before that is not kept.
    let live = (!whole
        && batches.kept.valid
        && batches.kept.size == (fb_w, fb_h)
        && batches.kept.epoch == atlas_epoch())
    .then(|| {
        let started = web_time::Instant::now();
        let mut live = Mesh::new();
        if let Some(collected) = &batches.kept.collected {
            draw_timeline_heads(&mut live, collected, waveforms, spectrograms, inputs, theme);
        }
        draw_live_places(&mut live, tree, &batches.kept.places, inputs, theme);
        batches.cost.build = started.elapsed();
        live
    })
    .filter(|_| batches.kept.epoch == atlas_epoch());
    let drawn = if let Some(live) = live {
        // One that is where it was -- a line that crossed no fraction of a
        // pixel worth showing, a meter at rest -- is the frame already on
        // screen.
        if live.near(&batches.kept.live, LIVE_SLACK) {
            batches.asked.drawn();
            return Drawn::Still;
        }
        let started = web_time::Instant::now();
        let bytes = batches
            .live
            .upload(&gpu.device, &gpu.queue, &live, fb_w, fb_h);
        batches.cost.upload = started.elapsed();
        batches.cost.uploaded = bytes as u64;
        batches.kept.live = live;
        Drawn::Live
    } else {
        draw_whole(
            gpu,
            renderers,
            batches,
            waveforms,
            spectrograms,
            canvases,
            tree,
            inputs,
            theme,
        );
        Drawn::Whole
    };

    // Offscreen there is nothing to present: the batches hold the frame, and
    // a capture draws it into a texture of its own.
    let Some(surface) = gpu.surface() else {
        batches.asked.drawn();
        return drawn;
    };
    let acquiring = web_time::Instant::now();
    let frame = match surface.get_current_texture() {
        wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
        _ => {
            // No drawable this turn (outdated/timed-out surface -- e.g. the
            // compositor stopped consuming a covered window's frames):
            // reconfigure and ask for another redraw, so the frame that was
            // requested is not silently dropped and the window never shows
            // stale state once it is presentable again. What it was asked for
            // stands: the batches already hold it.
            surface.configure(&gpu.device, &gpu.config);
            gpu.request_redraw();
            return Drawn::Owed;
        }
    };
    let acquired = acquiring.elapsed();
    let encoding = web_time::Instant::now();
    let target = frame
        .texture
        .create_view(&wgpu::TextureViewDescriptor::default());
    let encoder = encode_pass(
        gpu,
        renderers,
        batches,
        spectrograms,
        canvases,
        theme,
        &target,
    );
    gpu.queue.submit(std::iter::once(encoder.finish()));
    batches.cost.encode = encoding.elapsed();
    let presenting = web_time::Instant::now();
    // The winit present contract: lets winit attach the compositor frame
    // callback to this commit, so later `request_redraw`s are delivered (and
    // throttled) correctly -- without it, Wayland redraw delivery can stall on
    // an unfocused or covered window until the compositor repaints it anyway.
    if let Some(window) = gpu.window() {
        window.pre_present_notify();
    }
    frame.present();
    batches.cost.present = acquired + presenting.elapsed();
    batches.asked.drawn();
    drawn
}

/// **The passes a window's frame is**, into `target`: the batches as they
/// stand, the heavy views between them, resolved out of the multisampled
/// attachment when the window has one. The screen's frame and a capture's
/// are these passes into two textures, which is what keeps a capture the
/// screen's picture.
///
/// A window with live layers keeps everything else ([`Underlay`]): its
/// picture is drawn into a texture of its own when a whole frame changed it,
/// and every frame copies that texture and draws the live layers over it,
/// multisampled in the boxes they cover, so a frame asked for by a moving
/// line costs a copy and that line. A window with nothing live, with a
/// dialog up (whose frames are all whole ones), or whose live layers meet a
/// menu or a band drawn over them draws everything in one pass.
fn encode_pass(
    gpu: &Gpu,
    renderers: &mut Renderers,
    batches: &mut Batches,
    spectrograms: &HashMap<SlotAt, SpectrogramSlot>,
    canvases: &HashMap<i32, CanvasView>,
    theme: &Theme,
    target: &wgpu::TextureView,
) -> wgpu::CommandEncoder {
    let (fb_w, fb_h) = (gpu.config.width.max(1), gpu.config.height.max(1));
    let fb = (fb_w, fb_h);
    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("gui frame"),
        });
    let clear = batches.kept.clear.unwrap_or_else(|| clear_color(theme));
    let boxes = (batches.kept.valid && batches.kept.has_live())
        .then(|| underlay::plan(&batches.kept.live, &batches.kept.over, fb))
        .flatten();
    #[cfg(test)]
    let boxes = boxes.filter(|_| !batches.one_pass);
    let Some(boxes) = boxes else {
        batches.underlay.release();
        let mut pass = begin_pass(&mut encoder, gpu, target, clear);
        draw_under(&mut pass, renderers, batches, spectrograms, canvases, fb);
        batches.live.draw(&mut pass);
        draw_above(&mut pass, renderers, batches, spectrograms, canvases, fb);
        drop(pass);
        return encoder;
    };
    if !batches.underlay.holds(fb) {
        let kept = batches.underlay.target(&gpu.device, &gpu.queue, fb);
        let mut pass = begin_pass(&mut encoder, gpu, &kept, clear);
        draw_under(&mut pass, renderers, batches, spectrograms, canvases, fb);
        draw_above(&mut pass, renderers, batches, spectrograms, canvases, fb);
        drop(pass);
        batches.underlay.fresh = true;
    }
    // Multisampled, the live layers are drawn in their boxes, each over the
    // picture under it; single-sampled, straight over the copy.
    let multisampled = gpu.msaa_view().is_some();
    let atlas = if multisampled && !boxes.is_empty() {
        batches.underlay.place(
            &gpu.device,
            &gpu.queue,
            &batches.base,
            &batches.kept.live,
            &boxes,
        )
    } else {
        batches.underlay.unplace();
        None
    };
    if let Some((samples, resolved)) = &atlas {
        let mut pass = begin_into(&mut encoder, samples, Some(resolved), clear);
        batches.underlay.draw_boxes(&mut pass);
        drop(pass);
    }
    if multisampled && atlas.is_none() && !boxes.is_empty() {
        // An atlas the card cannot hold: the single pass's way.
        let mut pass = begin_pass(&mut encoder, gpu, target, clear);
        draw_under(&mut pass, renderers, batches, spectrograms, canvases, fb);
        batches.live.draw(&mut pass);
        draw_above(&mut pass, renderers, batches, spectrograms, canvases, fb);
        drop(pass);
        return encoder;
    }
    let mut pass = begin_into(&mut encoder, target, None, clear);
    batches.underlay.draw_frame(&mut pass);
    if !multisampled {
        pass.set_viewport(0.0, 0.0, fb_w as f32, fb_h as f32, 0.0, 1.0);
        pass.set_scissor_rect(0, 0, fb_w, fb_h);
        batches.live.draw(&mut pass);
    }
    drop(pass);
    encoder
}

/// A pass into `target`, cleared to `clear`.
///
/// Antialiasing is a property of the **attachment**, so it is the whole of
/// what MSAA changes here: with it on, every pipeline draws into the
/// multisampled texture and the GPU resolves that into the target as the
/// pass ends. One flag, one texture per window, nothing per widget.
fn begin_pass<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    gpu: &Gpu,
    target: &wgpu::TextureView,
    clear: wgpu::Color,
) -> wgpu::RenderPass<'e> {
    match gpu.msaa_view() {
        Some(ms) => begin_into(encoder, ms, Some(target), clear),
        None => begin_into(encoder, target, None, clear),
    }
}

/// A pass into `attachment`, resolved into `resolve` when it is
/// multisampled. The samples are not kept past the resolve: no pass reads
/// them again.
fn begin_into<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    attachment: &wgpu::TextureView,
    resolve: Option<&wgpu::TextureView>,
    clear: wgpu::Color,
) -> wgpu::RenderPass<'e> {
    let store = if resolve.is_some() {
        wgpu::StoreOp::Discard
    } else {
        wgpu::StoreOp::Store
    };
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("gui pass"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: attachment,
            resolve_target: resolve,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(clear),
                store,
            },
            depth_slice: None,
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    })
}

/// What lies under the live layers: the flat widgets, the window's heavy
/// views and what its elements draw over them.
fn draw_under(
    pass: &mut wgpu::RenderPass<'_>,
    renderers: &mut Renderers,
    batches: &Batches,
    spectrograms: &HashMap<SlotAt, SpectrogramSlot>,
    canvases: &HashMap<i32, CanvasView>,
    fb: (u32, u32),
) {
    batches.base.draw(pass);
    if let Some(collected) = &batches.kept.collected {
        draw_heavy(pass, renderers, spectrograms, canvases, collected, fb);
    }
    batches.over.draw(pass);
}

/// What is over the live layers: a dialog's heavy views, the host's bands,
/// the menus, the lists and the tip.
fn draw_above(
    pass: &mut wgpu::RenderPass<'_>,
    renderers: &mut Renderers,
    batches: &Batches,
    spectrograms: &HashMap<SlotAt, SpectrogramSlot>,
    canvases: &HashMap<i32, CanvasView>,
    fb: (u32, u32),
) {
    if let Some(inside) = &batches.kept.inside {
        draw_heavy(pass, renderers, spectrograms, canvases, inside, fb);
    }
    batches.top.draw(pass);
}

/// **A window's picture on its way back from the card**: the frame copied
/// into a buffer the CPU can map, and what it takes to read it as pixels.
///
/// Reading it is two steps because a page cannot block on the card: the
/// buffer is asked to be mapped as the copy is made ([`capture`]), and once
/// the card has said it is ([`ready`](Self::ready)) it is read
/// ([`pixels`](Self::pixels)). A native front waits between the two
/// ([`wait`](Self::wait)); a page asks again on its next tick.
pub(crate) struct Readback {
    buffer: wgpu::Buffer,
    width: u32,
    height: u32,
    /// The bytes of one row in the buffer: a copy's rows are aligned to
    /// [`wgpu::COPY_BYTES_PER_ROW_ALIGNMENT`].
    stride: u32,
    /// Whether the texture's channels are blue first.
    bgra: bool,
    /// What the card said about the mapping, once it has.
    mapped: Arc<std::sync::Mutex<Option<Result<(), String>>>>,
}

impl Readback {
    /// What the card has said about the mapping: `None` until it has.
    pub(crate) fn ready(&self) -> Option<Result<(), String>> {
        self.mapped.lock().ok().and_then(|slot| slot.clone())
    }

    /// **Blocks until the card has answered** (native only: a page's card
    /// answers on the browser's own loop).
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn wait(&self, device: &wgpu::Device) -> Result<(), String> {
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| e.to_string())?;
        self.ready()
            .unwrap_or_else(|| Err("the card did not map the capture".into()))
    }

    /// The picture, as `(width, height, rgba)` rows top to bottom -- once
    /// [`ready`](Self::ready) said the mapping succeeded.
    pub(crate) fn pixels(&self) -> (u32, u32, Vec<u8>) {
        let row = self.width as usize * 4;
        let mut out = Vec::with_capacity(row * self.height as usize);
        {
            let view = self.buffer.slice(..).get_mapped_range();
            for line in view.chunks(self.stride as usize).take(self.height as usize) {
                out.extend_from_slice(&line[..row]);
            }
        }
        self.buffer.unmap();
        for px in out.as_chunks_mut::<4>().0 {
            if self.bgra {
                px.swap(0, 2);
            }
            // The surface is opaque: what a theme's alpha would leave in the
            // texture is not what the screen shows.
            px[3] = 255;
        }
        (self.width, self.height, out)
    }
}

/// **The picture of a window, drawn offscreen**: a whole frame of `tree` laid
/// out and drawn into the window's batches ([`draw_whole`]), the one pass
/// ([`encode_pass`]) encoded into a texture of the window's size instead of
/// its surface, and that texture copied into a buffer asked to be mapped.
///
/// Nothing is presented and the window's next frame is what it would have
/// been: the batches hold the tree as it stands, which is what a frame keeps
/// anyway, and what the window was asked to draw is still asked.
#[allow(clippy::too_many_arguments)] // the per-window resource set, both fronts
pub(crate) fn capture(
    gpu: &mut Gpu,
    renderers: &mut Renderers,
    batches: &mut Batches,
    waveforms: &mut HashMap<SlotAt, WaveformSlot>,
    spectrograms: &mut HashMap<SlotAt, SpectrogramSlot>,
    canvases: &mut HashMap<i32, CanvasView>,
    tree: &Widget,
    inputs: &FrameInputs,
    theme: &Theme,
) -> Result<Readback, String> {
    let theme = tree.theme.as_deref().unwrap_or(theme);
    let format = gpu.config.format;
    let bgra = match format {
        wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb => true,
        wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgba8UnormSrgb => false,
        other => return Err(format!("cannot read back a {other:?} window")),
    };
    draw_whole(
        gpu,
        renderers,
        batches,
        waveforms,
        spectrograms,
        canvases,
        tree,
        inputs,
        theme,
    );
    let (width, height) = (gpu.config.width.max(1), gpu.config.height.max(1));
    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("gui capture"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let target = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let mut encoder = encode_pass(
        gpu,
        renderers,
        batches,
        spectrograms,
        canvases,
        theme,
        &target,
    );
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let stride = (width * 4).div_ceil(align) * align;
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("gui capture readback"),
        size: u64::from(stride) * u64::from(height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(stride),
                rows_per_image: Some(height),
            },
        },
        size,
    );
    gpu.queue.submit(std::iter::once(encoder.finish()));
    let mapped = Arc::new(std::sync::Mutex::new(None));
    let answer = Arc::clone(&mapped);
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            if let Ok(mut slot) = answer.lock() {
                *slot = Some(result.map_err(|e| e.to_string()));
            }
        });
    Ok(Readback {
        buffer,
        width,
        height,
        stride,
        bgra,
        mapped,
    })
}

/// **A whole frame's picture**, into the batches: the tree laid out, every
/// widget drawn, the heavy views framed -- and what the next frames may keep
/// of it ([`Kept`]).
#[allow(clippy::too_many_arguments)] // the per-window resource set, both fronts
fn draw_whole(
    gpu: &mut Gpu,
    renderers: &mut Renderers,
    batches: &mut Batches,
    waveforms: &mut HashMap<SlotAt, WaveformSlot>,
    spectrograms: &mut HashMap<SlotAt, SpectrogramSlot>,
    canvases: &mut HashMap<i32, CanvasView>,
    tree: &Widget,
    inputs: &FrameInputs,
    theme: &Theme,
) {
    let (fb_w, fb_h) = (gpu.config.width.max(1), gpu.config.height.max(1));
    // **Drawn again if the glyph atlas was packed again under it**: the text
    // drawn before that names texels that now hold other glyphs, and a
    // picture is kept, so it would stay wrong for as long as only lines
    // moved. The second pass finds what it needs already on the new sheet.
    let epoch = atlas_epoch();
    let started = web_time::Instant::now();
    let mut picture = picture(tree, inputs, theme, waveforms, spectrograms, (fb_w, fb_h));
    let mut layout = picture.layout;
    if atlas_epoch() != epoch {
        picture = self::picture(tree, inputs, theme, waveforms, spectrograms, (fb_w, fb_h));
        layout += picture.layout;
    }
    batches.cost.layout = layout;
    batches.cost.build = started.elapsed().saturating_sub(layout);
    batches.cost.placed = picture.placed;
    let uploading = web_time::Instant::now();
    let mut uploaded = 0;
    for (batch, mesh) in [
        (&mut batches.base, &picture.base),
        (&mut batches.over, &picture.over),
        (&mut batches.live, &picture.live),
        (&mut batches.top, &picture.top),
    ] {
        uploaded += batch.upload(&gpu.device, &gpu.queue, mesh, fb_w, fb_h);
    }
    batches.cost.uploaded = uploaded as u64;
    for heavy in std::iter::once(&picture.collected).chain(picture.inside.as_ref()) {
        upload_heavy(
            gpu,
            renderers,
            spectrograms,
            canvases,
            heavy,
            inputs,
            (fb_w, fb_h),
        );
    }
    batches.cost.upload = uploading.elapsed();
    let over = underlay::join(&picture.top.triangle_bounds(), 0.0);
    batches.kept = Kept {
        valid: picture.kept,
        size: (fb_w, fb_h),
        clear: Some(clear_color(theme)),
        epoch: atlas_epoch(),
        places: std::mem::take(&mut picture.places),
        collected: Some(picture.collected),
        inside: picture.inside,
        live: picture.live,
        over,
    };
    batches.underlay.fresh = false;
}

/// A whole frame as triangles and snapshots, before any of it reaches a card.
struct Picture {
    base: Mesh,
    over: Mesh,
    live: Mesh,
    top: Mesh,
    /// The heavy views of the window and, with a dialog up, of the dialog.
    collected: Collected,
    inside: Option<Collected>,
    /// Where each element with a live layer was placed.
    places: Vec<LivePlace>,
    /// Whether the frames after this one may keep it: the live batch is where
    /// the live layers belong, and every one of them can be found again.
    kept: bool,
    /// How long placing the tree took, and how many widgets it placed.
    layout: std::time::Duration,
    placed: u64,
}

/// **The picture of `tree` in a framebuffer of `fb`**: laid out, and drawn
/// into the four meshes of a frame. No card is touched, so it is what a test
/// reads -- and what says, mesh against mesh, that nothing but the live layer
/// follows the clock.
fn picture(
    tree: &Widget,
    inputs: &FrameInputs,
    theme: &Theme,
    waveforms: &HashMap<SlotAt, WaveformSlot>,
    spectrograms: &HashMap<SlotAt, SpectrogramSlot>,
    (fb_w, fb_h): (u32, u32),
) -> Picture {
    let window = Rect::new(0.0, 0.0, fb_w as f32, fb_h as f32);
    // The status band comes off the top of the frame, before anything is
    // placed: the same call the hit test makes (`Host::content_area`), so the
    // pixels a press lands on are the pixels the tree was drawn on.
    let bar = status::bar(tree, inputs.status, window, inputs.metrics);
    let area = status::content(tree, inputs.status, window, inputs.metrics);
    // ...and the menu bar's off the top, by the same rule.
    let menu_bar = super::menubar::bar(tree, window, inputs.metrics);
    let area = super::menubar::content(tree, area, inputs.metrics);
    // The lanes' clips are placed on the axis their group currently stands at,
    // so the layout of a multitrack follows the zoom and the pan.
    let laying = web_time::Instant::now();
    let placed = layout::layout_on(area, tree, inputs.metrics);
    let layout_time = laying.elapsed();
    let mut mesh = Mesh::new();
    let mut over = Mesh::new();
    let mut live = Mesh::new();
    let mut top = Mesh::new();
    // **A dialog is the tail of the placements** (`layout` places it last), and
    // it is drawn apart: everything before it is the window as it always was,
    // and the dialog goes over all of that, in the overlay.
    let (base, dialog) = placed.split_at(chrome::modal_start(&placed).unwrap_or(placed.len()));
    let collected = collect_widgets(base, &mut mesh, inputs, theme);
    draw_dividers(&mut mesh, &placed, 0..base.len(), inputs, theme);

    draw_timeline_meshes(
        &mut mesh,
        &mut over,
        &collected,
        waveforms,
        spectrograms,
        inputs,
        theme,
    );
    draw_static_meshes(&mut mesh, &mut over, &collected, inputs, theme, tree);
    let reading = draw_element_overlays(&mut over, base, inputs, theme);
    draw_bars(&mut over, base, inputs, theme);
    // **What moves, apart**: the window's live layers, kept by the placements
    // they were drawn in so the next tick can draw them alone.
    let places = live_places(&placed, 0..base.len());

    let inside = if dialog.is_empty() {
        draw_timeline_heads(
            &mut live,
            &collected,
            waveforms,
            spectrograms,
            inputs,
            theme,
        );
        draw_live_places(
            &mut live,
            tree,
            places.as_deref().unwrap_or_default(),
            inputs,
            theme,
        );
        None
    } else {
        // The window behind a dialog is out of reach, and looks it: a scrim
        // over the work area, then the dialog's own picture built on the side
        // and laid over it -- its flat widgets, then its textures, then what
        // its elements draw over themselves.
        //
        // What moves in the window is under the scrim with the rest of it, and
        // what moves in the dialog is over the dialog's own pictures, so with a
        // dialog up nothing is kept and every frame is a whole one.
        //
        // **The scrim and the dialog take the live batch, which is free.** A
        // batch draws all of its flat geometry and then all of its glyphs, so
        // a dialog laid into the batch that holds the window's overlays came
        // out under their text: a meter's ladder read through a dialog that
        // reached it, and a label behind the OK button through the button.
        // Drawn in the next batch, everything of the window is under the scrim
        // and the dialog is whole over it.
        draw_timeline_heads(
            &mut over,
            &collected,
            waveforms,
            spectrograms,
            inputs,
            theme,
        );
        draw_live_places(
            &mut over,
            tree,
            places.as_deref().unwrap_or_default(),
            inputs,
            theme,
        );
        let mut under = Mesh::new();
        let mut above = Mesh::new();
        let inside = collect_widgets(dialog, &mut under, inputs, theme);
        draw_dividers(&mut under, &placed, base.len()..placed.len(), inputs, theme);
        draw_timeline_meshes(
            &mut under,
            &mut above,
            &inside,
            waveforms,
            spectrograms,
            inputs,
            theme,
        );
        draw_static_meshes(&mut under, &mut above, &inside, inputs, theme, tree);
        draw_element_overlays(&mut above, dialog, inputs, theme);
        draw_bars(&mut above, dialog, inputs, theme);
        draw_timeline_heads(&mut above, &inside, waveforms, spectrograms, inputs, theme);
        let within = live_places(&placed, base.len()..placed.len());
        draw_live_places(
            &mut above,
            tree,
            within.as_deref().unwrap_or_default(),
            inputs,
            theme,
        );
        live.set_clip(None);
        live.set_ink(Ink::default());
        live.rect(area, with_alpha(theme.background, 0.6));
        live.append(&under);
        top.append(&above);
        Some(inside)
    };

    // Into the top batch after the tree, so the bar reads over whatever ran
    // up to its edge.
    if let Some(band) = bar {
        let progress = match &tree.kind {
            WidgetKind::Window { progress, .. } => progress.as_ref(),
            _ => None,
        };
        draw_status(&mut top, band, inputs, progress, reading.as_deref(), theme);
    }
    // **Work in progress over the window**, while the owner says it runs.
    if let WidgetKind::Window {
        progress: Some(progress),
        ..
    } = &tree.kind
    {
        draw_progress_overlay(&mut top, area, progress, inputs, theme);
    }
    // **The popup layer, last of all**: a list covers what it opened over. It
    // is placed inside `area` -- the window minus the host's bands -- by the
    // function the press hit-tests with, so it never runs under the bar and a
    // row is hit where it is drawn.
    if let Some(band) = menu_bar {
        draw_menu_bar(&mut top, tree, band, inputs, theme);
    }
    if let Some(popups) = inputs.popups {
        draw_popups(&mut top, popups, tree, &placed, window, area, inputs, theme);
    }
    for mesh in [&mut mesh, &mut over, &mut live, &mut top] {
        mesh.set_clip(None);
        mesh.set_ink(Ink::default());
    }
    Picture {
        base: mesh,
        over,
        live,
        top,
        kept: inside.is_none() && places.is_some(),
        places: places.unwrap_or_default(),
        collected,
        inside,
        layout: layout_time,
        placed: placed.len() as u64,
    }
}

/// Pushes this frame's textures and uniforms to the heavy views in
/// `collected`: the spectrograms framed on their bodies, and the shader canvases
/// recompiled where their source changed.
fn upload_heavy(
    gpu: &Gpu,
    renderers: &mut Renderers,
    spectrograms: &mut HashMap<SlotAt, SpectrogramSlot>,
    canvases: &mut HashMap<i32, CanvasView>,
    collected: &Collected,
    inputs: &FrameInputs,
    (fb_w, fb_h): (u32, u32),
) {
    for item in &collected.timeline_items {
        // The body the element stated when it described its frame: one
        // rectangle, so the picture and the chrome around it agree.
        let body = item.body;
        // The texture layer, when the stack has one: a waveform's picture is
        // triangles and went into the window's mesh with the rest of the
        // chrome, so there is nothing to prepare for it here.
        if item.look.layers.has(Paint::Spectrogram)
            && let Some(slot) = spectrograms.get_mut(&(item.id, item.key))
        {
            let look = &item.look.look;
            let nav = chrome_for(inputs, item.id, &item.editor, || {
                View::full(slot.total_samples())
            })
            .nav;
            let nav = item
                .placed
                .unwrap_or_else(|| placed_nav(&nav, item.editor.offset));
            let channels = slot.views.len();
            let rows = if item.look.overlay { 1 } else { channels };
            let alpha = item.look.layers.alpha_of(Paint::Spectrogram).unwrap_or(1.0);
            if slot.stale.get().is_none() {
                for view in &mut slot.views {
                    view.recall(look.analysis);
                }
            }
            for (ch, view) in slot.views.iter_mut().enumerate() {
                view.set_display(
                    look.db_floor,
                    look.db_ceil,
                    look.freq_scale,
                    look.colormap.max(0) as u32,
                );
                view.set_shaping(look.normalize, look.interpolate, look.tuning);
                view.set_alpha(alpha);
                view.set_freq_window(item.look.y.0, item.look.y.1);
                view.set_framing(framing_of(
                    item.look.lanes.rect(body, rows, ch.min(rows - 1)),
                    fb_w,
                    fb_h,
                ));
                view.upload(
                    &gpu.device,
                    &gpu.queue,
                    renderers,
                    &nav,
                    body.w.max(1.0) as u32,
                );
            }
        }
    }
    // Recompile any canvas whose shader changed, then push its per-frame uniforms
    // (viewport size, elapsed time, resolved params).
    for frame in &collected.canvas_frames {
        if let Some(view) = canvases.get_mut(&frame.id) {
            view.set_shader(&gpu.device, &frame.shader);
            let time = view.elapsed();
            let res = [frame.body.w.max(1.0), frame.body.h.max(1.0)];
            let framing = framing_of(frame.body, fb_w, fb_h);
            view.upload(&gpu.queue, res, time, frame.params, framing);
        }
    }
}

/// Draws the heavy views in `collected` into `pass`, each through its own
/// viewport and scissor, and gives the pass back with the whole framebuffer as
/// both: the flat batch drawn next is in window space, already clipped by its
/// geometry where it needed to be.
fn draw_heavy(
    pass: &mut wgpu::RenderPass<'_>,
    renderers: &Renderers,
    spectrograms: &HashMap<SlotAt, SpectrogramSlot>,
    canvases: &HashMap<i32, CanvasView>,
    collected: &Collected,
    (fb_w, fb_h): (u32, u32),
) {
    for item in &collected.timeline_items {
        // The body the element stated when it described its frame: one
        // rectangle, so the picture and the chrome around it agree.
        let body = item.body;
        if body.w < 1.0 || body.h < 1.0 {
            continue;
        }
        if !apply_scissor(pass, item.clip, fb_w, fb_h) {
            continue;
        }
        if !item.look.layers.has(Paint::Spectrogram) {
            continue;
        }
        let Some(slot) = spectrograms.get(&(item.id, item.key)) else {
            continue;
        };
        let channels = slot.views.len();
        let rows = if item.look.overlay { 1 } else { channels };
        // A stack scrolled past its body draws only inside the body's rows,
        // never over the chrome above or below it.
        if item.look.lanes.overflow(rows, body.h) > 0.0
            && !apply_scissor(
                pass,
                Some(rows_clip(item.clip, item.rect, body)),
                fb_w,
                fb_h,
            )
        {
            continue;
        }
        for (ch, view) in slot.views.iter().enumerate() {
            let row = item.look.lanes.rect(body, rows, ch.min(rows - 1));
            let (x, y, w, h) = clamp_viewport(row, fb_w, fb_h);
            if w >= 1.0 && h >= 1.0 {
                pass.set_viewport(x, y, w, h, 0.0, 1.0);
                view.draw(pass, renderers);
            }
        }
    }
    for frame in &collected.canvas_frames {
        if frame.body.w >= 1.0
            && frame.body.h >= 1.0
            && let Some(view) = canvases.get(&frame.id)
            && apply_scissor(pass, frame.clip, fb_w, fb_h)
        {
            let (x, y, w, h) = clamp_viewport(frame.body, fb_w, fb_h);
            pass.set_viewport(x, y, w, h, 0.0, 1.0);
            view.draw(pass);
        }
    }
    pass.set_viewport(0.0, 0.0, fb_w as f32, fb_h as f32, 0.0, 1.0);
    pass.set_scissor_rect(0, 0, fb_w, fb_h);
}

/// Applies a placed widget's clip as the pass scissor (the full framebuffer
/// when it has none), returning `false` when the clip is empty -- the caller
/// skips the draw entirely. The heavy views draw through `set_viewport`, which
/// *positions and scales* but does not cut; a scrolled view poking out of its
/// `scroll` container is cut by this scissor, the GPU sibling of the mesh's
/// geometric clip. What the scissor cannot reach is the **window** edge, since
/// a viewport may not leave the attachment at all -- that is [`Framing`]'s
/// half of the same job.
fn apply_scissor(
    pass: &mut wgpu::RenderPass<'_>,
    clip: Option<Rect>,
    fb_w: u32,
    fb_h: u32,
) -> bool {
    let Some(c) = clip else {
        pass.set_scissor_rect(0, 0, fb_w, fb_h);
        return true;
    };
    let x = c.x.clamp(0.0, fb_w as f32) as u32;
    let y = c.y.clamp(0.0, fb_h as f32) as u32;
    let w = (c.w.max(0.0) as u32).min(fb_w - x);
    let h = (c.h.max(0.0) as u32).min(fb_h - y);
    if w == 0 || h == 0 {
        return false;
    }
    pass.set_scissor_rect(x, y, w, h);
    true
}

/// The part of a widget rect the framebuffer can hold, as `set_viewport` wants
/// it (that call rejects a viewport leaving the attachment).
///
/// It is the **intersection**, not a clamp of the origin: a rect starting above
/// the window keeps its far edge where it is instead of sliding down with its
/// origin. What the viewport still cannot do is cut -- it scales whatever the
/// view draws into whatever rectangle it is given -- so a view that is only
/// partly visible also gets a [`Framing`] built from this pair, and places its
/// geometry for the full rect inside it.
pub(crate) fn clamp_viewport(r: Rect, fb_w: u32, fb_h: u32) -> (f32, f32, f32, f32) {
    let x0 = r.x.max(0.0);
    let y0 = r.y.max(0.0);
    let x1 = (r.x + r.w).min(fb_w as f32);
    let y1 = (r.y + r.h).min(fb_h as f32);
    (x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0))
}

/// The [`Framing`] a rect is drawn with in this framebuffer: the identity while
/// it fits, and the placement that keeps its picture at a fixed size once the
/// window edge starts cutting it.
pub(crate) fn framing_of(r: Rect, fb_w: u32, fb_h: u32) -> Framing {
    Framing::new((r.x, r.y, r.w, r.h), clamp_viewport(r, fb_w, fb_h))
}

#[cfg(test)]
mod tests;
