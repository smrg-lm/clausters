//! Live control-bus plumbing shared by the native and browser fronts.
//!
//! Meters, scopes and `canvas` bus parameters read control buses every
//! animation frame. Natively the values come from the shared-memory segment
//! ([`super::shm`], zero messages); in the browser they arrive as periodic
//! `/c_set` snapshots from the server's `/c_stream` subscription (the network
//! counterpart of the segment). Everything around that difference — which
//! buses a tree reads, how a scope's rolling history advances, how a window
//! decides it is animated — is platform-independent and lives here, so both
//! fronts share one implementation and only the [`BusSource`] fill differs.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use super::BusSource;
use super::widget::{Widget, WidgetKind};

/// Most recent control-bus samples a `scope` keeps and plots.
pub(crate) const SCOPE_HISTORY: usize = 512;

/// The `/c_stream` period the browser front subscribes with: the same ~30 fps
/// the animation tick runs at, so every frame paints a fresh snapshot.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))] // browser-front only
pub(crate) const STREAM_PERIOD_MS: i32 = 33;

/// Appends `(widget_id, bus)` for every **control-rate** `scope` in the tree,
/// so the frame tick can sample each one's bus into its rolling history (an
/// audio-rate scope reads a tap window instead — see [`collect_tap_scopes`]).
pub(crate) fn collect_scopes(widget: &Widget, out: &mut Vec<(i32, i32)>) {
    if let WidgetKind::Scope { bus, tap, .. } = &widget.kind
        && *tap < 0
        && let Some(id) = widget.id
    {
        out.push((id, *bus));
    }
    for child in &widget.children {
        collect_scopes(child, out);
    }
}

/// One audio-rate scope's per-tick read spec: which tap, how big a window,
/// where to trigger, and whether the trace is frozen.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct TapScope {
    pub widget_id: i32,
    pub tap: i32,
    pub window_ms: f32,
    pub trigger: f32,
    pub hold: bool,
}

/// Appends the [`TapScope`] of every audio-rate `scope` in the tree.
pub(crate) fn collect_tap_scopes(widget: &Widget, out: &mut Vec<TapScope>) {
    if let WidgetKind::Scope {
        tap,
        window_ms,
        trigger,
        hold,
        ..
    } = &widget.kind
        && *tap >= 0
        && let Some(id) = widget.id
    {
        out.push(TapScope {
            widget_id: id,
            tap: *tap,
            window_ms: *window_ms,
            trigger: *trigger,
            hold: *hold,
        });
    }
    for child in &widget.children {
        collect_tap_scopes(child, out);
    }
}

/// The distinct, sorted tap indices a tree reads live each frame — every
/// audio-rate scope, spectrum and phasescope (two taps each). The browser front
/// subscribes exactly this set with `/tap_stream`.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))] // browser-front only
pub(crate) fn collect_live_taps(widget: &Widget, out: &mut Vec<i32>) {
    let mut taps = Vec::new();
    widget.kind.taps_read(&mut taps);
    for tap in taps {
        if !out.contains(&tap) {
            out.push(tap);
        }
    }
    for child in &widget.children {
        collect_live_taps(child, out);
    }
    out.sort_unstable();
}

/// Whether a widget tree contains a live widget: a bus-backed meter/scope, or
/// any audio-tap consumer (scope/spectrum/phasescope), so the window animates.
pub(crate) fn tree_has_live_widget(widget: &Widget) -> bool {
    let mut taps = Vec::new();
    widget.kind.taps_read(&mut taps);
    widget.kind.live_bus().is_some()
        || !taps.is_empty()
        || widget.children.iter().any(tree_has_live_widget)
}

