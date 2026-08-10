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
pub(crate) fn collect_scopes(tree: &Widget) -> Vec<(i32, i32)> {
    tree.descendants()
        .filter_map(|w| Some((w.id?, w.kind.live_bus()?)))
        .collect()
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
pub(crate) fn collect_tap_scopes(tree: &Widget) -> Vec<TapScope> {
    tree.descendants()
        .filter_map(|w| {
            let el = w.kind.signal()?;
            let bus = el.source.bus()?;
            (el.presentation == Presentation::Signal && bus.rate.is_audio()).then_some(TapScope {
                widget_id: w.id?,
                bus: bus.bus,
                channels: bus.channels,
                window_ms: bus.window_ms,
                trigger: bus.trigger,
                hold: bus.hold,
            })
        })
        .collect()
}

/// The distinct, sorted tap indices a tree reads live each frame — every
/// audio-rate scope, spectrum and phasescope (two taps each). The browser front
/// subscribes exactly this set with `/bus_tapStream`.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))] // browser-front only
pub(crate) fn collect_live_taps(tree: &Widget, out: &mut Vec<i32>) {
    let mut taps: Vec<i32> = tree
        .descendants()
        .flat_map(|w| w.kind.needs().taps)
        .collect();
    taps.retain(|tap| !out.contains(tap));
    out.extend(taps);
    out.sort_unstable();
    out.dedup();
}

/// Whether a widget tree contains a live widget: a bus-backed meter/scope,
/// any audio-tap consumer (scope/spectrum/phasescope), or a timeline view
/// with an active playhead (its line tracks the engine clock every frame) —
/// so the window animates.
pub(crate) fn tree_has_live_widget(widget: &Widget, groups: &TimelineGroups) -> bool {
    let needs = widget.kind.needs();
    !needs.buses.is_empty()
        || !needs.levels.is_empty()
        || !needs.taps.is_empty()
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
    let specs = collect_tap_scopes(tree);
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
pub(crate) fn collect_phase_scopes(tree: &Widget) -> Vec<PhaseScope> {
    tree.descendants()
        .filter_map(|w| {
            let el = w.kind.signal()?;
            let bus = el.source.bus()?;
            (el.presentation == Presentation::Phase).then_some(PhaseScope {
                widget_id: w.id?,
                bus_l: bus.bus,
                bus_r: bus.bus + 1,
                window_ms: bus.window_ms,
                hold: bus.hold,
            })
        })
        .collect()
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
    let specs = collect_phase_scopes(tree);
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

/// **A retained history of one bus** — the addressable past a forward-only
/// source does not have, and the whole of what `retention` buys.
///
/// The ring is sized by the axis's declared span (seconds times the sample
/// rate) and filled from the same per-tick read every live view already does,
/// appending only the samples past the last stream position it saw. Two
/// consequences are worth stating, because they are what makes the history
/// *true* rather than merely long:
///
/// - **The append is exact.** Two ticks read overlapping windows and the
///   overlap depends on the frame rate; taking the tail past the position is
///   what keeps a second of history a second wide at any frame rate.
/// - **A gap is a gap.** When more samples elapsed than the source's window
///   holds, the missing ones are gone — the engine wrote them and the ring
///   they passed through wrapped. They are appended as **silence** rather than
///   skipped, so the time axis stays honest and the drop-out is visible instead
///   of being compressed away.
#[derive(Clone, Debug, Default)]
pub(crate) struct BusHistory {
    /// The retained samples, oldest first. A plain `Vec` drained from the front
    /// rather than a wrapping index: the readers want a contiguous slice (an
    /// FFT window, a texture column run), and the drain is a memmove of at most
    /// the span, once per tick.
    samples: Vec<f32>,
    /// The capacity in samples the current span asks for.
    capacity: usize,
    /// The stream position of the newest sample retained, or `None` before the
    /// first read.
    at: Option<u64>,
}

impl BusHistory {
    /// The retained samples, oldest first.
    pub fn samples(&self) -> &[f32] {
        &self.samples
    }

    /// How many samples are retained. A test accessor: the readers want the
    /// slice, not its length.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// The stream position just past the newest retained sample — the anchor a
    /// rolling analysis measures its columns against. `None` before the first
    /// read.
    pub fn end(&self) -> Option<u64> {
        self.at.map(|at| at + 1)
    }

    /// Resizes the history to `capacity` samples, dropping the oldest when it
    /// shrinks — a live `/gui_set retention` narrowing the span, which must
    /// take effect on this frame rather than when the ring next fills.
    fn set_capacity(&mut self, capacity: usize) {
        self.capacity = capacity;
        self.trim();
    }

    fn trim(&mut self) {
        if self.samples.len() > self.capacity {
            let excess = self.samples.len() - self.capacity;
            self.samples.drain(..excess);
        }
    }

    /// Appends whatever of `window` is new, given that its last sample sits at
    /// stream position `at`. Returns how many samples were appended.
    fn append(&mut self, window: &[f32], at: u64) -> usize {
        if self.capacity == 0 || window.is_empty() {
            return 0;
        }
        let Some(seen) = self.at else {
            // The first read seeds the history with the whole window: there is
            // no earlier position to measure a gap against.
            self.at = Some(at);
            let n = window.len().min(self.capacity);
            self.samples.extend_from_slice(&window[window.len() - n..]);
            self.trim();
            return n;
        };
        if at <= seen {
            return 0; // the same window again: a tick faster than the engine
        }
        let fresh = (at - seen) as usize;
        self.at = Some(at);
        let gap = fresh.saturating_sub(window.len());
        if gap > 0 {
            // Samples the engine wrote and this reader never saw. Silence keeps
            // the axis honest -- see the type's documentation.
            self.samples
                .extend(std::iter::repeat_n(0.0, gap.min(self.capacity)));
        }
        let take = fresh.min(window.len());
        self.samples
            .extend_from_slice(&window[window.len() - take..]);
        self.trim();
        gap.min(self.capacity) + take
    }
}

/// One retaining view's per-tick spec: the bus it watches and how many samples
/// of it the axis declared.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct RetentionSpec {
    pub bus: i32,
    pub capacity: usize,
}

/// Appends the [`RetentionSpec`] of every element declaring a retention span,
/// **merged per bus**: a bus watched by two views is retained once, at the
/// longest span either asked for, since the history is the bus's and not the
/// drawing's.
pub(crate) fn collect_retention(tree: &Widget, sample_rate: f64) -> Vec<RetentionSpec> {
    let sr = if sample_rate > 0.0 {
        sample_rate
    } else {
        48_000.0
    };
    let mut specs: Vec<RetentionSpec> = Vec::new();
    for widget in tree.descendants() {
        let Some(el) = widget.kind.signal() else {
            continue;
        };
        let Some(bus) = el.source.bus() else { continue };
        if bus.retention <= 0.0 {
            continue;
        }
        let capacity = (bus.retention as f64 * sr).round().max(0.0) as usize;
        for k in 0..bus.channels.max(1) {
            let bus_id = bus.bus + k as i32;
            match specs.iter_mut().find(|s| s.bus == bus_id) {
                Some(s) => s.capacity = s.capacity.max(capacity),
                None => specs.push(RetentionSpec {
                    bus: bus_id,
                    capacity,
                }),
            }
        }
    }
    specs
}

/// The raw window a retaining read asks for: a quarter second, capped by what
/// the source will serve in one read (`limit`, 0 = it does not say).
///
/// A quarter second is an order of magnitude more than a frame tick's worth of
/// samples at any frame rate anyone runs, and reading more than elapsed costs
/// nothing — the retainer takes only the tail past the position it saw, so the
/// slack is the point. The cap is not an optimization: a source refuses a
/// window it cannot copy safely, and a refusal is indistinguishable from a bus
/// nobody writes, so asking for more than the ring can give retains **nothing**
/// and looks like silence.
pub(crate) fn retention_window(sample_rate: f64, limit: usize) -> usize {
    let sr = if sample_rate > 0.0 {
        sample_rate
    } else {
        48_000.0
    };
    let want = (sr * 0.25).round() as usize;
    if limit > 0 { want.min(limit) } else { want }
}

/// Advances every retained bus of one window's tree by whatever the engine
/// wrote since the last tick, and forgets the buses nothing retains any more
/// (a `/gui_set retention 0`, or the view leaving the tree).
///
/// `read_at` is the source's positioned read — the segment natively, the
/// `/bus_tapStream.reply` store in the browser — filling the newest window and
/// saying where it ends. `window` sizes that read: it must be at least a
/// tick's worth of samples, or every tick reports a gap it could have carried.
pub(crate) fn update_retention(
    tree: &Widget,
    sample_rate: f64,
    window: usize,
    read_at: impl Fn(i32, &mut [f32]) -> Option<u64>,
    histories: &mut HashMap<i32, BusHistory>,
) {
    let specs = collect_retention(tree, sample_rate);
    histories.retain(|bus, _| specs.iter().any(|s| s.bus == *bus));
    if specs.is_empty() {
        return;
    }
    let mut raw = vec![0.0f32; window.max(1)];
    for spec in specs {
        let history = histories.entry(spec.bus).or_default();
        history.set_capacity(spec.capacity);
        if let Some(at) = read_at(spec.bus, &mut raw) {
            history.append(&raw, at);
        }
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
pub(crate) fn collect_spectra(tree: &Widget) -> Vec<SpectrumSpec> {
    tree.descendants()
        .filter_map(|w| {
            let el = w.kind.signal()?;
            let bus = el.source.bus()?;
            (el.presentation == Presentation::Spectrum).then_some(SpectrumSpec {
                widget_id: w.id?,
                bus: bus.bus,
                channels: bus.channels,
                fft_size: el.spectral.fft_size,
                averaging: el.spectral.averaging,
                peak_hold: el.spectral.peak_hold,
            })
        })
        .collect()
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
    let specs = collect_spectra(tree);
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

/// Advances every **retained time-frequency view**'s rolling analysis from the
/// histories the tick just filled, and forgets the views that left the tree.
///
/// The two stores are deliberately keyed differently, and the split is the
/// design rather than an accident: a **history is the bus's** (one per bus,
/// shared by everything watching it) and a **transform is the view's** (two
/// views of one bus may analyze it at different sizes). This is where the two
/// meet, once per tick.
pub(crate) fn update_waterfalls(
    tree: &Widget,
    sample_rate: f64,
    histories: &HashMap<i32, BusHistory>,
    rolls: &mut HashMap<i32, super::waterfall::Waterfall>,
) {
    let sr = if sample_rate > 0.0 {
        sample_rate as f32
    } else {
        48_000.0
    };
    let mut live: Vec<i32> = Vec::new();
    for widget in tree.descendants() {
        let (Some(id), Some(el)) = (widget.id, widget.kind.signal()) else {
            continue;
        };
        let Some(bus) = el.source.bus() else { continue };
        if el.presentation != Presentation::TimeFrequency || bus.retention <= 0.0 {
            continue;
        }
        live.push(id);
        let (window_size, hop) = (el.spectral.fft_size, el.spectral.hop.max(1));
        // The span in columns: the seconds the axis declared, at this
        // transform's own hop. The texture's cap applies on top (`set_capacity`).
        let columns = ((bus.retention as f64 * sr as f64) / hop as f64).ceil() as usize;
        let roll = match rolls.get_mut(&id) {
            // A `/gui_set` of the analysis restarts the roll: the columns of
            // one transform are not the columns of another.
            Some(roll) if roll.matches(window_size, hop, sr) => roll,
            _ => rolls
                .entry(id)
                .insert_entry(super::waterfall::Waterfall::new(
                    window_size,
                    hop,
                    sr,
                    columns,
                ))
                .into_mut(),
        };
        roll.set_capacity(columns);
        if let Some(history) = histories.get(&bus.bus)
            && let Some(end) = history.end()
        {
            roll.advance(history.samples(), end);
        }
    }
    rolls.retain(|id, _| live.contains(id));
}

/// The largest raw tap window any of a tree's tap consumers needs — an
/// oscilloscope's `window_ms` (with the trigger slack), a phasescope's window,
/// or a spectrum's FFT size — so the browser's `/bus_tapStream` subscription is
/// sized to feed all three. Zero when the tree reads no taps.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))] // browser-front only
pub(crate) fn tap_stream_frames(tree: &Widget, sample_rate: f64) -> usize {
    let mut frames = 0usize;
    for s in collect_tap_scopes(tree) {
        let display = oscil::display_frames(s.window_ms, sample_rate);
        frames = frames.max(oscil::raw_frames(display));
    }
    for p in collect_phase_scopes(tree) {
        frames = frames.max(oscil::display_frames(p.window_ms, sample_rate));
    }
    for s in collect_spectra(tree) {
        frames = frames.max(s.fft_size);
    }
    // A retained view reads a whole tick's worth per frame, not a display
    // window: the subscription has to carry it or the history never fills.
    if !collect_retention(tree, sample_rate).is_empty() {
        frames = frames.max(retention_window(sample_rate, 0));
    }
    frames
}

/// Whether a widget tree holds anything whose picture follows the clock rather
/// than a value (a `canvas`), so the window animates each frame.
pub(crate) fn tree_animates(tree: &Widget) -> bool {
    tree.descendants().any(|w| w.kind.needs().animated)
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
    for (id, bus) in collect_scopes(tree) {
        push_sample(scopes.entry(id).or_default(), read(bus));
    }
}

/// The distinct, sorted control buses a tree reads live each frame: whatever
/// each widget declares it reads. The browser front subscribes exactly this set
/// with `/bus_stream`.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))] // browser-front only
pub(crate) fn collect_live_buses(tree: &Widget, out: &mut Vec<i32>) {
    for widget in tree.descendants() {
        for bus in widget.kind.needs().buses {
            if bus >= 0 && !out.contains(&bus) {
                out.push(bus);
            }
        }
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
        out.animated |= tree_animates(tree) || tree_has_live_widget(tree, groups);
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
    windows: Mutex<HashMap<i32, (Vec<f32>, u64)>>,
}

impl StreamedTaps {
    /// Stores the newest streamed window of one tap, with the stream position
    /// its last sample sits at — what a retainer appends by (see
    /// [`super::BusSource::read_bus_at`]).
    pub fn set(&self, tap: i32, samples: Vec<f32>, at: u64) {
        self.windows.lock().unwrap().insert(tap, (samples, at));
    }

    /// Fills `out` with the newest raw samples of `tap`, right-aligned (the
    /// newest sample last, a short store zero-padded at the front), or returns
    /// `false` when nothing has been streamed for it yet.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))] // browser-front only
    pub(crate) fn read_raw(&self, tap: i32, out: &mut [f32]) -> bool {
        self.read_raw_at(tap, out).is_some()
    }

    /// [`read_raw`](Self::read_raw), returning the stream position the window
    /// ends at — the browser half of
    /// [`super::BusSource::read_bus_at`].
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))] // browser-front only
    pub(crate) fn read_raw_at(&self, tap: i32, out: &mut [f32]) -> Option<u64> {
        let map = self.windows.lock().unwrap();
        let (w, at) = map.get(&tap).filter(|(w, _)| !w.is_empty())?;
        out.fill(0.0);
        let n = w.len().min(out.len());
        let start = out.len() - n;
        out[start..].copy_from_slice(&w[w.len() - n..]);
        Some(*at)
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

    /// A tree with one retained waterfall on bus 0.
    fn retaining(seconds: f32) -> Widget {
        tree(&format!(
            r#"{{"type":"window","children":[
                {{"id":5,"type":"signal","view":"spectrogram","bus":0,
                  "retention":{seconds},"navigable":1,"window_size":256}}
            ]}}"#
        ))
    }

    /// The span is declared in seconds and resolved against the sample rate:
    /// the same seconds are the same seconds whatever the rate.
    #[test]
    fn a_span_in_seconds_becomes_a_capacity_in_samples() {
        let w = retaining(2.0);
        assert_eq!(
            collect_retention(&w, 48_000.0),
            vec![RetentionSpec {
                bus: 0,
                capacity: 96_000
            }]
        );
        assert_eq!(collect_retention(&w, 96_000.0)[0].capacity, 192_000);
        // No span declared, nothing retained.
        assert!(collect_retention(&retaining(0.0), 48_000.0).is_empty());
    }

    /// The history is the **bus's**: two views of one bus retain it once, at
    /// the longest span either asked for.
    #[test]
    fn two_views_of_one_bus_share_one_history() {
        let w = tree(
            r#"{"type":"window","children":[
                {"id":5,"type":"signal","view":"spectrogram","bus":3,
                 "retention":1.0,"navigable":1},
                {"id":6,"type":"signal","view":"spectrogram","bus":3,
                 "retention":4.0,"navigable":1}
            ]}"#,
        );
        let specs = collect_retention(&w, 48_000.0);
        assert_eq!(specs.len(), 1, "one history for one bus");
        assert_eq!(
            specs[0],
            RetentionSpec {
                bus: 3,
                capacity: 192_000
            }
        );
    }

    /// The append is by **stream position**, not by window: two ticks reading
    /// overlapping windows retain each sample once, so a second of history is a
    /// second wide at any frame rate.
    #[test]
    fn the_append_takes_only_what_is_past_the_position_it_saw() {
        let mut h = BusHistory::default();
        h.set_capacity(100);
        // The first read seeds with the whole window.
        assert_eq!(h.append(&[1.0, 2.0, 3.0, 4.0], 4), 4);
        assert_eq!(h.samples(), &[1.0, 2.0, 3.0, 4.0]);
        // The same window again: nothing new.
        assert_eq!(h.append(&[1.0, 2.0, 3.0, 4.0], 4), 0);
        assert_eq!(h.len(), 4);
        // Two samples on: only the two past the position land, though the
        // window still carries four.
        assert_eq!(h.append(&[3.0, 4.0, 5.0, 6.0], 6), 2);
        assert_eq!(h.samples(), &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        assert_eq!(h.end(), Some(7));
    }

    /// More elapsed than the window holds: the samples in between are gone,
    /// and they are appended as silence rather than skipped — a drop-out is
    /// visible instead of being compressed out of the time axis.
    #[test]
    fn a_gap_is_retained_as_silence_so_the_axis_stays_true() {
        let mut h = BusHistory::default();
        h.set_capacity(100);
        h.append(&[1.0, 2.0], 2);
        // Ten samples elapsed, the window carries two: eight are lost.
        assert_eq!(h.append(&[9.0, 9.0], 12), 10);
        assert_eq!(h.len(), 12);
        assert_eq!(&h.samples()[2..10], &[0.0; 8], "the lost stretch");
        assert_eq!(&h.samples()[10..], &[9.0, 9.0]);
    }

    /// The span caps the history, and narrowing it live drops the oldest at
    /// once rather than when the ring next fills.
    #[test]
    fn the_span_caps_the_history_and_narrows_live() {
        let mut h = BusHistory::default();
        h.set_capacity(4);
        h.append(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], 6);
        assert_eq!(h.samples(), &[3.0, 4.0, 5.0, 6.0]);
        h.set_capacity(2);
        assert_eq!(h.samples(), &[5.0, 6.0]);
    }

    /// The tick forgets a bus nothing retains any more — a `/gui_set
    /// retention 0`, or the view leaving the tree.
    #[test]
    fn the_tick_forgets_a_bus_nothing_retains() {
        let mut histories = HashMap::new();
        let read = |_bus: i32, out: &mut [f32]| {
            out.fill(0.5);
            Some(out.len() as u64)
        };
        update_retention(&retaining(1.0), 48_000.0, 64, read, &mut histories);
        assert_eq!(histories.len(), 1);
        update_retention(&retaining(0.0), 48_000.0, 64, read, &mut histories);
        assert!(histories.is_empty(), "the history goes with the span");
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

    /// An audio-rate meter reads a **level**, which costs neither a streamed
    /// bus nor a recording — and used to cost the window its animation too,
    /// because the liveness walk only knew about control buses and taps. The
    /// declaration says it reads something, so the window follows it.
    #[test]
    fn a_level_meter_animates_its_window() {
        let w = tree(r#"{"type":"window","children":[{"id":1,"type":"meter","bus":2}]}"#);
        let d = demand([&w], &groups(), 48_000.0);
        assert!(
            d.buses.is_empty() && d.taps.is_empty(),
            "a level costs neither"
        );
        assert!(d.animated, "but the column still has to move");
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
