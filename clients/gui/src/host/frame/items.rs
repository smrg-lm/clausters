//! What one frame *collects*: the per-widget snapshots the draw passes work
//! from, and the single tree walk that fills them.
//!
//! A frame reads the host tree exactly once. Every data-driven widget is copied
//! out of it into one of the item structs below, so the meshes and the GPU
//! uploads that follow never hold the tree borrow -- which is what lets a heavy
//! view upload while the chrome is still being built. The flat widgets (labels,
//! controls, panels, the patcher, the score, the piano) never become items:
//! they draw straight into the mesh during the same walk, having nothing to
//! defer.

use super::super::widget::element::{Samples, SlotKey, TextureLook};
use super::*;

/// A placed `track` lane and its clips, copied out of the host tree so the
/// graphic-unit overlay is drawn after the tree borrow is released. The clips'
/// shared time axis is computed once over all the window's tracks.
/// A placed free-standing `timeruler`: the strip and the group it labels.
pub(super) struct RulerItem {
    pub(super) id: i32,
    pub(super) rect: Rect,
    /// Where this member's group starts its body inside `rect`
    /// ([`layout::Placed::indent`]).
    pub(super) indent: f32,
    pub(super) clip: Option<Rect>,
    /// The opacity and corner radius this widget draws with
    /// ([`super::ink_of`]).
    pub(super) ink: Ink,
    pub(super) theme: Option<Arc<Theme>>,
    pub(super) editor: EditorProps,
}

pub(super) struct SpectralBodyItem {
    pub(super) id: i32,
    /// Which of the widget's pictures this is -- the pair `(id, key)` addresses
    /// a slot, so a view holding several boxes over several takes has one entry
    /// per take rather than one per widget.
    pub(super) key: SlotKey,
    pub(super) rect: Rect,
    pub(super) local: View,
    pub(super) clip: Option<Rect>,
    pub(super) db_floor: f32,
    pub(super) db_ceil: f32,
    pub(super) freq_scale: FreqScale,
    pub(super) colormap: i32,
}

/// **What a placed editor-grade signal view draws**, as the element stated it:
/// its layer stack and everything the layers are mapped through.
///
/// One struct rather than a variant per picture: a view is a *stack*, and the
/// question "is this a waveform or a spectrogram" stopped being the frame's the
/// moment one body could carry both.
pub(super) struct TimelineLook {
    /// The layers, back to front.
    pub(super) layers: crate::host::graphics::signal::layers::Stack,
    /// What the body's vertical measures, as the element resolved it.
    pub(super) axis: crate::host::graphics::signal::layers::Domain,
    /// The value domain the traces are mapped through --
    /// [`crate::waveform::DEFAULT_DOMAIN`] is the amplitude axis, and anything
    /// else is a plain value axis (dBFS, bits and percent are full-scale
    /// amplitude units).
    pub(super) domain: (f32, f32),
    /// The vertical window, as the element stated it: amplitude or frequency,
    /// whichever the stack put on the axis.
    pub(super) y: (f64, f64),
    pub(super) overlay: bool,
    /// The loudness layer, as the element stated it: the curve it measured,
    /// the scale it is read on and what its span measures.
    pub(super) loudness: crate::host::elements::signal::LoudnessFrame,
    /// How a texture layer is coloured and scaled.
    pub(super) look: TextureLook,
}

