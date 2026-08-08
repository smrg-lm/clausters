//! Live control-bus plumbing shared by the native and browser fronts.
//!
//! Meters, scopes and `canvas` bus parameters read control buses every
//! animation frame. Natively the values come from the shared-memory segment
//! ([`super::shm`], zero messages); in the browser they arrive as periodic
//! `/bus_stream.reply` snapshots from the server's `/bus_stream` subscription (the
//! counterpart of the segment). Everything around that difference — which
//! buses a tree reads, how a scope's rolling history advances, how a window
//! decides it is animated — is platform-independent and lives here, so both
//! fronts share one implementation and only the [`BusSource`] fill differs.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use clausters_core::oscil;

use super::BusSource;
use super::signal::Presentation;
use super::timeline::{TimelineGroups, group_key};
use super::widget::{Widget, WidgetKind};

/// Most recent control-bus samples a `scope` keeps and plots.
pub(crate) const SCOPE_HISTORY: usize = 512;

/// The `/bus_stream` period the browser front subscribes with: the same ~30 fps
/// the animation tick runs at, so every frame paints a fresh snapshot.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))] // browser-front only
pub(crate) const STREAM_PERIOD_MS: i32 = 33;

/// Appends `(widget_id, bus)` for every **control-rate** `scope` in the tree,
/// so the frame tick can sample each one's bus into its rolling history (an
/// audio-rate scope reads a tap window instead — see [`collect_tap_scopes`]).
pub(crate) fn collect_scopes(widget: &Widget, out: &mut Vec<(i32, i32)>) {
    if let Some(bus) = widget.kind.live_bus()
        && let Some(id) = widget.id
    {
        out.push((id, bus));
    }
    for child in &widget.children {
        collect_scopes(child, out);
    }
}

/// One tap consumer's per-tick display window: `channels` interleaved
/// channels of samples (frame-major, like every interleaved buffer in the
/// system), plus whether the oscilloscope's trigger locked this tick (always
/// `false` for a phasescope — it has no trigger). Stored per widget id by the
/// tick; the render draws it verbatim.
#[derive(Clone, PartialEq, Debug, Default)]
pub(crate) struct TapWindow {
    pub samples: Vec<f32>,
    pub channels: usize,
    pub locked: bool,
}

impl TapWindow {
    /// Frames per channel in this window.
    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels.max(1)
    }
}

/// One audio-rate scope's per-tick read spec: its first tap and how many
/// adjacent rings, how big a window, where to trigger, and whether the trace
/// is frozen.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct TapScope {
    pub widget_id: i32,
    /// The first audio bus it reads; `channels` adjacent buses follow. Where
    /// each one's samples live is looked up in the segment's directory.
    pub bus: i32,
    pub channels: usize,
    pub window_ms: f32,
    pub trigger: f32,
    pub hold: bool,
}

/// Appends the [`TapScope`] of every audio-rate `scope` in the tree.
pub(crate) fn collect_tap_scopes(widget: &Widget, out: &mut Vec<TapScope>) {
    if let Some(el) = widget.kind.signal()
        && el.presentation == Presentation::Signal
        && let Some(bus) = el.source.bus()
        && bus.rate.is_audio()
        && let Some(id) = widget.id
    {
        out.push(TapScope {
            widget_id: id,
            bus: bus.bus,
            channels: bus.channels,
            window_ms: bus.window_ms,
            trigger: bus.trigger,
            hold: bus.hold,
        });
    }
    for child in &widget.children {
        collect_tap_scopes(child, out);
    }
}