/// Refreshes the aligned display window of every audio-rate scope in `tree`,
/// once per animation tick. `read_raw` fills a raw window of tap samples
/// (newest at the end) from wherever the platform gets them — the shm segment
/// natively, the `/tap_data` store in the browser — returning `false` when no
/// data is available yet. The stored value per widget id is the triggered
/// display window the render then draws verbatim; a `hold` scope keeps its
/// last window.
pub(crate) fn update_tap_windows(
    tree: &Widget,
    sample_rate: f64,
    read_raw: impl Fn(i32, &mut [f32]) -> bool,
    windows: &mut HashMap<i32, Vec<f32>>,
) {
    let mut specs = Vec::new();
    collect_tap_scopes(tree, &mut specs);
    let mut raw = Vec::new();
    for spec in specs {
        if spec.hold {
            continue;
        }
        let display = super::oscil::display_frames(spec.window_ms, sample_rate);
        raw.resize(super::oscil::raw_frames(display), 0.0);
        if !read_raw(spec.tap, &mut raw) {
            continue;
        }
        let start = super::oscil::align(&raw, display, spec.trigger);
        let end = (start + display).min(raw.len());
        windows.insert(spec.widget_id, raw[start..end].to_vec());
    }
}

/// One phasescope's per-tick read spec: its two taps (left, right), how big a
/// window of pairs to keep, and whether the trace is frozen.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct PhaseScope {
    pub widget_id: i32,
    pub tap_l: i32,
    pub tap_r: i32,
    pub window_ms: f32,
    pub hold: bool,
}

/// Appends the [`PhaseScope`] of every `phasescope` in the tree.
pub(crate) fn collect_phase_scopes(widget: &Widget, out: &mut Vec<PhaseScope>) {
    if let WidgetKind::Phasescope {
        tap,
        tap2,
        window_ms,
        hold,
        ..
    } = &widget.kind
        && let Some(id) = widget.id
    {
        out.push(PhaseScope {
            widget_id: id,
            tap_l: *tap,
            tap_r: *tap2,
            window_ms: *window_ms,
            hold: *hold,
        });
    }
    for child in &widget.children {
        collect_phase_scopes(child, out);
    }
}

/// Refreshes each phasescope's interleaved `[l, r, l, r, …]` window (the same
/// `windows` map the oscilloscope uses — the widget ids do not collide) from the
/// two taps' newest samples, once per tick. `read_raw` fills a channel's newest
/// window (the shm rings natively, the `/tap_data` store in the browser); a
/// `hold` scope keeps its last window, and a channel with no data yet is
/// skipped. Unlike the oscilloscope there is no trigger — the goniometer shows
/// the freshest pairs directly.
pub(crate) fn update_phase_windows(
    tree: &Widget,
    sample_rate: f64,
    read_raw: impl Fn(i32, &mut [f32]) -> bool,
    windows: &mut HashMap<i32, Vec<f32>>,
) {
    let mut specs = Vec::new();
    collect_phase_scopes(tree, &mut specs);
    let (mut l, mut r) = (Vec::new(), Vec::new());
    for spec in specs {
        if spec.hold {
            continue;
        }
        let n = super::oscil::display_frames(spec.window_ms, sample_rate);
        l.resize(n, 0.0);
        r.resize(n, 0.0);
        if !read_raw(spec.tap_l, &mut l) || !read_raw(spec.tap_r, &mut r) {
            continue;
        }
        let mut inter = Vec::with_capacity(n * 2);
        for i in 0..n {
            inter.push(l[i]);
            inter.push(r[i]);
        }
        windows.insert(spec.widget_id, inter);
    }
}

/// One spectrum's per-tick read spec: its tap, FFT size and display smoothing.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct SpectrumSpec {
    pub widget_id: i32,
    pub tap: i32,
    pub fft_size: usize,
    pub averaging: f32,
    pub peak_hold: bool,
}

/// Appends the [`SpectrumSpec`] of every `spectrum` in the tree.
pub(crate) fn collect_spectra(widget: &Widget, out: &mut Vec<SpectrumSpec>) {
    if let WidgetKind::Spectrum {
        tap,
        fft_size,
        averaging,
        peak_hold,
        ..
    } = &widget.kind
        && let Some(id) = widget.id
    {
        out.push(SpectrumSpec {
            widget_id: id,
            tap: *tap,
            fft_size: *fft_size,
            averaging: *averaging,
            peak_hold: *peak_hold,
        });
    }
    for child in &widget.children {
        collect_spectra(child, out);
    }
}

