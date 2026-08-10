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

use super::*;

/// A placed `plot` widget and the data its (static) draw needs, copied out of
/// the host tree so the mesh is built after the tree borrow is released.
pub(super) struct PlotItem {
    pub(super) rect: Rect,
    pub(super) clip: Option<Rect>,
    pub(super) theme: Option<Arc<Theme>>,
    pub(super) samples: Arc<[f32]>,
    pub(super) channels: usize,
    pub(super) view: plot::PlotView,
    pub(super) overlay: bool,
    pub(super) sample_rate: f64,
    pub(super) min: Option<f32>,
    pub(super) max: Option<f32>,
    pub(super) ruler: Ruler,
    pub(super) ruler_y: bool,
    pub(super) spectrum: Option<Arc<plot::PlotSpectrum>>,
    pub(super) db_floor: f32,
    pub(super) db_ceil: f32,
    pub(super) freq_scale: FreqScale,
    /// The element's own frequency window, normalized (a spectrum view only).
    pub(super) x_view: (f64, f64),
    pub(super) label: Option<String>,
}

impl PlotItem {
    pub(super) fn params(&self) -> plot::PlotParams<'_> {
        plot::PlotParams {
            samples: &self.samples,
            channels: self.channels,
            view: self.view,
            overlay: self.overlay,
            sample_rate: self.sample_rate,
            min: self.min,
            max: self.max,
            ruler: self.ruler,
            ruler_y: self.ruler_y,
            spectrum: self.spectrum.as_deref(),
            db_floor: self.db_floor,
            db_ceil: self.db_ceil,
            freq_scale: self.freq_scale,
            x_view: self.x_view,
            label: self.label.as_deref(),
        }
    }
}

