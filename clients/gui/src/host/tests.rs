//! The host's protocol, driven the way a client drives it: definitions,
//! sets, queries, frees, acknowledgements, bindings and the counters a
//! playhead reads.

use super::*;
use std::net::{Ipv4Addr, SocketAddr};

fn from() -> ClientId {
    ClientId::Udp(SocketAddr::from((Ipv4Addr::LOCALHOST, 9000)))
}

fn def_msg(id: i32, json: &str) -> OscPacket {
    OscPacket::Message(OscMessage {
        addr: GUI_DEF.into(),
        args: vec![OscType::Int(id), OscType::String(json.into())],
    })
}

/// **A join is asked for again when its stitch is done.** Its box names
/// the buffer in the turn the stitch is sent, so the first ask can find
/// the buffer empty; the server's `/done` is when the samples are there.
#[test]
fn a_stitched_take_is_asked_for_again() {
    let mut host = Host::new();
    host.handle_packet(
        def_msg(
            1,
            r#"{"type":"window","title":"w","children":[
                {"id":10,"type":"multitrack",
                 "lanes":["one","",100,0,0,1,1],
                 "clips":["j","one",0,10,0,"",3]}]}"#,
        ),
        from(),
    );
    let ask = |host: &mut Host| {
        host.window_def_mut(1)
            .and_then(|t| t.find_mut(10))
            .and_then(|w| w.kind.as_samples_mut())
            .map(|el| el.ask_takes(false))
            .expect("a multitrack")
    };
    assert_eq!(ask(&mut host), vec![3], "the first repaint asks");
    assert!(ask(&mut host).is_empty(), "and the next does not");

    let done = |command: &str, bufnum: i32| OscMessage {
        addr: "/done".into(),
        args: vec![OscType::String(command.into()), OscType::Int(bufnum)],
    };
    assert!(host.forget_stitched(&done("/buffer_alloc", 3)).is_empty());
    assert!(host.forget_stitched(&done("/buffer_stitch", 4)).is_empty());
    assert!(ask(&mut host).is_empty(), "nothing else forgets it");

    assert_eq!(host.forget_stitched(&done("/buffer_stitch", 3)), vec![1]);
    assert_eq!(ask(&mut host), vec![3], "made, it is asked for again");

    // **And the `/done` is not the only way to hear it.** When a *client*
    // owns the multitrack it is the client that sends the stitch, so the `/done`
    // goes to the client and never reaches here; what does reach here is
    // the write the server announces to every other peer, which is the same
    // news under another name.
    assert!(ask(&mut host).is_empty(), "the second ask stands");
    assert_eq!(host.forget_take(3), vec![1]);
    assert_eq!(ask(&mut host), vec![3], "written, it is asked for again");
    assert!(
        host.forget_take(4).is_empty(),
        "and a take nobody drew is nobody's"
    );
}

/// **A playhead reads the counter named on its view, its window, or the
/// host.** The segment publishes the device clock, which never stops, and
/// each transport's position, which holds while stopped, jumps on a locate
/// and wraps in a loop. A widget reads the one named on it or its nearest
/// ancestor, then its window's, then the host's default -- so one window
/// can hold two views playing two transports.
#[test]
fn a_playhead_reads_the_counter_named_nearest_it() {
    struct Both;
    impl BusSource for Both {
        fn control(&self, _index: usize) -> f32 {
            0.0
        }
        fn sample_clock(&self) -> f64 {
            48_000.0
        }
        fn transport_position(&self, transport: usize) -> f64 {
            1_200.0 + transport as f64
        }
    }
    const TWO: &str = r#"{"type":"window","children":[
        {"id":20,"type":"layout","flow":"row","children":[
            {"id":21,"type":"knob"},
            {"id":22,"type":"knob"}]},
        {"id":30,"type":"knob"}
    ]}"#;
    let mut host = Host::new();
    host.handle_packet(def_msg(1, TWO), from());
    let bus = Both;
    let clocks = |host: &Host| host.head_clocks(1, Some(&bus as &dyn BusSource));
    assert_eq!(clocks(&host).at(Some(21)), 48_000.0, "the host's default");

    host.set_head_clock(HeadClock::Transport(0));
    assert_eq!(clocks(&host).at(Some(21)), 1_200.0);
    host.set_head_clock_of(1, HeadClock::Transport(3));
    assert_eq!(clocks(&host).at(Some(30)), 1_203.0, "the window's");
    host.set_head_clock_of(20, HeadClock::Transport(1));
    host.set_head_clock_of(22, HeadClock::Device);
    let read = clocks(&host);
    assert_eq!(read.at(Some(21)), 1_201.0, "its ancestor's");
    assert_eq!(read.at(Some(22)), 48_000.0, "its own");
    assert_eq!(read.at(Some(30)), 1_203.0, "the window's still");
    assert_eq!(read.at(None), 1_203.0);
    assert_eq!(host.head_clock_of(1, Some(21)), HeadClock::Transport(1));
    let mut drawn = host.transports_drawn(1);
    drawn.sort();
    assert_eq!(drawn, [1, 3]);
    assert!(host.device_drawn(1));
    // No source at all is the same answer everywhere: nothing to read.
    assert_eq!(host.head_clocks(1, None).at(Some(21)), 0.0);

    // Freeing a widget forgets what was named on it.
    host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: GUI_FREE.into(),
            args: vec![OscType::Int(20)],
        }),
        from(),
    );
    assert_eq!(host.transports_drawn(1), [3]);
}

/// **A redefine forgets what a removed widget left, as a free does.** The
/// two used to prune different sets: a clock named on a widget that a
/// redefine dropped stayed named, and a transport stayed polled for it.
#[test]
fn a_redefine_forgets_a_removed_widget_like_a_free() {
    let mut host = Host::new();
    host.handle_packet(
        def_msg(
            1,
            r#"{"type":"window","children":[{"id":20,"type":"knob"},{"id":21,"type":"knob"}]}"#,
        ),
        from(),
    );
    host.set_head_clock_of(20, HeadClock::Transport(4));
    host.focus(1, 20);
    assert_eq!(host.transports_drawn(1), [4]);
    host.handle_packet(
        def_msg(
            1,
            r#"{"type":"window","children":[{"id":21,"type":"knob"}]}"#,
        ),
        from(),
    );
    assert!(
        host.transports_drawn(1).is_empty(),
        "the clock went with it"
    );
    assert!(!host.head_clocks.contains_key(&20));
    assert_eq!(host.focused(), None, "and so did the focus");
}

