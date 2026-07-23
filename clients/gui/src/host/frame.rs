//! Rendering one window's widget tree into its wgpu surface — the shared frame
//! path, agnostic of platform and of how the host is driven.
//!
//! This is the code the milestone calls "isolate the surface/GPU/loop port":
//! both fronts feed the **same** [`render`] one tree plus its per-window GPU
//! resources, so the browser is pixel-faithful to the desktop by construction,
//! not by a parallel renderer. The native windowed front ([`super::gui`]) calls
//! it with live inputs (the shared-memory bus source, scope histories, the node
//! tree, the held-button highlight); the browser entry point ([`super::web`])
//! calls it with the streamed equivalents. It builds the flat-geometry [`Mesh`]
//! from the placed widgets ([`super::layout`] + [`super::paint`]/
//! [`super::font`]), uploads the heavy `waveform`/`spectrogram`/`canvas` views,
//! and draws the whole frame in one pass — the editor chrome (rulers,
//! selection, playhead, cursor readout) as a second, *overlay* mesh drawn
//! after the heavy views so it reads on top of them.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use crate::gpu::Gpu;
use crate::spectrogram::{FreqScale, SpectrogramView, Stft, hop_capped};
use crate::view::TimelineView;
use crate::viewport::View;
use crate::waveform::{WaveformData, WaveformView};

use super::canvas::{self, CanvasView};
use super::layout::{self, Rect};
use super::nodetree::{self, NodeTree};
use super::paint::{Mesh, Painter};
use super::ruler::{self, TimeUnit};
use super::spectrum::SpectrumState;
use super::theme::{Theme, with_alpha};
use super::timeline::{TimelineGroups, group_key};
use super::widget::{EditorProps, Ruler, RulerY, Widget, WidgetKind};
use super::{
    BusSource, bpf, controls, live, meters, patch, phasescope, piano, pianoroll, plot, spectrum,
    track,
};

const LABEL_SCALE: f32 = 2.0;

// Editor chrome of the timeline views (waveform/spectrogram).
/// Height of the time-ruler strip under a timeline view, device pixels.
pub(crate) const RULER_H: f32 = 18.0;
/// Width of the vertical-ruler strip beside a timeline view, device pixels
/// (sized for the widest labels: `-32768`, `20K`, `-INF`).
pub(crate) const RULER_W: f32 = 46.0;
use super::ruler::RULER_SCALE;

/// The window clear color: the theme's `background` role as a `wgpu::Color`.
pub(crate) fn clear_color(theme: &Theme) -> wgpu::Color {
    wgpu::Color {
        r: theme.background[0] as f64,
        g: theme.background[1] as f64,
        b: theme.background[2] as f64,
        a: theme.background[3] as f64,
    }
}

/// A waveform widget's GPU view. Its navigation window lives in the widget's
/// timeline group ([`super::timeline`]), not here — a slot is per window, a
/// group may span windows.
pub(crate) struct WaveformSlot {
    pub(crate) view: WaveformView,
}

/// A `WaveformSlot` (the GPU view) for ready data.
pub(crate) fn waveform_slot(data: WaveformData, gpu: &Gpu) -> WaveformSlot {
    let view = WaveformView::new(&gpu.device, gpu.config.format, data);
    WaveformSlot { view }
}

/// A spectrogram widget's GPU views — one [`SpectrogramView`] (own STFT and
/// texture) per channel lane. Navigation lives in the timeline group.
pub(crate) struct SpectrogramSlot {
    pub(crate) views: Vec<SpectrogramView>,
}

impl SpectrogramSlot {
    /// The per-channel sample count of this slot's data.
    pub(crate) fn total_samples(&self) -> usize {
        self.views.first().map_or(1, |v| v.total_samples())
    }
}

/// A `SpectrogramSlot` from per-channel analyses (empty `stfts` yields none).
pub(crate) fn spectrogram_slot(stfts: Vec<Stft>, gpu: &Gpu) -> Option<SpectrogramSlot> {
    if stfts.is_empty() {
        return None;
    }
    let views = stfts
        .into_iter()
        .map(|stft| {
            SpectrogramView::new(&gpu.device, &gpu.queue, gpu.config.format, Arc::new(stft))
        })
        .collect();
    Some(SpectrogramSlot { views })
}

/// One STFT per channel for a spectrogram lane set: de-interleaved `channels`,
/// analyzed at `window_size`/`hop` (the hop raised by [`hop_capped`] so a long
/// buffer fits the magnitude texture) and `sample_rate` (48 kHz when unknown,
/// so the frequency axis is still drawable). Shared by both fronts and every
/// data source (mapped path, fetched buffer, inline samples).
pub(crate) fn stft_lanes(
    channels: Vec<Vec<f32>>,
    window_size: usize,
    hop: usize,
    sample_rate: f64,
) -> Vec<Stft> {
    let sr = if sample_rate > 0.0 {
        sample_rate as f32
    } else {
        48_000.0
    };
    channels
        .into_iter()
        .map(|ch| {
            let hop = hop_capped(ch.len(), window_size, hop);
            Stft::compute(&ch, window_size, hop, sr)
        })
        .collect()
}

/// De-interleaves `channels` channels out of a flat buffer (a trailing partial
/// frame is ignored) — the front half of [`stft_lanes`] for inline sources.
pub(crate) fn deinterleave(samples: &[f32], channels: usize) -> Vec<Vec<f32>> {
    let channels = channels.max(1);
    let frames = samples.len() / channels;
    (0..channels)
        .map(|ch| (0..frames).map(|f| samples[f * channels + ch]).collect())
        .collect()
}

/// The body a timeline view draws into: its rect minus the time-ruler strip
/// under it (when the x ruler is on) and the vertical-ruler strip to its left
/// (when the y ruler is on) — each ruler gets its own space instead of
/// overlaying the view.
pub(crate) fn timeline_body(rect: Rect, editor: &EditorProps) -> Rect {
    let (mut x, mut w, mut h) = (rect.x, rect.w, rect.h);
    if editor.ruler != Ruler::Off {
        h = (h - RULER_H).max(0.0);
    }
    if editor.ruler_y != RulerY::Off {
        x += RULER_W.min(w);
        w = (w - RULER_W).max(0.0);
    }
    Rect::new(x, rect.y, w, h)
}

/// A placed `plot` widget and the data its (static) draw needs, copied out of
/// the host tree so the mesh is built after the tree borrow is released.
struct PlotItem {
    rect: Rect,
    clip: Option<Rect>,
    theme: Option<Arc<Theme>>,
    samples: Arc<[f32]>,
    channels: usize,
    view: plot::PlotView,
    overlay: bool,
    sample_rate: f64,
    min: Option<f32>,
    max: Option<f32>,
    ruler: Ruler,
    ruler_y: bool,
    spectrum: Option<Arc<plot::PlotSpectrum>>,
    db_floor: f32,
    db_ceil: f32,
    freq_scale: FreqScale,
    label: Option<String>,
}

impl PlotItem {
    fn params(&self) -> plot::PlotParams<'_> {
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
            label: self.label.as_deref(),
        }
    }
}

/// A placed `bpf` widget and the data its draw needs, copied out of the host
/// tree so the mesh is built after the tree borrow is released.
struct BpfItem {
    rect: Rect,
    clip: Option<Rect>,
    theme: Option<Arc<Theme>>,
    points: Vec<bpf::BpfPoint>,
    min: f32,
    max: f32,
    duration: f64,
    exp: bool,
    label: Option<String>,
}