/// Sorts one signal element into the item list of the renderer its
/// presentation picks. This is the one place the model's product becomes a
/// choice of destination — a navigable heavy view to the window's GPU slots, a
/// forward-only source to its per-tick window, a stored non-navigable one to
/// the mesh — and it is deliberately the *only* place, so nothing downstream
/// has to ask what a view "is".
#[allow(clippy::too_many_arguments)] // one element, six destinations
pub(super) fn signal_item(
    id: i32,
    el: &signal::SignalElement,
    // The server's rate, placing a frequency axis whose element names none.
    server_rate: f64,
    rect: Rect,
    indent: f32,
    clip: Option<Rect>,
    theme: Option<Arc<Theme>>,
    timelines: &mut Vec<TimelineItem>,
    waves: &mut Vec<WaveItem>,
    scopes: &mut Vec<ScopeItem>,
    phases: &mut Vec<PhaseItem>,
    spectra: &mut Vec<SpectrumItem>,
    plots: &mut Vec<PlotItem>,
) {
    let (min, max) = (el.value.min.unwrap_or(-1.0), el.value.max.unwrap_or(1.0));
    let strip_x = el.editor.ruler != Ruler::Off;
    let strip_y = el.editor.ruler_y != RulerY::Off;
    match (el.presentation, &el.source) {
        // The navigable heavy views: their geometry is built on the window's
        // pipelines from the slot keyed by this id.
        (Presentation::Signal | Presentation::TimeFrequency, _) if el.caps.navigable => {
            timelines.push(TimelineItem {
                id,
                rect,
                indent,
                clip,
                theme,
                kind: if el.presentation == Presentation::TimeFrequency {
                    TimelineKind::Spectrogram {
                        db_floor: el.spectral.db_floor,
                        db_ceil: el.spectral.db_ceil,
                        freq_scale: el.spectral.freq_scale,
                        colormap: el.spectral.colormap,
                    }
                } else {
                    TimelineKind::Waveform {
                        overlay: el.display.overlay,
                    }
                },
                editor: el.editor.clone(),
            });
        }
        // A forward-only trace: the audio-rate window the tick aligned, or the
        // control bus's rolling history.
        (Presentation::Signal, signal::Source::Bus(bus)) => {
            if bus.rate.is_audio() {
                waves.push(WaveItem {
                    id,
                    rect,
                    clip,
                    theme,
                    min,
                    max,
                    window_ms: bus.window_ms,
                    trigger: bus.trigger,
                    overlay: el.display.overlay,
                    ruler: strip_x,
                    ruler_y: strip_y,
                    label: el.display.label.clone(),
                });
            } else {
                scopes.push(ScopeItem {
                    id,
                    rect,
                    clip,
                    theme,
                    min,
                    max,
                    label: el.display.label.clone(),
                });
            }
        }
        (Presentation::Phase, _) => phases.push(PhaseItem {
            id,
            rect,
            clip,
            theme,
            label: el.display.label.clone(),
        }),
        (Presentation::Spectrum, signal::Source::Bus(_)) => spectra.push(SpectrumItem {
            id,
            rect,
            clip,
            theme,
            fft_size: el.spectral.fft_size,
            db_floor: el.spectral.db_floor,
            db_ceil: el.spectral.db_ceil,
            freq_scale: el.spectral.freq_scale,
            peak_hold: el.spectral.peak_hold,
            ruler: strip_x,
            ruler_y: strip_y,
            // The window the axis can show, not the one that was asked for:
            // an item is a drawing instruction, and the floor of the analysis
            // is part of what there is to draw.
            x_view: el.freq_window(server_rate),
            label: el.display.label.clone(),
        }),
        // A stored signal nobody navigates: the mesh renderer, whichever of
        // the two presentations it shows.
        (_, signal::Source::Data(data)) => plots.push(PlotItem {
            rect,
            clip,
            theme,
            samples: Arc::clone(&data.samples),
            channels: data.channels,
            view: if el.presentation == Presentation::Spectrum {
                plot::PlotView::Spectrum
            } else {
                plot::PlotView::Signal
            },
            overlay: el.display.overlay,
            sample_rate: el.editor.sample_rate,
            min: el.value.min,
            max: el.value.max,
            ruler: el.editor.ruler,
            ruler_y: strip_y,
            spectrum: el.analysis.clone(),
            db_floor: el.spectral.db_floor,
            db_ceil: el.spectral.db_ceil,
            freq_scale: el.spectral.freq_scale,
            x_view: el.freq_window(server_rate),
            label: el.display.label.clone(),
        }),
        // A live source with no live renderer for its presentation (a stored
        // presentation over a bus): nothing to draw until it has one.
        (_, signal::Source::Bus(_)) => {}
    }
}

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
    pub(super) rect: Rect,
    pub(super) clip: Option<Rect>,
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
    pub(super) theme: Option<Arc<Theme>>,
    /// The placement's size table and zoom — an element body draws through
    /// them exactly as it does anywhere else.
    pub(super) metrics: Metrics,
    pub(super) scale: f32,
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

pub(super) struct PianoRollItem {
    pub(super) id: i32,
    pub(super) rect: Rect,
    /// Where this member's group starts its body inside `rect`
    /// ([`layout::Placed::indent`]).
    pub(super) indent: f32,
    pub(super) clip: Option<Rect>,
    pub(super) theme: Option<Arc<Theme>>,
    pub(super) notes: Vec<pianoroll::Note>,
    pub(super) osc: Vec<pianoroll::OscMark>,
    /// The multi-note selection (note indices), drawn highlighted.
    pub(super) selected: Vec<usize>,
    pub(super) min: f32,
    pub(super) max: f32,
    pub(super) velocity_lane: bool,
    pub(super) osc_lane: bool,
    pub(super) label: Option<String>,
    pub(super) editor: EditorProps,
}

/// A placed **control-rate** `scope`, copied out of the host tree: its id (to
/// fetch the rolling history the tick advanced) and the scale it draws over.
pub(super) struct ScopeItem {
    pub(super) id: i32,
    pub(super) rect: Rect,
    pub(super) clip: Option<Rect>,
    pub(super) theme: Option<Arc<Theme>>,
    pub(super) min: f32,
    pub(super) max: f32,
    pub(super) label: Option<String>,
}

