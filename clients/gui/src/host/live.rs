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

/// Appends `(widget_id, bus)` for every `scope` in the tree, so the frame tick
/// can sample each one's bus into its rolling history.
pub(crate) fn collect_scopes(widget: &Widget, out: &mut Vec<(i32, i32)>) {
    if let WidgetKind::Scope { bus, .. } = &widget.kind
        && let Some(id) = widget.id
    {
        out.push((id, *bus));
    }
    for child in &widget.children {
        collect_scopes(child, out);
    }
}

/// Whether a widget tree contains a live (bus-backed) meter or scope.
pub(crate) fn tree_has_live_widget(widget: &Widget) -> bool {
    widget.kind.live_bus().is_some() || widget.children.iter().any(tree_has_live_widget)
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