/// A placed `track` lane and its clips, copied out of the host tree so the
/// graphic-unit overlay is drawn after the tree borrow is released. The clips'
/// shared time axis is computed once over all the window's tracks.
struct TrackItem {
    id: i32,
    rect: Rect,
    clip: Option<Rect>,
    theme: Option<Arc<Theme>>,
    label: Option<String>,
    clips: Vec<track::ClipDraw>,
    /// The lane's chrome: its time ruler (off by default), its playhead anchor
    /// and its `link` — the navigation group whose shared window it draws
    /// through (the lanes of a window are linked by default, so they zoom and
    /// pan as one).
    editor: EditorProps,
}

/// A placed `pianoroll` widget, copied out of the host tree: the note/OSC
/// content and the pitch window, plus the editor chrome (ruler/selection/
/// playhead/link — its navigation group). Drawn as flat geometry, the
/// static-view posture, sharing the `pianoroll` primitives with the clip body.
struct PianoRollItem {
    id: i32,
    rect: Rect,
    clip: Option<Rect>,
    theme: Option<Arc<Theme>>,
    notes: Vec<pianoroll::Note>,
    osc: Vec<pianoroll::OscMark>,
    /// The multi-note selection (note indices), drawn highlighted.
    selected: Vec<usize>,
    min: f32,
    max: f32,
    velocity_lane: bool,
    osc_lane: bool,
    label: Option<String>,
    editor: EditorProps,
}

/// A placed `meter`, copied out of the host tree: its rect, the control bus it
/// reads each frame and the scale it shows it over.
struct MeterItem {
    rect: Rect,
    clip: Option<Rect>,
    theme: Option<Arc<Theme>>,
    bus: i32,
    min: f32,
    max: f32,
    label: Option<String>,
}

/// A placed **control-rate** `scope`, copied out of the host tree: its id (to
/// fetch the rolling history the tick advanced) and the scale it draws over.
struct ScopeItem {
    id: i32,
    rect: Rect,
    clip: Option<Rect>,
    theme: Option<Arc<Theme>>,
    min: f32,
    max: f32,
    label: Option<String>,
}

/// A placed `nodetree` view, copied out of the host tree: the server group it
/// mirrors and whether it lists each node's controls.
struct NodeTreeItem {
    rect: Rect,
    clip: Option<Rect>,
    theme: Option<Arc<Theme>>,
    group: i32,
    controls: bool,
    label: Option<String>,
}

/// A placed audio-rate `scope`, copied out of the host tree: its id (to fetch
/// the tick's aligned tap window) and display parameters.
struct WaveItem {
    id: i32,
    rect: Rect,
    clip: Option<Rect>,
    theme: Option<Arc<Theme>>,
    min: f32,
    max: f32,
    window_ms: f32,
    trigger: f32,
    overlay: bool,
    ruler: bool,
    ruler_y: bool,
    label: Option<String>,
}

/// A placed `spectrum` widget, copied out of the host tree: its id (to fetch the
/// analysis states), rect and display parameters (the dB window and axis flags).
struct SpectrumItem {
    id: i32,
    rect: Rect,
    clip: Option<Rect>,
    theme: Option<Arc<Theme>>,
    fft_size: usize,
    db_floor: f32,
    db_ceil: f32,
    freq_scale: FreqScale,
    peak_hold: bool,
    ruler: bool,
    ruler_y: bool,
    label: Option<String>,
}

/// A placed `phasescope`, copied out of the host tree (drawn from the
/// interleaved L/R window the tick stored in `tap_windows`).
struct PhaseItem {
    id: i32,
    rect: Rect,
    clip: Option<Rect>,
    theme: Option<Arc<Theme>>,
    label: Option<String>,
}

/// Which timeline view a placed editor-grade widget is, with its display props.
enum TimelineKind {
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
struct TimelineItem {
    id: i32,
    rect: Rect,
    clip: Option<Rect>,
    theme: Option<Arc<Theme>>,
    kind: TimelineKind,
    editor: EditorProps,
}

/// A placed `canvas` widget, copied out of the host tree: its viewport body, the
/// shader source (for an in-place recompile when it changed) and the param
/// vector, with the bus-mapped slots already resolved from shared memory.
struct CanvasFrame {
    id: i32,
    body: Rect,
    clip: Option<Rect>,
    shader: String,
    params: [f32; canvas::PARAM_COUNT],
}

/// The live inputs the frame needs beyond the tree and the GPU resources. The
/// native front fills them from its state; the browser front passes the
/// streamed equivalents.
pub(crate) struct FrameInputs<'a> {
    /// The control-bus source for `meter`/`canvas` reads (`None` reads zero).
    pub(crate) bus: Option<&'a dyn BusSource>,
    /// The server node trees the `nodetree` view draws, by group.
    pub(crate) node_trees: &'a HashMap<i32, NodeTree>,
    /// The id of a momentary button currently held down (drawn pressed).
    pub(crate) active_button: Option<i32>,
    /// The id of the focused editable `text` field in this window (drawn with a
    /// caret and its selection), if any.
    pub(crate) focused_text: Option<i32>,
    /// Whether an audio server is attached (the `nodetree` placeholder text).
    pub(crate) server_attached: bool,
    /// The server's sample rate, placing the `spectrum` frequency axis and the
    /// timeline rulers when a widget names no rate of its own (0.0 → unknown).
    pub(crate) sample_rate: f64,
    /// The engine's sample clock (samples since boot; the shm header natively,
    /// the polled `/clock` in the browser). Drives the playhead: a timeline
    /// view with `playhead_at >= 0` draws its line at
    /// `sample_clock - playhead_at`.
    pub(crate) sample_clock: f64,
    /// The pointer position in device pixels, for the cursor readout of the
    /// timeline views (`None` = no pointer over the window).
    pub(crate) cursor: Option<(f64, f64)>,
    /// The host's timeline navigation groups: each waveform/spectrogram draws
    /// its group's shared window (linked views navigate as one).
    pub(crate) timelines: &'a TimelineGroups,
    /// A selection marquee in flight on a patch: the widget and the
    /// rectangle (device pixels), drawn over the canvas.
    pub(crate) marquee: Option<(i32, Rect)>,
    /// A cord drag in flight on a patch: the widget, the grabbed port
    /// (box, side, index) and the cursor — drawn as a cord following the pointer.
    #[allow(clippy::type_complexity)] // node id, (port), (cursor) — documented above
    pub(crate) wiring: Option<(i32, (usize, super::patch::Side, usize), (f32, f32))>,
}

impl Default for FrameInputs<'_> {
    fn default() -> Self {
        // 'static empties for the no-transport case.
        static EMPTY: std::sync::OnceLock<HashMap<i32, NodeTree>> = std::sync::OnceLock::new();
        static NO_GROUPS: std::sync::OnceLock<TimelineGroups> = std::sync::OnceLock::new();
        Self {
            bus: None,
            node_trees: EMPTY.get_or_init(HashMap::new),
            active_button: None,
            focused_text: None,
            server_attached: false,
            sample_rate: 0.0,
            sample_clock: 0.0,
            cursor: None,
            timelines: NO_GROUPS.get_or_init(TimelineGroups::default),
            wiring: None,
            marquee: None,
        }
    }
}