/// **A take at another rate than the engine's reads a transport as its
/// own frames.** The transport counts the engine's samples and the take is
/// read faster or slower to sound at its pitch, so 44.1 kHz under 48 kHz
/// draws its line at the position times 44.1 over 48 -- on the take's view
/// only, and not on the device clock, whose anchor is the engine's.
#[test]
fn a_take_at_another_rate_reads_the_transport_as_its_own_frames() {
    struct At;
    impl BusSource for At {
        fn control(&self, _index: usize) -> f32 {
            0.0
        }
        fn sample_clock(&self) -> f64 {
            96_000.0
        }
        fn transport_position(&self, _transport: usize) -> f64 {
            48_000.0
        }
    }
    const TAKES: &str = r#"{"type":"window","children":[
        {"id":10,"type":"signal","data":[0.0,0.0],
            "axes":{"x":{"sample_rate":44100.0}}},
        {"id":11,"type":"signal","data":[0.0,0.0],
            "axes":{"x":{"sample_rate":48000.0}}},
        {"id":12,"type":"knob"}
    ]}"#;
    let mut host = Host::new();
    host.handle_packet(def_msg(1, TAKES), from());
    host.set_head_clock_of(1, HeadClock::Transport(2));
    let clocks = |host: &Host| host.head_clocks(1, Some(&At as &dyn BusSource));
    assert_eq!(clocks(&host).at(Some(10)), 48_000.0, "no engine rate yet");

    host.server_rate = 48_000.0;
    let read = clocks(&host);
    assert_eq!(read.at(Some(10)), 44_100.0, "a second of the take");
    assert_eq!(read.at(Some(11)), 48_000.0, "the engine's rate");
    assert_eq!(read.at(Some(12)), 48_000.0, "no samples of its own");

    // A locate of frame 25_021 sent the nearest engine sample, 27_234,
    // which scales back to 25_021.24: the line stands on the frame.
    struct Located;
    impl BusSource for Located {
        fn control(&self, _index: usize) -> f32 {
            0.0
        }
        fn transport_position(&self, _transport: usize) -> f64 {
            (25_021.0_f64 * 48_000.0 / 44_100.0).round()
        }
    }
    let located = host.head_clocks(1, Some(&Located as &dyn BusSource));
    assert_eq!(located.at(Some(10)), 25_021.0);

    host.set_head_clock_of(1, HeadClock::Device);
    assert_eq!(clocks(&host).at(Some(10)), 96_000.0, "the device clock");
}

/// **A client says which counter a window's or a view's playheads read**,
/// by id. Before this the choice was fixed where the segment was opened,
/// so a host launched by a script drew the device clock and nothing else --
/// and a script driving the transport had to anchor the line itself, which
/// cannot express a locate or a loop.
#[test]
fn a_client_says_which_counter_the_playheads_read() {
    let mut host = Host::new();
    host.handle_packet(def_msg(1, TREE), from());
    assert_eq!(
        host.head_clock_of(1, None),
        HeadClock::Device,
        "the default"
    );

    let clock = |id: i32, which: &str, transport: Option<i32>| {
        let mut args = vec![OscType::Int(id), OscType::String(which.into())];
        args.extend(transport.map(OscType::Int));
        OscPacket::Message(OscMessage {
            addr: GUI_CLOCK.into(),
            args,
        })
    };
    assert!(
        !host
            .handle_packet(clock(1, "transport", None), from())
            .is_empty(),
        "the window redraws: the line it draws now means something else"
    );
    assert_eq!(
        host.head_clock_of(1, Some(10)),
        HeadClock::Transport(0),
        "transport 0 unnamed"
    );
    host.handle_packet(clock(10, "transport", Some(2)), from());
    assert_eq!(
        host.head_clock_of(1, Some(10)),
        HeadClock::Transport(2),
        "or the one named, on the view"
    );
    assert_eq!(host.head_clock_of(1, None), HeadClock::Transport(0));

    // A word the host does not know, and an id no window holds, leave it
    // drawing what it was drawing.
    assert!(
        host.handle_packet(clock(10, "nonsense", None), from())
            .is_empty()
    );
    assert!(
        host.handle_packet(clock(99, "device", None), from())
            .is_empty()
    );
    assert_eq!(host.head_clock_of(1, Some(10)), HeadClock::Transport(2));

    host.handle_packet(clock(10, "device", None), from());
    assert_eq!(host.head_clock_of(1, Some(10)), HeadClock::Device);
}

/// **The report's start frame rides as a long**, and a reader that took
/// only `Int` dropped every report without a word -- which is how a whole
/// wire looked like a drawing bug for an afternoon.
#[test]
fn a_stream_report_reads_the_start_frame_in_either_width() {
    let blob = OscType::Blob(vec![0u8; 12]); // one bucket, one channel
    let long = vec![
        OscType::Int(7),
        OscType::Long(512),
        OscType::Int(256),
        blob.clone(),
    ];
    let int = vec![
        OscType::Int(7),
        OscType::Int(512),
        OscType::Int(256),
        blob.clone(),
    ];
    for args in [long, int] {
        let (bufnum, start, bucket, stats) = stream_report(&args).expect("a report");
        assert_eq!((bufnum, start, bucket), (7, 512, 256));
        assert_eq!(stats.len(), 3);
    }
    assert!(
        stream_report(&[OscType::Int(7), OscType::Long(0), OscType::Int(256)]).is_none(),
        "a report without its blob is not one"
    );
}

/// The reply messages among a batch of effects.
fn replies(effects: Vec<HostEffect>) -> Vec<OscMessage> {
    effects
        .into_iter()
        .filter_map(|e| match e {
            HostEffect::Reply(m) => Some(m),
            _ => None,
        })
        .collect()
}

