//! What one frame *collects*: the per-widget snapshots the draw passes work
//! from, and the single tree walk that fills them.
//!
//! A frame reads the host tree exactly once. Every data-driven widget is copied
//! out of it into one of the item structs below, so the meshes and the GPU
//! uploads that follow never hold the tree borrow — which is what lets a heavy
//! view upload while the chrome is still being built. The flat widgets (labels,
//! controls, panels, the patcher, the score, the piano) never become items:
//! they draw straight into the mesh during the same walk, having nothing to
//! defer.

use super::super::widget::element::TextureLook;
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

pub(super) struct TrackItem {
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
    pub(super) label: Option<String>,
    /// The lane's gutter: its width and the controls it carries.
    pub(super) header: track::Header,
    /// The lane's chrome: its time ruler (off by default), its playhead anchor
    /// and its `link` — the navigation group whose shared window it draws
    /// through (the lanes of a window are linked by default, so they zoom and
    /// pan as one).
    pub(super) editor: EditorProps,
}

/// A placed `pianoroll` widget, copied out of the host tree: the note/OSC
/// content and the pitch window, plus the editor chrome (ruler/selection/
/// playhead/link — its navigation group). Drawn as flat geometry, the
/// static-view posture, sharing the `pianoroll` primitives with the clip body.
/// One placed `clip`: the box the layout put it in and its name. Its bodies
/// are separate items ([`ClipBodyItem`]), collected after it — the placements
/// are emitted parent-before-child, so drawing the vectors in order paints
/// every clip and then every body over its own clip.
pub(super) struct ClipItem {
    pub(super) id: Option<i32>,
    pub(super) rect: Rect,
    /// Which of the clip's own ends are on screen, so a grip is only ever
    /// drawn where the clip actually ends
    /// (`track::clip_ends_on_screen`).
    pub(super) ends: (bool, bool),
    /// Whether the clip's **placement** is its active edit layer — the grips
    /// are its affordance, so they are drawn while it is and not while a hand
    /// is editing something inside the clip.
    pub(super) placement_active: bool,
    pub(super) clip: Option<Rect>,
    /// The opacity and corner radius this widget draws with
    /// ([`super::ink_of`]).
    pub(super) ink: Ink,
    pub(super) theme: Option<Arc<Theme>>,
    pub(super) label: Option<String>,
}

/// One placed **clip body**: a child element of a clip, with the rectangle and
/// the clip-local window it is drawn against ([`layout::Placed::time`]) and the
/// clip's span, which is what maps a source frame onto that window.
///
/// The element is copied out whole, like every other data-driven item, so the
/// heavier mesh work happens after the host-tree borrow is released.
pub(super) struct ClipBodyItem {
    pub(super) rect: Rect,
    pub(super) local: View,
    pub(super) dur: f64,
    pub(super) clip: Option<Rect>,
    /// The opacity and corner radius this widget draws with
    /// ([`super::ink_of`]).
    pub(super) ink: Ink,
    pub(super) theme: Option<Arc<Theme>>,
    /// The placement's size table — an element body draws through it exactly
    /// as it does anywhere else. Its **zoom** is not here because a body's
    /// door takes no context: what a zoom resolves is already in the table.
    pub(super) metrics: Metrics,
    /// What of its material the container is showing (the clip's own
    /// [`SourceWindow`](crate::host::widget::SourceWindow)).
    pub(super) window: crate::host::widget::SourceWindow,
    /// Whether this body is its container's **active edit layer** — what it is
    /// told so it draws affordances only when they are promises it can keep
    /// (see [`crate::host::layers`]).
    pub(super) active: bool,
    pub(super) kind: WidgetKind,
}

pub(super) struct SpectralBodyItem {
    pub(super) id: i32,
    pub(super) rect: Rect,
    pub(super) local: View,
    pub(super) clip: Option<Rect>,
    pub(super) db_floor: f32,
    pub(super) db_ceil: f32,
    pub(super) freq_scale: FreqScale,
    pub(super) colormap: i32,
}

