//! **What a window costs while it is open, and why** -- the host's record of
//! its recurring work, read by both fronts and answered on the wire
//! ([`GUI_ACTIVITY`]).
//!
//! A window does work for two kinds of reason. Something happened -- a key, a
//! `/gui_set`, a reply -- and a frame answers it once. Or something *may*
//! happen with nothing to say so -- a level on a bus, a transport's position,
//! a bound control -- and the front has to look on a period: the frame tick,
//! a query to the server, a stream it subscribes. The second kind is what a
//! settled window must not keep without a reason, so the front states its
//! reasons here ([`Reason`]), and counts what each tick did.
//!
//! Two rules keep the tick honest, and both live outside this module:
//!
//! - **A tick that found nothing moved draws nothing**: an element's
//!   [`tick`](super::widget::Element::tick) answers whether what it shows
//!   changed, and a playhead whose counter stands is a frame already on
//!   screen ([`Host::playheads_moved`]).
//! - **A transport that is stopped is told, not watched**: its moves are
//!   pushed by the server, so a playhead over it asks for the tick only while
//!   it rolls and briefly after each push ([`Host::transport_ticks`]).
//!
//! What is counted is the front's own work, by window: ticks, frames by what
//! they drew and how long they took, the queries a tick sends and the bytes a
//! download brings. A query answered for the whole page -- one `/clock_query`
//! however many canvases draw from the device clock -- is counted on every
//! window that asked for it.

use std::collections::HashMap;
use std::time::Duration;

use clausters_core::osc::{OscMessage, OscType};

use super::Host;

/// `/gui_activity <window>` -- what window `window` costs: replies
/// [`GUI_ACTIVITY_REPLY`] with its counters since it opened and the reasons
/// it is ticking now.
pub const GUI_ACTIVITY: &str = "/gui_activity";
/// `/gui_activity.reply <window> <key> <value> ...` -- the answer to a
/// [`GUI_ACTIVITY`]. An empty reply (the window id alone) is a window the host
/// does not have.
pub const GUI_ACTIVITY_REPLY: &str = "/gui_activity.reply";

/// How many ticks a playhead over a stopped transport keeps after the server
/// said the transport moved: the position a push reports is the engine's
/// after the command, and a front reading a shared segment may read the
/// counter a block before it lands. A quarter of a second at the frame rate.
pub const SETTLE_TICKS: u32 = 8;

/// **Why a window is ticking**: each is a source of change that sends no
/// message when it changes, so the front looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Reason {
    /// An element moves on its own -- a `canvas` shader, a progress bar with
    /// no value, a meter whose mark is still falling.
    Animated,
    /// An element reads a bus, a level or a tap: a value the server writes
    /// with nothing to say it did.
    Fed,
    /// A playhead whose counter can move: the device clock, or a transport
    /// that rolls or has just been told to move.
    Playhead,
    /// A take that is being recorded, drawn as it fills.
    Recording,
    /// A download in flight, whose replies are read on the tick.
    Fetching,
    /// A gesture timer: a tip waiting for the pointer to rest, a held press.
    Timer,
    /// A clip held against a lane's edge, scrolling the view under it.
    EdgeScroll,
    /// A control bound to the server, read back on its period.
    ReadBack,
    /// A node tree, queried again on its period.
    NodeTree,
    /// A live MIDI input port, drained on the tick.
    Midi,
}

impl Reason {
    /// The word the wire and the clients name it by.
    pub fn name(self) -> &'static str {
        match self {
            Reason::Animated => "animated",
            Reason::Fed => "fed",
            Reason::Playhead => "playhead",
            Reason::Recording => "recording",
            Reason::Fetching => "fetching",
            Reason::Timer => "timer",
            Reason::EdgeScroll => "edge_scroll",
            Reason::ReadBack => "read_back",
            Reason::NodeTree => "node_tree",
            Reason::Midi => "midi",
        }
    }
}