/// The def ids of any OpenWindow effects.
fn opened(effects: &[HostEffect]) -> Vec<i32> {
    effects
        .iter()
        .filter_map(|e| match e {
            HostEffect::OpenWindow(id) => Some(*id),
            _ => None,
        })
        .collect()
}

const TREE: &str = r#"{"type":"window","title":"Filter","children":[
    {"id":10,"type":"knob","label":"cutoff","min":20.0,"max":20000.0,"value":800.0}
]}"#;

/// A def names **any** widget, not only a window, and until now only a
/// window reached the typed tree the front draws -- so a def of anything
/// else was recorded, logged and invisible.
///
/// It is the one channel a widget that was not there can arrive by. Sending
/// it for the *window* rebuilds every widget in it, which is how a clip
/// appearing in one lane came to take the zoom, the scroll and the
/// selection of every other lane; splicing the subtree keeps all of that,
/// because everything outside it is the same object it was.
#[test]
fn a_def_of_a_widget_inside_a_window_is_spliced_into_what_the_front_draws() {
    const STACK: &str = r#"{"type":"window","children":[
        {"id":20,"type":"layout","flow":"col",
         "children":[{"id":30,"type":"knob","value":1.0}]},
        {"id":21,"type":"layout","flow":"col",
         "children":[{"id":40,"type":"knob","value":2.0}]}
    ]}"#;
    let mut host = Host::new();
    host.handle_packet(def_msg(1, STACK), from());

    // One column grows a second knob: a widget that was not there.
    let grown = r#"{"type":"layout","flow":"col","id":21,"children":[
        {"id":40,"type":"knob","value":2.0},
        {"id":41,"type":"knob","value":3.0}
    ]}"#;
    let effects = host.handle_packet(def_msg(21, grown), from());
    let tree = host.window_def(1).expect("the window is still open");
    assert!(
        tree.find(41).is_some(),
        "the new widget reached the tree the front draws"
    );
    assert!(
        tree.find(30).is_some(),
        "and the other column was not rebuilt out from under it"
    );
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, HostEffect::OpenWindow(1))),
        "the window it belongs to is brought up to the tree: {effects:?}"
    );
}

/// A face this host cannot use is **not an error**: the embedded bitmap
/// face is the floor every build draws on, so `/gui_font` never fails a
/// client and never redraws a window it did not change. (A build without
/// the rasterizer takes the same path, one branch earlier.)
#[test]
fn a_typeface_the_host_cannot_read_leaves_it_drawing() {
    let mut host = Host::new();
    host.handle_packet(def_msg(1, TREE), from());
    let font = OscPacket::Message(OscMessage {
        addr: GUI_FONT.into(),
        args: vec![OscType::Blob(b"not a typeface".to_vec())],
    });
    assert!(
        host.handle_packet(font, from()).is_empty(),
        "nothing to redraw: the face did not change"
    );
    assert!(
        host.window_def(1).is_some(),
        "and the window is where it was"
    );
}