/// Which timeline view a placed editor-grade widget is, with its display props.
pub(super) enum TimelineKind {
    Waveform {
        /// The value domain the geometry is mapped through, as the element
        /// stated it — [`crate::waveform::DEFAULT_DOMAIN`] is the amplitude
        /// axis, and anything else is a plain value axis (dBFS, bits and
        /// percent are full-scale amplitude units).
        domain: (f32, f32),
        /// The amplitude window, as the element stated it.
        amp: (f64, f64),
        overlay: bool,
        /// What the picture measures, as the element stated it — one drawing
        /// per measure into the one body.
        measures: crate::host::graphics::signal::trace::Measures,
    },
    Spectrogram {
        /// The frequency window, as the element stated it.
        freq: (f64, f64),
        look: TextureLook,
    },
}

/// A placed timeline view (waveform/spectrogram), copied out of the host tree.
///
/// Half of it is the **element's** answer — the body its picture is drawn in
/// and the vertical window, which arrived as a
/// [`SlotFrame`] — and half is the
/// **axis'**: the placement, the group gutter and the editor chrome, which the
/// frame draws around every member of a navigation group alike, a lane and a
/// roll included.
pub(super) struct TimelineItem {
    pub(super) id: i32,
    pub(super) rect: Rect,
    /// Where the picture goes, as the element resolved it out of `rect`.
    pub(super) body: Rect,
    pub(super) clip: Option<Rect>,
    /// The opacity and corner radius this widget draws with
    /// ([`super::ink_of`]).
    pub(super) ink: Ink,
    pub(super) theme: Option<Arc<Theme>>,
    pub(super) kind: TimelineKind,
    pub(super) editor: EditorProps,
    /// The sample the hand is holding on this view, copied out with the rest —
    /// the overlay pass draws it *over* the picture, since the material under
    /// it has not changed and must not be re-summarized to show an edit that
    /// nobody has applied yet.
    pub(super) pending: Option<crate::host::widget::element::PendingEdit>,
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
    pub(super) track_items: Vec<TrackItem>,
    pub(super) clip_items: Vec<ClipItem>,
    pub(super) clip_bodies: Vec<ClipBodyItem>,
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
    let mut track_items: Vec<TrackItem> = Vec::new();
    // A clip and its bodies are placed widgets, so they are collected from
    // their own placements: the clip's box first, its bodies after (the pass
    // emits parent-before-child), which is the layering the drawing needs.
    let mut clip_items: Vec<ClipItem> = Vec::new();
    let mut clip_bodies: Vec<ClipBodyItem> = Vec::new();
    let mut spectral_bodies: Vec<SpectralBodyItem> = Vec::new();
    let mut ruler_items: Vec<RulerItem> = Vec::new();
    let mut canvas_frames: Vec<CanvasFrame> = Vec::new();
    for p in placed {
        // Everything a scrolled widget paints clips to its container's area...
        mesh.set_clip(p.clip);
        // ...and everything it paints carries its own opacity and corner
        // radius, set here for the whole run of triangles this widget is about
        // to contribute — an element draws what it always drew.
        let ink = super::ink_of(p);
        mesh.set_ink(ink);
        // A **clip body** is drawn as a body, not as the element it also is:
        // it has no chrome of its own (no ruler, no keyboard gutter, no
        // navigation), because it is drawn against the axes of the clip
        // holding it. That is what the containment buys, and it is decided
        // here — once — rather than by each element asking where it is.
        if let Some(parent) = p.parent
            && let WidgetKind::Clip { dur, .. } = placed[parent].widget.kind
        {
            // **The body's own span and window when it has them**, the clip's
            // otherwise: a body that names a stretch of the clip is drawn
            // against that stretch and reads its own segment of material, and
            // one that names neither is the whole-clip body every clip written
            // as flat props holds.
            let dur = p.widget.span.map_or(dur, |(_, len)| len);
            let window = p
                .widget
                .window
                .or(placed[parent].widget.window)
                .unwrap_or_default();
            // The one body whose picture is not geometry: a time-frequency take
            // samples an uploaded texture, so it goes to the GPU pass with the
            // clip's own axis and the clip's id (the slot's key).
            if let Some(look) = p.widget.kind.texture_body()
                && let Some(id) = placed[parent].widget.id
            {
                spectral_bodies.push(SpectralBodyItem {
                    id,
                    rect: p.rect,
                    local: p.time.unwrap_or_else(|| View::full(1)),
                    clip: p.clip,
                    db_floor: look.db_floor,
                    db_ceil: look.db_ceil,
                    freq_scale: look.freq_scale,
                    colormap: look.colormap,
                });
                continue;
            }
            // Which layer this body is, asked of the container that placed it:
            // the child's own index among the clip's children, which is the
            // address the whole model uses.
            let active = placed[parent]
                .widget
                .children
                .iter()
                .position(|c| std::ptr::eq(c, p.widget))
                .is_some_and(|index| {
                    crate::host::layers::child_is_active(placed[parent].widget, index)
                });
            clip_bodies.push(ClipBodyItem {
                window,
                rect: p.rect,
                local: p.time.unwrap_or_else(|| View::full(1)),
                dur,
                active,
                clip: p.clip,
                ink,
                theme: p.widget.theme.clone(),
                metrics: p.metrics,
                kind: p.widget.kind.clone(),
            });
            continue;
        }
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
            WidgetKind::Track {
                label,
                header,
                editor,
                ..
            } => {
                // The lane's clips are placed widgets of their own, collected
                // from their own placements below — a lane draws what a lane
                // is, and nothing that is on it.
                track_items.push(TrackItem {
                    id: p.widget.id.unwrap_or(-1),
                    rect: p.rect,
                    indent: p.indent,
                    clip: p.clip,
                    ink,
                    theme: p.widget.theme.clone(),
                    label: label.clone(),
                    header: header.clone(),
                    editor: editor.clone(),
                });
            }
            WidgetKind::Clip { label, dur, .. } => {
                clip_items.push(ClipItem {
                    id: p.widget.id,
                    rect: p.rect,
                    ends: p.time.as_ref().map_or((true, true), |local| {
                        crate::host::graphics::track::clip_ends_on_screen(local, *dur)
                    }),
                    placement_active: crate::host::layers::active(p.widget)
                        == crate::host::layers::Layer::Placement,
                    clip: p.clip,
                    ink,
                    theme: p.widget.theme.clone(),
                    label: label.clone(),
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
                if let (Some(id), Some(slot)) = (p.widget.id, el.slot(&ctx)) {
                    // A timeline slot is half an item: the element said where
                    // its picture goes and at what vertical window, the axis
                    // says the rest (the chrome every group member shares).
                    let mut timeline = |body: Rect, kind: TimelineKind| {
                        if let Some(editor) = p.widget.kind.editor() {
                            timeline_items.push(TimelineItem {
                                id,
                                rect: p.rect,
                                body,
                                clip: p.clip,
                                ink,
                                theme: p.widget.theme.clone(),
                                kind,
                                editor: editor.clone(),
                                pending: el.pending_edit().cloned(),
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
                        SlotFrame::Waveform {
                            body,
                            domain,
                            amp,
                            overlay,
                            measures,
                        } => timeline(
                            body,
                            TimelineKind::Waveform {
                                domain,
                                amp,
                                overlay,
                                measures,
                            },
                        ),
                        SlotFrame::Spectrogram { body, freq, look } => {
                            timeline(body, TimelineKind::Spectrogram { freq, look })
                        }
                    }
                }
            }
            _ => {}
        }
        // The **focus ring**, drawn by the host over whatever the widget drew:
        // one role, one look, and no element painting its own — the ring says
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
        track_items,
        clip_items,
        clip_bodies,
        spectral_bodies,
        ruler_items,
        canvas_frames,
    }
}