/// What one tick, frame or message a front counts against a window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Count {
    /// The frame tick reached the window.
    Tick,
    /// A frame drew the window's whole picture, in this long.
    Whole(Duration),
    /// A frame drew only the live layers over a kept picture.
    Live(Duration),
    /// A frame was asked for and found nothing to draw.
    Still,
    /// A `/clock_query` the window's playheads asked for.
    ClockQuery,
    /// A `/transport_query` the window's playheads asked for.
    TransportQuery,
    /// A read-back of the window's bound controls.
    ReadBack,
    /// A node-tree query the window's views asked for.
    TreeQuery,
    /// A download step the window's views asked for, and the bytes it
    /// brought (`0` for the request).
    Fetch(usize),
    /// What a drawn frame cost, phase by phase.
    Cost(super::frame::FrameCost),
}

/// **One window's counters**, since it opened, with what it ticks for now.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Activity {
    pub ticks: u64,
    pub whole: u64,
    pub live: u64,
    pub still: u64,
    pub whole_time: Duration,
    pub live_time: Duration,
    pub clock_queries: u64,
    pub transport_queries: u64,
    pub read_backs: u64,
    pub tree_queries: u64,
    pub fetches: u64,
    pub fetched_bytes: u64,
    /// The drawn frames' cost by phase, summed (`super::frame::FrameCost`).
    pub layout_time: Duration,
    pub build_time: Duration,
    pub upload_time: Duration,
    pub encode_time: Duration,
    pub present_time: Duration,
    pub uploaded_bytes: u64,
    pub placed: u64,
    /// The buses and taps the front streams for the window now (a page's
    /// subscriptions; `0` natively, where a segment is read and nothing is
    /// streamed).
    pub streamed: u64,
    /// Whether the window can be seen: a native window not covered or
    /// minimized, a canvas in the viewport of a visible tab.
    pub visible: bool,
    /// Why the window ticks now, sorted; empty for a settled window.
    pub demand: Vec<Reason>,
}

impl Activity {
    fn count(&mut self, what: Count) {
        match what {
            Count::Tick => self.ticks += 1,
            Count::Whole(took) => {
                self.whole += 1;
                self.whole_time += took;
            }
            Count::Live(took) => {
                self.live += 1;
                self.live_time += took;
            }
            Count::Still => self.still += 1,
            Count::ClockQuery => self.clock_queries += 1,
            Count::TransportQuery => self.transport_queries += 1,
            Count::ReadBack => self.read_backs += 1,
            Count::TreeQuery => self.tree_queries += 1,
            Count::Fetch(bytes) => {
                self.fetches += 1;
                self.fetched_bytes += bytes as u64;
            }
            Count::Cost(cost) => {
                self.layout_time += cost.layout;
                self.build_time += cost.build;
                self.upload_time += cost.upload;
                self.encode_time += cost.encode;
                self.present_time += cost.present;
                self.uploaded_bytes += cost.uploaded;
                self.placed += cost.placed;
            }
        }
    }

    /// The reply's arguments after the window id: flat key/value pairs, the
    /// demand as one comma-separated string.
    fn args(&self) -> Vec<OscType> {
        let long = |v: u64| OscType::Long(v.min(i64::MAX as u64) as i64);
        let micros = |d: Duration| long(d.as_micros().min(u128::from(u64::MAX)) as u64);
        let demand = self
            .demand
            .iter()
            .map(|r| r.name())
            .collect::<Vec<_>>()
            .join(",");
        let pairs: [(&str, OscType); 22] = [
            ("ticks", long(self.ticks)),
            ("whole", long(self.whole)),
            ("live", long(self.live)),
            ("still", long(self.still)),
            ("whole_us", micros(self.whole_time)),
            ("live_us", micros(self.live_time)),
            ("clock_queries", long(self.clock_queries)),
            ("transport_queries", long(self.transport_queries)),
            ("read_backs", long(self.read_backs)),
            ("tree_queries", long(self.tree_queries)),
            ("fetches", long(self.fetches)),
            ("fetched_bytes", long(self.fetched_bytes)),
            ("layout_us", micros(self.layout_time)),
            ("build_us", micros(self.build_time)),
            ("upload_us", micros(self.upload_time)),
            ("encode_us", micros(self.encode_time)),
            ("present_us", micros(self.present_time)),
            ("uploaded_bytes", long(self.uploaded_bytes)),
            ("placed", long(self.placed)),
            ("streamed", long(self.streamed)),
            ("visible", OscType::Int(i32::from(self.visible))),
            ("demand", OscType::String(demand)),
        ];
        pairs
            .into_iter()
            .flat_map(|(k, v)| [OscType::String(k.into()), v])
            .collect()
    }
}