/// The distinct, sorted tap indices a tree reads live each frame — every
/// audio-rate scope, spectrum and phasescope (two taps each). The browser front
/// subscribes exactly this set with `/bus_tapStream`.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))] // browser-front only
pub(crate) fn collect_live_taps(widget: &Widget, out: &mut Vec<i32>) {
    let mut taps = Vec::new();
    widget.kind.audio_buses_read(&mut taps);
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

/// Whether a widget tree contains a live widget: a bus-backed meter/scope,
/// any audio-tap consumer (scope/spectrum/phasescope), or a timeline view
/// with an active playhead (its line tracks the engine clock every frame) —
/// so the window animates.
pub(crate) fn tree_has_live_widget(widget: &Widget, groups: &TimelineGroups) -> bool {
    let mut taps = Vec::new();
    widget.kind.audio_buses_read(&mut taps);
    widget.kind.live_bus().is_some()
        || !taps.is_empty()
        || has_playhead(widget, groups)
        || widget
            .children
            .iter()
            .any(|child| tree_has_live_widget(child, groups))
}

/// Whether `widget` shows a live playhead — so its window must animate, the
/// line tracking the engine sample clock every frame. A timeline view has no
/// anchor of its own: it draws its navigation group's, and only the group's
/// props (its member's are the def-time seed) say whether one is running. A
/// `score` carries its own and must be asked separately, or its cursor freezes
/// where it was anchored.
fn has_playhead(widget: &Widget, groups: &TimelineGroups) -> bool {
    if let WidgetKind::Score(data) = &widget.kind {
        return data.playhead_at >= 0.0;
    }
    let Some(editor) = widget.kind.editor() else {
        return false;
    };
    let anchor = widget
        .id
        .and_then(|id| groups.state(group_key(id, editor.link)))
        .map_or(editor.playhead_at, |state| state.playhead_at);
    anchor >= 0.0
}

/// Whether a widget tree contains a view with an active playhead (a timeline
/// view or a `score`).
/// The browser front polls the server clock (`/clock_query`) each tick only then;
/// the native front reads the shm header, which needs no message at all.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))] // browser-front only
pub(crate) fn tree_has_playhead(widget: &Widget, groups: &TimelineGroups) -> bool {
    has_playhead(widget, groups)
        || widget
            .children
            .iter()
            .any(|child| tree_has_playhead(child, groups))
}

/// Refreshes the aligned display window of every audio-rate scope in `tree`,
/// once per animation tick. `read_raw` fills a raw window of one tap's
/// samples (newest at the end) from wherever the platform gets them — the shm
/// segment natively, the `/bus_tapStream.reply` store in the browser — returning `false`
/// when no data is available yet. The trigger is searched in the **first**
/// channel and the found alignment applied to every channel, so the channels
/// keep their true relative phase; the stored [`TapWindow`] per widget id is
/// what the render draws verbatim. A `hold` scope keeps its last window; a
/// scope whose first channel has no data yet is skipped (later channels with
/// no data draw silence, so a short run does not blank the whole scope).
pub(crate) fn update_tap_windows(
    tree: &Widget,
    sample_rate: f64,
    read_raw: impl Fn(i32, &mut [f32]) -> bool,
    windows: &mut HashMap<i32, TapWindow>,
) {
    let mut specs = Vec::new();
    collect_tap_scopes(tree, &mut specs);
    let mut raw = Vec::new();
    let mut chans: Vec<Vec<f32>> = Vec::new();
    for spec in specs {
        if spec.hold {
            continue;
        }
        let display = oscil::display_frames(spec.window_ms, sample_rate);
        raw.resize(oscil::raw_frames(display), 0.0);
        if !read_raw(spec.bus, &mut raw) {
            continue;
        }
        let (start, locked) = oscil::align(&raw, display, spec.trigger);
        let end = (start + display).min(raw.len());
        chans.clear();
        chans.push(raw[start..end].to_vec());
        for k in 1..spec.channels {
            raw.fill(0.0);
            let _ = read_raw(spec.bus + k as i32, &mut raw);
            chans.push(raw[start..end].to_vec());
        }
        let frames = end - start;
        let mut samples = Vec::with_capacity(frames * spec.channels);
        for f in 0..frames {
            for ch in &chans {
                samples.push(ch[f]);
            }
        }
        windows.insert(
            spec.widget_id,
            TapWindow {
                samples,
                channels: spec.channels,
                locked,
            },
        );
    }
}

/// One phasescope's per-tick read spec: its two taps (left, right), how big a
/// window of pairs to keep, and whether the trace is frozen.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct PhaseScope {
    pub widget_id: i32,
    pub bus_l: i32,
    pub bus_r: i32,
    pub window_ms: f32,
    pub hold: bool,
}

/// Appends the [`PhaseScope`] of every `phasescope` in the tree.
pub(crate) fn collect_phase_scopes(widget: &Widget, out: &mut Vec<PhaseScope>) {
    if let Some(el) = widget.kind.signal()
        && el.presentation == Presentation::Phase
        && let Some(bus) = el.source.bus()
        && let Some(id) = widget.id
    {
        out.push(PhaseScope {
            widget_id: id,
            bus_l: bus.bus,
            bus_r: bus.bus + 1,
            window_ms: bus.window_ms,
            hold: bus.hold,
        });
    }
    for child in &widget.children {
        collect_phase_scopes(child, out);
    }
}