/// Folds each spectrum's newest FFT window into its persistent
/// [`SpectrumState`], once per tick. `read_raw` fills a full FFT window of the
/// tap; a tap with no data yet leaves the state (and its curve) as it was.
pub(crate) fn update_spectra(
    tree: &Widget,
    read_raw: impl Fn(i32, &mut [f32]) -> bool,
    states: &mut HashMap<i32, super::spectrum::SpectrumState>,
) {
    let mut specs = Vec::new();
    collect_spectra(tree, &mut specs);
    let mut raw = Vec::new();
    for spec in specs {
        let state = states
            .entry(spec.widget_id)
            .or_insert_with(|| super::spectrum::SpectrumState::new(spec.fft_size));
        state.ensure_size(spec.fft_size);
        raw.resize(state.window_len(), 0.0);
        if read_raw(spec.tap, &mut raw) {
            state.update(&raw, spec.averaging, spec.peak_hold);
        }
    }
}

/// The largest raw tap window any of a tree's tap consumers needs — an
/// oscilloscope's `window_ms` (with the trigger slack), a phasescope's window,
/// or a spectrum's FFT size — so the browser's `/tap_stream` subscription is
/// sized to feed all three. Zero when the tree reads no taps.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))] // browser-front only
pub(crate) fn tap_stream_frames(tree: &Widget, sample_rate: f64) -> usize {
    let mut frames = 0usize;
    let mut scopes = Vec::new();
    collect_tap_scopes(tree, &mut scopes);
    for s in scopes {
        let display = super::oscil::display_frames(s.window_ms, sample_rate);
        frames = frames.max(super::oscil::raw_frames(display));
    }
    let mut phases = Vec::new();
    collect_phase_scopes(tree, &mut phases);
    for p in phases {
        frames = frames.max(super::oscil::display_frames(p.window_ms, sample_rate));
    }
    let mut spectra = Vec::new();
    collect_spectra(tree, &mut spectra);
    for s in spectra {
        frames = frames.max(s.fft_size);
    }
    frames
}

/// Whether a widget tree contains a `canvas` (so the window animates each frame).
pub(crate) fn tree_has_canvas(widget: &Widget) -> bool {
    matches!(widget.kind, WidgetKind::Canvas { .. }) || widget.children.iter().any(tree_has_canvas)
}

/// Pushes one sample into a scope's rolling history, capped at [`SCOPE_HISTORY`].
pub(crate) fn push_sample(history: &mut VecDeque<f32>, value: f32) {
    history.push_back(value);
    while history.len() > SCOPE_HISTORY {
        history.pop_front();
    }
}

/// Advances every `scope` history of one window's tree by one sample read from
/// `read` (called once per animation tick, not per repaint, so the scroll speed
/// stays time-based). The native front keeps its own two-phase variant across
/// several windows; the single-window browser front uses this directly.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))] // browser-front only
pub(crate) fn advance_scope_histories(
    tree: &Widget,
    read: impl Fn(i32) -> f32,
    scopes: &mut HashMap<i32, VecDeque<f32>>,
) {
    let mut pairs = Vec::new();
    collect_scopes(tree, &mut pairs);
    for (id, bus) in pairs {
        push_sample(scopes.entry(id).or_default(), read(bus));
    }
}

/// The distinct, sorted control buses a tree reads live each frame: every
/// `meter`/`scope` bus plus a `canvas`'s non-negative `buses` entries. The
/// browser front subscribes exactly this set with `/c_stream`.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))] // browser-front only
pub(crate) fn collect_live_buses(widget: &Widget, out: &mut Vec<i32>) {
    let mut push = |bus: i32| {
        if bus >= 0 && !out.contains(&bus) {
            out.push(bus);
        }
    };
    if let Some(bus) = widget.kind.live_bus() {
        push(bus);
    }
    if let WidgetKind::Canvas { buses, .. } = &widget.kind {
        for &bus in buses {
            push(bus);
        }
    }
    for child in &widget.children {
        collect_live_buses(child, out);
    }
    out.sort_unstable();
}

