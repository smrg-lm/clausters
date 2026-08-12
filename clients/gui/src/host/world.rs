//! **The world**: the read-only per-frame facts that no widget owns.
//!
//! A frame assembles two very different things and used to carry them in one
//! struct. Some of it is *one widget's own interaction state* fed back down so
//! that widget can draw itself mid-gesture — a button's held id, a menu's open
//! list — and that is not the world: it goes home to the element that owns it.
//! What is left is genuinely nobody's, identical for every element of the
//! frame: the bus source, the queried node trees, whether a server is attached,
//! the sample rate and the sample clock, the pointer, and the timeline
//! navigation groups.
//!
//! That is this struct, and it is what an [`Element`](super::widget::Element)
//! is handed beside its [`Draw`](super::paint::Draw). It is deliberately not a
//! god-context: it grows when the *host* learns a new fact about the outside,
//! never when a widget is added — a widget's own state lives in the widget.
//!
//! [`World::default`] is an empty one (no bus source, no trees, no server),
//! which is both the no-transport case the fronts fall back to and what an
//! element's own tests draw against.

use std::collections::HashMap;
use std::sync::OnceLock;

use super::BusSource;
use super::timeline::TimelineGroups;
use super::widget::Rate;
use crate::host::graphics::nodetree::NodeTree;

/// The outside, as one frame sees it. Read-only, identical for every element.
pub struct World<'a> {
    /// The control-bus source (`None` reads zero everywhere).
    pub bus: Option<&'a dyn BusSource>,
    /// The server node trees the last query returned, by group.
    pub node_trees: &'a HashMap<i32, NodeTree>,
    /// Whether an audio server is attached at all — the difference between "no
    /// nodes" and "nobody to ask".
    pub server_attached: bool,
    /// The server's sample rate, placing a frequency axis or a time ruler whose
    /// widget names no rate of its own (`0.0` → unknown).
    pub sample_rate: f64,
    /// The engine's sample clock (samples since boot; the shm header natively,
    /// the polled `/clock_query` in the browser). What a playhead is drawn from.
    pub sample_clock: f64,
    /// The pointer in device pixels, for the cursor readouts (`None` = the
    /// pointer is not over this window).
    pub cursor: Option<(f64, f64)>,
    /// The host's timeline navigation groups: linked views share one window,
    /// which is a fact about the window and not about any of its members.
    pub timelines: &'a TimelineGroups,
}

impl World<'_> {
    /// The current value of control bus `bus` (`0.0` without a source, or for a
    /// negative or out-of-range bus) — the one rule, so no reader repeats it.
    pub fn control(&self, bus: i32) -> f32 {
        if bus < 0 {
            return 0.0;
        }
        self.bus.map_or(0.0, |s| s.control(bus as usize))
    }

    /// What a level reader draws for `bus` at `rate`: the published block level
    /// of an audio bus, or the current value of a control bus. Both are one
    /// atomic load out of the same source — neither costs a message, and the
    /// audio one costs no tap either.
    pub fn level(&self, bus: i32, rate: Rate) -> f32 {
        if bus < 0 {
            return 0.0;
        }
        match rate {
            Rate::Audio => self.bus.map_or(0.0, |s| s.level(bus)),
            Rate::Control => self.control(bus),
        }
    }

    /// The node tree of server group `group`, if one was queried.
    pub fn node_tree(&self, group: i32) -> Option<&NodeTree> {
        self.node_trees.get(&group)
    }
}

impl Default for World<'_> {
    fn default() -> Self {
        // 'static empties, so a world with nothing in it costs no allocation
        // and needs no lifetime of its own.
        static EMPTY: OnceLock<HashMap<i32, NodeTree>> = OnceLock::new();
        static NO_GROUPS: OnceLock<TimelineGroups> = OnceLock::new();
        Self {
            bus: None,
            node_trees: EMPTY.get_or_init(HashMap::new),
            server_attached: false,
            sample_rate: 0.0,
            sample_clock: 0.0,
            cursor: None,
            timelines: NO_GROUPS.get_or_init(TimelineGroups::default),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A source that answers with the bus index itself, so a read that went to
    /// the wrong place is visible rather than plausible.
    struct Buses;

    impl BusSource for Buses {
        fn control(&self, index: usize) -> f32 {
            index as f32
        }

        fn level(&self, bus: i32) -> f32 {
            -(bus as f32)
        }
    }

    /// The empty world is the no-transport case: every read is zero, and none
    /// of them is a panic or a `None` a caller has to handle.
    #[test]
    fn a_world_with_no_source_reads_zero() {
        let w = World::default();
        assert_eq!(w.control(3), 0.0);
        assert_eq!(w.level(3, Rate::Audio), 0.0);
        assert_eq!(w.level(3, Rate::Control), 0.0);
        assert_eq!(w.node_tree(0).map(|_| ()), None);
    }

    /// The rate picks the *table*, not the widget: an audio bus reports its
    /// published block level, a control bus its current value. A negative bus
    /// is silence in both, which is what an unset `bus` prop means.
    #[test]
    fn a_level_reads_the_table_its_rate_names() {
        let buses = Buses;
        let w = World {
            bus: Some(&buses),
            ..Default::default()
        };
        assert_eq!(w.control(5), 5.0);
        assert_eq!(w.level(5, Rate::Control), 5.0);
        assert_eq!(w.level(5, Rate::Audio), -5.0);
        assert_eq!(w.control(-1), 0.0);
        assert_eq!(w.level(-1, Rate::Audio), 0.0);
    }
}