/// Refreshes each phasescope's interleaved `[l, r, l, r, …]` window (the same
/// `windows` map the oscilloscope uses — the widget ids do not collide) from the
/// two taps' newest samples, once per tick. `read_raw` fills a channel's newest
/// window (the shm rings natively, the `/bus_tapStream.reply` store in the browser); a
/// `hold` scope keeps its last window, and a channel with no data yet is
/// skipped. Unlike the oscilloscope there is no trigger — the goniometer shows
/// the freshest pairs directly.
pub(crate) fn update_phase_windows(
    tree: &Widget,
    sample_rate: f64,
    read_raw: impl Fn(i32, &mut [f32]) -> bool,
    windows: &mut HashMap<i32, TapWindow>,
) {
    let mut specs = Vec::new();
    collect_phase_scopes(tree, &mut specs);
    let (mut l, mut r) = (Vec::new(), Vec::new());
    for spec in specs {
        if spec.hold {
            continue;
        }
        let n = oscil::display_frames(spec.window_ms, sample_rate);
        l.resize(n, 0.0);
        r.resize(n, 0.0);
        if !read_raw(spec.bus_l, &mut l) || !read_raw(spec.bus_r, &mut r) {
            continue;
        }
        let mut inter = Vec::with_capacity(n * 2);
        for i in 0..n {
            inter.push(l[i]);
            inter.push(r[i]);
        }
        windows.insert(
            spec.widget_id,
            TapWindow {
                samples: inter,
                channels: 2,
                locked: false,
            },
        );
    }
}

/// One spectrum's per-tick read spec: its first tap and how many adjacent
/// rings, FFT size and display smoothing.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct SpectrumSpec {
    pub widget_id: i32,
    pub bus: i32,
    pub channels: usize,
    pub fft_size: usize,
    pub averaging: f32,
    pub peak_hold: bool,
}

/// Appends the [`SpectrumSpec`] of every `spectrum` in the tree.
pub(crate) fn collect_spectra(widget: &Widget, out: &mut Vec<SpectrumSpec>) {
    if let Some(el) = widget.kind.signal()
        && el.presentation == Presentation::Spectrum
        && let Some(bus) = el.source.bus()
        && let Some(id) = widget.id
    {
        out.push(SpectrumSpec {
            widget_id: id,
            bus: bus.bus,
            channels: bus.channels,
            fft_size: el.spectral.fft_size,
            averaging: el.spectral.averaging,
            peak_hold: el.spectral.peak_hold,
        });
    }
    for child in &widget.children {
        collect_spectra(child, out);
    }
}

/// Folds each spectrum channel's newest FFT window into its persistent
/// [`SpectrumState`](super::spectrum::SpectrumState) (one state per channel,
/// kept in step with the widget's
/// `channels`), once per tick. `read_raw` fills a full FFT window of one tap;
/// a tap with no data yet leaves that channel's state (and curve) as it was.
pub(crate) fn update_spectra(
    tree: &Widget,
    read_raw: impl Fn(i32, &mut [f32]) -> bool,
    states: &mut HashMap<i32, Vec<super::spectrum::SpectrumState>>,
) {
    let mut specs = Vec::new();
    collect_spectra(tree, &mut specs);
    let mut raw = Vec::new();
    for spec in specs {
        let chans = states.entry(spec.widget_id).or_default();
        chans.resize_with(spec.channels, || {
            super::spectrum::SpectrumState::new(spec.fft_size)
        });
        for (k, state) in chans.iter_mut().enumerate() {
            state.ensure_size(spec.fft_size);
            raw.resize(state.window_len(), 0.0);
            if read_raw(spec.bus + k as i32, &mut raw) {
                state.update(&raw, spec.averaging, spec.peak_hold);
            }
        }
    }
}