/// A placed timeline view (waveform/spectrogram), copied out of the host tree.
///
/// Half of it is the **element's** answer -- the body its picture is drawn in
/// and the vertical window, which arrived as a
/// [`SlotFrame`] -- and half is the
/// **axis'**: the placement, the group gutter and the editor chrome, which the
/// frame draws around every member of a navigation group alike, a lane and a
/// roll included.
pub(super) struct TimelineItem {
    pub(super) id: i32,
    /// Which of the widget's pictures this is: `(id, key)` addresses a slot.
    pub(super) key: SlotKey,
    pub(super) rect: Rect,
    /// Where the picture goes, as the element resolved it out of `rect`.
    pub(super) body: Rect,
    pub(super) clip: Option<Rect>,
    /// The opacity and corner radius this widget draws with
    /// ([`super::ink_of`]).
    pub(super) ink: Ink,
    pub(super) theme: Option<Arc<Theme>>,
    pub(super) look: TimelineLook,
    pub(super) editor: EditorProps,
    /// The sample the hand is holding on this view, copied out with the rest --
    /// the overlay pass draws it *over* the picture, since the samples under
    /// it has not changed and must not be re-summarized to show an edit that
    /// nobody has applied yet.
    pub(super) pending: Option<crate::host::widget::element::PendingEdit>,
    /// How much of the samples exists, for a take being written into as it is
    /// drawn (`fills`); `None` when all of it does.
    pub(super) written: Option<u64>,
}

/// A placed `canvas` widget, copied out of the host tree: its viewport body, the
/// shader source (for an in-place recompile when it changed) and the param
/// vector, with the bus-mapped slots already resolved from shared memory.
pub(super) struct CanvasFrame {
    pub(super) id: i32,
    pub(super) body: Rect,
    pub(super) clip: Option<Rect>,
    pub(super) shader: String,
    pub(super) params: [f32; canvas::PARAM_COUNT],
}

/// The data-driven widgets copied out of the host tree by [`collect_widgets`],
/// grouped by kind. Each group is drawn in its own pass once the tree borrow is
/// released, so the meshes and GPU uploads never touch the host tree.
pub(super) struct Collected {
    pub(super) timeline_items: Vec<TimelineItem>,
    pub(super) spectral_bodies: Vec<SpectralBodyItem>,
    pub(super) ruler_items: Vec<RulerItem>,
    pub(super) canvas_frames: Vec<CanvasFrame>,
}