/// What the host knows of a transport from what the server pushed: whether
/// it rolls, and how many ticks remain of the settling after its last push.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct TransportSeen {
    pub rolling: bool,
    pub position: i64,
    pub settling: u32,
}

/// The host's half: the counters by window, and the transports as the server
/// last described them.
#[derive(Debug, Default)]
pub(crate) struct Activities {
    windows: HashMap<i32, Activity>,
    transports: HashMap<usize, TransportSeen>,
}

impl Host {
    /// Counts `what` against window `def_id`.
    pub fn count(&mut self, def_id: i32, what: Count) {
        self.activity.windows.entry(def_id).or_default().count(what);
    }

    /// States why window `def_id` ticks now, whether it can be seen and what
    /// the front streams for it -- the front's own reading, kept for the
    /// next [`GUI_ACTIVITY`].
    pub fn set_demand(
        &mut self,
        def_id: i32,
        mut demand: Vec<Reason>,
        visible: bool,
        streamed: usize,
    ) {
        demand.sort_unstable();
        demand.dedup();
        let a = self.activity.windows.entry(def_id).or_default();
        a.demand = demand;
        a.visible = visible;
        a.streamed = streamed as u64;
    }

    /// **Whether window `def_id` is drawn at the display's rate**: it can be
    /// seen and a playhead over it moves. A front then asks for the next
    /// frame of what moves as soon as one is drawn, so the presentation paces
    /// the playhead -- the vertical sync natively, the animation frame in a
    /// page -- instead of the frame tick. A line moved by a 30 Hz timer the
    /// display does not follow stays on screen for two refreshes and then
    /// three, which reads as a cursor stepping unevenly; a frame of the live
    /// layers costs about a millisecond, so drawing every refresh while the
    /// transport rolls is the smaller cost.
    pub fn paced_by_display(&self, def_id: i32) -> bool {
        self.activity(def_id)
            .is_some_and(|a| a.visible && a.demand.contains(&Reason::Playhead))
    }

    /// **Whether a front asks for window `def_id`'s next live frame** after a
    /// frame that came to `drawn`: only after one that was presented, since
    /// presenting is what waits for the display. A frame that found nothing
    /// to draw presents nothing and waits for nothing, so asking again after
    /// it spins -- natively, a window looping over a short span asked for
    /// tens of thousands of empty frames a second, a whole core, until its
    /// playhead crossed a pixel. After one, the frame tick carries the
    /// playhead on, and the next frame that draws paces it again. The same
    /// rule in both fronts.
    pub(crate) fn asks_next_frame(&self, def_id: i32, drawn: crate::host::frame::Drawn) -> bool {
        use crate::host::frame::Drawn;
        matches!(drawn, Drawn::Whole | Drawn::Live) && self.paced_by_display(def_id)
    }

    /// Window `def_id`'s counters, if it has any.
    pub fn activity(&self, def_id: i32) -> Option<&Activity> {
        self.activity.windows.get(&def_id)
    }

    /// Forgets window `def_id`'s counters: it closed, and what it cost went
    /// with it.
    pub(crate) fn forget_activity(&mut self, def_id: i32) {
        self.activity.windows.remove(&def_id);
    }

    /// The [`GUI_ACTIVITY_REPLY`] for window `def_id`: its counters, or the id
    /// alone for a window this host does not have.
    pub(crate) fn activity_reply(&self, def_id: i32) -> OscMessage {
        let mut args = vec![OscType::Int(def_id)];
        if self.window_def(def_id).is_some() {
            args.extend(self.activity(def_id).cloned().unwrap_or_default().args());
        }
        OscMessage {
            addr: GUI_ACTIVITY_REPLY.into(),
            args,
        }
    }

    /// **A transport's state, as the server reported it**: whether it rolls
    /// and where it stands (`position`, its `positionSample`). A report that
    /// changes either starts a settling of [`SETTLE_TICKS`] in which a
    /// playhead over it keeps the tick, since whatever moved it may not have
    /// reached the counter a front reads yet. A report that changes nothing
    /// -- the answer to a page's own poll of a transport that stands --
    /// starts none, or the poll would keep itself going.
    pub(crate) fn transport_seen(&mut self, transport: usize, rolling: bool, position: i64) {
        let seen = self
            .activity
            .transports
            .entry(transport)
            .or_insert(TransportSeen {
                rolling: !rolling,
                position,
                settling: 0,
            });
        if seen.rolling != rolling || seen.position != position {
            seen.settling = SETTLE_TICKS;
        }
        seen.rolling = rolling;
        seen.position = position;
    }