/// The largest raw tap window any of a tree's tap consumers needs — an
/// oscilloscope's `window_ms` (with the trigger slack), a phasescope's window,
/// or a spectrum's FFT size — so the browser's `/bus_tapStream` subscription is
/// sized to feed all three. Zero when the tree reads no taps.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))] // browser-front only
pub(crate) fn tap_stream_frames(tree: &Widget, sample_rate: f64) -> usize {
    let mut frames = 0usize;
    let mut scopes = Vec::new();
    collect_tap_scopes(tree, &mut scopes);
    for s in scopes {
        let display = oscil::display_frames(s.window_ms, sample_rate);
        frames = frames.max(oscil::raw_frames(display));
    }
    let mut phases = Vec::new();
    collect_phase_scopes(tree, &mut phases);
    for p in phases {
        frames = frames.max(oscil::display_frames(p.window_ms, sample_rate));
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
/// browser front subscribes exactly this set with `/bus_stream`.
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

/// What a set of drawing trees asks of the audio server and of the frame clock.
///
/// The browser front holds one canvas per `window`-rooted def and derives all
/// three of these from the canvases that are **visible**, so a component
/// scrolled out of the viewport costs nothing: not a frame computed here, not a
/// bus streamed over the wire, not the server CPU that fills it. (The same
/// waste exists on the desktop behind an occluded window; only the browser front
/// acts on it so far.)
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct LiveDemand {
    /// The `/bus_stream` set: distinct, sorted.
    pub buses: Vec<i32>,
    /// The `/bus_tapStream` set: distinct, sorted.
    pub taps: Vec<i32>,
    /// The window every tap consumer needs, in frames — the largest any of them
    /// asks for, since one subscription serves them all.
    pub tap_frames: usize,
    /// Whether anything on screen animates (a live widget or a `canvas`), which
    /// is what decides the ~30 fps tick runs at all.
    pub animated: bool,
    /// Whether anything on screen shows a playhead, which needs the engine's
    /// sample clock polled each tick.
    pub playhead: bool,
}

/// The union of what `trees` demand — the drawing canvases' trees, in any
/// order. Pure and platform-independent: the front supplies the set, this
/// decides the subscriptions.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))] // browser-front only
pub(crate) fn demand<'a>(
    trees: impl IntoIterator<Item = &'a Widget>,
    groups: &TimelineGroups,
    sample_rate: f64,
) -> LiveDemand {
    let mut out = LiveDemand::default();
    for tree in trees {
        collect_live_buses(tree, &mut out.buses);
        collect_live_taps(tree, &mut out.taps);
        out.tap_frames = out.tap_frames.max(tap_stream_frames(tree, sample_rate));
        out.animated |= tree_has_canvas(tree) || tree_has_live_widget(tree, groups);
        out.playhead |= tree_has_playhead(tree, groups);
    }
    out.buses.sort_unstable();
    out.buses.dedup();
    out.taps.sort_unstable();
    out.taps.dedup();
    out
}

/// A [`BusSource`] filled from `/bus_stream`'s periodic snapshots — the
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

/// The `/bus_tapStream.reply` store — the message-based counterpart of the shared-memory
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

    /// No navigation group registered: a timeline view then answers from its
    /// own def-time props, and a `score` (which is in no group ever) from its
    /// own either way.
    fn groups() -> TimelineGroups {
        TimelineGroups::default()
    }

    fn tree(json: &str) -> Widget {
        let node = GuiNode::parse(json.as_bytes()).unwrap();
        Widget::from_node(1, &node, &[]).unwrap()
    }

    /// The union over several drawing canvases: buses and taps merge and
    /// deduplicate, the tap window is the largest any of them needs, and one
    /// animated tree is enough to run the frame clock for the page.
    #[test]
    fn demand_unions_what_the_drawing_canvases_ask_for() {
        let one = tree(
            r#"{"type":"window","children":[
                {"id":1,"type":"meter","bus":9,"rate":"control"},
                {"id":2,"type":"signal","view":"trace","bus":3,"rate":"control"}]}"#,
        );
        let two = tree(
            r#"{"type":"window","children":[
                {"id":3,"type":"meter","bus":3,"rate":"control"},
                {"id":4,"type":"meter","bus":1,"rate":"control"}]}"#,
        );
        let d = demand([&one, &two], &groups(), 48_000.0);
        assert_eq!(d.buses, vec![1, 3, 9]);
        assert!(d.animated, "meters and scopes animate");
    }

    /// The point of the visibility flag: what the caller leaves out of the set
    /// leaves the subscription with it. A component scrolled out of the
    /// viewport stops costing wire and server CPU, not just compositing.
    #[test]
    fn a_canvas_left_out_of_the_set_drops_its_buses() {
        let shown = tree(
            r#"{"type":"window","children":[{"id":1,"type":"meter","bus":9,"rate":"control"}]}"#,
        );
        let hidden = tree(
            r#"{"type":"window","children":[{"id":2,"type":"meter","bus":4,"rate":"control"}]}"#,
        );
        assert_eq!(
            demand([&shown, &hidden], &groups(), 48_000.0).buses,
            vec![4, 9]
        );
        assert_eq!(demand([&shown], &groups(), 48_000.0).buses, vec![9]);
        // Nothing drawing at all: no subscription and no frame clock.
        let none: [&Widget; 0] = [];
        let quiet = demand(none, &groups(), 48_000.0);
        assert!(quiet.buses.is_empty() && !quiet.animated);
    }

    /// A still tree asks for nothing per frame — the tick stays off, however
    /// many canvases the document holds.
    #[test]
    fn a_still_tree_does_not_animate() {
        let still = tree(r#"{"type":"window","children":[{"id":1,"type":"label","text":"hi"}]}"#);
        let d = demand([&still], &groups(), 48_000.0);
        assert!(d.buses.is_empty());
        assert!(!d.animated);
    }

    #[test]
    fn live_buses_cover_meters_scopes_and_canvases_deduped() {
        let w = tree(
            r#"{"type":"window","children":[
                {"id":1,"type":"meter","bus":9,"rate":"control"},
                {"id":2,"type":"signal","view":"trace","bus":3,"rate":"control"},
                {"id":3,"type":"meter","bus":3,"rate":"control"},
                {"id":4,"type":"canvas","shader":"fn shade(){}","buses":[7]},
                {"id":5,"type":"label","text":"no bus"}]}"#,
        );
        let mut buses = Vec::new();
        collect_live_buses(&w, &mut buses);
        // Deduplicated, sorted, and the canvas's unset (-1) slots are skipped.
        assert_eq!(buses, vec![3, 7, 9]);
    }

    #[test]
    fn an_anchored_score_makes_its_window_animate() {
        // A score whose cursor is anchored to the engine clock has to be a live
        // widget, or the window repaints only on messages and the cursor freezes
        // where the anchor left it.
        let anchored = tree(
            r#"{"type":"window","children":[{"type":"plane","id":1,"children":[
                {"id":2,"type":"score","vb":[100,50],"playhead_at":48000.0}]}]}"#,
        );
        assert!(tree_has_live_widget(&anchored, &groups()));
        assert!(tree_has_playhead(&anchored, &groups()));
        // A static cursor (or none) needs no animation: it does not move.
        let still = tree(
            r#"{"type":"window","children":[
                {"id":2,"type":"score","vb":[100,50],"playhead":250.0}]}"#,
        );
        assert!(!tree_has_live_widget(&still, &groups()));
        assert!(!tree_has_playhead(&still, &groups()));
    }

    #[test]
    fn scope_history_advances_and_caps() {
        let w = tree(
            r#"{"type":"window","children":[{"id":2,"type":"signal","view":"trace","bus":3,"rate":"control"}]}"#,
        );
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
    fn tap_windows_interleave_channels_aligned_on_the_first() {
        // A 2-channel scope: the trigger crossing found in channel 0 aligns
        // both channels, so their relative phase is preserved verbatim.
        let w = tree(
            r#"{"type":"window","children":[
                {"id":7,"type":"signal","view":"trace","bus":0,"channels":2,"window_ms":1.0}]}"#,
        );
        let mut windows = HashMap::new();
        // Channel 0 rises through zero at a known index; channel 1 counts, so
        // the alignment applied to it is directly observable.
        let read = |tap: i32, out: &mut [f32]| {
            for (i, s) in out.iter_mut().enumerate() {
                *s = if tap == 0 {
                    if i % 24 < 12 { -1.0 } else { 1.0 }
                } else {
                    i as f32
                };
            }
            true
        };
        update_tap_windows(&w, 48_000.0, read, &mut windows);
        let win = &windows[&7];
        assert_eq!(win.channels, 2);
        assert!(win.locked, "a periodic square locks");
        let frames = win.frames();
        assert!(frames >= 16);
        assert_eq!(win.samples.len(), frames * 2);
        // Channel 0 starts at its rising crossing; channel 1 carries the same
        // start index (a multiple of nothing in particular, but consecutive).
        assert_eq!(win.samples[0], 1.0, "starts at the rising edge");
        let ch1_start = win.samples[1];
        assert_eq!(win.samples[3], ch1_start + 1.0, "channel 1 is consecutive");
        assert_eq!(ch1_start % 24.0, 12.0, "aligned to channel 0's crossing");
    }

    #[test]
    fn spectra_keep_one_state_per_channel() {
        let w = tree(
            r#"{"type":"window","children":[
                {"id":9,"type":"signal","view":"spectrum","bus":2,"channels":2,"fft_size":256}]}"#,
        );
        let mut states = HashMap::new();
        // Bus 2 carries a tone, bus 3 silence: the two channel states diverge.
        let read = |bus: i32, out: &mut [f32]| {
            for (i, s) in out.iter_mut().enumerate() {
                *s = if bus == 2 {
                    (std::f32::consts::TAU * i as f32 / 8.0).sin()
                } else {
                    0.0
                };
            }
            true
        };
        update_spectra(&w, read, &mut states);
        let chans = &states[&9];
        assert_eq!(chans.len(), 2);
        let peak = |s: &super::super::spectrum::SpectrumState| {
            s.avg_db.iter().cloned().fold(f32::NEG_INFINITY, f32::max)
        };
        assert!(peak(&chans[0]) > -6.0, "the tone channel peaks near 0 dB");
        assert!(peak(&chans[1]) < -60.0, "the silent channel stays down");
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