/// One immutable pass over the placed widgets: the flat widgets (labels,
/// controls, panels, the patcher, the score, the piano) draw straight into
/// `mesh`; every data-driven widget is copied out of the host tree into the
/// returned [`Collected`], so the heavier meshes and the GPU uploads are built
/// after the tree borrow is released.
pub(super) fn collect_widgets(
    placed: &[layout::Placed<'_>],
    mesh: &mut Mesh,
    inputs: &FrameInputs,
    theme: &Theme,
) -> Collected {
    let mut timeline_items: Vec<TimelineItem> = Vec::new();
    let mut spectral_bodies: Vec<SpectralBodyItem> = Vec::new();
    let mut ruler_items: Vec<RulerItem> = Vec::new();
    let mut canvas_frames: Vec<CanvasFrame> = Vec::new();
    for p in placed {
        // Everything a scrolled widget paints clips to its container's area...
        mesh.set_clip(p.clip);
        // ...and everything it paints carries its own opacity and corner
        // radius, set here for the whole run of triangles this widget is about
        // to contribute -- an element draws what it always drew.
        let ink = super::ink_of(p);
        mesh.set_ink(ink);
        // This widget's own size table: the host's, resolved at the scale it is
        // seen through ([`layout::Placed::metrics`]). Identical to the window's
        // outside a workspace; inside a zoomed one it carries the zoom, so a
        // box's padding, parts and text enlarge together.
        let m = &p.metrics;
        // The widget's resolved theme (a theme group's overlay, a `color`
        // accent), resolved at mutation points -- one reference per widget.
        let th = p.widget.theme.as_deref().unwrap_or(theme);
        match &p.widget.kind {
            WidgetKind::Panel { .. } | WidgetKind::Scroll { .. } | WidgetKind::Stack { .. } => {
                mesh.rect(p.rect, th.panel)
            }
            WidgetKind::TimeRuler { editor, .. } => {
                ruler_items.push(RulerItem {
                    id: p.widget.id.unwrap_or(-1),
                    rect: p.rect,
                    indent: p.indent,
                    clip: p.clip,
                    ink,
                    theme: p.widget.theme.clone(),
                    editor: editor.clone(),
                });
            }
            // A registered element draws straight into the window's one mesh
            // during this walk, with the placement's theme and size table...
            WidgetKind::Custom(el) => {
                let ctx = Ctx {
                    world: &inputs.world,
                    metrics: m,
                    rect: p.rect,
                    indent: p.indent,
                    scale: p.scale,
                    clip: p.clip,
                    // The axis this element was **placed on**, when it is a
                    // member of a navigation group: the group's window, its
                    // shared selection and where its playhead stands. A leaf
                    // that draws on that axis reads it here rather than being
                    // handed a picture of it, which is what makes one element
                    // both a standalone view and a lane's content.
                    time: p.widget.id.zip(p.widget.kind.editor()).and_then(|(id, e)| {
                        inputs
                            .world
                            .timelines
                            .space_of(id, e.link, Some(inputs.world.sample_clock))
                    }),
                    focused: p.widget.id.is_some() && p.widget.id == inputs.focused,
                };
                el.draw(&mut Draw::new(mesh, m, th), &ctx);
                // ...and, for a view the shared mesh cannot carry, what its
                // claimed slot draws this frame. The set of slots is closed and
                // is the frame's, so this match is over pipelines the window
                // already has -- never over what the element is.
                // The time-frequency pictures a container draws **inside**
                // itself, each over its own box: they sample a texture, so they
                // go to the GPU pass rather than into the mesh, keyed by the
                // slot they come from.
                if let Some(id) = p.widget.id
                    && let Some(slotted) = el.slotted()
                {
                    for body in slotted.texture_bodies(&ctx) {
                        spectral_bodies.push(SpectralBodyItem {
                            id,
                            key: body.key,
                            rect: body.rect,
                            local: body.local,
                            clip: p.clip,
                            db_floor: body.look.db_floor,
                            db_ceil: body.look.db_ceil,
                            freq_scale: body.look.freq_scale,
                            colormap: body.look.colormap,
                        });
                    }
                }
                for (key, slot) in p.widget.id.into_iter().flat_map(|id| {
                    el.slotted()
                        .map(|s| s.slots(&ctx))
                        .unwrap_or_default()
                        .into_iter()
                        .map(move |(key, s)| ((id, key), s))
                }) {
                    let (id, slot_key) = key;
                    // A timeline slot is half an item: the element said where
                    // its picture goes and at what vertical window, the axis
                    // says the rest (the chrome every group member shares).
                    let mut timeline = |body: Rect, look: TimelineLook| {
                        if let Some(editor) = p.widget.kind.editor() {
                            timeline_items.push(TimelineItem {
                                id,
                                key: slot_key,
                                rect: p.rect,
                                body,
                                clip: p.clip,
                                ink,
                                theme: p.widget.theme.clone(),
                                look,
                                editor: editor.clone(),
                                pending: el.pending_edit().cloned(),
                                written: el.samples().and_then(Samples::written),
                            });
                        }
                    };
                    match slot {
                        SlotFrame::Shader {
                            body,
                            source,
                            params,
                        } => canvas_frames.push(CanvasFrame {
                            id,
                            body,
                            clip: p.clip,
                            shader: source,
                            params,
                        }),
                        SlotFrame::Signal {
                            body,
                            layers,
                            axis,
                            domain,
                            y,
                            overlay,
                            loudness,
                            look,
                        } => timeline(
                            body,
                            TimelineLook {
                                layers,
                                axis,
                                domain,
                                y,
                                overlay,
                                loudness,
                                look,
                            },
                        ),
                    }
                }
            }
            _ => {}
        }
        // The **focus ring**, drawn by the host over whatever the widget drew:
        // one role, one look, and no element painting its own -- the ring says
        // where the keyboard points, which is a window's answer rather than a
        // widget's. What being focused means *inside* an element (a field's
        // caret) is the element's, and reaches it as `Ctx::focused`.
        if p.widget.id.is_some() && p.widget.id == inputs.focused {
            mesh.set_clip(p.clip);
            mesh.border(p.rect, m.focus_ring, th.focus);
        }
    }

    Collected {
        timeline_items,
        spectral_bodies,
        ruler_items,
        canvas_frames,
    }
}