    /// **Whether a playhead drawn from `transport` needs the tick**: while it
    /// rolls, while it settles after a push, and while nothing has been heard
    /// of it -- a host the server never told cannot know it stands.
    pub fn transport_ticks(&self, transport: usize) -> bool {
        self.activity
            .transports
            .get(&transport)
            .is_none_or(|seen| seen.rolling || seen.settling > 0)
    }

    /// One frame tick of the transports' settling, run once per tick by the
    /// front.
    pub fn settle_transports(&mut self) {
        for seen in self.activity.transports.values_mut() {
            seen.settling = seen.settling.saturating_sub(1);
        }
    }

    /// **Whether window `def_id`'s playheads need the tick**: it draws one,
    /// and one of the counters it draws from can move -- the device clock,
    /// which never stops, or a transport that [ticks](Self::transport_ticks).
    pub fn playhead_ticks(&self, def_id: i32) -> bool {
        let Some(tree) = self.window_def(def_id) else {
            return false;
        };
        if !super::live::tree_has_playhead(tree, self.timelines()) {
            return false;
        }
        self.device_drawn(def_id)
            || self
                .transports_drawn(def_id)
                .into_iter()
                .any(|t| self.transport_ticks(t))
    }

    /// **Why window `def_id`'s tree asks for the tick**, from what it holds:
    /// what moves on its own, what is fed, a playhead that can move. `segment`
    /// is whether the front reads a source a fed element can be filled from
    /// (the shared segment natively, the streams in a page). What only the
    /// front knows -- a recording, a download, a gesture -- the front adds.
    pub fn tree_demand(&self, def_id: i32, segment: bool) -> Vec<Reason> {
        let Some(tree) = self.window_def(def_id) else {
            return Vec::new();
        };
        let live = super::live::LiveNeeds::of(tree);
        let mut out = Vec::new();
        if live.animated {
            out.push(Reason::Animated);
        }
        if segment && live.fed {
            out.push(Reason::Fed);
        }
        if self.playhead_ticks(def_id) {
            out.push(Reason::Playhead);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **A frame's cost adds up by phase**, and the reply carries each phase
    /// beside the frame counts, so a profile can say where frames go.
    #[test]
    fn a_frame_cost_adds_up_by_phase_on_the_wire() {
        let mut a = Activity::default();
        let cost = super::super::frame::FrameCost {
            layout: Duration::from_micros(100),
            build: Duration::from_micros(300),
            upload: Duration::from_micros(50),
            encode: Duration::from_micros(20),
            present: Duration::from_micros(500),
            uploaded: 4096,
            placed: 12,
        };
        a.count(Count::Cost(cost));
        a.count(Count::Cost(cost));
        assert_eq!(a.layout_time, Duration::from_micros(200));
        assert_eq!((a.uploaded_bytes, a.placed), (8192, 24));
        let args = a.args();
        let value = |key: &str| {
            let at = args
                .iter()
                .position(|v| *v == OscType::String(key.into()))
                .unwrap();
            args[at + 1].clone()
        };
        assert_eq!(value("build_us"), OscType::Long(600));
        assert_eq!(value("present_us"), OscType::Long(1000));
        assert_eq!(value("uploaded_bytes"), OscType::Long(8192));
        assert_eq!(value("placed"), OscType::Long(24));
    }

    #[test]
    fn a_transport_ticks_until_told_it_stands_and_settles() {
        let mut host = Host::new();
        assert!(host.transport_ticks(3), "nothing heard: it may be rolling");
        host.transport_seen(3, false, 0);
        for _ in 0..SETTLE_TICKS {
            assert!(host.transport_ticks(3), "a push settles first");
            host.settle_transports();
        }
        assert!(!host.transport_ticks(3), "a stopped transport is told");
        host.transport_seen(3, false, 0);
        assert!(
            !host.transport_ticks(3),
            "a report that changes nothing -- a poll's answer -- settles nothing"
        );
        host.transport_seen(3, false, 480);
        assert!(host.transport_ticks(3), "a locate settles");
        for _ in 0..SETTLE_TICKS {
            host.settle_transports();
        }
        host.transport_seen(3, true, 480);
        for _ in 0..SETTLE_TICKS * 2 {
            host.settle_transports();
        }
        assert!(host.transport_ticks(3), "a rolling one is watched");
    }

    /// **Only a presented frame asks for the next one**: a frame that drew
    /// nothing waits for no refresh, so asking again after it spins.
    #[test]
    fn a_frame_that_drew_nothing_does_not_ask_for_the_next() {
        use crate::host::frame::Drawn;
        let mut host = Host::new();
        host.set_demand(7, vec![Reason::Playhead], true, 0);
        assert!(host.asks_next_frame(7, Drawn::Live));
        assert!(host.asks_next_frame(7, Drawn::Whole));
        assert!(
            !host.asks_next_frame(7, Drawn::Still),
            "the tick carries it on"
        );
        assert!(!host.asks_next_frame(7, Drawn::Owed));
        host.set_demand(7, vec![Reason::Playhead], false, 0);
        assert!(
            !host.asks_next_frame(7, Drawn::Live),
            "a window out of view"
        );
    }

    #[test]
    fn the_reply_names_every_counter_and_the_demand() {
        let mut a = Activity::default();
        a.count(Count::Tick);
        a.count(Count::Live(Duration::from_micros(250)));
        a.count(Count::Fetch(1024));
        a.demand = vec![Reason::Fed, Reason::Playhead];
        let args = a.args();
        let get = |key: &str| {
            args.chunks(2)
                .find(|kv| kv[0] == OscType::String(key.into()))
                .map(|kv| kv[1].clone())
        };
        assert_eq!(get("ticks"), Some(OscType::Long(1)));
        assert_eq!(get("live_us"), Some(OscType::Long(250)));
        assert_eq!(get("fetched_bytes"), Some(OscType::Long(1024)));
        assert_eq!(get("demand"), Some(OscType::String("fed,playhead".into())));
    }
}

/// **The audio editor's window, as the application draws it**: what it asks
/// for stopped, playing and closed. The window is the one
/// `clausters_apps::audio::editor` composes, so a change there that makes a
/// settled editor tick fails here.
#[cfg(test)]
mod editor_tests {
    use super::*;
    use crate::host::{ClientId, GUI_DEF, GUI_FREE, GUI_SET, HeadClock};
    use clausters_apps::audio::editor::new_json;
    use clausters_core::osc::OscPacket;
    use std::net::{Ipv4Addr, SocketAddr};

    const WINDOW: i32 = 10;
    const METER: i32 = 20;

    fn from() -> ClientId {
        ClientId::Udp(SocketAddr::from((Ipv4Addr::LOCALHOST, 9000)))
    }

    fn send(host: &mut Host, addr: &str, args: Vec<OscType>) {
        host.handle_packet(
            OscPacket::Message(OscMessage {
                addr: addr.into(),
                args,
            }),
            from(),
        );
    }

    /// A `/transport_query.reply` push: `playing` at 3, the position at 7,
    /// the transport at 12.
    fn push_at(host: &mut Host, transport: i32, playing: bool, position: i64) {
        let mut args = vec![OscType::Int(0); 15];
        args[3] = OscType::Int(i32::from(playing));
        args[7] = OscType::Long(position);
        args[12] = OscType::Int(transport);
        host.on_transport_state(&args);
    }

    fn push(host: &mut Host, transport: i32, playing: bool) {
        push_at(host, transport, playing, 0);
    }

    /// An editor of a stereo take, stopped: its meter reads no bus and its
    /// playheads read transport 0, as both clients set them.
    fn editor_window() -> Host {
        let mut editor =
            new_json(r#"{"take":1,"frames":48000,"channels":2,"rate":48000}"#).unwrap();
        editor.set_meters(Some((-2, 2)));
        let mut def = editor.window(WINDOW, Some(METER));
        // A client names every node it sends, as both clients' drawers do:
        // the host registers only what has an id.
        fn name(node: &mut serde_json::Value, next: &mut i64) {
            if node.get("id").is_none() {
                node["id"] = (*next).into();
                *next += 1;
            }
            if let Some(children) = node.get_mut("children").and_then(|c| c.as_array_mut()) {
                for child in children {
                    name(child, next);
                }
            }
        }
        name(&mut def, &mut 100_000);
        let mut host = Host::new();
        send(
            &mut host,
            GUI_DEF,
            vec![OscType::Int(WINDOW), OscType::String(def.to_string())],
        );
        assert!(
            host.window_def(WINDOW)
                .is_some_and(|tree| tree.find(METER).is_some()),
            "the editor draws its meter"
        );
        host.set_head_clock_of(WINDOW, HeadClock::Transport(0));
        host
    }

    fn settle(host: &mut Host) {
        for _ in 0..SETTLE_TICKS {
            host.settle_transports();
        }
    }

    #[test]
    fn a_settled_stopped_editor_asks_for_nothing() {
        let mut host = editor_window();
        assert_eq!(
            host.tree_demand(WINDOW, true),
            vec![Reason::Playhead],
            "until the server says where the transport stands"
        );
        push(&mut host, 0, false);
        assert_eq!(host.tree_demand(WINDOW, true), vec![Reason::Playhead]);
        settle(&mut host);
        assert!(
            host.tree_demand(WINDOW, true).is_empty(),
            "a stopped editor has no recurring work"
        );
    }

    #[test]
    fn a_playing_editor_reads_its_meter_and_its_transport() {
        let mut host = editor_window();
        push(&mut host, 0, true);
        settle(&mut host);
        send(
            &mut host,
            GUI_SET,
            vec![
                OscType::Int(METER),
                OscType::String("bus".into()),
                OscType::Int(5),
            ],
        );
        assert_eq!(
            host.tree_demand(WINDOW, true),
            vec![Reason::Fed, Reason::Playhead]
        );
        // Stopping gives the meter's bus back and the transport stands.
        send(
            &mut host,
            GUI_SET,
            vec![
                OscType::Int(METER),
                OscType::String("bus".into()),
                OscType::Int(-2),
            ],
        );
        push(&mut host, 0, false);
        settle(&mut host);
        assert!(host.tree_demand(WINDOW, true).is_empty());
    }

    #[test]
    fn a_locate_while_stopped_is_watched_briefly() {
        let mut host = editor_window();
        push(&mut host, 0, false);
        settle(&mut host);
        push(&mut host, 0, false);
        assert!(
            host.tree_demand(WINDOW, true).is_empty(),
            "the same report again is a poll's answer"
        );
        push_at(&mut host, 0, false, 4800);
        assert_eq!(
            host.tree_demand(WINDOW, true),
            vec![Reason::Playhead],
            "a locate settles before it stands"
        );
    }

    #[test]
    fn closing_the_window_lets_go_of_its_demand_and_counters() {
        let mut host = editor_window();
        host.count(WINDOW, Count::Tick);
        host.set_demand(WINDOW, vec![Reason::Playhead], true, 0);
        send(&mut host, GUI_FREE, vec![OscType::Int(WINDOW)]);
        assert!(host.tree_demand(WINDOW, true).is_empty());
        assert!(host.activity(WINDOW).is_none());
        let reply = host.activity_reply(WINDOW);
        assert_eq!(reply.args, vec![OscType::Int(WINDOW)], "no such window");
    }

    #[test]
    fn the_wire_answers_what_the_window_costs() {
        let mut host = editor_window();
        host.count(WINDOW, Count::Tick);
        host.count(WINDOW, Count::Live(Duration::from_micros(40)));
        let effects = host.handle_packet(
            OscPacket::Message(OscMessage {
                addr: GUI_ACTIVITY.into(),
                args: vec![OscType::Int(WINDOW)],
            }),
            from(),
        );
        let [crate::host::HostEffect::Reply(reply)] = &effects[..] else {
            panic!("one reply: {effects:?}");
        };
        assert_eq!(reply.addr, GUI_ACTIVITY_REPLY);
        assert_eq!(reply.args[0], OscType::Int(WINDOW));
        assert!(reply.args.contains(&OscType::String("ticks".into())));
        assert!(reply.args.contains(&OscType::Long(1)));
    }
}
