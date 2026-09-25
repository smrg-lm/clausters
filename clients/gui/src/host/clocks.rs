//! **Which counter a playhead is drawn from**, and its reading for a frame.
//!
//! A view draws its play cursor from the device clock or from a transport's
//! position, named on it, on an ancestor, on its window or on the host
//! (`/gui_headClock`, the launch-time `--clock`); [`Host::head_clocks`] reads
//! them once a frame for both fronts. A view of a take reads a transport in
//! its own frames.

use super::wire::int_arg;
use super::*;

/// Which of an engine's counters a **playhead** reads.
///
/// A [`BusSource`] carries them all and a widget draws one number. The choice
/// is the host's to hold ([`Host::set_head_clock_of`]) and is made **per
/// view**: a widget draws from the counter set on it or on its nearest
/// ancestor, its window's when none is, and the host's default
/// ([`Host::set_head_clock`], the launch-time `--clock`) when the window names
/// none either -- so a window holding two views that play two timelines draws
/// each from its own. It sits on the host and not on the source because a host
/// launched by a client opens its segment before it is told what it is
/// drawing, and because the browser has no segment at all and answers the same
/// question from `/transport_query`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum HeadClock {
    /// The device clock: samples processed since boot, never stopping. What a
    /// host attached to a live server wants -- its meters, scopes and taps are
    /// all on that axis.
    #[default]
    Device,
    /// A transport's **position**: it holds while stopped, jumps on a locate
    /// and wraps in a loop. What an editor wants, because it is the time of
    /// the samples rather than of the machine. It names which transport, since
    /// a server has several.
    Transport(usize),
}

impl Host {
    /// The counter a playhead is drawn from where no window or widget names
    /// one.
    pub fn head_clock(&self) -> HeadClock {
        self.head_clock
    }

    /// Draws every playhead nothing names a counter for from `head` -- the
    /// launch-time `--clock`.
    pub fn set_head_clock(&mut self, head: HeadClock) {
        self.head_clock = head;
    }

    /// Draws the playheads of `id` -- a window, or a widget and everything
    /// under it -- from `head` from now on.
    ///
    /// It takes effect on the next frame and touches nothing else: a view's
    /// `playhead_at` anchor keeps its meaning, and under
    /// [`Transport`](HeadClock::Transport) an anchor of `0` is what a script
    /// wants, because the counter is already the transport's own time.
    pub fn set_head_clock_of(&mut self, id: i32, head: HeadClock) {
        self.head_clocks.insert(id, head);
    }

    /// The counter widget `id` of window `def_id` draws from: its own, its
    /// nearest ancestor's, the window's, or the host's default. `None` for
    /// the window itself.
    pub fn head_clock_of(&self, def_id: i32, id: Option<i32>) -> HeadClock {
        let window = self.window_head_clock(def_id);
        let (Some(id), Some(tree)) = (id, self.window_defs.get(&def_id)) else {
            return window;
        };
        fn find(
            w: &Widget,
            id: i32,
            above: HeadClock,
            named: &HashMap<i32, HeadClock>,
        ) -> Option<HeadClock> {
            let here = w.id.and_then(|i| named.get(&i).copied()).unwrap_or(above);
            if w.id == Some(id) {
                return Some(here);
            }
            w.children.iter().find_map(|c| find(c, id, here, named))
        }
        find(tree, id, window, &self.head_clocks).unwrap_or(window)
    }

    pub(super) fn window_head_clock(&self, def_id: i32) -> HeadClock {
        self.head_clocks
            .get(&def_id)
            .copied()
            .unwrap_or(self.head_clock)
    }

    /// Every transport window `def_id` draws a playhead from -- what a front
    /// with no segment polls the position of.
    pub fn transports_drawn(&self, def_id: i32) -> Vec<usize> {
        let mut out = Vec::new();
        if let HeadClock::Transport(t) = self.window_head_clock(def_id) {
            out.push(t);
        }
        if let Some(tree) = self.window_defs.get(&def_id) {
            for w in tree.descendants() {
                if let Some(HeadClock::Transport(t)) = w.id.and_then(|i| self.head_clocks.get(&i))
                    && !out.contains(t)
                {
                    out.push(*t);
                }
            }
        }
        out
    }

    /// Whether window `def_id` draws anything from the device clock -- the one
    /// counter a front with no segment polls apart from the transports.
    pub fn device_drawn(&self, def_id: i32) -> bool {
        self.window_head_clock(def_id) == HeadClock::Device
            || self.window_defs.get(&def_id).is_some_and(|tree| {
                tree.descendants().any(|w| {
                    w.id.and_then(|i| self.head_clocks.get(&i)) == Some(&HeadClock::Device)
                })
            })
    }