/// A [`BusSource`] filled from `/c_stream`'s periodic `/c_set` snapshots — the
/// message-based counterpart of the shared-memory segment, for the browser.
/// Unsubscribed or never-streamed buses read `0.0`, exactly like unmapped or
/// out-of-range buses natively. The `Mutex` only satisfies the trait's
/// `Send + Sync` bound; on the single-threaded wasm runtime it is uncontended.
#[derive(Default)]
pub struct StreamedBuses {
    values: Mutex<HashMap<usize, f32>>,
}

impl StreamedBuses {
    /// Stores one streamed `(busIndex, value)` pair.
    pub fn set(&self, index: usize, value: f32) {
        self.values.lock().unwrap().insert(index, value);
    }
}

impl BusSource for StreamedBuses {
    fn control(&self, index: usize) -> f32 {
        self.values
            .lock()
            .unwrap()
            .get(&index)
            .copied()
            .unwrap_or(0.0)
    }
}

/// The `/tap_data` store — the message-based counterpart of the shared-memory
/// tap rings, for the browser. Keeps the newest streamed raw window per tap;
/// the tick reads it through [`update_tap_windows`] exactly as the native
/// front reads the segment. The `Mutex` is uncontended on the single-threaded
/// wasm runtime, as in [`StreamedBuses`].
#[derive(Default)]
pub struct StreamedTaps {
    windows: Mutex<HashMap<i32, Vec<f32>>>,
}

impl StreamedTaps {
    /// Stores the newest streamed window of one tap.
    pub fn set(&self, tap: i32, samples: Vec<f32>) {
        self.windows.lock().unwrap().insert(tap, samples);
    }

    /// Fills `out` with the newest raw samples of `tap`, right-aligned (the
    /// newest sample last, a short store zero-padded at the front), or returns
    /// `false` when nothing has been streamed for it yet.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))] // browser-front only
    pub(crate) fn read_raw(&self, tap: i32, out: &mut [f32]) -> bool {
        let map = self.windows.lock().unwrap();
        let Some(w) = map.get(&tap).filter(|w| !w.is_empty()) else {
            return false;
        };
        out.fill(0.0);
        let n = w.len().min(out.len());
        let at = out.len() - n;
        out[at..].copy_from_slice(&w[w.len() - n..]);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::super::guidef::GuiNode;
    use super::*;

    fn tree(json: &str) -> Widget {
        let node = GuiNode::parse(json.as_bytes()).unwrap();
        Widget::from_node(1, &node, &[]).unwrap()
    }

    #[test]
    fn live_buses_cover_meters_scopes_and_canvases_deduped() {
        let w = tree(
            r#"{"type":"window","children":[
                {"id":1,"type":"meter","bus":9},
                {"id":2,"type":"scope","bus":3},
                {"id":3,"type":"meter","bus":3},
                {"id":4,"type":"canvas","shader":"fn shade(){}","buses":[7]},
                {"id":5,"type":"label","text":"no bus"}]}"#,
        );
        let mut buses = Vec::new();
        collect_live_buses(&w, &mut buses);
        // Deduplicated, sorted, and the canvas's unset (-1) slots are skipped.
        assert_eq!(buses, vec![3, 7, 9]);
    }

    #[test]
    fn scope_history_advances_and_caps() {
        let w = tree(r#"{"type":"window","children":[{"id":2,"type":"scope","bus":3}]}"#);
        let mut scopes = HashMap::new();
        for i in 0..(SCOPE_HISTORY + 10) {
            advance_scope_histories(&w, |bus| bus as f32 + i as f32, &mut scopes);
        }
        let history = &scopes[&2];
        assert_eq!(history.len(), SCOPE_HISTORY, "history is capped");
        // Oldest samples fell off the front; the newest is the last push.
        assert_eq!(
            *history.back().unwrap(),
            3.0 + (SCOPE_HISTORY + 9) as f32,
            "newest sample read from the scope's bus"
        );
    }

    #[test]
    fn streamed_buses_read_back_and_default_to_zero() {
        let buses = StreamedBuses::default();
        assert_eq!(buses.control(5), 0.0, "never-streamed buses read zero");
        buses.set(5, 0.25);
        buses.set(9, -1.5);
        assert_eq!(buses.control(5), 0.25);
        assert_eq!(buses.control(9), -1.5);
        assert_eq!(buses.control(1000), 0.0);
    }
}