/// The navigation window a placed timeline view draws: its group's shared
/// window, or the full extent of its own data when it is in no group yet (the
/// defensive fallback; `total` is the slot's sample count).
fn nav_for(inputs: &FrameInputs, item: &TimelineItem, total: usize) -> View {
    inputs
        .timelines
        .nav(group_key(item.id, item.editor.link))
        .unwrap_or_else(|| View::full(total))
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

/// The current value of control bus `bus` from `source` (`0.0` without a source
/// or for a negative/out-of-range bus) — the same rule the native front used.
fn read_bus(source: Option<&dyn BusSource>, bus: i32) -> f32 {
    if bus < 0 {
        return 0.0;
    }
    source.map_or(0.0, |s| s.control(bus as usize))
}

/// Maps sample position `s` into `body`'s x range through `nav`.
fn sample_to_x(s: f64, nav: &View, body: Rect) -> f32 {
    (body.x as f64 + (s - nav.start) / nav.len * body.w as f64) as f32
}

/// The lane sub-rectangle `ch` of `lanes` inside `body` (stacked top to
/// bottom, no gap — the divider line is overlay chrome).
pub(crate) fn lane_rect(body: Rect, lanes: usize, ch: usize) -> Rect {
    let lanes = lanes.max(1) as f32;
    let h = body.h / lanes;
    Rect::new(body.x, body.y + ch as f32 * h, body.w, h)
}

/// The time-ruler unit of `editor` (the beats grid rides its props).
fn time_unit(editor: &EditorProps) -> TimeUnit {
    match editor.ruler {
        Ruler::Samples => TimeUnit::Samples,
        Ruler::Beats => TimeUnit::Beats {
            tempo: editor.tempo,
            beat_at: editor.beat_at,
            quant: editor.quant,
        },
        _ => TimeUnit::Seconds,
    }
}

/// Draws the time-ruler strip under `body` for the visible `nav` window
/// (aligned with the body, so its ticks sit under the samples they label even
/// when a vertical ruler indents the body).
fn draw_time_ruler(
    mesh: &mut Mesh,
    rect: Rect,
    body: Rect,
    nav: &View,
    rate: f64,
    editor: &EditorProps,
    theme: &Theme,
) {
    if editor.ruler == Ruler::Off {
        return;
    }
    let strip = Rect::new(body.x, body.y + body.h, body.w, (rect.h - body.h).max(0.0));
    if strip.h <= 2.0 || strip.w <= 0.0 {
        return;
    }
    let ticks = ruler::time_ticks(nav.start, nav.len, strip.w as f64, rate, time_unit(editor));
    ruler::draw_ticks_h(mesh, strip, &ticks, theme);
}

/// The visible MIDI pitch window `[lo, hi]` of a piano-roll: the widget's
/// `[min, max]` axis sliced by the vertical display window (`y_start`/`y_len`,
/// `0` = the low pitch at the bottom), so pitch zoom/pan holds the same way the
/// heavy views' amplitude/frequency windows do.
fn pitch_window(item: &PianoRollItem) -> (f32, f32) {
    let (y0, yl) = item.editor.y_view();
    let span = (item.max - item.min) as f64;
    let lo = item.min as f64 + y0 * span;
    let hi = item.min as f64 + (y0 + yl) * span;
    (lo as f32, hi as f32)
}

/// Draws a `pianoroll`: keyboard gutter, note grid, the velocity/OSC lanes and
/// the time ruler into `mesh`; the selection band and the playhead into `over`.
/// Everything rides the shared `nav` window, so it zooms/pans/plays in lockstep
/// with linked sibling views.
#[allow(clippy::too_many_arguments)] // one view's flat draw inputs
fn draw_pianoroll_item(
    mesh: &mut Mesh,
    over: &mut Mesh,
    item: &PianoRollItem,
    nav: &View,
    rate: f64,
    sample_clock: f64,
    cursor: Option<(f64, f64)>,
    theme: &Theme,
) {
    let ruler_on = item.editor.ruler != Ruler::Off;
    let r = pianoroll::regions(item.rect, ruler_on, item.osc_lane, item.velocity_lane);
    let (lo, hi) = pitch_window(item);
    pianoroll::draw_grid_background(mesh, r.grid, lo, hi, theme);
    pianoroll::draw_notes(
        mesh,
        r.grid,
        nav,
        0.0,
        &item.notes,
        lo,
        hi,
        true,
        &item.selected,
        theme,
    );
    pianoroll::draw_keyboard(mesh, r.keyboard, lo, hi, theme);
    if item.osc_lane {
        pianoroll::draw_osc_lane(mesh, r.osc, nav, 0.0, &item.osc, theme);
    }
    if item.velocity_lane {
        pianoroll::draw_velocity_lane(mesh, r.velocity, nav, 0.0, &item.notes, theme);
    }
    if let Some(t) = &item.label {
        super::font::text(
            mesh,
            t,
            r.grid.x + 4.0,
            r.grid.y + 2.0,
            RULER_SCALE,
            theme.ruler_text,
        );
    }
    if ruler_on {
        // The ruler strip sits under the grid, aligned to the grid's x-range —
        // build the "body" `draw_time_ruler` derives the strip from.
        let ruler_body = Rect::new(r.grid.x, item.rect.y, r.grid.w, r.ruler.y - item.rect.y);
        draw_time_ruler(mesh, item.rect, ruler_body, nav, rate, &item.editor, theme);
    }
    // Selection band over the grid.
    if let Some((start, len)) = item.editor.selection() {
        let x0 = sample_to_x(start, nav, r.grid).clamp(r.grid.x, r.grid.x + r.grid.w);
        let x1 = sample_to_x(start + len, nav, r.grid).clamp(r.grid.x, r.grid.x + r.grid.w);
        if x1 > x0 {
            over.rect(
                Rect::new(x0, r.grid.y, x1 - x0, r.grid.h),
                with_alpha(theme.selection, 0.18),
            );
            over.rect(
                Rect::new(x0, r.grid.y, 1.0, r.grid.h),
                with_alpha(theme.selection, 0.75),
            );
            over.rect(
                Rect::new(x1 - 1.0, r.grid.y, 1.0, r.grid.h),
                with_alpha(theme.selection, 0.75),
            );
        }
    }
    // Playhead: swept by the engine clock while playing, else the static cursor.
    let head = if item.editor.playhead_at >= 0.0 && sample_clock > 0.0 {
        Some(sample_clock - item.editor.playhead_at)
    } else if item.editor.playhead >= 0.0 {
        Some(item.editor.playhead)
    } else {
        None
    };
    if let Some(pos) = head
        && pos >= nav.start
        && pos <= nav.start + nav.len
    {
        let x = sample_to_x(pos, nav, r.grid);
        over.rect(Rect::new(x, r.grid.y, 1.5, r.grid.h), theme.playhead);
    }
    // Cursor readout: the note name (the pitch under the cursor, via the core's
    // MIDI-note spelling) and the time (per the ruler mode), in the grid's
    // bottom-right corner — pure math over the view mapping.
    if let Some((cx, cy)) = cursor
        && r.grid.contains(cx, cy)
    {
        let pitch = pianoroll::y_to_pitch(cy as f32, lo, hi, r.grid).round() as i32;
        let s = nav.start + nav.len * ((cx - r.grid.x as f64) / r.grid.w.max(1.0) as f64);
        let time = match item.editor.ruler {
            Ruler::Samples => ruler::readout_samples(s),
            Ruler::Beats => ruler::readout_beats(
                s,
                rate,
                item.editor.tempo,
                item.editor.beat_at,
                item.editor.quant,
                nav.len / rate * item.editor.tempo / r.grid.w.max(1.0) as f64,
            ),
            _ => ruler::readout_time(s, rate, nav.len / rate / r.grid.w.max(1.0) as f64),
        };
        let text = format!("{}  {time}", clausters_core::scale::note_name(pitch));
        let w = super::font::width(&text, RULER_SCALE);
        super::font::text(
            over,
            &text,
            r.grid.x + r.grid.w - w - 4.0,
            r.grid.y + r.grid.h - super::font::height(RULER_SCALE) - 2.0,
            RULER_SCALE,
            theme.ruler_text,
        );
    }
}

/// Draws the selection overlay and playhead of one timeline view, plus its
/// cursor readout when the pointer is inside the body. `lanes` is the lane
/// count of the stacked layout (1 when overlaid), so the vertical readout is
/// computed within the lane under the cursor.
#[allow(clippy::too_many_arguments)] // one chrome pass, all inputs by value
fn draw_editor_overlay(
    mesh: &mut Mesh,
    item: &TimelineItem,
    body: Rect,
    nav: &View,
    rate: f64,
    lanes: usize,
    inputs: &FrameInputs,
    nyquist_scale: Option<(f64, FreqScale, f64)>,
    theme: &Theme,
) {
    mesh.border(body, 1.0, theme.view_frame);
    // Selection: a translucent band with hard edges, clipped to the body.
    if let Some((start, len)) = item.editor.selection() {
        let x0 = sample_to_x(start, nav, body).clamp(body.x, body.x + body.w);
        let x1 = sample_to_x(start + len, nav, body).clamp(body.x, body.x + body.w);
        if x1 > x0 {
            mesh.rect(
                Rect::new(x0, body.y, x1 - x0, body.h),
                with_alpha(theme.selection, 0.18),
            );
            mesh.rect(
                Rect::new(x0, body.y, 1.0, body.h),
                with_alpha(theme.selection, 0.75),
            );
            mesh.rect(
                Rect::new(x1 - 1.0, body.y, 1.0, body.h),
                with_alpha(theme.selection, 0.75),
            );
        }
    }
    // Playhead: the engine clock relative to the widget's origin while playing,
    // else the static cursor of a located, stopped transport.
    let head = if item.editor.playhead_at >= 0.0 && inputs.sample_clock > 0.0 {
        Some(inputs.sample_clock - item.editor.playhead_at)
    } else if item.editor.playhead >= 0.0 {
        Some(item.editor.playhead)
    } else {
        None
    };
    if let Some(pos) = head
        && pos >= nav.start
        && pos <= nav.start + nav.len
    {
        let x = sample_to_x(pos, nav, body);
        mesh.rect(Rect::new(x, body.y, 1.5, body.h), theme.playhead);
    }
    // Cursor readout: time (per the ruler mode) plus value/frequency (per the
    // vertical unit / frequency scale), in the body's bottom-right corner —
    // pure math over the view mapping, within the lane under the cursor.
    if let Some((cx, cy)) = inputs.cursor
        && body.contains(cx, cy)
    {
        let s = nav.start + nav.len * ((cx - body.x as f64) / body.w.max(1.0) as f64);
        let editor = &item.editor;
        let time = match editor.ruler {
            Ruler::Samples => ruler::readout_samples(s),
            Ruler::Beats => {
                let beats_per_px = nav.len / rate * editor.tempo / body.w.max(1.0) as f64;
                ruler::readout_beats(
                    s,
                    rate,
                    editor.tempo,
                    editor.beat_at,
                    editor.quant,
                    beats_per_px,
                )
            }
            _ => ruler::readout_time(s, rate, nav.len / rate / body.w.max(1.0) as f64),
        };
        let lane = lane_rect(body, lanes.max(1), lane_at(body, lanes.max(1), cy));
        let rel = ((cy - lane.y as f64) / lane.h.max(1.0) as f64).clamp(0.0, 1.0);
        // The cursor's height mapped through the visible vertical window into
        // an absolute display coordinate (0 = axis bottom) — so the readout
        // names exactly what is under the cursor at any vertical zoom/pan.
        let (y0, y_len) = editor.y_view();
        let display = y0 + (1.0 - rel) * y_len;
        let text = match nyquist_scale {
            // Spectrogram: invert the shader's display→bin mapping at the
            // cursor's height for the frequency under it.
            Some((nyquist, scale, f_lo)) => {
                let f = ruler::display_to_hz(display, nyquist, scale, f_lo);
                format!("{time}  {} HZ", f.round() as i64)
            }
            // Waveform: the amplitude at the cursor's height within its lane,
            // in the vertical ruler's unit.
            None => {
                let amp = (2.0 * display - 1.0) / crate::waveform::AMP_MARGIN as f64;
                let amp = amp.clamp(-1.0, 1.0);
                let amp_per_px =
                    2.0 * y_len / crate::waveform::AMP_MARGIN as f64 / lane.h.max(1.0) as f64;
                let value = ruler::readout_amp(amp, editor.ruler_y, editor.bit_depth, amp_per_px);
                format!("{time}  {value}")
            }
        };
        let w = super::font::width(&text, RULER_SCALE);
        let x = (body.x + body.w - w - 4.0).max(body.x);
        let y = body.y + body.h - super::font::height(RULER_SCALE) - 3.0;
        super::font::text(
            mesh,
            &text,
            x,
            y.max(body.y),
            RULER_SCALE,
            with_alpha(theme.text, 0.9),
        );
    }
}

/// The stacked-lane index under window y `cy` (clamped into range).
pub(crate) fn lane_at(body: Rect, lanes: usize, cy: f64) -> usize {
    let rel = ((cy - body.y as f64) / body.h.max(1.0) as f64).clamp(0.0, 1.0);
    ((rel * lanes as f64) as usize).min(lanes.saturating_sub(1))
}

/// Renders `tree` into `gpu`'s surface, using the window's `painter`/`overlay`
/// (chrome under and over the heavy views), the `waveforms`/`spectrograms`/
/// `canvases` GPU resources and (read-only) `scopes` histories, plus `inputs`
/// for the live values. One immutable mesh-building pass over the placed
/// widgets, then the GPU uploads and the single render pass.
#[allow(clippy::too_many_arguments)] // the per-window resource set, both fronts
pub(crate) fn render(
    gpu: &mut Gpu,
    painter: &mut Painter,
    overlay: &mut Painter,
    waveforms: &mut HashMap<i32, WaveformSlot>,
    spectrograms: &mut HashMap<i32, SpectrogramSlot>,
    canvases: &mut HashMap<i32, CanvasView>,
    scopes: &HashMap<i32, VecDeque<f32>>,
    tap_windows: &HashMap<i32, live::TapWindow>,
    spectra: &HashMap<i32, Vec<SpectrumState>>,
    tree: &Widget,
    inputs: &FrameInputs,
    theme: &Theme,
) {
    let (fb_w, fb_h) = (gpu.config.width.max(1), gpu.config.height.max(1));
    let area = Rect::new(0.0, 0.0, fb_w as f32, fb_h as f32);
    let placed = layout::layout(area, tree);
    let mut mesh = Mesh::new();
    let mut over = Mesh::new();
    let mut timeline_items: Vec<TimelineItem> = Vec::new();
    // Meter/scope rects, copied out so their shared-memory values and the scope
    // history can be read after the host-tree borrow is released.
    let mut meter_rects: Vec<MeterItem> = Vec::new();
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
    let mut bpf_rects: Vec<BpfItem> = Vec::new();
    let mut track_items: Vec<TrackItem> = Vec::new();
    let mut pianoroll_items: Vec<PianoRollItem> = Vec::new();
    let mut nodetree_rects: Vec<NodeTreeItem> = Vec::new();
    let mut canvas_frames: Vec<CanvasFrame> = Vec::new();
    let active_button = inputs.active_button;
    for p in &placed {
        // Everything a scrolled widget paints clips to its container's area.
        mesh.set_clip(p.clip);
        // The widget's resolved theme (a theme group's overlay, a `color`
        // accent), resolved at mutation points -- one reference per widget.
        let th = p.widget.theme.as_deref().unwrap_or(theme);
        match &p.widget.kind {
            WidgetKind::Panel { .. } | WidgetKind::Scroll { .. } => mesh.rect(p.rect, th.panel),
            WidgetKind::Label {
                text,
                text_size,
                wrap,
                align,
            } => {
                controls::draw_label(
                    &mut mesh,
                    text,
                    p.rect,
                    *text_size * p.scale,
                    *wrap,
                    *align,
                    th,
                );
            }
            WidgetKind::Waveform {
                overlay, editor, ..
            } => {
                if let Some(id) = p.widget.id {
                    timeline_items.push(TimelineItem {
                        id,
                        rect: p.rect,
                        clip: p.clip,
                        theme: p.widget.theme.clone(),
                        kind: TimelineKind::Waveform { overlay: *overlay },
                        editor: editor.clone(),
                    });
                }
            }
            WidgetKind::Spectrogram {
                db_floor,
                db_ceil,
                freq_scale,
                colormap,
                editor,
                ..
            } => {
                if let Some(id) = p.widget.id {
                    timeline_items.push(TimelineItem {
                        id,
                        rect: p.rect,
                        clip: p.clip,
                        theme: p.widget.theme.clone(),
                        kind: TimelineKind::Spectrogram {
                            db_floor: *db_floor,
                            db_ceil: *db_ceil,
                            freq_scale: *freq_scale,
                            colormap: *colormap,
                        },
                        editor: editor.clone(),
                    });
                }
            }
            WidgetKind::Meter {
                bus,
                min,
                max,
                label,
            } => meter_rects.push(MeterItem {
                rect: p.rect,
                clip: p.clip,
                theme: p.widget.theme.clone(),
                bus: *bus,
                min: *min,
                max: *max,
                label: label.clone(),
            }),
            WidgetKind::Scope {
                tap,
                overlay,
                window_ms,
                trigger,
                min,
                max,
                ruler,
                ruler_y,
                label,
                ..
            } => {
                if let Some(id) = p.widget.id {
                    if *tap >= 0 {
                        wave_rects.push(WaveItem {
                            id,
                            rect: p.rect,
                            clip: p.clip,
                            theme: p.widget.theme.clone(),
                            min: *min,
                            max: *max,
                            window_ms: *window_ms,
                            trigger: *trigger,
                            overlay: *overlay,
                            ruler: *ruler,
                            ruler_y: *ruler_y,
                            label: label.clone(),
                        });
                    } else {
                        scope_rects.push(ScopeItem {
                            id,
                            rect: p.rect,
                            clip: p.clip,
                            theme: p.widget.theme.clone(),
                            min: *min,
                            max: *max,
                            label: label.clone(),
                        });
                    }
                }
            }
            WidgetKind::Phasescope { label, .. } => {
                if let Some(id) = p.widget.id {
                    phase_rects.push(PhaseItem {
                        id,
                        rect: p.rect,
                        clip: p.clip,
                        theme: p.widget.theme.clone(),
                        label: label.clone(),
                    });
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
                &mut mesh,
                p.rect,
                *min,
                *max,
                *overview,
                *active_min,
                *active_max,
                pressed,
                label.as_deref(),
                th,
            ),
            WidgetKind::Spectrum {
                fft_size,
                db_floor,
                db_ceil,
                freq_scale,
                peak_hold,
                ruler,
                ruler_y,
                label,
                ..
            } => {
                if let Some(id) = p.widget.id {
                    spectrum_rects.push(SpectrumItem {
                        id,
                        rect: p.rect,
                        clip: p.clip,
                        theme: p.widget.theme.clone(),
                        fft_size: *fft_size,
                        db_floor: *db_floor,
                        db_ceil: *db_ceil,
                        freq_scale: *freq_scale,
                        peak_hold: *peak_hold,
                        ruler: *ruler,
                        ruler_y: *ruler_y,
                        label: label.clone(),
                    });
                }
            }
            WidgetKind::Plot {
                samples,
                channels,
                view,
                overlay,
                sample_rate,
                min,
                max,
                ruler,
                ruler_y,
                spectrum,
                db_floor,
                db_ceil,
                freq_scale,
                label,
                ..
            } => plot_rects.push(PlotItem {
                rect: p.rect,
                clip: p.clip,
                theme: p.widget.theme.clone(),
                samples: Arc::clone(samples),
                channels: *channels,
                view: *view,
                overlay: *overlay,
                sample_rate: *sample_rate,
                min: *min,
                max: *max,
                ruler: *ruler,
                ruler_y: *ruler_y,
                spectrum: spectrum.clone(),
                db_floor: *db_floor,
                db_ceil: *db_ceil,
                freq_scale: *freq_scale,
                label: label.clone(),
            }),
            WidgetKind::Bpf {
                points,
                min,
                max,
                duration,
                exp,
                label,
            } => bpf_rects.push(BpfItem {
                rect: p.rect,
                clip: p.clip,
                theme: p.widget.theme.clone(),
                points: points.clone(),
                min: *min,
                max: *max,
                duration: *duration,
                exp: *exp,
                label: label.clone(),
            }),
            WidgetKind::Track { label, editor, .. } => {
                // A track carries its clips as children (not laid out by the
                // layout engine — they are placed by offset/dur on the shared
                // time axis in the overlay pass below).
                let clips = p
                    .widget
                    .children
                    .iter()
                    .filter_map(track::clip_draw)
                    .collect();
                track_items.push(TrackItem {
                    id: p.widget.id.unwrap_or(-1),
                    rect: p.rect,
                    clip: p.clip,
                    theme: p.widget.theme.clone(),
                    label: label.clone(),
                    clips,
                    editor: editor.clone(),
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
                    &mut mesh,
                    p.rect,
                    patch,
                    label.as_deref(),
                    &patch::CanvasState {
                        live,
                        selected,
                        marquee,
                        scale: p.scale,
                    },
                    th,
                );
            }
            WidgetKind::NodeTree {
                group,
                controls,
                label,
            } => nodetree_rects.push(NodeTreeItem {
                rect: p.rect,
                clip: p.clip,
                theme: p.widget.theme.clone(),
                group: *group,
                controls: *controls,
                label: label.clone(),
            }),
            WidgetKind::Canvas {
                shader,
                params,
                buses,
                label,
            } => {
                if let Some(id) = p.widget.id {
                    if let Some(text) = label {
                        super::font::text(
                            &mut mesh,
                            text,
                            p.rect.x + 4.0,
                            p.rect.y + 4.0,
                            LABEL_SCALE,
                            th.text,
                        );
                    }
                    // Resolve the param vector: a `-1` slot keeps its script-set
                    // value; a bus slot is read from shared memory this frame
                    // (zero messages, like a meter).
                    let mut resolved = *params;
                    for (slot, &bus) in resolved.iter_mut().zip(buses.iter()) {
                        if bus >= 0 {
                            *slot = read_bus(inputs.bus, bus);
                        }
                    }
                    canvas_frames.push(CanvasFrame {
                        id,
                        body: controls::body_rect(p.rect, label.is_some()),
                        clip: p.clip,
                        shader: shader.clone(),
                        params: resolved,
                    });
                }
            }
            WidgetKind::Score(data) => {
                // Notation tessellates straight into the shared triangle mesh:
                // a paper panel under the engraving, glyphs and fills in ink,
                // the playback cursor over it in the playhead accent.
                mesh.rect(p.rect, th.panel);
                data.render(&mut mesh, p.rect, p.clip, th.text, th.playhead);
            }
            WidgetKind::Window { .. } | WidgetKind::Unknown(_) => {}
            kind => controls::draw(
                &mut mesh,
                kind,
                p.rect,
                p.widget.id == active_button,
                p.widget.id.is_some() && p.widget.id == inputs.focused_text,
                p.scale,
                th,
            ),
        }
    }

    // Meters and scopes read their control bus straight from shared memory each
    // frame (zero messages); the scope keeps a per-widget rolling history in this
    // window's state.
    for item in &meter_rects {
        mesh.set_clip(item.clip);
        let th = item.theme.as_deref().unwrap_or(theme);
        let value = read_bus(inputs.bus, item.bus);
        let frac = meters::fraction(value, item.min, item.max);
        meters::draw_meter(&mut mesh, item.rect, value, frac, item.label.as_deref(), th);
    }
    // The history is advanced on the frame tick (`advance_scopes`), not here, so a
    // repaint only ever *draws* the current samples — never adds one.
    for item in &scope_rects {
        mesh.set_clip(item.clip);
        let th = item.theme.as_deref().unwrap_or(theme);
        let samples: Vec<f32> = scopes
            .get(&item.id)
            .map(|h| h.iter().copied().collect())
            .unwrap_or_default();
        meters::draw_scope(
            &mut mesh,
            item.rect,
            &samples,
            item.min,
            item.max,
            item.label.as_deref(),
            th,
        );
    }
    // Audio-rate scopes likewise draw the triggered multichannel window stored
    // on the tick (`live::update_tap_windows`); an empty one draws just the
    // framed field.
    let empty_window = live::TapWindow::default();
    for item in &wave_rects {
        mesh.set_clip(item.clip);
        let th = item.theme.as_deref().unwrap_or(theme);
        let window = tap_windows.get(&item.id).unwrap_or(&empty_window);
        meters::draw_wave(
            &mut mesh,
            item.rect,
            &meters::WaveParams {
                window,
                min: item.min,
                max: item.max,
                window_ms: item.window_ms,
                trigger: item.trigger,
                overlay: item.overlay,
                ruler: item.ruler,
                ruler_y: item.ruler_y,
                label: item.label.as_deref(),
            },
            th,
        );
    }
    // Phasescopes draw the interleaved L/R window the tick stored (the same
    // `tap_windows` map, keyed by their own ids); spectra draw the per-bin
    // curves the tick folded into their per-channel analysis states.
    for item in &phase_rects {
        mesh.set_clip(item.clip);
        let th = item.theme.as_deref().unwrap_or(theme);
        let inter = tap_windows
            .get(&item.id)
            .map(|w| w.samples.as_slice())
            .unwrap_or(&[]);
        phasescope::draw_phasescope(&mut mesh, item.rect, inter, item.label.as_deref(), th);
    }
    for item in &spectrum_rects {
        mesh.set_clip(item.clip);
        let th = item.theme.as_deref().unwrap_or(theme);
        if let Some(states) = spectra.get(&item.id) {
            spectrum::draw_spectrum(
                &mut mesh,
                item.rect,
                states,
                &spectrum::SpectrumParams {
                    sample_rate: inputs.sample_rate,
                    fft_size: item.fft_size,
                    db_floor: item.db_floor,
                    db_ceil: item.db_ceil,
                    freq_scale: item.freq_scale,
                    peak_hold: item.peak_hold,
                    ruler: item.ruler,
                    ruler_y: item.ruler_y,
                    label: item.label.as_deref(),
                },
                th,
            );
        }
    }

    // Timeline views (waveform/spectrogram): the field, time ruler and the
    // vertical-ruler strip go into the base mesh (under the GPU view); the
    // border, lane dividers, selection, playhead and cursor readout into the
    // overlay mesh (over it).
    for item in &timeline_items {
        mesh.set_clip(item.clip);
        over.set_clip(item.clip);
        let th = item.theme.as_deref().unwrap_or(theme);
        let body = timeline_body(item.rect, &item.editor);
        mesh.rect(body, th.view_field);
        match &item.kind {
            TimelineKind::Waveform { overlay: overlaid } => {
                let Some(slot) = waveforms.get(&item.id) else {
                    over.border(body, 1.0, th.view_frame);
                    continue;
                };
                let nav = nav_for(inputs, item, slot.view.total_samples());
                let rate = if item.editor.sample_rate > 0.0 {
                    item.editor.sample_rate
                } else {
                    inputs.sample_rate
                };
                draw_time_ruler(&mut mesh, item.rect, body, &nav, rate, &item.editor, th);
                let lanes = slot.view.num_channels();
                // Overlaid traces share one lane (and one amplitude axis).
                let draw_lanes = if *overlaid { 1 } else { lanes };
                if item.editor.ruler_y != RulerY::Off {
                    let (y0, y_len) = item.editor.y_view();
                    for ch in 0..draw_lanes {
                        let lane = lane_rect(body, draw_lanes, ch);
                        let ticks = ruler::amp_ticks(
                            item.editor.ruler_y,
                            lane.h as f64,
                            item.editor.bit_depth,
                            y0,
                            y_len,
                        );
                        ruler::draw_ticks_v(&mut mesh, body.x, item.rect.x, lane, &ticks, th);
                    }
                }
                for ch in 1..draw_lanes {
                    let lane = lane_rect(body, draw_lanes, ch);
                    over.rect(Rect::new(lane.x, lane.y, lane.w, 1.0), th.lane_divider);
                }
                draw_editor_overlay(
                    &mut over, item, body, &nav, rate, draw_lanes, inputs, None, th,
                );
            }
            TimelineKind::Spectrogram { freq_scale, .. } => {
                let Some(slot) = spectrograms.get(&item.id) else {
                    over.border(body, 1.0, th.view_frame);
                    continue;
                };
                let nav = nav_for(inputs, item, slot.total_samples());
                let (nyquist, f_lo) = slot
                    .views
                    .first()
                    .map(|v| (v.stft().nyquist() as f64, v.log_floor() as f64))
                    .unwrap_or((24_000.0, 20.0 / 24_000.0));
                let rate = if item.editor.sample_rate > 0.0 {
                    item.editor.sample_rate
                } else {
                    nyquist * 2.0
                };
                draw_time_ruler(&mut mesh, item.rect, body, &nav, rate, &item.editor, th);
                let lanes = slot.views.len();
                for ch in 0..lanes {
                    let lane = lane_rect(body, lanes, ch);
                    if ch > 0 {
                        over.rect(Rect::new(lane.x, lane.y, lane.w, 1.0), th.lane_divider);
                    }
                    if item.editor.ruler_y != RulerY::Off {
                        let ticks = ruler::hz_ticks(
                            nyquist,
                            *freq_scale,
                            f_lo,
                            lane.h as f64,
                            item.editor.y_view().0,
                            item.editor.y_view().1,
                        );
                        ruler::draw_ticks_v(&mut mesh, body.x, item.rect.x, lane, &ticks, th);
                    }
                }
                // The active scale, named over the view (the live views'
                // corner slot) — log/mel/bark are not tellable apart from
                // the tick spacing at a glance.
                meters::value_text(&mut over, ruler::scale_tag(*freq_scale), body, th);
                draw_editor_overlay(
                    &mut over,
                    item,
                    body,
                    &nav,
                    rate,
                    lanes,
                    inputs,
                    Some((nyquist, *freq_scale, f_lo)),
                    th,
                );
            }
        }
    }

    // Static plots draw from their (already mapped) samples; node trees draw from
    // the model last read off the client leg. Both are pure mesh work with the
    // host-tree borrow already released.
    for item in &plot_rects {
        mesh.set_clip(item.clip);
        over.set_clip(item.clip);
        let th = item.theme.as_deref().unwrap_or(theme);
        let params = item.params();
        plot::draw(&mut mesh, item.rect, &params, th);
        // The hover readout (hairline + the value under the cursor) rides the
        // overlay mesh, like the editor views' chrome.
        if let Some(cursor) = inputs.cursor {
            plot::draw_readout(&mut over, item.rect, &params, cursor, th);
        }
    }
    // Envelope editors are pure mesh work: the curve evaluated per pixel
    // column through the shared shape math, discs for the breakpoints.
    for item in &bpf_rects {
        mesh.set_clip(item.clip);
        let th = item.theme.as_deref().unwrap_or(theme);
        bpf::draw(
            &mut mesh,
            item.rect,
            &item.points,
            item.min,
            item.max,
            item.duration,
            item.exp,
            item.label.as_deref(),
            th,
        );
    }
    for item in &nodetree_rects {
        mesh.set_clip(item.clip);
        let th = item.theme.as_deref().unwrap_or(theme);
        nodetree::draw(
            &mut mesh,
            item.rect,
            inputs.node_trees.get(&item.group),
            item.controls,
            item.label.as_deref(),
            inputs.server_attached,
            th,
        );
    }
    // Multitrack lanes: the window's tracks share one time axis (aligned
    // lanes), spanning the longest clip end; each lane's clips are placed on it.
    // The hit-test (`interact::clip_hit`) reads the same `window_nav`, so a clip
    // maps to the same pixels for drawing and dragging.
    if !track_items.is_empty() {
        // The lanes navigate as a group (linked by default across a window), so
        // the axis zooms and pans as one; the full span is the fallback for a
        // lane not yet in a group.
        let full = track::window_nav(tree);
        for item in &track_items {
            mesh.set_clip(item.clip);
            over.set_clip(item.clip);
            let th = item.theme.as_deref().unwrap_or(theme);
            let nav = inputs
                .timelines
                .nav(group_key(item.id, item.editor.link))
                .unwrap_or(full);
            let ruler_on = item.editor.ruler != Ruler::Off;
            track::draw(
                &mut mesh,
                item.rect,
                &nav,
                item.label.as_deref(),
                &item.clips,
                ruler_on,
                th,
            );
            let body = track::lane_body(item.rect, ruler_on);
            // The lane's own time ruler, in the strip the lane body reserved —
            // the same tick math the timeline views use, over the shared axis.
            if ruler_on {
                let rate = if item.editor.sample_rate > 0.0 {
                    item.editor.sample_rate
                } else {
                    inputs.sample_rate
                };
                draw_time_ruler(&mut mesh, item.rect, body, &nav, rate, &item.editor, th);
            }
            // The playhead, over the clips: the engine clock as a timeline
            // position (`playhead_at` anchors timeline sample 0 to a clock
            // value), so it sweeps the lane as the composition plays.
            let pos = if item.editor.playhead_at >= 0.0 && inputs.sample_clock > 0.0 {
                // Playing: the line is the engine clock, and it sweeps.
                Some(inputs.sample_clock - item.editor.playhead_at)
            } else if item.editor.playhead >= 0.0 {
                // Located and stopped: the cursor stands where it was put.
                Some(item.editor.playhead)
            } else {
                None
            };
            if let Some(pos) = pos
                && let Some(x) = track::playhead_x(body, &nav, pos)
            {
                over.rect(Rect::new(x, body.y, 1.5, body.h), th.playhead);
            }
        }
    }
    // Piano-roll views: flat geometry (keyboard/grid/lanes/ruler) into the base
    // mesh, selection/playhead into the overlay. Each draws through its
    // navigation group's shared window (a linked pianoroll zooms/pans with its
    // siblings), falling back to its own content extent when in no group.
    for item in &pianoroll_items {
        mesh.set_clip(item.clip);
        over.set_clip(item.clip);
        let th = item.theme.as_deref().unwrap_or(theme);
        let nav = inputs
            .timelines
            .nav(group_key(item.id, item.editor.link))
            .unwrap_or_else(|| {
                let mut span = 0.0f64;
                for n in &item.notes {
                    span = span.max(n.start + n.dur);
                }
                for m in &item.osc {
                    span = span.max(m.time);
                }
                View::full(span.ceil().max(1.0) as usize)
            });
        let rate = if item.editor.sample_rate > 0.0 {
            item.editor.sample_rate
        } else {
            inputs.sample_rate
        };
        draw_pianoroll_item(
            &mut mesh,
            &mut over,
            item,
            &nav,
            rate,
            inputs.sample_clock,
            inputs.cursor,
            th,
        );
    }

    mesh.set_clip(None);
    over.set_clip(None);
    painter.upload(&gpu.device, &gpu.queue, &mesh, fb_w, fb_h);
    overlay.upload(&gpu.device, &gpu.queue, &over, fb_w, fb_h);
    for item in &timeline_items {
        let body = timeline_body(item.rect, &item.editor);
        match &item.kind {
            TimelineKind::Waveform { .. } => {
                if let Some(slot) = waveforms.get_mut(&item.id) {
                    let nav = nav_for(inputs, item, slot.view.total_samples());
                    let nav = placed_nav(&nav, item.editor.offset);
                    slot.view
                        .set_amp_window(item.editor.y_view().0, item.editor.y_view().1);
                    let th = item.theme.as_deref().unwrap_or(theme);
                    slot.view
                        .set_palette([th.series_1, th.series_2, th.series_3, th.series_4]);
                    slot.view
                        .upload(&gpu.device, &gpu.queue, &nav, body.w.max(1.0) as u32);
                }
            }
            TimelineKind::Spectrogram {
                db_floor,
                db_ceil,
                freq_scale,
                colormap,
            } => {
                if let Some(slot) = spectrograms.get_mut(&item.id) {
                    let nav = nav_for(inputs, item, slot.total_samples());
                    let nav = placed_nav(&nav, item.editor.offset);
                    for view in &mut slot.views {
                        view.set_display(
                            *db_floor,
                            *db_ceil,
                            *freq_scale,
                            (*colormap).max(0) as u32,
                        );
                        view.set_freq_window(item.editor.y_view().0, item.editor.y_view().1);
                        view.upload(&gpu.device, &gpu.queue, &nav, body.w.max(1.0) as u32);
                    }
                }
            }
        }
    }
    // Recompile any canvas whose shader changed, then push its per-frame uniforms
    // (viewport size, elapsed time, resolved params).
    for frame in &canvas_frames {
        if let Some(view) = canvases.get_mut(&frame.id) {
            view.set_shader(&gpu.device, &frame.shader);
            let time = view.elapsed();
            let res = [frame.body.w.max(1.0), frame.body.h.max(1.0)];
            view.upload(&gpu.queue, res, time, frame.params);
        }
    }

    let frame = match gpu.surface.get_current_texture() {
        wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
        _ => {
            // No drawable this turn (outdated/timed-out surface — e.g. the
            // compositor stopped consuming a covered window's frames):
            // reconfigure and ask for another redraw, so the frame that was
            // requested is not silently dropped and the window never shows
            // stale state once it is presentable again.
            gpu.surface.configure(&gpu.device, &gpu.config);
            gpu.window.request_redraw();
            return;
        }
    };
    let target = frame
        .texture
        .create_view(&wgpu::TextureViewDescriptor::default());
    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("gui frame"),
        });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("gui pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(clear_color(theme)),
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        painter.draw(&mut pass);
        for item in &timeline_items {
            let body = timeline_body(item.rect, &item.editor);
            if body.w < 1.0 || body.h < 1.0 {
                continue;
            }
            if !apply_scissor(&mut pass, item.clip, fb_w, fb_h) {
                continue;
            }
            match &item.kind {
                TimelineKind::Waveform { overlay: overlaid } => {
                    let Some(slot) = waveforms.get(&item.id) else {
                        continue;
                    };
                    let lanes = slot.view.num_channels();
                    if *overlaid || lanes == 1 {
                        let (x, y, w, h) = clamp_viewport(body, fb_w, fb_h);
                        if w >= 1.0 && h >= 1.0 {
                            pass.set_viewport(x, y, w, h, 0.0, 1.0);
                            slot.view.draw(&mut pass);
                        }
                    } else {
                        for ch in 0..lanes {
                            let lane = lane_rect(body, lanes, ch);
                            let (x, y, w, h) = clamp_viewport(lane, fb_w, fb_h);
                            if w >= 1.0 && h >= 1.0 {
                                pass.set_viewport(x, y, w, h, 0.0, 1.0);
                                slot.view.draw_channel(&mut pass, ch);
                            }
                        }
                    }
                }
                TimelineKind::Spectrogram { .. } => {
                    let Some(slot) = spectrograms.get(&item.id) else {
                        continue;
                    };
                    let lanes = slot.views.len();
                    for (ch, view) in slot.views.iter().enumerate() {
                        let lane = lane_rect(body, lanes, ch);
                        let (x, y, w, h) = clamp_viewport(lane, fb_w, fb_h);
                        if w >= 1.0 && h >= 1.0 {
                            pass.set_viewport(x, y, w, h, 0.0, 1.0);
                            view.draw(&mut pass);
                        }
                    }
                }
            }
        }
        for frame in &canvas_frames {
            if frame.body.w >= 1.0
                && frame.body.h >= 1.0
                && let Some(view) = canvases.get(&frame.id)
                && apply_scissor(&mut pass, frame.clip, fb_w, fb_h)
            {
                let (x, y, w, h) = clamp_viewport(frame.body, fb_w, fb_h);
                pass.set_viewport(x, y, w, h, 0.0, 1.0);
                view.draw(&mut pass);
            }
        }
        // The editor chrome reads over the heavy views: reset the viewport
        // (and the scissor) to the full framebuffer first (the overlay mesh is
        // in window space, already geometry-clipped where it needed to be).
        pass.set_viewport(0.0, 0.0, fb_w as f32, fb_h as f32, 0.0, 1.0);
        pass.set_scissor_rect(0, 0, fb_w, fb_h);
        overlay.draw(&mut pass);
    }
    gpu.queue.submit(std::iter::once(encoder.finish()));
    // The winit present contract: lets winit attach the compositor frame
    // callback to this commit, so later `request_redraw`s are delivered (and
    // throttled) correctly — without it, Wayland redraw delivery can stall on
    // an unfocused or covered window until the compositor repaints it anyway.
    gpu.window.pre_present_notify();
    frame.present();
}

/// Applies a placed widget's clip as the pass scissor (the full framebuffer
/// when it has none), returning `false` when the clip is empty — the caller
/// skips the draw entirely. The heavy views draw through `set_viewport`, which
/// *positions* but does not cut; a scrolled view poking out of its `scroll`
/// container is cut by this scissor, the GPU sibling of the mesh's geometric
/// clip.
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

/// Clamps a widget rect to the framebuffer for `set_viewport` (which rejects a
/// viewport that leaves the attachment).
fn clamp_viewport(r: Rect, fb_w: u32, fb_h: u32) -> (f32, f32, f32, f32) {
    let x = r.x.clamp(0.0, fb_w as f32);
    let y = r.y.clamp(0.0, fb_h as f32);
    let w = r.w.min(fb_w as f32 - x).max(0.0);
    let h = r.h.min(fb_h as f32 - y).max(0.0);
    (x, y, w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor(ruler: Ruler, ruler_y: RulerY) -> EditorProps {
        EditorProps {
            ruler,
            ruler_y,
            sample_rate: 0.0,
            bit_depth: 16,
            tempo: 1.0,
            beat_at: 0.0,
            quant: 4.0,
            sel_start: 0.0,
            sel_len: 0.0,
            playhead_at: -1.0,
            playhead: -1.0,
            y_start: 0.0,
            y_len: 1.0,
            link: None,
            offset: 0.0,
        }
    }

    #[test]
    fn timeline_body_reserves_the_ruler_strips() {
        let rect = Rect::new(10.0, 10.0, 400.0, 200.0);
        // Both rulers on: the body loses the bottom strip and the left strip.
        let body = timeline_body(rect, &editor(Ruler::Time, RulerY::Norm));
        assert_eq!(body.h, 200.0 - RULER_H);
        assert_eq!(body.x, 10.0 + RULER_W);
        assert_eq!(body.w, 400.0 - RULER_W);
        // Each ruler is independently optional.
        let x_only = timeline_body(rect, &editor(Ruler::Time, RulerY::Off));
        assert_eq!((x_only.x, x_only.w), (10.0, 400.0));
        assert_eq!(x_only.h, 200.0 - RULER_H);
        let y_only = timeline_body(rect, &editor(Ruler::Off, RulerY::Hz));
        assert_eq!(y_only.h, 200.0);
        assert_eq!(y_only.x, 10.0 + RULER_W);
        assert_eq!(timeline_body(rect, &editor(Ruler::Off, RulerY::Off)), rect);
    }

    #[test]
    fn placed_nav_shifts_the_body_window_by_the_offset() {
        let nav = View {
            start: 100.0,
            len: 400.0,
        };
        // The un-placed default is the identity.
        assert_eq!(placed_nav(&nav, 0.0), nav);
        // A member placed at timeline sample 100 draws its data sample 0 there:
        // the local window starts one clip-length earlier.
        let placed = placed_nav(&nav, 100.0);
        assert_eq!((placed.start, placed.len), (0.0, 400.0));
        // Placing further right pushes the local window negative (data before
        // the visible origin) without changing the span.
        let placed = placed_nav(&nav, 250.0);
        assert_eq!((placed.start, placed.len), (-150.0, 400.0));
    }

    #[test]
    fn lane_at_picks_the_lane_under_the_cursor() {
        let body = Rect::new(0.0, 0.0, 400.0, 300.0);
        assert_eq!(lane_at(body, 3, 50.0), 0);
        assert_eq!(lane_at(body, 3, 150.0), 1);
        assert_eq!(lane_at(body, 3, 299.0), 2);
        assert_eq!(lane_at(body, 3, 1000.0), 2, "clamped");
    }

    #[test]
    fn lanes_split_the_body_evenly_and_share_x() {
        let body = Rect::new(0.0, 0.0, 400.0, 300.0);
        let a = lane_rect(body, 3, 0);
        let b = lane_rect(body, 3, 1);
        let c = lane_rect(body, 3, 2);
        assert_eq!(a.h, 100.0);
        assert_eq!((a.x, a.w), (b.x, b.w));
        assert_eq!(b.y, 100.0);
        assert_eq!(c.y + c.h, 300.0);
    }

    #[test]
    fn deinterleave_splits_frames_and_drops_the_partial_tail() {
        let flat = [1.0, -1.0, 2.0, -2.0, 3.0];
        let chans = deinterleave(&flat, 2);
        assert_eq!(chans, vec![vec![1.0, 2.0], vec![-1.0, -2.0]]);
        assert_eq!(deinterleave(&flat, 1).len(), 1);
        assert_eq!(deinterleave(&flat, 1)[0].len(), 5);
    }

    #[test]
    fn stft_lanes_cap_the_hop_for_long_buffers() {
        // A buffer long enough that hop 8 would exceed MAX_FRAMES: the hop is
        // raised so every lane fits the texture.
        let n = 200_000;
        let chan: Vec<f32> = (0..n).map(|i| (i as f32 * 0.01).sin()).collect();
        let lanes = stft_lanes(vec![chan], 256, 8, 48_000.0);
        assert_eq!(lanes.len(), 1);
        assert!(lanes[0].n_frames() <= crate::spectrogram::MAX_FRAMES);
        assert_eq!(lanes[0].total_samples(), n);
    }
}