/// The two host-wide tables are verbs, not launch flags: a client hands one
/// over after the windows are up, and every window redraws -- which is the
/// half `--theme` could never do and the browser could only do by reaching
/// under its client to the binding.
#[test]
fn a_theme_the_host_is_handed_redraws_every_window() {
    let mut host = Host::new();
    host.handle_packet(def_msg(1, TREE), from());
    let before = host.theme.accent;
    let theme = OscPacket::Message(OscMessage {
        addr: GUI_THEME.into(),
        args: vec![OscType::String(r##"{"accent": "#ff0000"}"##.into())],
    });
    let effects = host.handle_packet(theme, from());
    assert_ne!(host.theme.accent, before, "the host's own base moved");
    assert_eq!(
        effects
            .iter()
            .filter(|e| matches!(e, HostEffect::Redraw(1)))
            .count(),
        1,
        "the open window redraws with it"
    );
}

/// A role the host does not know is reported and skipped, exactly as the
/// launch-time table's is -- the verb refuses nothing and never leaves a
/// window unpainted over a typo.
#[test]
fn an_unknown_role_is_skipped_rather_than_refused() {
    let mut host = Host::new();
    host.handle_packet(def_msg(1, TREE), from());
    let theme = OscPacket::Message(OscMessage {
        addr: GUI_THEME.into(),
        args: vec![OscType::String(r##"{"nonsense": "#ff0000"}"##.into())],
    });
    assert!(
        !host.handle_packet(theme, from()).is_empty(),
        "the window still redraws: the table was applied, one role short"
    );

    // And a payload that is not an object at all is the client's mistake,
    // so nothing happens rather than something partial.
    let bad = OscPacket::Message(OscMessage {
        addr: GUI_THEME.into(),
        args: vec![OscType::String("not json".into())],
    });
    assert!(host.handle_packet(bad, from()).is_empty());
}

/// The metrics table is the same verb for lengths, `scale` included -- the
/// reserved key that regenerates the whole set at a density.
#[test]
fn metrics_arrive_the_same_way_and_scale_regenerates_the_set() {
    let mut host = Host::new();
    host.handle_packet(def_msg(1, TREE), from());
    let before = host.metrics.pad;
    let metrics = OscPacket::Message(OscMessage {
        addr: GUI_METRICS.into(),
        args: vec![OscType::String(r#"{"scale": 2.0}"#.into())],
    });
    let effects = host.handle_packet(metrics, from());
    assert_ne!(host.metrics.pad, before, "every length regenerated");
    assert!(effects.iter().any(|e| matches!(e, HostEffect::Redraw(1))));
}

/// A `/gui_set` may carry its samples as a **blob**, which is the door a
/// client past the inline ceiling needs: a native one rewrites the file it
/// spilled to, and a page has no file. The bytes expand to exactly the
/// array the inline `data` prop would have held, so nothing downstream
/// learns a second shape -- what a live view then does with them is the
/// `data` path that already existed.
#[test]
fn a_set_carries_bulk_samples_as_a_blob() {
    const WAVE: &str = r#"{"type":"window","children":[
        {"id":20,"type":"waveform","data":[0.0,0.0]}
    ]}"#;
    let mut host = Host::new();
    host.handle_packet(def_msg(1, WAVE), from());

    let mut bytes = Vec::new();
    for s in [0.25f32, -0.5, 0.75] {
        bytes.extend_from_slice(&s.to_le_bytes());
    }
    let set = OscPacket::Message(OscMessage {
        addr: GUI_SET.into(),
        args: vec![
            OscType::Int(20),
            OscType::String("data".into()),
            OscType::Blob(bytes),
        ],
    });
    host.handle_packet(set, from());

    let widget = host.registry().get(20).expect("the widget is there");
    let Some(serde_json::Value::Array(data)) = widget.props.get("data") else {
        panic!("the blob did not become the samples: {:?}", widget.props);
    };
    let read: Vec<f64> = data.iter().filter_map(serde_json::Value::as_f64).collect();
    assert_eq!(read.len(), 3);
    assert!((read[1] + 0.5).abs() < 1e-6, "little-endian f32, in order");

    // A length that is not whole f32s is a client's mistake, and is dropped
    // rather than read as a shorter run: the pair never becomes a prop, so
    // the widget keeps the samples it had.
    let bad = OscPacket::Message(OscMessage {
        addr: GUI_SET.into(),
        args: vec![
            OscType::Int(20),
            OscType::String("data".into()),
            OscType::Blob(vec![1, 2, 3]),
        ],
    });
    host.handle_packet(bad, from());
    let after = host.registry().get(20).expect("still there");
    let Some(serde_json::Value::Array(kept)) = after.props.get("data") else {
        panic!("the samples went away");
    };
    assert_eq!(kept.len(), 3, "the ragged blob changed nothing");
}

#[test]
fn window_def_opens_a_window_and_stores_the_typed_def() {
    let mut host = Host::new();
    let effects = host.handle_packet(def_msg(1, TREE), from());
    assert_eq!(opened(&effects), vec![1], "a window root opens a window");
    assert_eq!(host.registry().len(), 2, "window + knob in the registry");
    assert!(
        host.window_def(1).is_some(),
        "the typed window def is stored"
    );
}

/// A window asks for what it declared -- and, with `hug`, for what it holds
/// on the axes its content settles, keeping the declared number on the
/// others. This is the workaround it retires: a single control in a window
/// used to need `weight` to stop being a strip under an empty pane, and now
/// the pane is the control's own size.
#[test]
fn a_hugging_window_asks_for_the_size_of_its_content() {
    let mut host = Host::new();
    host.handle_packet(def_msg(1, TREE), from());
    assert_eq!(host.window_size(1), Some((640, 360)), "the defaults");

    let hugging = r#"{"type":"window","w":420,"hug":1,"children":[
        {"id":10,"type":"knob","label":"cutoff","min":20.0,"max":20000.0,"value":800.0}
    ]}"#;
    host.handle_packet(def_msg(2, hugging), from());
    let (kw, kh) = host.window_size(2).expect("a window asks for a size");
    let knob = host.window_def(2).unwrap().hug_size(&host.metrics, 1.0);
    // The content **plus the status bar**: a window fitted to what it holds
    // is fitted to the chrome under it too, or the band would be taken out
    // of the pixels the knob was measured to need.
    assert_eq!(
        (kw as f32, kh as f32),
        (knob.0.unwrap(), knob.1.unwrap() + host.metrics.status_h),
    );
    assert!(kw < 420 && kh < 360, "the window is the knob: {kw}x{kh}");

    // The declared number is what stands where the content is elastic: a
    // horizontal slider spans whatever track it is given, and says so.
    let along = r#"{"type":"window","w":420,"hug":1,"children":[
        {"id":11,"type":"slider","label":"mix"}
    ]}"#;
    host.handle_packet(def_msg(3, along), from());
    let (w, h) = host.window_size(3).expect("a window asks for a size");
    assert_eq!(w, 420, "nothing under it knows a width");
    assert!(h < 360, "and it is a strip, not the default pane: {h}");

    // The second half of the question: once the shell has written a scale,
    // the answer is measured with the table the layout will actually use.
    // Only a hugging window has one -- a window that declared its size is
    // not to be resized under it.
    assert_eq!(host.window_size_px(1), None, "nothing to fit");
    host.set_ui_scale(2, 1.25);
    let (pw, ph) = host.window_size_px(2).expect("a hugging window is asked");
    let at_scale = host
        .window_def(2)
        .unwrap()
        .hug_size(host.metrics_for(2), 1.25);
    // Through the same rounding the accessor does: a window is asked for in
    // whole pixels and the measurement is not (a scaled text size need not
    // land on one), so comparing the raw float only ever passed by luck.
    assert_eq!(
        (pw as f32, ph as f32),
        (
            at_scale.0.unwrap().ceil().max(1.0),
            (at_scale.1.unwrap() + host.metrics_for(2).status_h)
                .ceil()
                .max(1.0)
        )
    );
    assert!(
        pw as f32 >= kw as f32 * 1.25 - 1.0 && ph as f32 >= kh as f32 * 1.25 - 1.0,
        "the exact answer is not allowed to come out under the estimate:              {pw}x{ph} vs {kw}x{kh} at 1.25"
    );
}

#[test]
fn def_then_query_replies_with_gui_info() {
    let mut host = Host::new();
    host.handle_packet(def_msg(1, TREE), from());

    let query = OscPacket::Message(OscMessage {
        addr: GUI_QUERY.into(),
        args: vec![OscType::Int(10)],
    });
    let out = replies(host.handle_packet(query, from()));
    assert_eq!(out.len(), 1);
    let info = &out[0];
    assert_eq!(info.addr, GUI_INFO);
    assert_eq!(info.args[0], OscType::Int(10));
    assert_eq!(info.args[1], OscType::String("knob".into()));
    // The reply carries the knob's props as k/v pairs, ints and floats kept
    // apart; `value` is a float.
    let pos = info
        .args
        .iter()
        .position(|a| *a == OscType::String("value".into()))
        .expect("value key present");
    assert_eq!(info.args[pos + 1], OscType::Float(800.0));
}

