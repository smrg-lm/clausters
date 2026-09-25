//! **The destructive edit, end to end inside the host**: a stroke over a take's
//! samples reaches the server's buffer and the picture of it at once, and the
//! log takes it back.
//!
//! What is exercised here is the seam the milestone added -- [`Host::can_write`]
//! refusing what the wire cannot carry, [`Host::write_buffer_samples`] sending and
//! patching, and the inverse the hand supplies coming back through an undo. The
//! hand itself (the drag that builds the payload) is tested in `gestures`, and
//! the server's own `/buffer_setRange` in the server crate; this is where the
//! three meet.

use super::*;
use crate::host::document::Owner;
use crate::host::widget::element::Loaded;
use crate::waveform::WaveformData;
use clausters_document::{Body, Document, Lifetime, Node, NodeId, Opaque, SourceRef};
use std::net::{SocketAddr, UdpSocket};
use std::time::Duration;

/// The session's own picture, in miniature: a take drawn **twice** -- as a
/// clip in a lane, and as the navigable editor under it -- both naming the
/// one buffer.
const TREE: &str = r#"{"type":"window","children":[
    {"id":50,"type":"signal","view":"trace","buffer":0,"navigable":1}
]}"#;

fn from() -> ClientId {
    ClientId::Udp("127.0.0.1:9000".parse::<SocketAddr>().unwrap())
}

fn blob(values: &[f32]) -> OscType {
    OscType::Blob(values.iter().flat_map(|v| v.to_le_bytes()).collect())
}

/// A host drawing `channels` interleaved channels of `frames` frames of
/// silence out of server buffer 0, with a document behind it whose one node
/// is that take, and a throwaway socket standing in for the audio server.
fn take_host(channels: usize, frames: usize) -> (Host, UdpSocket) {
    let server = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
    server
        .set_read_timeout(Some(Duration::from_millis(500)))
        .unwrap();
    let leg = ServerLeg::connect(server.local_addr().unwrap()).unwrap();

    let mut host = Host::new().with_server(leg);
    host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: GUI_DEF.into(),
            args: vec![OscType::Int(1), OscType::String(TREE.into())],
        }),
        from(),
    );
    // The form the samples arrive in, which is the whole reason a write
    // goes through the element: the navigable view keeps a pyramid.
    let samples = vec![0.0f32; frames * channels];
    let data = std::sync::Arc::new(WaveformData::from_interleaved(&samples, channels, 64));
    host.window_def_mut(1)
        .and_then(|t| t.find_mut(50))
        .expect("the waveform")
        .take_bulk(|| Loaded::Peaks(data.clone()))
        .then_some(())
        .expect("the element took the pyramid");

    let mut owner = Owner::new(Document::new(Node::new(
        NodeId(2),
        Body::Vector {
            source: SourceRef {
                source: clausters_document::SourceId(1),
                lifetime: Lifetime::Session,
                generation: 0,
                range: None,
            },
            config: Opaque::default(),
        },
    )));
    owner.bind(50, NodeId(2));
    host.owner = Some(owner);
    (host, server)
}

/// **What the server that sounds was sent, with its waits answered**:
/// every message the fake server got, in order, each awaited transport
/// command answered with its `/done` and each barrier with its reply --
/// what a server does, so the monitor's steps walk to the end.
fn exchange(host: &mut Host, server: &UdpSocket) -> Vec<OscMessage> {
    server
        .set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    let mut out = Vec::new();
    while let Some(m) = received(server) {
        let reply = match m.addr.as_str() {
            "/server_sync" => Some(OscMessage {
                addr: "/server_sync.reply".into(),
                args: vec![m.args[0].clone()],
            }),
            addr if addr.starts_with("/transport_") => Some(OscMessage {
                addr: "/done".into(),
                args: vec![OscType::String(addr.into())],
            }),
            _ => None,
        };
        out.push(m);
        if let Some(reply) = reply {
            host.on_server_reply(instance::Leg::Server, &reply);
        }
    }
    out
}

/// The addresses among `sent`, the defs left out.
fn addrs(sent: &[OscMessage]) -> Vec<&str> {
    sent.iter()
        .map(|m| m.addr.as_str())
        .filter(|a| *a != "/def_send")
        .collect()
}