/// A placed audio-rate `scope`, copied out of the host tree: its id (to fetch
/// the tick's aligned tap window) and display parameters.
pub(super) struct WaveItem {
    pub(super) id: i32,
    pub(super) rect: Rect,
    pub(super) clip: Option<Rect>,
    pub(super) theme: Option<Arc<Theme>>,
    pub(super) min: f32,
    pub(super) max: f32,
    pub(super) window_ms: f32,
    pub(super) trigger: f32,
    pub(super) overlay: bool,
    pub(super) ruler: bool,
    pub(super) ruler_y: bool,
    pub(super) label: Option<String>,
}

/// A placed `spectrum` widget, copied out of the host tree: its id (to fetch the
/// analysis states), rect and display parameters (the dB window and axis flags).
pub(super) struct SpectrumItem {
    pub(super) id: i32,
    pub(super) rect: Rect,
    pub(super) clip: Option<Rect>,
    pub(super) theme: Option<Arc<Theme>>,
    pub(super) fft_size: usize,
    pub(super) db_floor: f32,
    pub(super) db_ceil: f32,
    pub(super) freq_scale: FreqScale,
    pub(super) peak_hold: bool,
    pub(super) ruler: bool,
    pub(super) ruler_y: bool,
    /// The element's own frequency window, normalized.
    pub(super) x_view: (f64, f64),
    pub(super) label: Option<String>,
}

/// A placed `phasescope`, copied out of the host tree (drawn from the
/// interleaved L/R window the tick stored in `tap_windows`).
pub(super) struct PhaseItem {
    pub(super) id: i32,
    pub(super) rect: Rect,
    pub(super) clip: Option<Rect>,
    pub(super) theme: Option<Arc<Theme>>,
    pub(super) label: Option<String>,
}

/// Which timeline view a placed editor-grade widget is, with its display props.
pub(super) enum TimelineKind {
    Waveform {
        overlay: bool,
    },
    Spectrogram {
        db_floor: f32,
        db_ceil: f32,
        freq_scale: FreqScale,
        colormap: i32,
    },
}

/// A placed timeline view (waveform/spectrogram), copied out of the host tree.
pub(super) struct TimelineItem {
    pub(super) id: i32,
    pub(super) rect: Rect,
    /// Where this member's group starts its body inside `rect`
    /// ([`layout::Placed::indent`]).
    pub(super) indent: f32,
    pub(super) clip: Option<Rect>,
    pub(super) theme: Option<Arc<Theme>>,
    pub(super) kind: TimelineKind,
    pub(super) editor: EditorProps,
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
    pub(super) scope_rects: Vec<ScopeItem>,
    pub(super) wave_rects: Vec<WaveItem>,
    pub(super) phase_rects: Vec<PhaseItem>,
    pub(super) spectrum_rects: Vec<SpectrumItem>,
    pub(super) plot_rects: Vec<PlotItem>,
    pub(super) track_items: Vec<TrackItem>,
    pub(super) clip_items: Vec<ClipItem>,
    pub(super) clip_bodies: Vec<ClipBodyItem>,
    pub(super) spectral_bodies: Vec<SpectralBodyItem>,
    pub(super) ruler_items: Vec<RulerItem>,
    pub(super) pianoroll_items: Vec<PianoRollItem>,
    pub(super) canvas_frames: Vec<CanvasFrame>,
}