/// **A query answers what the widget is, not what it was defined as.** A
/// gesture writes the render tree and never the document, so a dragged
/// control used to report its def-time value forever -- which is the one
/// answer a script cannot check any other way.
#[test]
fn a_query_reports_what_a_gesture_left_behind() {
    use crate::host::gestures::{GestureCtx, Gestures};

    let mut host = Host::new();
    host.handle_packet(
        def_msg(
            1,
            r#"{"type":"window","margin":0,"children":[
                {"id":10,"type":"slider","min":0.0,"max":1.0,"value":0.0}]}"#,
        ),
        from(),
    );
    let queried = |host: &mut Host, key: &str| -> Option<OscType> {
        let out = replies(host.handle_packet(
            OscPacket::Message(OscMessage {
                addr: GUI_QUERY.into(),
                args: vec![OscType::Int(10)],
            }),
            from(),
        ));
        let args = &out[0].args;
        let at = args
            .iter()
            .position(|a| *a == OscType::String(key.into()))?;
        args.get(at + 1).cloned()
    };
    assert_eq!(queried(&mut host, "value"), Some(OscType::Float(0.0)));

    // Drag the slider to the right end of the groove it was actually
    // placed on, so the press lands where the renderer drew it.
    let rect = host
        .layout_window(1, 400, 200)
        .unwrap()
        .iter()
        .find(|p| p.widget.id == Some(10))
        .expect("the slider is placed")
        .rect;
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 400, 200);
    let (x, y) = (
        (rect.x + rect.w - 2.0) as f64,
        (rect.y + rect.h * 0.5) as f64,
    );
    g.press(&mut host, &ctx, x, y);
    g.release(&mut host, &ctx, x, y);
    let dragged = match queried(&mut host, "value") {
        Some(OscType::Float(v)) => v,
        other => panic!("no float value: {other:?}"),
    };
    assert!(
        dragged > 0.5,
        "the query reports the drag, not the def: {dragged}"
    );

    // ...and a `/gui_set` still wins, because it writes both surfaces.
    host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: GUI_SET.into(),
            args: vec![
                OscType::Int(10),
                OscType::String("value".into()),
                OscType::Float(0.25),
            ],
        }),
        from(),
    );
    assert_eq!(queried(&mut host, "value"), Some(OscType::Float(0.25)));
}

#[test]
fn query_for_unknown_id_still_answers() {
    let mut host = Host::new();
    let query = OscPacket::Message(OscMessage {
        addr: GUI_QUERY.into(),
        args: vec![OscType::Int(42)],
    });
    let out = replies(host.handle_packet(query, from()));
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].args[0], OscType::Int(42));
    assert_eq!(out[0].args[1], OscType::String(String::new()));
}

/// One host, one logical table, one resolved table per window -- and the
/// shell is the only side that says what a window's scale is.
#[test]
fn each_window_resolves_the_table_at_its_own_scale() {
    let mut host = Host::new();
    host.handle_packet(def_msg(1, TREE), from());
    host.handle_packet(def_msg(2, TREE), from());
    assert_eq!(host.ui_scale(1), 1.0, "no shell has reported one yet");
    assert_eq!(host.metrics_for(1), &host.metrics);

    assert!(host.set_ui_scale(1, 2.0));
    assert!(!host.set_ui_scale(1, 2.0), "the same scale changes nothing");
    assert_eq!(host.ui_scale(1), 2.0);
    assert_eq!(host.metrics_for(1).control_h, host.metrics.control_h * 2.0);
    assert_eq!(
        host.metrics_for(2),
        &host.metrics,
        "the other window is on its own display"
    );

    // A new logical table (a `[gui.metrics]` overlay, the browser's
    // `metrics(json)`) reaches every window at the scale it is on.
    host.metrics.overlay([("control_h", 30.0)]);
    host.refresh_metrics();
    assert_eq!(host.metrics_for(1).control_h, 60.0);
    assert_eq!(host.metrics_for(2).control_h, 30.0);

    // Freeing the window drops what the shell reported for it.
    host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: GUI_FREE.into(),
            args: vec![OscType::Int(1)],
        }),
        from(),
    );
    assert_eq!(host.ui_scale(1), 1.0);
}

#[test]
fn set_updates_a_live_widget() {
    let mut host = Host::new();
    host.handle_packet(def_msg(1, TREE), from());
    let set = OscPacket::Message(OscMessage {
        addr: GUI_SET.into(),
        args: vec![
            OscType::Int(10),
            OscType::String("value".into()),
            OscType::Float(440.0),
        ],
    });
    host.handle_packet(set, from());
    assert_eq!(
        host.registry().get(10).unwrap().props["value"],
        Value::from(440.0)
    );
}

#[test]
fn free_drops_the_subtree_and_closes_the_window() {
    let mut host = Host::new();
    host.handle_packet(def_msg(1, TREE), from());
    let free = OscPacket::Message(OscMessage {
        addr: GUI_FREE.into(),
        args: vec![OscType::Int(1)],
    });
    let effects = host.handle_packet(free, from());
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, HostEffect::CloseWindow(1))),
        "freeing a window def closes its window"
    );
    assert!(host.registry().is_empty());
    assert!(host.window_def(1).is_none());
}

#[test]
fn waveform_blob_rides_the_def_message() {
    let mut host = Host::new();
    let blob: Vec<u8> = [0.5f32, -0.5]
        .iter()
        .flat_map(|x| x.to_le_bytes())
        .collect();
    let json = r#"{"type":"window","children":[{"id":9,"type":"signal","view":"trace","blob":0}]}"#;
    let msg = OscPacket::Message(OscMessage {
        addr: GUI_DEF.into(),
        args: vec![
            OscType::Int(2),
            OscType::String(json.into()),
            OscType::Blob(blob),
        ],
    });
    let effects = host.handle_packet(msg, from());
    assert_eq!(opened(&effects), vec![2]);
    let tree = host.window_def(2).unwrap();
    let data = tree.children[0]
        .signal()
        .and_then(|el| el.source.data())
        .expect("expected a waveform");
    assert_eq!(&data.samples[..], &[0.5, -0.5]);
}