/// The one message to `addr` among `sent`.
fn one<'a>(sent: &'a [OscMessage], addr: &str) -> &'a OscMessage {
    let found: Vec<&OscMessage> = sent.iter().filter(|m| m.addr == addr).collect();
    assert_eq!(found.len(), 1, "one {addr} in {:?}", addrs(sent));
    found[0]
}

/// What the monitor sends the first time it plays: the editor's
/// structure, on the monitor's own transport, then the take.
const BUILT: [&str; 8] = [
    "/server_sync",
    "/group_new",
    "/transport_follow",
    "/group_new",
    "/transport_group",
    "/transport_fade",
    "/graph_new",
    "/graph_new",
];

/// What a play sends once the take is made: how the pass ends, where it
/// starts, and the transport rolling.
const PASS: [&str; 4] = [
    "/transport_loop",
    "/transport_end",
    "/transport_locateSample",
    "/transport_play",
];

/// The message the fake server received, or `None` if it sent nothing.
fn received(server: &UdpSocket) -> Option<OscMessage> {
    let mut buf = [0u8; 4096];
    let (len, _) = server.recv_from(&mut buf).ok()?;
    match clausters_core::osc::decode_packet(&buf[..len]).ok()? {
        OscPacket::Message(m) => Some(m),
        _ => None,
    }
}

/// What the widget's own picture holds at frame `i` -- read back through the
/// same door the drawing gesture reads it through, so the test sees what the
/// hand would.
fn sample(host: &Host, i: usize) -> f32 {
    sample_of(host, 50, i)
}

/// The same, for whichever of the two views is being asked.
fn sample_of(host: &Host, widget: i32, i: usize) -> f32 {
    sample_of_channel(host, widget, 0, i)
}

/// ...and of whichever channel.
fn sample_of_channel(host: &Host, widget: i32, ch: usize, i: usize) -> f32 {
    host.window_def(1)
        .and_then(|t| t.find(widget))
        .and_then(|w| {
            std::iter::once(w)
                .chain(w.children.iter())
                .find_map(|w| w.kind.sample_value(ch, i))
        })
        .expect("samples")
}

#[test]
fn a_stroke_reaches_the_buffer_and_the_picture_and_undo_puts_both_back() {
    let (mut host, server) = take_host(1, 16);
    let seq = host.outbox.borrow_mut().stamp(1, 50);
    assert!(host.answer_own(
        1,
        50,
        seq,
        &[
            OscType::String("draw".into()),
            OscType::Int(0),
            OscType::Long(4),
            blob(&[0.5, -0.5, 0.25]),
            blob(&[0.0, 0.0, 0.0]),
        ]
    ));

    let msg = received(&server).expect("the take's buffer was written");
    assert_eq!(msg.addr, "/buffer_setRangeChannel");
    assert_eq!(msg.args[0], OscType::Int(0), "buffer 0");
    assert_eq!(msg.args[1], OscType::Int(0), "channel 0");
    assert_eq!(msg.args[2], OscType::Int(4), "at the frame the hand named");
    assert_eq!(msg.args[3], blob(&[0.5, -0.5, 0.25]), "the run it drew");

    assert_eq!(sample(&host, 4), 0.5, "and the picture holds the same run");
    assert_eq!(sample(&host, 6), 0.25);
    assert_eq!(sample(&host, 7), 0.0, "past the span, nothing moved");

    // The document does not hold samples, so the undo restores them only
    // because the payload carried the run it painted over.
    let seq = host.outbox.borrow_mut().stamp(1, 1);
    assert!(host.answer_own(1, 1, seq, &[OscType::String("undo".into())]));
    let msg = received(&server).expect("the restore went to the server too");
    assert_eq!(msg.addr, "/buffer_setRangeChannel");
    assert_eq!(msg.args[3], blob(&[0.0, 0.0, 0.0]));
    assert_eq!(sample(&host, 4), 0.0, "the picture went back with it");
}