    /// The clocks window `def_id`'s playheads sweep from this frame, read
    /// once off `bus`: the window's own counter, and the reading of each
    /// widget that draws from another ([`HeadClocks::at`]).
    ///
    /// **The one place the choice is resolved.** Below here a playhead reads
    /// "its clock" by widget and never asks which counter it is, which is what
    /// keeps the two fronts from each answering it their own way.
    pub fn head_clocks(&self, def_id: i32, bus: Option<&dyn BusSource>) -> HeadClocks {
        let Some(bus) = bus else {
            return HeadClocks::default();
        };
        let read = |head: HeadClock| match head {
            HeadClock::Device => bus.sample_clock(),
            HeadClock::Transport(transport) => bus.transport_position(transport),
        };
        let window = self.window_head_clock(def_id);
        let mut clocks = HeadClocks {
            window: read(window),
            widgets: HashMap::new(),
        };
        // **A view of a take reads a transport in its own frames.** A
        // transport counts the engine's samples, and a take recorded at
        // another rate is read faster or slower to sound at its pitch, so the
        // frame it is at is the position times its rate over the engine's.
        // Without this a 44.1 kHz take in a 48 kHz session draws its line
        // ahead of what is heard, and ever further from where it was located.
        // The reading is rounded to the take's frame, which is what a position
        // cursor stands on.
        // The device clock is left alone: a view anchored on it names its own
        // anchor, in the engine's samples.
        let engine = self.server_rate;
        let scale = |w: &Widget, head: HeadClock| match (head, w.kind.as_samples()) {
            (HeadClock::Transport(_), Some(samples)) if engine > 0.0 => {
                samples.samples_rate().map_or(1.0, |rate| rate / engine)
            }
            _ => 1.0,
        };
        // Nothing named below the window and no rate to convert: every widget
        // reads its counter, and the tree is not walked.
        if self.head_clocks.keys().all(|id| *id == def_id)
            && (window == HeadClock::Device || engine <= 0.0)
        {
            return clocks;
        }
        fn walk(
            w: &Widget,
            above: HeadClock,
            window: HeadClock,
            named: &HashMap<i32, HeadClock>,
            out: &mut HashMap<i32, f64>,
            read: &dyn Fn(&Widget, HeadClock) -> f64,
            scale: &dyn Fn(&Widget, HeadClock) -> f64,
        ) {
            let here = w.id.and_then(|i| named.get(&i).copied()).unwrap_or(above);
            if let Some(id) = w.id
                && (here != window || scale(w, here) != 1.0)
            {
                out.insert(id, read(w, here));
            }
            for c in &w.children {
                walk(c, here, window, named, out, read, scale);
            }
        }
        if let Some(tree) = self.window_defs.get(&def_id) {
            walk(
                tree,
                window,
                window,
                &self.head_clocks,
                &mut clocks.widgets,
                &|w, head| match scale(w, head) {
                    1.0 => read(head),
                    // A whole frame: a locate sent the frame as the nearest
                    // engine sample, so scaled back it lands up to half a
                    // frame off, and at a zoom that draws samples the line
                    // stands beside the one it was put on.
                    s => (read(head) * s).round(),
                },
                &scale,
            );
        }
        clocks
    }

    /// `/gui_headClock <id> <which> [<int32 transport>]` -- draw the playheads
    /// of `id` from this counter from now on: a window, or a widget and every
    /// view under it that names none of its own. `"transport"` reads transport
    /// 0 unless it names another.
    ///
    /// The id is not optional, for the reason a transport's is not: the word
    /// after it may be followed by an integer, so an id that could be missing
    /// could not be told from a transport. An id no window holds is reported
    /// and ignored, as is a word this host does not know, so a typo leaves the
    /// line drawing what it was drawing.
    pub(super) fn on_clock(
        &mut self,
        args: &[OscType],
        from: ClientId,
        effects: &mut Vec<HostEffect>,
    ) {
        let Some(id) = int_arg(args, 0) else {
            return diag::warn!("{from}: {GUI_CLOCK} needs the id of a window or a widget");
        };
        let which = match args.get(1) {
            Some(OscType::String(s)) => s.as_str(),
            _ => return diag::warn!("{from}: {GUI_CLOCK} {id} needs \"device\" or \"transport\""),
        };
        let head = match which {
            "device" => HeadClock::Device,
            "transport" => match args.get(2) {
                None => HeadClock::Transport(0),
                Some(OscType::Int(k)) if *k >= 0 => HeadClock::Transport(*k as usize),
                Some(other) => {
                    return diag::warn!(
                        "{from}: {GUI_CLOCK} {id}: {other:?} is not a transport id; still \
                         drawing the one it was"
                    );
                }
            },
            other => {
                return diag::warn!(
                    "{from}: {GUI_CLOCK} {id}: no counter called {other:?}; still drawing the \
                     one it was"
                );
            }
        };
        let Some(window) = self.window_holding(id) else {
            return diag::warn!("{from}: {GUI_CLOCK} {id}: no window holds it");
        };
        self.set_head_clock_of(id, head);
        effects.push(HostEffect::Redraw(window));
        diag::info!("{from}: {GUI_CLOCK} {id}: playheads now read the {which}");
    }

    /// The window `id` is, or the one holding widget `id`.
    pub(super) fn window_holding(&self, id: i32) -> Option<i32> {
        if self.window_defs.contains_key(&id) {
            return Some(id);
        }
        self.window_defs
            .iter()
            .find(|(_, tree)| tree.find(id).is_some())
            .map(|(def, _)| *def)
    }
}