#[test]
fn named_def_persists_and_gui_load_reinstantiates_it() {
    let mut dir = std::env::temp_dir();
    dir.push(format!(
        "clausters_gui_host_store_{:?}",
        std::time::Instant::now()
    ));
    let store = store::GuiStore::open(&dir).unwrap();
    let mut host = Host::new().with_store(store);

    // A named GuiDef auto-persists on /gui_def.
    let tree = r#"{"type":"window","name":"inst","title":"I","children":[
        {"id":10,"type":"knob","value":0.5}
    ]}"#;
    host.handle_packet(def_msg(3, tree), from());
    assert!(host.window_def(3).is_some());

    // Free it: the live def is gone, but the persisted copy remains.
    host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: GUI_FREE.into(),
            args: vec![OscType::Int(3)],
        }),
        from(),
    );
    assert!(host.window_def(3).is_none());

    // /gui_load rebuilds it under its saved id and reopens the window.
    let effects = host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: GUI_LOAD.into(),
            args: vec![OscType::String("inst".into())],
        }),
        from(),
    );
    assert_eq!(opened(&effects), vec![3], "loading reopens the window");
    assert!(host.window_def(3).is_some());

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn bundle_is_unwrapped_in_order() {
    use clausters_core::osc::{IMMEDIATE, OscBundle};
    let mut host = Host::new();
    let bundle = OscPacket::Bundle(OscBundle {
        timetag: IMMEDIATE,
        content: vec![
            def_msg(1, TREE),
            OscPacket::Message(OscMessage {
                addr: GUI_QUERY.into(),
                args: vec![OscType::Int(1)],
            }),
        ],
    });
    let out = replies(host.handle_packet(bundle, from()));
    assert_eq!(out.len(), 1, "the query inside the bundle is answered");
    assert_eq!(out[0].args[1], OscType::String("window".into()));
}

fn ack_msg(args: Vec<OscType>) -> OscPacket {
    OscPacket::Message(OscMessage {
        addr: GUI_ACK.into(),
        args,
    })
}

#[test]
fn an_acknowledgement_retires_what_it_covers_and_records_the_state() {
    let mut host = Host::new();
    let a = host.outbox.borrow_mut().stamp(1, 10);
    let b = host.outbox.borrow_mut().stamp(1, 11);

    host.handle_packet(
        ack_msg(vec![
            OscType::Int(a),
            OscType::Int(7),
            OscType::Int(4),
            OscType::Int(2),
            OscType::String("snapped to the grid".into()),
        ]),
        ClientId::Web,
    );

    // One rule and no branch: everything at or below the stamp is settled,
    // and what is still out stays out.
    assert!(!host.outbox.borrow().is_pending(1, 10));
    assert!(host.outbox.borrow().is_pending(1, 11));
    assert_eq!(host.outbox.borrow().pending()[0].seq, b);

    let outbox = host.outbox.borrow();
    let last = outbox.last().expect("the owner said something");
    assert_eq!(last.doc_version, 7);
    assert_eq!(outbox.generation(4), Some(2));
    assert_eq!(last.reason.as_deref(), Some("snapped to the grid"));
}

#[test]
fn an_acknowledgement_with_nothing_to_say_is_still_an_acknowledgement() {
    // A refusal *is* this message: the owner pushed the previous value and
    // stamped it, so there is no reason and no generation to report -- and
    // the pending edit must still retire, or the host waits forever on an
    // answer it already has.
    let mut host = Host::new();
    let seq = host.outbox.borrow_mut().stamp(1, 10);
    host.handle_packet(
        ack_msg(vec![OscType::Int(seq), OscType::Int(0)]),
        ClientId::Web,
    );
    assert!(host.outbox.borrow().pending().is_empty());
    assert_eq!(host.outbox.borrow().last().unwrap().reason, None);
}

#[test]
fn a_malformed_acknowledgement_changes_nothing() {
    let mut host = Host::new();
    host.outbox.borrow_mut().stamp(1, 10);
    host.handle_packet(ack_msg(vec![]), ClientId::Web);
    assert_eq!(host.outbox.borrow().pending().len(), 1);
}

#[test]
fn freeing_a_window_drops_the_edits_it_had_in_flight() {
    let mut host = Host::new();
    host.handle_packet(
        def_msg(1, r#"{"type":"window","children":[]}"#),
        ClientId::Web,
    );
    host.outbox.borrow_mut().stamp(1, 10);
    host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: GUI_FREE.into(),
            args: vec![OscType::Int(1)],
        }),
        ClientId::Web,
    );
    assert!(host.outbox.borrow().pending().is_empty());
}