/// **The monitor plays the samples the window is drawing** -- the same
/// buffer, reached by the same widget lookup an edit takes, so what sounds
/// is what would be written -- through the audio editor's nodes, on a
/// transport of its own.
#[test]
fn the_monitor_plays_a_takes_buffer_and_stops_it() {
    let (mut host, server) = take_host(1, 16);
    assert!(
        host.play_buffer(1, 50, 0, play::Pass::Until { end: 16, back: 0 }),
        "a take with a buffer plays"
    );
    let sent = exchange(&mut host, &server);
    let mut want = BUILT.to_vec();
    want.push("/graph_addSlot");
    want.extend(PASS);
    assert_eq!(addrs(&sent), want);
    for addr in [
        "/transport_follow",
        "/transport_group",
        "/transport_fade",
        "/transport_play",
    ] {
        assert_eq!(
            one(&sent, addr).args[0],
            OscType::Int(play::MONITOR_TRANSPORT),
            "{addr} names the monitor's transport"
        );
    }
    assert_eq!(
        one(&sent, "/transport_end").args,
        vec![
            OscType::Int(play::MONITOR_TRANSPORT),
            OscType::Long(16),
            OscType::Long(0)
        ],
        "the take's end, and back to the start"
    );
    // A mono take is one reader, of the buffer the widget draws; its pass
    // puts it on both sides.
    let reader = one(&sent, "/graph_addSlot");
    for pair in [("buf", 0.0), ("chan", 0.0), ("span", 16.0)] {
        assert!(
            reader
                .args
                .windows(2)
                .any(|w| w[0] == OscType::String(pair.0.into()) && w[1] == OscType::Float(pair.1)),
            "{} is {}: {:?}",
            pair.0,
            pair.1,
            reader.args
        );
    }
    assert_eq!(host.playing_widget(), Some(50));
    let nodes = host.monitor_nodes();

    assert!(host.stop_playback(), "and it stops");
    let sent = exchange(&mut host, &server);
    assert_eq!(addrs(&sent), ["/transport_stop"], "the transport alone");
    assert_eq!(host.monitor_nodes(), nodes, "the readers stay, frozen");
    assert!(!host.stop_playback(), "stopping twice sends nothing");
    assert!(host.playing_widget().is_none());
}

/// **The play cursor is the transport's position**, before, during and
/// after a pass: an anchor of 0 on the transport's clock is the take's own
/// frame, and a stop leaves it anchored, since the stop puts the position
/// back on the mark.
#[test]
fn the_monitor_draws_the_play_cursor_from_the_transport() {
    let (mut host, _server) = take_host(1, 16);
    let key = host.timeline_key(50).expect("the take is on a timeline");
    assert!(host.play_buffer(1, 50, 0, play::Pass::Until { end: 16, back: 0 }));
    assert_eq!(
        host.head_clock_of(1, Some(50)),
        HeadClock::Transport(play::MONITOR_TRANSPORT as usize),
        "the take's view, and nothing else in the host"
    );
    assert_eq!(host.timelines().state(key).unwrap().playhead_at, 0.0);
    assert!(host.stop_playback());
    assert_eq!(host.timelines().state(key).unwrap().playhead_at, 0.0);
}

/// Where the pointer is over widget 50, for a gesture aimed at the take.
fn over_the_take(host: &Host, ctx: &gestures::GestureCtx) -> (f64, f64) {
    let area = host.content_area(ctx.def_id, ctx.fb_w, ctx.fb_h);
    let r = layout::layout(area, host.window_def(1).unwrap(), host.metrics_for(1))
        .into_iter()
        .find(|p| p.widget.id == Some(50))
        .expect("the take is placed")
        .rect;
    ((r.x + r.w * 0.5) as f64, (r.y + r.h * 0.5) as f64)
}

/// **Space plays from the position cursor, and a second press stops and
/// goes back to it** -- the transport is located at the mark, not left
/// wherever the pass ended, so the next press plays from the same place.
#[test]
fn space_plays_from_the_position_cursor_and_stops_back_at_it() {
    let (mut host, server) = take_host(1, 16);
    let ctx = gestures::GestureCtx::new(1, 800, 400);
    let (cx, cy) = over_the_take(&host, &ctx);
    let g = gestures::Gestures::default();
    host.set_timeline_cursor(50, 4.0);

    assert!(g.play_key(&mut host, &ctx, cx, cy).is_some());
    assert_eq!(host.playing_widget(), Some(50));
    let sent = exchange(&mut host, &server);
    assert_eq!(
        one(&sent, "/transport_end").args,
        vec![
            OscType::Int(play::MONITOR_TRANSPORT),
            OscType::Long(16),
            OscType::Long(4)
        ],
        "the take's end, and back to the mark"
    );
    let locate = one(&sent, "/transport_locateSample");
    assert_eq!(locate.args[1], OscType::Long(4), "from the mark");

    assert!(g.play_key(&mut host, &ctx, cx, cy).is_some());
    assert!(host.playing_widget().is_none(), "the second press stops");
    let sent = exchange(&mut host, &server);
    assert_eq!(addrs(&sent), ["/transport_stop", "/transport_locateSample"]);
    assert_eq!(sent[1].args[1], OscType::Long(4), "and back at the mark");
}

