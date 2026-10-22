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
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
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

    /// **Whether window `def_id`'s playheads moved since the frame tick last
    /// asked**: `clocks` is what they read now ([`Host::head_clocks`]), kept
    /// when it differs from what they read then.
    ///
    /// It is what a window that shows a playhead and nothing fed live
    /// ([`live::LiveNeeds`]) is repainted by. A
    /// playhead is drawn from a counter, and a transport that is stopped
    /// holds its position: the line is where it was, and a frame drawn for it
    /// is the frame already on screen. An editor's window sat at thirty
    /// frames a second for as long as it was open, playing or not, because
    /// its anchor said a line *could* move.
    pub fn playheads_moved(&mut self, def_id: i32, clocks: HeadClocks) -> bool {
        if self.drawn_clocks.get(&def_id) == Some(&clocks) {
            return false;
        }
        self.drawn_clocks.insert(def_id, clocks);
        true
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
        let now = web_time::Instant::now();
        let read = |head: HeadClock| {
            let value = match head {
                HeadClock::Device => bus.sample_clock(),
                HeadClock::Transport(transport) => bus.transport_position(transport),
            };
            self.glided(head, value, now)
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
        // A pane owns its members' time axis and draws their play cursor, so
        // it reads the transport at its axis' rate, which is its take's.
        let engine = self.server_rate;
        let rate_of = |w: &Widget| match &w.kind {
            WidgetKind::Pane { editor, .. } => {
                (editor.sample_rate > 0.0).then_some(editor.sample_rate)
            }
            kind => kind.as_samples().and_then(|samples| samples.samples_rate()),
        };
        let scale = |w: &Widget, head: HeadClock| match head {
            HeadClock::Transport(_) if engine > 0.0 => rate_of(w).map_or(1.0, |rate| rate / engine),
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

    /// **Counter `head` read as `value` at `now`, carried on between blocks**:
    /// see [`Glide`].
    fn glided(&self, head: HeadClock, value: f64, now: web_time::Instant) -> f64 {
        let mut glides = self.glides.borrow_mut();
        let glide = glides.entry(head).or_insert_with(|| Glide::new(value, now));
        glide.seen(value, now, self.server_rate);
        glide.at(now, self.server_rate)
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

/// **A counter carried on between the blocks that publish it.**
///
/// The engine publishes its clocks once per audio block, which is the
/// device's period and not the display's: at 1024 frames, 47 times a second,
/// so on a 60 Hz display two frames in five found the play cursor where the
/// last one drew it and the line moved in uneven steps. A rolling counter is
/// therefore drawn where it is *now* -- the last block's reading plus the
/// time since it was seen, at the engine's rate -- which is the reading the
/// next block will publish. The rest keeps it honest:
///
/// - **Rolling** means it moved forward within a few periods of its last
///   move, and by no more than that time plus a period at the engine's rate;
///   anything else is a locate, and a counter that only jumps is not carried. A jump back while
///   rolling is a loop wrapping and keeps the roll.
/// - The carry is at most one **period** (the average interval between
///   blocks), and past a period and a half with no block the counter has
///   stopped and the reading is drawn as it is.
/// - While rolling the line never steps back: a block that publishes less
///   than was drawn holds the line until the reading passes it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Glide {
    /// The last block's reading, and when it was first seen.
    value: f64,
    seen_at: web_time::Instant,
    /// The average interval between blocks, while rolling.
    period: Option<f64>,
    /// The last reading drawn.
    drawn: f64,
}

impl Glide {
    fn new(value: f64, now: web_time::Instant) -> Self {
        Self {
            value,
            seen_at: now,
            period: None,
            drawn: value,
        }
    }

    /// A reading of `value` at `now`, at `rate` samples a second.
    fn seen(&mut self, value: f64, now: web_time::Instant, rate: f64) {
        if value == self.value {
            return;
        }
        let dt = now.duration_since(self.seen_at).as_secs_f64();
        let step = value - self.value;
        let longest = self.period.unwrap_or(0.1);
        // a block comes within a few periods of the last; after a longer
        // silence the counter was stopped, and whatever it moved by is a locate
        let rolled = dt <= longest * 3.0 && step <= (dt + longest) * rate;
        self.period = if step > 0.0 && rate > 0.0 && rolled {
            // the average over a few blocks, since a callback may come in bursts
            Some(
                self.period
                    .map_or(dt, |p| p + (dt - p) * 0.25)
                    .clamp(0.001, 0.25),
            )
        } else if step < 0.0 {
            // a wrap while rolling stays rolling, and draws from the new place
            self.drawn = value;
            self.period
        } else {
            self.drawn = value;
            None
        };
        self.value = value;
        self.seen_at = now;
    }

    /// Where the counter is drawn at `now`.
    fn at(&mut self, now: web_time::Instant, rate: f64) -> f64 {
        let Some(period) = self.period.filter(|_| rate > 0.0) else {
            self.drawn = self.value;
            return self.value;
        };
        let since = now.duration_since(self.seen_at).as_secs_f64();
        if since > period * 1.5 {
            // stopped: the reading as it is
            self.drawn = self.value;
            return self.value;
        }
        let ahead = (self.value + since.min(period) * rate).floor();
        self.drawn = self.drawn.max(ahead);
        self.drawn
    }
}

#[cfg(test)]
mod glide_tests {
    use super::Glide;
    use web_time::{Duration, Instant};

    const RATE: f64 = 48_000.0;
    const BLOCK: f64 = 1024.0;

    fn ms(n: f64) -> Duration {
        Duration::from_secs_f64(n / 1000.0)
    }

    /// Blocks every 21.3 ms, read every 16.7 ms: the line moves every frame,
    /// forward, and lands on each block's reading rather than past it.
    #[test]
    fn a_rolling_counter_moves_every_frame_between_blocks() {
        let t0 = Instant::now();
        let mut g = Glide::new(0.0, t0);
        let period = BLOCK / RATE * 1000.0;
        let mut last = -1.0;
        let mut moved = 0;
        for frame in 1..60 {
            let now = t0 + ms(f64::from(frame) * 16.7);
            let blocks = (now.duration_since(t0).as_secs_f64() * 1000.0 / period).floor();
            g.seen(blocks * BLOCK, now, RATE);
            let at = g.at(now, RATE);
            assert!(at >= last, "never back: {at} after {last}");
            assert!(
                at <= (blocks + 1.0) * BLOCK,
                "no further than the next block"
            );
            if at > last {
                moved += 1;
            }
            last = at;
        }
        assert!(moved >= 57, "moved on {moved} of 59 frames");
    }

    /// A counter that stops is drawn where it stopped once a period and a
    /// half passes with no block.
    #[test]
    fn a_stopped_counter_is_drawn_as_it_reads() {
        let t0 = Instant::now();
        let mut g = Glide::new(0.0, t0);
        for i in 1..=4 {
            g.seen(f64::from(i) * BLOCK, t0 + ms(f64::from(i) * 21.3), RATE);
        }
        let last = t0 + ms(4.0 * 21.3);
        assert!(
            g.at(last + ms(10.0), RATE) > 4.0 * BLOCK,
            "carried while rolling"
        );
        assert_eq!(g.at(last + ms(40.0), RATE), 4.0 * BLOCK, "stopped");
    }

    /// A locate after a stop is not a roll, however near it lands.
    #[test]
    fn a_locate_after_a_stop_is_not_carried() {
        let t0 = Instant::now();
        let mut g = Glide::new(0.0, t0);
        let later = t0 + Duration::from_secs(10);
        g.seen(2.0 * RATE, later, RATE);
        assert_eq!(g.at(later + ms(100.0), RATE), 2.0 * RATE);
    }

    /// A locate is not a roll, and a wrap while rolling draws from where it
    /// wrapped to.
    #[test]
    fn a_locate_is_drawn_where_it_lands_and_a_wrap_keeps_rolling() {
        let t0 = Instant::now();
        let mut g = Glide::new(0.0, t0);
        g.seen(96_000.0, t0 + ms(5.0), RATE);
        assert_eq!(g.at(t0 + ms(10.0), RATE), 96_000.0, "a jump is not carried");
        for i in 1..=4 {
            g.seen(
                96_000.0 + f64::from(i) * BLOCK,
                t0 + ms(5.0 + f64::from(i) * 21.3),
                RATE,
            );
        }
        let wrapped = t0 + ms(5.0 + 5.0 * 21.3);
        g.seen(1_000.0, wrapped, RATE);
        let at = g.at(wrapped + ms(8.0), RATE);
        assert!(
            at > 1_000.0 && at < 1_000.0 + BLOCK,
            "rolls on from the wrap: {at}"
        );
    }
}