#[test]
fn redefining_a_window_drops_the_edits_it_had_in_flight() {
    // Found while writing the owner's side of the version check: a redefine replaces the
    // whole tree, so an edit in flight against the old one has nothing to
    // resolve to -- its widget may be gone, or its id may now belong to
    // something else. The owner answers nothing for it, so without this the
    // pending set stays open against an acknowledgement never coming.
    let mut host = Host::new();
    host.handle_packet(
        def_msg(1, r#"{"type":"window","children":[]}"#),
        ClientId::Web,
    );
    host.outbox.borrow_mut().stamp(1, 10);
    let other = host.outbox.borrow_mut().stamp(2, 20);
    host.handle_packet(
        def_msg(1, r#"{"type":"window","children":[]}"#),
        ClientId::Web,
    );
    let outbox = host.outbox.borrow();
    assert_eq!(outbox.pending().len(), 1, "only that window's");
    assert_eq!(outbox.pending()[0].seq, other);
}

fn bind_msg(id: i32, target: Vec<OscType>) -> OscPacket {
    let mut args = vec![OscType::Int(id)];
    args.extend(target);
    OscPacket::Message(OscMessage {
        addr: GUI_BIND.into(),
        args,
    })
}

#[test]
fn bound_widget_forwards_to_the_audio_server_and_unbinds() {
    use clausters_core::osc::decode_packet;
    use std::net::UdpSocket;
    use std::time::Duration;

    // A throwaway socket standing in for the audio server, to capture the
    // message a bound widget forwards (one process, so loopback delivers).
    let fake_server = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
    fake_server
        .set_read_timeout(Some(Duration::from_millis(500)))
        .unwrap();
    let leg = ServerLeg::connect(fake_server.local_addr().unwrap()).unwrap();

    let mut host = Host::new().with_server(leg);
    host.handle_packet(def_msg(1, TREE), from()); // a window with knob id 10

    host.handle_packet(
        bind_msg(
            10,
            vec![
                OscType::String("server".into()),
                OscType::String("/node_set".into()),
                OscType::Int(1000),
                OscType::String("cutoff".into()),
            ],
        ),
        from(),
    );
    assert!(host.is_bound(10));

    // A value change goes straight to the server (bypassing the script).
    assert!(host.forward(10, OscType::Float(440.0), &mut Vec::new()));
    let mut buf = [0u8; 1024];
    let (len, _) = fake_server.recv_from(&mut buf).expect("forwarded datagram");
    let msg = match decode_packet(&buf[..len]).unwrap() {
        OscPacket::Message(m) => m,
        other => panic!("expected a message, got {other:?}"),
    };
    assert_eq!(msg.addr, "/node_set");
    assert_eq!(
        msg.args,
        vec![
            OscType::Int(1000),
            OscType::String("cutoff".into()),
            OscType::Float(440.0)
        ]
    );

    // Unbinding (no target) restores the event path: forward stops handling.
    host.handle_packet(bind_msg(10, vec![]), from());
    assert!(!host.is_bound(10));
    assert!(!host.forward(10, OscType::Float(1.0), &mut Vec::new()));
}

/// The other destination: a widget bound to a widget. A toggle drives a
/// `stack`'s page with no script and no server in the process -- the whole
/// of tabs, and what makes a persisted GuiDef an autonomous application.
#[test]
fn a_widget_binding_applies_to_the_other_widget_and_never_cascades() {
    const TABS: &str = r#"{"type":"window","children":[
        {"id":10,"type":"toggle","label":"view"},
        {"id":20,"type":"layout","flow":"stack","index":0,"children":[
            {"id":21,"type":"label","text":"one"},
            {"id":22,"type":"label","text":"two"}]}]}"#;
    let mut host = Host::new();
    host.handle_packet(def_msg(1, TABS), from());
    host.handle_packet(
        bind_msg(
            10,
            vec![
                OscType::String("widget".into()),
                OscType::Int(20),
                OscType::String("index".into()),
            ],
        ),
        from(),
    );
    assert!(host.is_bound(10));

    // The toggle's value applies as a `/gui_set 20 index 1` would: the
    // typed tree switches page, and the window is asked to repaint.
    let mut effects = Vec::new();
    assert!(
        host.forward(10, OscType::Int(1), &mut effects),
        "bound: the script never sees it"
    );
    assert!(matches!(effects.as_slice(), [HostEffect::Redraw(1)]));
    let index = match host.window_def(1).unwrap().find(20).unwrap().kind {
        widget::WidgetKind::Stack { index, .. } => index,
        ref other => panic!("expected a stack, got {other:?}"),
    };
    assert_eq!(index, 1, "the page the toggle names");
    // The generic registry moved with it, so a `/gui_query` agrees.
    assert_eq!(
        host.registry().get(20).unwrap().props.get("index"),
        Some(&Value::from(1))
    );

    // A binding fires an apply, never another binding: binding the stack
    // back to the toggle cannot make the apply re-enter delivery, so the
    // pair settles instead of cascading.
    host.handle_packet(
        bind_msg(
            20,
            vec![
                OscType::String("widget".into()),
                OscType::Int(10),
                OscType::String("value".into()),
            ],
        ),
        from(),
    );
    let mut effects = Vec::new();
    host.forward(10, OscType::Int(0), &mut effects);
    assert_eq!(
        host.registry().get(10).unwrap().props.get("value"),
        None,
        "the stack's own binding did not fire from the apply"
    );
}

/// A hidden page is still on the axis: a `stack` skips a page's *layout*,
/// not its membership, so a scroll bound to one view moves the one behind
/// it too and a switch shows it already there. Same property as the GPU
/// slot it keeps -- both are read from the tree, not from the placements.
#[test]
fn a_hidden_stack_page_still_belongs_to_its_navigation_group() {
    const PAGES: &str = r#"{"type":"window","children":[
        {"id":20,"type":"layout","flow":"stack","index":0,"children":[
            {"id":21,"type":"signal","view":"trace","data":[0.0,1.0,0.0,-1.0,0.0,1.0,0.0,-1.0],"link":1},
            {"id":22,"type":"signal","view":"spectrogram","data":[0.0,1.0,0.0,-1.0,0.0,1.0,0.0,-1.0],"link":1}]}]}"#;
    let mut host = Host::new();
    host.handle_packet(def_msg(1, PAGES), from());
    let nav = |host: &Host, id: i32| {
        host.timelines()
            .nav(timeline::group_key(id, Some(1)))
            .expect("a linked view is on a group")
    };
    // The extent is the front's to report (it is the loaded data's), so a
    // headless test says it: eight samples on the axis.
    host.set_timeline_total(21, 8);
    host.set_timeline_total(22, 8);
    // One group, so the hidden page reads the same window as the shown one.
    host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: GUI_SET.into(),
            args: vec![
                OscType::Int(21),
                OscType::String("view_len".into()),
                OscType::Float(4.0),
                OscType::String("view_start".into()),
                OscType::Float(2.0),
            ],
        }),
        from(),
    );
    assert!((nav(&host, 21).start - 2.0).abs() < 0.001);
    assert_eq!(
        nav(&host, 22),
        nav(&host, 21),
        "the page nobody is looking at moved with the axis"
    );
    // And it is a member because the tree says so: the collector that
    // registers the groups walks the widgets, not the rectangles.
    assert!(host.window_def(1).unwrap().find(22).unwrap().is_timeline());
}