/// **A pass the engine ended is over; a stop this host sent is not taken
/// for one.** The broadcasts are full of "stopped" -- every
/// command before a play says it -- so only a transition counts, and each
/// stop the host sent cancels one.
#[test]
fn an_end_the_engine_reached_ends_the_pass_and_a_hosts_stop_does_not() {
    let state = |playing: i32| {
        let mut args = vec![
            OscType::Long(0),
            OscType::Double(0.0),
            OscType::Int(0),
            OscType::Int(playing),
        ];
        args.extend([OscType::Double(0.0), OscType::Int(1)]);
        args.extend([
            OscType::Long(0),
            OscType::Long(0),
            OscType::Long(0),
            OscType::Long(0),
            OscType::Long(-1),
            OscType::Long(-1),
            OscType::Int(play::MONITOR_TRANSPORT),
        ]);
        OscMessage {
            addr: "/transport_query.reply".into(),
            args,
        }
    };
    let (mut host, _server) = take_host(1, 16);

    // A stop this host sent, then a play: the stop's transition arrives
    // after the new pass started, and it is not that pass's end.
    assert!(host.play_buffer(1, 50, 0, play::Pass::Until { end: 16, back: 0 }));
    host.on_server_reply(instance::Leg::Server, &state(0)); // the commands before it
    host.on_server_reply(instance::Leg::Server, &state(1)); // the play
    assert!(host.stop_playback());
    assert!(host.play_buffer(1, 50, 0, play::Pass::Until { end: 16, back: 0 }));
    host.on_server_reply(instance::Leg::Server, &state(0)); // the host's stop
    host.on_server_reply(instance::Leg::Server, &state(1)); // the new play
    assert_eq!(host.playing_widget(), Some(50), "the new pass stands");

    // Then the engine stops on the mark: nobody here sent that one.
    host.on_server_reply(instance::Leg::Server, &state(0));
    assert!(host.playing_widget().is_none(), "the pass is over");
}

/// **`L` switches the loop, and a play reads it**: looping, a take with no
/// selection repeats whole; not, it stops at its end.
#[test]
fn l_switches_the_monitors_loop() {
    let (mut host, server) = take_host(1, 16);
    let ctx = gestures::GestureCtx::new(1, 800, 400);
    let (cx, cy) = over_the_take(&host, &ctx);
    let g = gestures::Gestures::default();
    assert!(!host.monitor_loops());
    g.loop_key(&mut host, &ctx);
    assert!(host.monitor_loops());
    let said = host.statuses();
    let line = said.get(&1).and_then(|s| s.last()).expect("a line");
    assert_eq!(line.text, "loop on");
    drop(said);

    assert!(g.play_key(&mut host, &ctx, cx, cy).is_some());
    let sent = exchange(&mut host, &server);
    assert_eq!(
        one(&sent, "/transport_loop").args,
        vec![
            OscType::Int(play::MONITOR_TRANSPORT),
            OscType::Long(0),
            OscType::Long(16)
        ],
        "the whole take"
    );
    assert_eq!(
        one(&sent, "/transport_end").args,
        [OscType::Int(play::MONITOR_TRANSPORT)],
        "and no end while it loops"
    );
}

/// **With no pointer, the keys reach the window's one take** -- the first
/// press after a window opens has none, since the pointer is unknown until
/// it moves, and it used to do nothing (found 2026-09-24 by the user).
#[test]
fn with_no_pointer_the_keys_reach_the_windows_one_take() {
    let (mut host, _server) = take_host(1, 16);
    let ctx = gestures::GestureCtx::new(1, 800, 400);
    let g = gestures::Gestures::default();
    assert!(g.ends_key(&mut host, &ctx, true, -1.0, -1.0).is_some());
    let key = host.timeline_key(50).unwrap();
    assert_eq!(host.timelines().state(key).unwrap().cursor(), Some(15.0));
    assert!(g.play_key(&mut host, &ctx, -1.0, -1.0).is_some());
    assert_eq!(host.playing_widget(), Some(50), "space plays it");
}