/// One immutable pass over the placed widgets: the flat widgets (labels,
/// controls, panels, the patcher, the score, the piano) draw straight into
/// `mesh`; every data-driven widget is copied out of the host tree into the
/// returned [`Collected`], so the heavier meshes and the GPU uploads are built
/// after the tree borrow is released.
pub(super) fn collect_widgets(
    placed: &[layout::Placed],
    mesh: &mut Mesh,
    inputs: &FrameInputs,
    theme: &Theme,
) -> Collected {
    let mut timeline_items: Vec<TimelineItem> = Vec::new();
    // Scope rects carry no bus: the value is sampled on the frame tick
    // (`advance_scopes`); the render only draws the stored history. Audio-rate
    // scopes draw their stored tap window instead (`wave_rects`).
    let mut scope_rects: Vec<ScopeItem> = Vec::new();
    let mut wave_rects: Vec<WaveItem> = Vec::new();
    // Phasescope rects (drawn from the interleaved L/R window in `tap_windows`)
    // and spectrum rects (drawn from the persistent `spectra` analysis states).
    let mut phase_rects: Vec<PhaseItem> = Vec::new();
    let mut spectrum_rects: Vec<SpectrumItem> = Vec::new();
    // Plot items (with a cheap Arc clone of the samples) and node-tree rects,
    // likewise copied out so the host-tree borrow can be released before the
    // node-tree models and the GPU resources are read.
    let mut plot_rects: Vec<PlotItem> = Vec::new();
    let mut track_items: Vec<TrackItem> = Vec::new();
    // A clip and its bodies are placed widgets, so they are collected from
    // their own placements: the clip's box first, its bodies after (the pass
    // emits parent-before-child), which is the layering the drawing needs.
    let mut clip_items: Vec<ClipItem> = Vec::new();
    let mut clip_bodies: Vec<ClipBodyItem> = Vec::new();
    let mut spectral_bodies: Vec<SpectralBodyItem> = Vec::new();
    let mut ruler_items: Vec<RulerItem> = Vec::new();
    let mut pianoroll_items: Vec<PianoRollItem> = Vec::new();
    let mut canvas_frames: Vec<CanvasFrame> = Vec::new();
    for p in placed {
        // Everything a scrolled widget paints clips to its container's area.
        mesh.set_clip(p.clip);
        // A **clip body** is drawn as a body, not as the element it also is:
        // it has no chrome of its own (no ruler, no keyboard gutter, no
        // navigation), because it is drawn against the axes of the clip
        // holding it. That is what the containment buys, and it is decided
        // here — once — rather than by each element asking where it is.
        if let Some(parent) = p.parent
            && let WidgetKind::Clip { dur, .. } = placed[parent].widget.kind
        {
            // The one body whose picture is not geometry: a time-frequency take
            // samples an uploaded texture, so it goes to the GPU pass with the
            // clip's own axis and the clip's id (the slot's key).
            if let WidgetKind::Signal(el) = &p.widget.kind
                && el.is_texture_view()
                && let Some(id) = placed[parent].widget.id
            {
                spectral_bodies.push(SpectralBodyItem {
                    id,
                    rect: p.rect,
                    local: p.time.unwrap_or_else(|| View::full(1)),
                    clip: p.clip,
                    db_floor: el.spectral.db_floor,
                    db_ceil: el.spectral.db_ceil,
                    freq_scale: el.spectral.freq_scale,
                    colormap: el.spectral.colormap,
                });
                continue;
            }
            clip_bodies.push(ClipBodyItem {
                rect: p.rect,
                local: p.time.unwrap_or_else(|| View::full(1)),
                dur,
                clip: p.clip,
                theme: p.widget.theme.clone(),
                metrics: p.metrics,
                scale: p.scale,
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
            // Every signal element, sorted to the renderer its presentation
            // picks: the two navigable heavy views to the GPU slots, the live
            // ones to their per-tick windows, the stored non-navigable ones to
            // the mesh plot. The element is one thing; only its destination
            // differs.
            WidgetKind::Signal(el) => {
                if let Some(id) = p.widget.id {
                    signal_item(
                        id,
                        el,
                        inputs.world.sample_rate,
                        p.rect,
                        p.indent,
                        p.clip,
                        p.widget.theme.clone(),
                        &mut timeline_items,
                        &mut wave_rects,
                        &mut scope_rects,
                        &mut phase_rects,
                        &mut spectrum_rects,
                        &mut plot_rects,
                    );
                }
            }
            WidgetKind::Piano {
                min,
                max,
                active_min,
                active_max,
                overview,
                pressed,
                label,
                ..
            } => piano::draw_widget(
                &mut Draw::new(mesh, m, th),
                p.rect,
                *min,
                *max,
                *overview,
                *active_min,
                *active_max,
                pressed,
                label.as_deref(),
            ),
            WidgetKind::TimeRuler { editor, .. } => {
                ruler_items.push(RulerItem {
                    id: p.widget.id.unwrap_or(-1),
                    rect: p.rect,
                    indent: p.indent,
                    clip: p.clip,
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
                    theme: p.widget.theme.clone(),
                    label: label.clone(),
                    header: header.clone(),
                    editor: editor.clone(),
                });
            }
            WidgetKind::Clip { label, .. } => {
                clip_items.push(ClipItem {
                    rect: p.rect,
                    clip: p.clip,
                    theme: p.widget.theme.clone(),
                    label: label.clone(),
                });
            }
            WidgetKind::PianoRoll {
                notes,
                osc,
                selected,
                min,
                max,
                velocity_lane,
                osc_lane,
                label,
                editor,
                ..
            } => {
                if let Some(id) = p.widget.id {
                    pianoroll_items.push(PianoRollItem {
                        id,
                        rect: p.rect,
                        indent: p.indent,
                        clip: p.clip,
                        theme: p.widget.theme.clone(),
                        notes: notes.clone(),
                        osc: osc.clone(),
                        selected: selected.clone(),
                        min: *min,
                        max: *max,
                        velocity_lane: *velocity_lane,
                        osc_lane: *osc_lane,
                        label: label.clone(),
                        editor: editor.clone(),
                    });
                }
            }
            WidgetKind::Patch {
                patch,
                selected,
                label,
            } => {
                // The patcher view of a logical group: drawn in the base mesh
                // (flat geometry, like the other static views). The canvas
                // scales with the enclosing workspace's zoom (`p.scale`), so
                // boxes, wires and text zoom together.
                let live = inputs
                    .wiring
                    .filter(|(id, _, _)| Some(*id) == p.widget.id)
                    .map(|(_, port, cursor)| (port, cursor));
                let marquee = inputs
                    .marquee
                    .filter(|(id, _)| Some(*id) == p.widget.id)
                    .map(|(_, r)| r);
                patch::draw(
                    &mut Draw::new(mesh, m, th),
                    p.rect,
                    patch,
                    label.as_deref(),
                    &patch::CanvasState {
                        live,
                        selected,
                        marquee,
                        scale: p.scale,
                    },
                );
            }
            // A registered element draws straight into the window's one mesh
            // during this walk, with the placement's theme and size table...
            WidgetKind::Custom(el) => {
                let ctx = Ctx {
                    world: &inputs.world,
                    metrics: m,
                    rect: p.rect,
                    scale: p.scale,
                    clip: p.clip,
                    time: None,
                    focused: p.widget.id.is_some() && p.widget.id == inputs.focused,
                };
                el.draw(&mut Draw::new(mesh, m, th), &ctx);
                // ...and, for a view the shared mesh cannot carry, what its
                // claimed slot draws this frame. The set of slots is closed and
                // is the frame's, so this match is over pipelines the window
                // already has -- never over what the element is.
                if let (Some(id), Some(slot)) = (p.widget.id, el.slot(&ctx)) {
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
        scope_rects,
        wave_rects,
        phase_rects,
        spectrum_rects,
        plot_rects,
        track_items,
        clip_items,
        clip_bodies,
        spectral_bodies,
        ruler_items,
        pianoroll_items,
        canvas_frames,
    }
}