/// A def written in the model's vocabulary is recorded -- and answered to a
/// query -- with its chrome flat: the type stays as the script wrote it, so
/// a reply is in the vocabulary it asked in, while the axis pair (a
/// structural prop, which `/gui_info` cannot carry) lands where the host
/// itself reads it.
#[test]
fn a_query_answers_with_the_chrome_an_axis_pair_carried() {
    const LANE: &str = r#"{"type":"window","children":[
        {"id":40,"type":"signal","view":"trace","data":[0.0,1.0],
         "axes":{"x":{"unit":"beats","tempo":2.0},"y":{"min":-2.0,"max":2.0}}}]}"#;
    let mut host = Host::new();
    host.handle_packet(def_msg(1, LANE), from());
    let widget = host.registry.get(40).expect("the element is registered");
    assert_eq!(widget.kind, "signal", "the type is answered as written");
    assert!(!widget.props.contains_key("axes"));
    assert_eq!(widget.props["ruler"], serde_json::json!("beats"));
    assert_eq!(widget.props["tempo"], serde_json::json!(2.0));
    assert_eq!(widget.props["min"], serde_json::json!(-2.0));
}

/// An inline `bind` carries a widget target too, which is what lets a saved
/// GuiDef boot with its pages already wired.
#[test]
fn an_inline_widget_bind_is_registered_at_define_time() {
    const TABS: &str = r#"{"type":"window","children":[
        {"id":10,"type":"menu","items":["a","b"],"bind":["widget",20,"index"]},
        {"id":20,"type":"layout","flow":"stack","children":[
            {"id":21,"type":"label","text":"one"},
            {"id":22,"type":"label","text":"two"}]}]}"#;
    let mut host = Host::new();
    host.handle_packet(def_msg(1, TABS), from());
    assert!(host.is_bound(10));
    host.forward(10, OscType::Int(1), &mut Vec::new());
    assert!(matches!(
        host.window_def(1).unwrap().find(20).unwrap().kind,
        widget::WidgetKind::Stack { index: 1, .. }
    ));
}

#[test]
fn freeing_a_bound_widget_drops_its_binding() {
    let mut host = Host::new();
    host.handle_packet(def_msg(1, TREE), from());
    host.handle_packet(
        bind_msg(
            10,
            vec![
                OscType::String("server".into()),
                OscType::String("/bus_set".into()),
                OscType::Int(7),
            ],
        ),
        from(),
    );
    assert!(host.is_bound(10));
    // Freeing the window (root 1) takes knob 10 -- and its binding -- with it.
    host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: GUI_FREE.into(),
            args: vec![OscType::Int(1)],
        }),
        from(),
    );
    assert!(!host.is_bound(10));
}

#[test]
fn binding_without_a_server_is_registered_but_swallows() {
    // No --server: the bind is accepted (and warned), and forward still
    // reports it handled the value so it does not leak to the script.
    let mut host = Host::new();
    host.handle_packet(def_msg(1, TREE), from());
    host.handle_packet(
        bind_msg(
            10,
            vec![
                OscType::String("server".into()),
                OscType::String("/node_set".into()),
                OscType::Int(1000),
                OscType::String("cutoff".into()),
            ],
        ),
        from(),
    );
    assert!(host.is_bound(10));
    assert!(
        host.forward(10, OscType::Float(1.0), &mut Vec::new()),
        "swallowed, not emitted"
    );
}

// ---- the two doors: a def sent as JSON, and a def built in Rust ----

/// **Parity is the whole promise of the Rust door**: a tree built with the
/// typed builder and the same tree sent as a `/gui_def` document must
/// leave the host in the same state -- the same typed widget tree, the same
/// recorded document. They meet at `GuiNode`, so this is what proves the
/// two entrances share one definition path rather than resembling it.
#[test]
fn a_tree_built_in_rust_defines_what_the_document_defines() {
    let json = r#"{"type":"window","title":"Mixer","w":400,"h":300,"children":[
        {"id":2,"type":"layout","flow":"row","children":[
            {"id":3,"type":"knob","label":"amp","max":2.0},
            {"id":4,"type":"meter","bus":0}]}]}"#;
    let mut sent = Host::new();
    sent.handle_packet(def_msg(1, json), from());

    let mut built = Host::new();
    let effects = built.define(
        1,
        crate::tree::window()
            .prop("title", "Mixer")
            .prop("w", 400)
            .prop("h", 300)
            .child(
                crate::tree::layout()
                    .id(2)
                    .prop("flow", "row")
                    .child(
                        crate::tree::node("knob")
                            .id(3)
                            .prop("label", "amp")
                            .prop("max", 2.0),
                    )
                    .child(crate::tree::node("meter").id(4).prop("bus", 0)),
            ),
    );

    assert!(
        effects
            .iter()
            .any(|e| matches!(e, HostEffect::OpenWindow(1))),
        "a window root opens its window through either door: {effects:?}"
    );
    assert_eq!(
        format!("{:?}", built.window_def(1).unwrap()),
        format!("{:?}", sent.window_def(1).unwrap()),
        "the typed trees differ"
    );
    // And the document each recorded is the same one, which is what
    // persistence, reload and `/gui_query` all read.
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&built.def_json[&1]).unwrap(),
        serde_json::from_slice::<serde_json::Value>(&sent.def_json[&1]).unwrap(),
    );
}

/// A registered element reaches the host through the Rust door with nothing
/// added to the builder -- the host's seam and the builder meeting, which is
/// the case a program embedding the crate actually has.
#[test]
fn a_registered_element_defines_through_the_rust_door() {
    #[derive(Debug, Clone)]
    struct Pad(i32);
    impl crate::Element for Pad {
        fn set(&mut self, _key: &str, _v: &Value) -> bool {
            false
        }
        fn draw(
            &self,
            _d: &mut crate::host::paint::Draw,
            _ctx: &crate::host::widget::element::Ctx,
        ) {
        }
        fn value(&self) -> Option<OscType> {
            Some(OscType::Int(self.0))
        }
        fn clone_box(&self) -> Box<dyn crate::Element> {
            Box::new(self.clone())
        }
    }
    crate::register("test_door_pad", |props, _| {
        Ok(Box::new(Pad(
            props.get("n").and_then(Value::as_i64).unwrap_or(0) as i32,
        )))
    });

    let mut host = Host::new();
    host.define(
        1,
        crate::tree::window().child(crate::tree::node("test_door_pad").id(2).prop("n", 7)),
    );
    assert_eq!(
        host.window_def(1)
            .unwrap()
            .find(2)
            .unwrap()
            .kind
            .event_value(),
        Some(OscType::Int(7)),
    );
    crate::unregister("test_door_pad");
}