/// **Home and End put the position cursor at the ends of the take** --
/// frame 0 and the last frame, the one a cursor can stand on.
#[test]
fn home_and_end_put_the_position_cursor_at_the_ends() {
    let (mut host, _server) = take_host(1, 16);
    let ctx = gestures::GestureCtx::new(1, 800, 400);
    let (cx, cy) = over_the_take(&host, &ctx);
    let g = gestures::Gestures::default();
    let key = host.timeline_key(50).unwrap();
    let cursor = |host: &Host| host.timelines().state(key).unwrap().cursor();

    assert!(g.ends_key(&mut host, &ctx, true, cx, cy).is_some());
    assert_eq!(cursor(&host), Some(15.0), "End: the take's last frame");
    assert!(g.ends_key(&mut host, &ctx, false, cx, cy).is_some());
    assert_eq!(cursor(&host), Some(0.0), "Home: its start");
}

/// **The seek and the loop are the transport's**: a span plays by a loop
/// and a locate, and the readers are made once and follow it.
#[test]
fn a_span_plays_by_the_transports_loop_and_locate() {
    let (mut host, server) = take_host(1, 16);
    assert!(host.play_buffer(1, 50, 4, play::Pass::Loop { from: 4, to: 12 }));
    let sent = exchange(&mut host, &server);
    assert_eq!(
        one(&sent, "/transport_loop").args,
        vec![
            OscType::Int(play::MONITOR_TRANSPORT),
            OscType::Long(4),
            OscType::Long(12)
        ],
        "half-open, so the span is the selection's own bounds"
    );
    assert_eq!(
        one(&sent, "/transport_end").args,
        [OscType::Int(play::MONITOR_TRANSPORT)],
        "a loop has no end"
    );
    assert_eq!(
        one(&sent, "/transport_locateSample").args[1],
        OscType::Long(4)
    );

    // The same take again is a play and nothing else: it is made.
    assert!(host.play_buffer(1, 50, 0, play::Pass::Loop { from: 0, to: 16 }));
    let sent = exchange(&mut host, &server);
    assert_eq!(addrs(&sent), PASS, "no node is made twice");
}

/// **Pausing is not stopping**: the readers stay, so resuming continues the
/// sound rather than starting a second copy of it. The freeze is the
/// server's -- this host sends one command and remembers nothing about where
/// the take was.
#[test]
fn pausing_keeps_the_readers_and_resuming_continues() {
    let (mut host, server) = take_host(1, 16);
    assert!(host.play_buffer(1, 50, 0, play::Pass::Until { end: 16, back: 0 }));
    exchange(&mut host, &server);

    assert_eq!(host.pause_playback(), Some(false), "rolling -> paused");
    let sent = exchange(&mut host, &server);
    assert_eq!(
        addrs(&sent),
        ["/transport_stop"],
        "one command, the transport's"
    );
    assert!(host.monitor().is_some(), "and the take is still loaded");

    assert_eq!(host.pause_playback(), Some(true), "paused -> rolling again");
    let sent = exchange(&mut host, &server);
    assert_eq!(addrs(&sent), ["/transport_play"], "and it rolls");
    assert_eq!(host.playing_widget(), Some(50));
}

/// A host nothing gave a server to drive **drives no transport**: it is a
/// guest on somebody else's server, and a script owns its own.
#[test]
fn a_host_given_no_server_owns_no_transport() {
    let (host, _server) = take_host(1, 16);
    assert!(!host.owns_transport());
}

/// **A reader per channel**, which is the server's own convention: the
/// buffer readers are mono, so a stereo take is two readers -- channel
/// `ch` reading itself -- in one play graph, and a stop is the
/// transport's alone.
#[test]
fn a_stereo_take_plays_a_reader_per_channel() {
    let (mut host, server) = take_host(2, 16);
    assert!(host.play_buffer(1, 50, 0, play::Pass::Until { end: 16, back: 0 }));
    let sent = exchange(&mut host, &server);
    let readers: Vec<&OscMessage> = sent.iter().filter(|m| m.addr == "/graph_addSlot").collect();
    assert_eq!(readers.len(), 2);
    for (ch, reader) in readers.iter().enumerate() {
        assert!(
            reader.args.windows(2).any(
                |w| w[0] == OscType::String("chan".into()) && w[1] == OscType::Float(ch as f32)
            ),
            "channel {ch} reads itself: {:?}",
            reader.args
        );
    }
    assert!(host.stop_playback());
    assert_eq!(addrs(&exchange(&mut host, &server)), ["/transport_stop"]);
}

/// **A window whose owner plays it is the owner's**: the space bar is
/// the window's `play` verb, with the loop switch beside it, and the
/// monitor stays out.
#[test]
fn a_window_that_plays_its_own_take_leaves_the_monitor_out() {
    let (mut host, _server) = take_host(1, 16);
    host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: GUI_SET.into(),
            args: vec![
                OscType::Int(1),
                OscType::String("plays".into()),
                OscType::Int(1),
            ],
        }),
        from(),
    );
    assert!(host.window_plays(1));
    let ctx = gestures::GestureCtx::new(1, 800, 400);
    let (cx, cy) = over_the_take(&host, &ctx);
    let g = gestures::Gestures::default();
    assert!(
        g.play_key(&mut host, &ctx, cx, cy).is_none(),
        "not the monitor's"
    );
    assert!(host.playing_widget().is_none());
    assert_eq!(
        host.play_verb(),
        vec![OscType::String("play".into()), OscType::Int(0)]
    );
    host.toggle_monitor_loop();
    assert_eq!(
        host.play_verb()[1],
        OscType::Int(1),
        "the loop rides beside it"
    );
}

/// **One channel of a stereo take**, which is what the server's
/// channel-addressed write is for: the span is frames of that channel on
/// both sides of the seam, and the other channel is not mentioned.
#[test]
fn a_stereo_take_is_written_one_channel_at_a_time() {
    let (mut host, server) = take_host(2, 16);
    let seq = host.outbox.borrow_mut().stamp(1, 50);
    assert!(host.answer_own(
        1,
        50,
        seq,
        &[
            OscType::String("draw".into()),
            OscType::Int(1), // the right channel
            OscType::Long(4),
            blob(&[0.5, -0.5]),
            blob(&[0.0, 0.0]),
        ]
    ));
    let msg = received(&server).expect("the take's buffer was written");
    assert_eq!(msg.addr, "/buffer_setRangeChannel");
    assert_eq!(msg.args[1], OscType::Int(1), "the channel the hand was on");
    assert_eq!(msg.args[2], OscType::Int(4), "in frames of that channel");
    assert_eq!(msg.args[3], blob(&[0.5, -0.5]));

    assert_eq!(sample_of_channel(&host, 50, 1, 4), 0.5, "the picture too");
    assert_eq!(
        sample_of_channel(&host, 50, 0, 4),
        0.0,
        "and the other channel was not touched"
    );

    // The undo knows which channel it is putting back, because the channel
    // travels in the intent rather than being assumed to be the first.
    let seq = host.outbox.borrow_mut().stamp(1, 1);
    assert!(host.answer_own(1, 1, seq, &[OscType::String("undo".into())]));
    let msg = received(&server).expect("the restore went out");
    assert_eq!(msg.args[1], OscType::Int(1), "to the same channel");
    assert_eq!(sample_of_channel(&host, 50, 1, 4), 0.0);
}

/// A channel the take does not have is refused, the way the server
/// refuses one the buffer does not have.
#[test]
fn a_channel_the_take_does_not_have_refuses_the_write() {
    let (mut host, server) = take_host(1, 16);
    let seq = host.outbox.borrow_mut().stamp(1, 50);
    assert!(host.answer_own(
        1,
        50,
        seq,
        &[
            OscType::String("draw".into()),
            OscType::Int(1),
            OscType::Long(4),
            blob(&[0.5]),
            blob(&[0.0]),
        ]
    ));
    assert!(received(&server).is_none(), "nothing was written");
    assert!(
        !host.owner.as_ref().unwrap().can_undo(),
        "and no edit entered the log"
    );
}

/// A span that runs past the end of the take is refused for the same
/// reason and by the same door: what the server holds and what the window
/// draws must stay one thing.
#[test]
fn a_span_past_the_end_refuses_the_write() {
    let (mut host, server) = take_host(1, 8);
    let seq = host.outbox.borrow_mut().stamp(1, 50);
    assert!(host.answer_own(
        1,
        50,
        seq,
        &[
            OscType::String("draw".into()),
            OscType::Int(0),
            OscType::Long(6),
            blob(&[0.5, 0.5, 0.5, 0.5]),
            blob(&[0.0, 0.0, 0.0, 0.0]),
        ]
    ));
    assert!(received(&server).is_none());
    assert!(!host.owner.as_ref().unwrap().can_undo());
}
