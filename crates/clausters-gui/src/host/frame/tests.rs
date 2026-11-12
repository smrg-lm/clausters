//! The frame path's tests: the slots a view's data hangs on, what a view owes
//! at a zoom, and the passes of a frame, with no window.

use super::*;

/// **A redefine keeps the pictures of the views that read what they
/// read**: a view on the same buffer keeps every slot it had, a view moved
/// to another buffer or gone keeps none, and a slot the new load filled
/// is not replaced by the old one.
#[test]
fn a_redefine_keeps_the_pictures_of_views_on_the_same_buffer() {
    use crate::host::widget::element::SlotKey;
    let before = reads_of(&[(1, 7, false), (2, 8, false), (3, 9, false)]);
    let now = [(1, 7, false), (2, 5, false), (4, 9, false)];
    let body = SlotKey(0);
    let old: HashMap<SlotAt, &str> = [
        ((1, SlotKey::SELF), "one"),
        ((1, body), "one's body"),
        ((2, SlotKey::SELF), "two"),
        ((3, SlotKey::SELF), "three"),
    ]
    .into();
    let mut into: HashMap<SlotAt, &str> = HashMap::new();
    carry_slots(&before, &now, old.clone(), &mut into);
    let mut kept: Vec<_> = into.values().copied().collect();
    kept.sort_unstable();
    assert_eq!(kept, ["one", "one's body"]);
    let mut loaded: HashMap<SlotAt, &str> = [((1, SlotKey::SELF), "new")].into();
    carry_slots(&before, &now, old, &mut loaded);
    assert_eq!(loaded[&(1, SlotKey::SELF)], "new");
    assert_eq!(loaded[&(1, body)], "one's body");
}

/// **A window drawn offscreen and read back is the window's picture**:
/// the size of its framebuffer, the theme's background where nothing is
/// drawn and something else where a slider is -- through the one pass the
/// screen's frame takes. Skipped, and said so, on a machine where no
/// adapter answers at all, a software one included.
#[test]
#[cfg(not(target_arch = "wasm32"))]
fn a_window_is_drawn_offscreen_and_read_back() {
    use clausters_core::osc::{OscMessage, OscPacket, OscType};

    let Ok(mut gpu) = pollster::block_on(Gpu::offscreen(320, 200, 4)) else {
        eprintln!("no adapter on this machine: nothing to draw offscreen with");
        return;
    };
    let mut host = crate::host::Host::new();
    host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: crate::host::GUI_DEF.into(),
            args: vec![
                OscType::Int(1),
                OscType::String(
                    r#"{"type":"window","w":320,"h":200,"children":[
                            {"id":2,"type":"slider","value":0.7}]}"#
                        .into(),
                ),
            ],
        }),
        crate::host::ClientId::Udp(std::net::SocketAddr::from((
            std::net::Ipv4Addr::LOCALHOST,
            9000,
        ))),
    );
    let mut renderers = Renderers::new(&gpu.device, gpu.target());
    let mut batches = Batches::new(&gpu.device, gpu.target());
    let inputs = FrameInputs {
        metrics: host.metrics_for(1),
        world: World {
            timelines: host.timelines(),
            ..World::default()
        },
        ..FrameInputs::default()
    };
    let readback = capture(
        &mut gpu,
        &mut renderers,
        &mut batches,
        &mut HashMap::new(),
        &mut HashMap::new(),
        &mut HashMap::new(),
        host.window_def(1).expect("the window is defined"),
        &inputs,
        &host.theme,
    )
    .expect("a plain window reads back");
    readback.wait(&gpu.device).expect("the card maps it");
    let (w, h, rgba) = readback.pixels();
    assert_eq!((w, h), (320, 200));
    let background = clear_color(&host.theme);
    let bg = [background.r, background.g, background.b].map(|c| (c * 255.0).round() as i32);
    let differs = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|px| (0..3).any(|c| (i32::from(px[c]) - bg[c]).abs() > 2))
        .count();
    assert_eq!(
        &rgba[..3].iter().map(|&c| i32::from(c)).collect::<Vec<_>>(),
        &bg,
        "the corner is the background"
    );
    assert!(
        differs > 500,
        "the slider is drawn: {differs} pixels differ"
    );
}

/// **A span is drawn to the take's edges it reaches**: a run read back
/// from the take's first frame is drawn from the start and one whose
/// windows reach the end to it, as a whole transform is, where they once
/// left half a window dark (a strip at the start of a page's
/// spectrogram). A run inside the take is drawn where its columns are.
#[test]
#[cfg(not(target_arch = "wasm32"))]
fn a_span_reaches_the_edges_of_the_take_it_reaches() {
    let Ok(gpu) = pollster::block_on(Gpu::offscreen(64, 64, 1)) else {
        eprintln!("no adapter on this machine: nothing to draw offscreen with");
        return;
    };
    let renderers = Renderers::new(&gpu.device, gpu.target());
    let total = 96_000;
    let run = |a: usize, b: usize| {
        let samples: Vec<f32> = (a..b).map(|k| (k as f32 * 0.05).sin()).collect();
        let stft = Stft::compute(&samples, 2048, 128, 48_000.0).placed_at(a, total);
        spectrogram_slot(vec![stft], &gpu, &renderers).expect("a slot")
    };
    let whole = run(0, total);
    assert_eq!(whole.views[0].covers(), (f64::NEG_INFINITY, f64::INFINITY));
    assert!(whole.views[0].is_whole(), "the run is the whole take");
    let (a, b) = run(0, 30_000).views[0].covers();
    assert!(a == f64::NEG_INFINITY && b < 30_000.0, "from the start");
    let (a, b) = run(60_000, total).views[0].covers();
    assert!(a > 60_000.0 && b == f64::INFINITY, "to the end");
    let inside = run(20_000, 40_000);
    let (a, b) = inside.views[0].covers();
    assert!(a > 20_000.0 && b < 40_000.0, "where its columns are");
    assert!(!inside.views[0].is_whole());
}

/// **A picture of the whole take by an analysis left is kept, and going
/// back to that analysis draws it again with nothing asked**: over the
/// whole take the view becomes it, over a span the overview does, and
/// the one it replaces is kept in its place. An edit keeps none.
#[test]
#[cfg(not(target_arch = "wasm32"))]
fn a_picture_of_an_analysis_left_is_brought_back() {
    let Ok(gpu) = pollster::block_on(Gpu::offscreen(64, 64, 1)) else {
        eprintln!("no adapter on this machine: nothing to draw offscreen with");
        return;
    };
    let renderers = Renderers::new(&gpu.device, gpu.target());
    let total = 480_000;
    let whole = |window: usize| {
        let bins = window / 2 + 1;
        Stft::of_span(
            vec![0.5; bins * 60],
            bins,
            0,
            total / 60 + 1,
            window,
            48_000.0,
            total,
        )
        .expect("a whole take's columns")
        .with_analysis(window / 2, 0)
    };
    let slot = |stft: Stft| spectrogram_slot(vec![stft], &gpu, &renderers).expect("a slot");
    let (a, b) = ((1024, 512, 0), (4096, 2048, 0));
    let nav = View::full(total);

    let mut shown = slot(whole(4096));
    keep_overviews(&mut shown, slot(whole(1024)));
    assert!(shown.views[0].analyzed_as(b));
    assert!(shown.views[0].can_recall(a), "the analysis left is kept");
    assert!(
        columns_owed(&shown, &nav, 400.0, a).is_none(),
        "nothing is asked"
    );
    assert!(shown.views[0].recall(a));
    assert!(shown.views[0].analyzed_as(a), "the whole take is it again");
    assert!(
        shown.views[0].can_recall(b),
        "and the one it replaced is kept"
    );

    let bins = 2049;
    let span = Stft::of_span(
        vec![0.5; bins * 8],
        bins,
        96_000,
        2048,
        4096,
        48_000.0,
        total,
    )
    .expect("a span's columns")
    .with_analysis(2048, 0);
    let mut zoomed = slot(span);
    keep_overviews(&mut zoomed, shown);
    let view = &mut zoomed.views[0];
    assert!(view.overview().is_some_and(|o| o.analyzed_as(a)));
    assert!(view.recall(b), "the overview of the span's own analysis");
    assert!(view.overview().is_some_and(|o| o.analyzed_as(b)));
    assert!(view.can_recall(a));

    zoomed.stale.set(Some(total));
    let mut edited = slot(whole(1024));
    keep_overviews(&mut edited, zoomed);
    assert!(!edited.views[0].can_recall(b), "an edit keeps none");
}

/// **A frame of the kept picture is the single pass's frame**: a window
/// of everything that moves, drawn with its live layers in their boxes
/// over the kept picture, reads back as the same window drawn in one
/// pass: exactly when single-sampled, and multisampled but for the few
/// pixels where a live edge crosses an edge of the picture. Skipped, and
/// said so, where no adapter answers.
#[test]
#[cfg(not(target_arch = "wasm32"))]
fn a_kept_picture_frame_is_the_single_pass_frame() {
    use crate::host::world::HeadClocks;
    use clausters_core::osc::{OscMessage, OscPacket, OscType};

    struct Flat(f32);
    impl crate::host::BusSource for Flat {
        fn control(&self, _index: usize) -> f32 {
            self.0
        }
    }
    for samples in [1, 4] {
        let Ok(mut gpu) = pollster::block_on(Gpu::offscreen(360, 420, samples)) else {
            eprintln!("no adapter on this machine: nothing to draw offscreen with");
            return;
        };
        let mut host = crate::host::Host::new();
        host.handle_packet(
            OscPacket::Message(OscMessage {
                addr: crate::host::GUI_DEF.into(),
                args: vec![OscType::Int(1), OscType::String(MOVING.into())],
            }),
            crate::host::ClientId::Udp(std::net::SocketAddr::from((
                std::net::Ipv4Addr::LOCALHOST,
                9000,
            ))),
        );
        let bus = Flat(0.6);
        let inputs = FrameInputs {
            metrics: host.metrics_for(1),
            world: World {
                bus: Some(&bus),
                clocks: HeadClocks::uniform(0.4),
                timelines: host.timelines(),
                ..World::default()
            },
            ..FrameInputs::default()
        };
        let mut renderers = Renderers::new(&gpu.device, gpu.target());
        let mut batches = Batches::new(&gpu.device, gpu.target());
        let mut read = |one_pass: bool| {
            batches.one_pass = one_pass;
            let readback = capture(
                &mut gpu,
                &mut renderers,
                &mut batches,
                &mut HashMap::new(),
                &mut HashMap::new(),
                &mut HashMap::new(),
                host.window_def(1).expect("the window is defined"),
                &inputs,
                &host.theme,
            )
            .expect("the window reads back");
            readback.wait(&gpu.device).expect("the card maps it");
            (readback.pixels().2, batches.underlay.holds((360, 420)))
        };
        let (kept, held) = read(false);
        assert!(held, "{samples} samples: the window keeps its picture");
        let (single, _) = read(true);
        // A live edge that crosses an edge of the picture inside one
        // pixel blends over the picture's resolved color rather than
        // over each sample of it: both antialiased, a few levels apart
        // on those pixels alone.
        let (mut off, mut worst) = (0, 0);
        for (a, b) in kept
            .as_chunks::<4>()
            .0
            .iter()
            .zip(single.as_chunks::<4>().0)
        {
            let d = (0..3)
                .map(|c| (i32::from(a[c]) - i32::from(b[c])).abs())
                .max()
                .unwrap_or(0);
            off += usize::from(d > 2);
            worst = worst.max(d);
        }
        let edges = if samples == 1 { 0 } else { 360 * 420 / 2000 };
        assert!(
            off <= edges && worst <= 32,
            "{samples} samples: {off} pixels differ, by up to {worst}"
        );
    }
}

/// **A window drawn offscreen is the picture kept beside this test**,
/// within two levels a channel: what a change to the drawing of text, a
/// slider, a knob or a roll's notes moves, it shows here. The tolerance
/// is the measured distance between a card and a software adapter
/// (`docs/decisions.md`, "A capture is the same picture on any adapter"),
/// so the picture holds on whatever machine runs the suite.
///
/// A change that is meant to move it writes the picture again:
/// `CLAUSTERS_GUI_BLESS=1 cargo test --lib -- reference_picture`, then
/// look at `tests/references/window.png` before committing it. Skipped,
/// and said so, where no adapter answers.
#[test]
#[cfg(all(not(target_arch = "wasm32"), feature = "font-atlas"))]
fn a_window_is_its_reference_picture() {
    let Some((w, h, rgba)) = draw_reference(false) else {
        eprintln!("no adapter on this machine: nothing to compare");
        return;
    };
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("references")
        .join("window.png");
    if std::env::var_os("CLAUSTERS_GUI_BLESS").is_some() {
        std::fs::create_dir_all(path.parent().expect("a directory")).expect("made");
        std::fs::write(&path, crate::host::png::encode_rgba(w, h, &rgba)).expect("written");
        eprintln!("wrote {}", path.display());
        return;
    }
    let kept = std::fs::read(&path).expect("the reference picture is kept beside the tests");
    let (kw, kh, reference) =
        crate::host::png::decode_stored(&kept).expect("a picture this host wrote");
    assert_eq!((w, h), (kw, kh), "the window's size");
    let far: Vec<(usize, u8)> = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .zip(reference.as_chunks::<4>().0)
        .enumerate()
        .filter_map(|(i, (a, b))| {
            let d = (0..3).map(|c| a[c].abs_diff(b[c])).max().unwrap_or(0);
            (d > 2).then_some((i, d))
        })
        .collect();
    assert!(
        far.is_empty(),
        "{} pixels moved more than two levels, the first at ({}, {}) by {}",
        far.len(),
        far[0].0 % w as usize,
        far[0].0 / w as usize,
        far[0].1
    );
}

/// The window the reference picture is of, drawn offscreen -- on a
/// software adapter when `software` -- as `(width, height, rgba)`.
#[cfg(all(not(target_arch = "wasm32"), feature = "font-atlas"))]
fn draw_reference(software: bool) -> Option<(u32, u32, Vec<u8>)> {
    use clausters_core::osc::{OscMessage, OscPacket, OscType};

    const TREE: &str = r#"{"type":"window","w":240,"h":150,"layout":"col","children":[
            {"id":2,"type":"label","text":"Eight bars, 0123"},
            {"id":3,"type":"slider","value":0.37},
            {"id":4,"type":"notes","h":70,"min":58,"max":70,
             "sample_rate":48000.0,"notes":[0,24000,60,100,0, 18000,30000,64,100,0,
                                            41000,12000,67,100,0]}]}"#;
    let mut gpu = pollster::block_on(Gpu::offscreen_on(240, 150, 4, software)).ok()?;
    let mut host = crate::host::Host::new();
    #[cfg(feature = "font-atlas")]
    host.load_default_face();
    host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: crate::host::GUI_DEF.into(),
            args: vec![OscType::Int(1), OscType::String(TREE.into())],
        }),
        crate::host::ClientId::Udp(std::net::SocketAddr::from((
            std::net::Ipv4Addr::LOCALHOST,
            9000,
        ))),
    );
    let mut renderers = Renderers::new(&gpu.device, gpu.target());
    let mut batches = Batches::new(&gpu.device, gpu.target());
    let inputs = FrameInputs {
        metrics: host.metrics_for(1),
        world: World {
            timelines: host.timelines(),
            ..World::default()
        },
        ..FrameInputs::default()
    };
    let readback = capture(
        &mut gpu,
        &mut renderers,
        &mut batches,
        &mut HashMap::new(),
        &mut HashMap::new(),
        &mut HashMap::new(),
        host.window_def(1)?,
        &inputs,
        &host.theme,
    )
    .ok()?;
    readback.wait(&gpu.device).ok()?;
    Some(readback.pixels())
}

/// **The reference picture holds on a software adapter too**: where a
/// machine has one, the picture drawn on it is compared with the kept
/// one exactly as a card's is -- which is what makes two levels the
/// tolerance, rather than a number picked for one machine.
#[test]
#[cfg(all(not(target_arch = "wasm32"), feature = "font-atlas"))]
fn the_reference_picture_holds_on_a_software_adapter() {
    let (Some(card), Some(software)) = (draw_reference(false), draw_reference(true)) else {
        eprintln!("this machine does not have both adapters");
        return;
    };
    assert_eq!((card.0, card.1), (software.0, software.1));
    let largest = card
        .2
        .as_chunks::<4>()
        .0
        .iter()
        .zip(software.2.as_chunks::<4>().0)
        .map(|(a, b)| (0..3).map(|c| a[c].abs_diff(b[c])).max().unwrap_or(0))
        .max()
        .unwrap_or(0);
    assert!(largest <= 2, "the two adapters differ by {largest} levels");
}

/// **How far a software adapter's picture is from a card's**, for the
/// same window: text, controls with rounded ends, and a roll's notes,
/// drawn at four samples through each adapter and compared pixel by
/// pixel. A measurement, run by hand on a machine that has both:
/// `cargo test --lib -- --ignored --nocapture software_against`.
#[test]
#[ignore = "a measurement; needs a card and a software adapter"]
#[cfg(not(target_arch = "wasm32"))]
fn software_against_hardware() {
    use clausters_core::osc::{OscMessage, OscPacket, OscType};

    const TREE: &str = r#"{"type":"window","w":480,"h":320,"layout":"col","children":[
            {"id":2,"type":"label","text":"The quick brown fox, 0123456789"},
            {"id":3,"type":"slider","value":0.37},
            {"id":4,"type":"knob","value":0.62,"h":90},
            {"id":5,"type":"notes","h":140,"min":48,"max":72,
             "sample_rate":48000.0,"notes":[0,24000,60,100,0, 18000,30000,64,100,0,
                                            41000,12000,67,100,0]}]}"#;
    let draw = |software: bool, samples: u32| -> Option<(String, Vec<u8>)> {
        let mut gpu = pollster::block_on(Gpu::offscreen_on(480, 320, samples, software)).ok()?;
        let mut host = crate::host::Host::new();
        // Text through the typeface where the build rasterizes one.
        #[cfg(feature = "font-atlas")]
        host.load_default_face();
        host.handle_packet(
            OscPacket::Message(OscMessage {
                addr: crate::host::GUI_DEF.into(),
                args: vec![OscType::Int(1), OscType::String(TREE.into())],
            }),
            crate::host::ClientId::Udp(std::net::SocketAddr::from((
                std::net::Ipv4Addr::LOCALHOST,
                9000,
            ))),
        );
        let mut renderers = Renderers::new(&gpu.device, gpu.target());
        let mut batches = Batches::new(&gpu.device, gpu.target());
        let inputs = FrameInputs {
            metrics: host.metrics_for(1),
            world: World {
                timelines: host.timelines(),
                ..World::default()
            },
            ..FrameInputs::default()
        };
        let readback = capture(
            &mut gpu,
            &mut renderers,
            &mut batches,
            &mut HashMap::new(),
            &mut HashMap::new(),
            &mut HashMap::new(),
            host.window_def(1)?,
            &inputs,
            &host.theme,
        )
        .ok()?;
        readback.wait(&gpu.device).ok()?;
        let name = format!("{}, {}x", gpu.adapter, gpu.target().samples);
        Some((name, readback.pixels().2))
    };
    let compare = |what: &str, a: &[u8], b: &[u8]| {
        let mut differ = 0usize;
        let mut largest = 0u8;
        for (pa, pb) in a.as_chunks::<4>().0.iter().zip(b.as_chunks::<4>().0) {
            let d = (0..3).map(|c| pa[c].abs_diff(pb[c])).max().unwrap_or(0);
            differ += usize::from(d > 0);
            largest = largest.max(d);
        }
        let total = a.len() / 4;
        eprintln!(
            "{what}: {differ} of {total} pixels differ ({:.2}%), by at most {largest}",
            100.0 * differ as f64 / total as f64,
        );
    };
    for samples in [4, 1] {
        let (Some((card, a)), Some((software, b))) = (draw(false, samples), draw(true, samples))
        else {
            eprintln!("this machine does not have both adapters");
            return;
        };
        eprintln!("card: {card}\nsoftware: {software}");
        compare("card against software", &a, &b);
        if let (Some((_, a2)), Some((_, b2))) = (draw(false, samples), draw(true, samples)) {
            compare("card against itself", &a, &a2);
            compare("software against itself", &b, &b2);
        }
    }
}

/// The picture of window 1 of a host holding `json`, with every playhead
/// reading `clock` and the control buses reading `level`.
fn picture_at(json: &str, clock: f64, level: f32) -> Picture {
    use crate::host::world::HeadClocks;
    use clausters_core::osc::{OscMessage, OscPacket, OscType};

    /// Every control bus at one value: a meter's level, moved by a test.
    struct Flat(f32);
    impl crate::host::BusSource for Flat {
        fn control(&self, _index: usize) -> f32 {
            self.0
        }
    }

    let mut host = crate::host::Host::new();
    host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: crate::host::GUI_DEF.into(),
            args: vec![OscType::Int(1), OscType::String(json.into())],
        }),
        crate::host::ClientId::Udp(std::net::SocketAddr::from((
            std::net::Ipv4Addr::LOCALHOST,
            9000,
        ))),
    );
    let bus = Flat(level);
    let inputs = FrameInputs {
        metrics: host.metrics_for(1),
        world: World {
            bus: Some(&bus),
            clocks: HeadClocks::uniform(clock),
            timelines: host.timelines(),
            ..World::default()
        },
        ..FrameInputs::default()
    };
    picture(
        host.window_def(1).expect("the window is defined"),
        &inputs,
        &host.theme,
        &HashMap::new(),
        &HashMap::new(),
        (800, 600),
    )
}

#[test]
fn a_panes_playhead_moves_without_a_signal_body() {
    let json = r#"{"type":"window","children":[
            {"type":"pane","id":10,"layer":"points","sample_rate":1,"playhead_at":0,
             "children":[{"type":"curve","id":11,"duration":100,
                "points":[0,0.2,1,0,100,0.8,1,0]}]}]}"#;
    let still = picture_at(json, 0.0, 0.0);
    let moved = picture_at(json, 20.0, 0.0);
    assert!(still.base.near(&moved.base, 0.0));
    assert!(still.over.near(&moved.over, 0.0));
    assert!(
        !still.live.near(&moved.live, LIVE_SLACK),
        "the pane draws its own head without a signal slot"
    );
}

/// A window of everything a clock or a level moves that is drawn into
/// the mesh: a roll and a page, each anchored, a metered multitrack, a
/// meter, a sweeping bar and a clock's reading beside them.
const MOVING: &str = r#"{"type":"window","children":[
        {"id":2,"type":"label","text":"0.000 s","live":true,"h":20},
        {"id":3,"type":"notes","notes":[0.0,50.0,60.0,100.0,0.0],
         "playhead_at":0,"sample_rate":48000},
        {"id":4,"type":"score","playhead_at":0,"vb":[100,100],"prims":[],
         "cursors":[{"t":0,"x":10,"y0":10,"y1":90},{"t":500,"x":60,"y0":10,"y1":90}]},
        {"id":5,"type":"multitrack","playhead_at":0,
         "tracks":["one","",100,0,0,1.0,1],"meters":["one",10,12,2],
         "clips":["a","one",0,48000,0,"a",-1]},
        {"id":6,"type":"meter","bus":20,"rate":"control","h":80},
        {"id":7,"type":"progress","h":30}]}"#;

/// **Nothing but the live layer follows the clock or a level.** The
/// window's picture -- its base, what is drawn over its textures, the
/// host's own bands -- is the same triangles wherever the transport is
/// and whatever a meter reads, which is what lets a tick keep it; and the
/// live layer is not, which is what a tick draws.
#[test]
fn only_the_live_layer_follows_the_clock_and_the_levels() {
    let still = picture_at(MOVING, 0.0, 0.0);
    assert!(still.kept, "nothing here keeps a frame from being kept");
    assert!(
        !still.places.is_empty(),
        "the elements that move are placed for the next tick"
    );
    for (what, clock, level) in [("the clock", 24_000.0, 0.0), ("a level", 0.0, 0.5)] {
        let moved = picture_at(MOVING, clock, level);
        assert!(
            still.base.near(&moved.base, 0.0),
            "the window's picture does not read {what}"
        );
        assert!(
            still.over.near(&moved.over, 0.0),
            "nor does what is drawn over its textures ({what})"
        );
        assert!(still.top.near(&moved.top, 0.0), "nor the host's bands");
        assert!(
            !still.live.near(&moved.live, LIVE_SLACK),
            "the live layer follows {what}"
        );
    }
}

/// **The live layers drawn again are the live layers of a whole frame.**
/// A tick finds each element by the way down to it and hands it the
/// placement it was laid out in; what comes out is what a whole frame at
/// the same clock draws there, so a kept picture and a fresh one cannot
/// be told apart.
#[test]
fn a_tick_draws_the_live_layers_a_whole_frame_would() {
    use crate::host::world::HeadClocks;
    use clausters_core::osc::{OscMessage, OscPacket, OscType};

    let mut host = crate::host::Host::new();
    host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: crate::host::GUI_DEF.into(),
            args: vec![OscType::Int(1), OscType::String(MOVING.into())],
        }),
        crate::host::ClientId::Udp(std::net::SocketAddr::from((
            std::net::Ipv4Addr::LOCALHOST,
            9000,
        ))),
    );
    let tree = host.window_def(1).unwrap();
    let at = |clock: f64| FrameInputs {
        metrics: host.metrics_for(1),
        world: World {
            clocks: HeadClocks::uniform(clock),
            timelines: host.timelines(),
            ..World::default()
        },
        ..FrameInputs::default()
    };
    let none = (HashMap::new(), HashMap::new());
    let kept = picture(tree, &at(0.0), &host.theme, &none.0, &none.1, (800, 600));
    let whole = picture(
        tree,
        &at(24_000.0),
        &host.theme,
        &none.0,
        &none.1,
        (800, 600),
    );
    let mut tick = Mesh::new();
    draw_timeline_heads(
        &mut tick,
        &kept.collected,
        &none.0,
        &none.1,
        &at(24_000.0),
        &host.theme,
    );
    draw_live_places(&mut tick, tree, &kept.places, &at(24_000.0), &host.theme);
    assert!(tick.near(&whole.live, 0.0));
}

/// **A multitrack's meters stay inside the multitrack**: tracks taller
/// than the window, so the stack runs past the rectangle it is given at
/// the bottom, with a label under it -- and the live layer, drawn over
/// everything else, paints nothing outside the multitrack's placement.
#[test]
fn a_tall_multitracks_meters_stay_inside_it() {
    use clausters_core::osc::{OscMessage, OscPacket, OscType};
    const TALL: &str = r#"{"type":"window","children":[
            {"id":2,"type":"label","text":"above","h":40},
            {"id":5,"type":"multitrack",
             "tracks":["one","",250,0,0,1.0,1,"two","",250,0,0,1.0,1,"three","",250,0,0,1.0,1],
             "meters":["one",10,12,2,"two",14,16,2,"three",18,20,2],
             "clips":["a","one",0,48000,0,"a",-1]},
            {"id":7,"type":"label","text":"below","h":40}]}"#;
    let mut host = crate::host::Host::new();
    host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: crate::host::GUI_DEF.into(),
            args: vec![OscType::Int(1), OscType::String(TALL.into())],
        }),
        crate::host::ClientId::Udp(std::net::SocketAddr::from((
            std::net::Ipv4Addr::LOCALHOST,
            9000,
        ))),
    );
    let tree = host.window_def(1).unwrap();
    let inputs = FrameInputs {
        metrics: host.metrics_for(1),
        ..FrameInputs::default()
    };
    let none = (HashMap::new(), HashMap::new());
    let frame = picture(tree, &inputs, &host.theme, &none.0, &none.1, (800, 600));
    let place = frame
        .places
        .iter()
        .find(|p| p.path == [1])
        .expect("the multitrack");
    // The work area: the window less its status band and its menu bar,
    // which are drawn over it after everything else.
    let window = Rect::new(0.0, 0.0, 800.0, 600.0);
    let area = status::content(tree, inputs.status, window, inputs.metrics);
    let area = crate::host::menubar::content(tree, area, inputs.metrics);
    let bounds = place
        .clip
        .map_or(place.rect, |c| c.intersect(place.rect))
        .intersect(area);
    let drawn = frame.live.extent().expect("the meters drew");
    assert!(
        drawn.y >= bounds.y - 0.5 && drawn.y + drawn.h <= bounds.y + bounds.h + 0.5,
        "the live layer spans {}..{}, the multitrack {}..{}",
        drawn.y,
        drawn.y + drawn.h,
        bounds.y,
        bounds.y + bounds.h
    );
}

/// **Work in progress is drawn over the window**: a window whose owner
/// says it is rendering draws a panel over its content -- the dimming,
/// the panel and its frame, the bar and its fill, the name and the
/// percent -- on the top layer, and one that is not draws none of it.
#[test]
fn work_in_progress_is_drawn_over_the_window() {
    use clausters_core::osc::{OscMessage, OscPacket, OscType};
    let top = |progress: f64| {
        let mut host = crate::host::Host::new();
        let def = format!(
            r#"{{"type":"window","status":false,"progress":{progress},
                    "progress_label":"Reverse",
                    "children":[{{"id":2,"type":"label","text":"take","h":20}}]}}"#
        );
        host.handle_packet(
            OscPacket::Message(OscMessage {
                addr: crate::host::GUI_DEF.into(),
                args: vec![OscType::Int(1), OscType::String(def)],
            }),
            crate::host::ClientId::Udp(std::net::SocketAddr::from((
                std::net::Ipv4Addr::LOCALHOST,
                9000,
            ))),
        );
        let tree = host.window_def(1).unwrap();
        let none = (HashMap::new(), HashMap::new());
        let inputs = FrameInputs {
            metrics: host.metrics_for(1),
            ..FrameInputs::default()
        };
        picture(tree, &inputs, &host.theme, &none.0, &none.1, (800, 600))
            .top
            .vertex_count()
    };
    let (idle, working) = (top(-1.0), top(0.5));
    assert!(
        working > idle + 4 * 6,
        "a panel over the window: {idle} -> {working} vertices"
    );
}

/// **The focus ring is over what moves**: a focused multitrack's live layer
/// ends with the ring, so the meters drawn there -- a column the height of
/// its track, at rest -- do not cover the ring where they reach the edge.
#[test]
fn the_focus_ring_is_drawn_over_the_live_layer() {
    use clausters_core::osc::{OscMessage, OscPacket, OscType};
    let mut host = crate::host::Host::new();
    host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: crate::host::GUI_DEF.into(),
            args: vec![OscType::Int(1), OscType::String(MOVING.into())],
        }),
        crate::host::ClientId::Udp(std::net::SocketAddr::from((
            std::net::Ipv4Addr::LOCALHOST,
            9000,
        ))),
    );
    let tree = host.window_def(1).unwrap();
    let none = (HashMap::new(), HashMap::new());
    let live = |focused: Option<i32>| {
        let inputs = FrameInputs {
            metrics: host.metrics_for(1),
            focused,
            ..FrameInputs::default()
        };
        picture(tree, &inputs, &host.theme, &none.0, &none.1, (800, 600))
            .live
            .vertex_count()
    };
    assert_eq!(
        live(Some(5)),
        live(None) + 4 * 6,
        "the four strips of the ring, last"
    );
}

/// **What a whole frame of the score editor costs**, the heaviest window
/// this host draws: the toolbar's symbols, the palettes, the menus and a
/// page of two staves, engraved by the real engraver. Prints the median
/// of a whole frame's picture -- the layout, the meshes, no card -- which
/// is what a set, a gesture or a resize asks for.
///
/// Run with `cargo test --release --features score --lib
/// what_a_whole_frame_costs -- --ignored --nocapture`.
#[cfg(feature = "score")]
#[test]
#[ignore = "timing: only meaningful under --release"]
fn what_a_whole_frame_costs() {
    use clausters_core::ratio::Ratio;
    use clausters_notation::{Item, Marks, Pitch, Sheet, Staff, Step, Voice};

    clausters_gui_draw::font::atlas::set_embedded();
    let notes = |octave: i32| -> Vec<Item> {
        (0..128u64)
            .map(|i| Item::Note {
                id: 0,
                pitches: vec![Pitch {
                    step: [Step::C, Step::E, Step::G, Step::B][i as usize % 4],
                    alter: if i % 7 == 0 { 1 } else { 0 },
                    octave,
                    forced: false,
                }],
                dur: Ratio::new(1, 8),
                tie: false,
                marks: Marks::default(),
            })
            .collect()
    };
    let mut sheet = Sheet {
        staves: vec![
            Staff {
                clef: "G2".into(),
                voices: vec![Voice { items: notes(5) }],
                ..Staff::default()
            },
            Staff {
                clef: "F4".into(),
                voices: vec![Voice { items: notes(3) }],
                ..Staff::default()
            },
        ],
        ..Sheet::default()
    };
    let mut id = 1;
    for staff in &mut sheet.staves {
        for item in &mut staff.voices[0].items {
            if let Item::Note { id: at, .. } = item {
                *at = id;
                id += 1;
            }
        }
    }
    sheet.next_id = id;
    let mei = clausters_notation::sheet_to_mei(&sheet).unwrap();
    let mut host = crate::host::Host::new();
    let (engine, _processed) = clausters_app_engine::AppEngine::detached(48_000.0, 2);
    host.hold_audio_engine(std::sync::Arc::new(std::sync::Mutex::new(engine)));
    let def = host
        .open_app_score_for(
            None,
            Some(mei),
            None,
            serde_json::json!({"w": 1280, "h": 900}),
            "score",
            None,
        )
        .expect("a window");
    while host.window_def(def).is_none() {
        host.take_effects();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let tree = host.window_def(def).unwrap();
    let inputs = FrameInputs {
        metrics: host.metrics_for(def),
        world: World {
            timelines: host.timelines(),
            ..World::default()
        },
        ..FrameInputs::default()
    };
    let none = (HashMap::new(), HashMap::new());
    let draw = || picture(tree, &inputs, &host.theme, &none.0, &none.1, (1280, 900));
    let first = draw();
    let mut took: Vec<f64> = (0..30)
        .map(|_| {
            let start = std::time::Instant::now();
            std::hint::black_box(draw());
            start.elapsed().as_secs_f64() * 1e3
        })
        .collect();
    took.sort_by(f64::total_cmp);
    let verts = |m: &Mesh| m.vertex_count();
    println!(
        "whole frame: {:.3} ms median, {:.3} ms best; {} + {} + {} vertices",
        took[took.len() / 2],
        took[0],
        verts(&first.base),
        verts(&first.over),
        verts(&first.top),
    );
}

/// **What a change inside a complex element costs a whole frame**: the
/// two cases the partial invalidation is measured on. A score of several
/// pages whose own state changed (the selection set, which lets every
/// kept triangle of the page go today), against the same frame with
/// nothing changed; and a multitrack of many tracks, each with boxes of
/// notes, drawn whole as a drag of one box draws it on every move; and a
/// roll of many notes, which keeps no parts.
///
/// Run with `cargo test --release --features score --lib
/// what_a_change_costs -- --ignored --nocapture`.
#[cfg(feature = "score")]
#[test]
#[ignore = "timing: only meaningful under --release"]
fn what_a_change_costs() {
    use clausters_core::ratio::Ratio;
    use clausters_notation::{Item, Marks, Pitch, Sheet, Staff, Step, Voice};
    use serde_json::json;

    clausters_gui_draw::font::atlas::set_embedded();
    let median = |mut took: Vec<f64>| {
        took.sort_by(f64::total_cmp);
        took[took.len() / 2]
    };
    // ---- a score of several pages ----
    let mut id = 1;
    let mut notes = |octave: i32| -> Vec<Item> {
        (0..512u64)
            .map(|i| {
                id += 1;
                Item::Note {
                    id,
                    pitches: vec![Pitch {
                        step: [Step::C, Step::E, Step::G, Step::B][i as usize % 4],
                        alter: if i % 7 == 0 { 1 } else { 0 },
                        octave,
                        forced: false,
                    }],
                    dur: Ratio::new(1, 8),
                    tie: false,
                    marks: Marks::default(),
                }
            })
            .collect()
    };
    let staves = ["G2", "G2", "C3", "F4"]
        .iter()
        .zip([5, 4, 4, 3])
        .map(|(clef, octave)| Staff {
            clef: (*clef).into(),
            voices: vec![Voice {
                items: notes(octave),
            }],
            ..Staff::default()
        })
        .collect();
    let mut sheet = Sheet {
        staves,
        ..Sheet::default()
    };
    sheet.next_id = id + 1;
    let mei = clausters_notation::sheet_to_mei(&sheet).unwrap();
    let mut host = crate::host::Host::new();
    let (engine, _processed) = clausters_app_engine::AppEngine::detached(48_000.0, 2);
    host.hold_audio_engine(std::sync::Arc::new(std::sync::Mutex::new(engine)));
    let def = host
        .open_app_score_for(
            None,
            Some(mei),
            None,
            serde_json::json!({"w": 1280, "h": 900}),
            "score",
            None,
        )
        .expect("a window");
    while host.window_def(def).is_none() {
        host.take_effects();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let page = host.app_widget(def, "widget").unwrap();
    let none = (HashMap::new(), HashMap::new());
    let frame = |host: &crate::host::Host| {
        let inputs = FrameInputs {
            metrics: host.metrics_for(def),
            world: World {
                timelines: host.timelines(),
                ..World::default()
            },
            ..FrameInputs::default()
        };
        let start = std::time::Instant::now();
        let drawn = picture(
            host.window_def(def).unwrap(),
            &inputs,
            &host.theme,
            &none.0,
            &none.1,
            (1280, 900),
        );
        let took = start.elapsed().as_secs_f64() * 1e3;
        (took, drawn.base.vertex_count())
    };
    let (_, vertices) = frame(&host);
    let still = median((0..30).map(|_| frame(&host).0).collect());
    let changed = median(
        (0..30)
            .map(|i| {
                let mut fx = Vec::new();
                let selected = if i % 2 == 0 { "n2" } else { "" };
                host.set_props(page, vec![("selected".into(), json!(selected))], &mut fx);
                frame(&host).0
            })
            .collect(),
    );
    println!(
        "score, 4 x 512 notes: {still:.3} ms unchanged, {changed:.3} ms after its \
             selection changed; {vertices} vertices"
    );

    // ---- a multitrack of many tracks ----
    let (tracks, boxes) = (32, 24);
    let mut track_list = Vec::new();
    let mut clips = Vec::new();
    let mut placed = Vec::new();
    for t in 0..tracks {
        let name = format!("t{t}");
        track_list.extend([
            json!(name),
            json!(name),
            json!(60),
            json!(0),
            json!(0),
            json!(1),
            json!(0),
        ]);
        for b in 0..boxes {
            let clip = format!("c{t}-{b}");
            clips.extend([
                json!(clip),
                json!(name),
                json!(b * 24_000),
                json!(20_000),
                json!(0),
                json!(clip),
                json!(-1),
            ]);
            for n in 0..8 {
                placed.extend([
                    json!(clip),
                    json!(n * 2_500),
                    json!(2_000),
                    json!(48 + (n * 5 + t) % 36),
                    json!(100),
                    json!(0),
                ]);
            }
        }
    }
    let tree = json!({"type": "window", "w": 1280, "h": 900, "children": [
            {"id": 3, "type": "multitrack", "tracks": track_list, "clips": clips,
             "notes": placed, "sample_rate": 48_000}]});
    let mut host = crate::host::Host::new();
    host.handle_packet(
        clausters_core::osc::OscPacket::Message(clausters_core::osc::OscMessage {
            addr: crate::host::GUI_DEF.into(),
            args: vec![
                clausters_core::osc::OscType::Int(1),
                clausters_core::osc::OscType::String(tree.to_string()),
            ],
        }),
        crate::host::ClientId::Udp(std::net::SocketAddr::from((
            std::net::Ipv4Addr::LOCALHOST,
            9000,
        ))),
    );
    let inputs = FrameInputs {
        metrics: host.metrics_for(1),
        world: World {
            timelines: host.timelines(),
            ..World::default()
        },
        ..FrameInputs::default()
    };
    let draw = || {
        picture(
            host.window_def(1).unwrap(),
            &inputs,
            &host.theme,
            &none.0,
            &none.1,
            (1280, 900),
        )
    };
    let vertices = draw().base.vertex_count();
    let whole = median(
        (0..30)
            .map(|_| {
                let start = std::time::Instant::now();
                std::hint::black_box(draw());
                start.elapsed().as_secs_f64() * 1e3
            })
            .collect(),
    );
    println!(
        "multitrack, {tracks} tracks x {boxes} boxes of notes: {whole:.3} ms a whole \
             frame, as every move of a drag draws it; {vertices} vertices"
    );
    // ---- a roll of many notes ----
    let mut notes = Vec::new();
    for n in 0..4_000 {
        notes.extend([
            json!(n as f64 * 0.25),
            json!(0.2),
            json!(36 + (n * 7) % 60),
            json!(100),
            json!(0),
        ]);
    }
    let tree = json!({"type": "window", "w": 1280, "h": 900, "children": [
            {"id": 3, "type": "notes", "notes": notes}]});
    let mut host = crate::host::Host::new();
    host.handle_packet(
        clausters_core::osc::OscPacket::Message(clausters_core::osc::OscMessage {
            addr: crate::host::GUI_DEF.into(),
            args: vec![
                clausters_core::osc::OscType::Int(1),
                clausters_core::osc::OscType::String(tree.to_string()),
            ],
        }),
        crate::host::ClientId::Udp(std::net::SocketAddr::from((
            std::net::Ipv4Addr::LOCALHOST,
            9000,
        ))),
    );
    let inputs = FrameInputs {
        metrics: host.metrics_for(1),
        world: World {
            timelines: host.timelines(),
            ..World::default()
        },
        ..FrameInputs::default()
    };
    let draw = || {
        picture(
            host.window_def(1).unwrap(),
            &inputs,
            &host.theme,
            &none.0,
            &none.1,
            (1280, 900),
        )
    };
    let vertices = draw().base.vertex_count();
    let whole = median(
        (0..30)
            .map(|_| {
                let start = std::time::Instant::now();
                std::hint::black_box(draw());
                start.elapsed().as_secs_f64() * 1e3
            })
            .collect(),
    );
    println!("a roll of 4000 notes: {whole:.3} ms a whole frame; {vertices} vertices");
}

/// **With a dialog up nothing is kept**, and the window's moving lines and
/// its overlays stay in the batch under the one the scrim and the dialog
/// are drawn in: a batch draws its glyphs after all its flat geometry, so
/// sharing one let the window's text show through the dialog.
#[test]
fn a_window_with_a_dialog_up_keeps_no_picture() {
    let dialog = picture_at(
        r#"{"type":"window","children":[
                {"id":3,"type":"notes","notes":[0.0,50.0,60.0,100.0,0.0],"playhead_at":0},
                {"id":9,"type":"layout","modal":true,"title":"t","children":[
                    {"id":10,"type":"label","text":"x"}]}]}"#,
        0.0,
        0.0,
    );
    assert!(!dialog.kept);
    assert!(
        !dialog.over.is_empty(),
        "the window's lines are under the scrim"
    );
    assert!(
        !dialog.live.is_empty(),
        "the scrim and the dialog are in their own batch"
    );
}

/// **A frame is the least it may be and never less.** Asked for what
/// moves alone it may keep the picture; asked for the whole by anyone, or
/// by nobody of this host, it is whole -- and a tick asking beside a set
/// does not take the set's frame away.
#[test]
fn a_frame_is_whole_unless_only_a_tick_asked_for_it() {
    let asked = Asked::default();
    assert!(asked.whole(), "nobody asked: the platform's frame is whole");
    asked.want_live();
    assert!(!asked.whole(), "a tick alone asks for what moves");
    asked.want_whole();
    asked.want_live();
    assert!(asked.whole(), "a set beside a tick is still drawn whole");
    asked.drawn();
    assert!(asked.whole(), "and the next one starts from nobody asking");
}

/// The viewport is the rect **intersected** with the framebuffer, not its
/// origin clamped into it: a lane starting above the window keeps its far
/// edge where it is instead of sliding down, and the framing built from the
/// pair then cuts the picture there.
#[test]
fn a_viewport_is_the_intersection_and_the_framing_follows() {
    let (fb_w, fb_h) = (800u32, 600u32);
    // Wholly inside: the viewport is the rect and nothing is framed.
    let inside = Rect::new(10.0, 20.0, 300.0, 120.0);
    assert_eq!(
        clamp_viewport(inside, fb_w, fb_h),
        (10.0, 20.0, 300.0, 120.0)
    );
    assert_eq!(framing_of(inside, fb_w, fb_h), Framing::IDENTITY);

    // Past the bottom: the height is what is left, and the picture keeps
    // its size (a scale of 2 for half of it showing).
    let below = Rect::new(0.0, 560.0, 300.0, 80.0);
    assert_eq!(clamp_viewport(below, fb_w, fb_h), (0.0, 560.0, 300.0, 40.0));
    assert_eq!(framing_of(below, fb_w, fb_h).scale[1], 2.0);

    // Above the top: the far edge stays at y 40, so 40 px are visible -
    // where clamping the origin would have kept the full 80 and slid the
    // whole picture down into the window.
    let above = Rect::new(0.0, -40.0, 300.0, 80.0);
    assert_eq!(clamp_viewport(above, fb_w, fb_h), (0.0, 0.0, 300.0, 40.0));
    let f = framing_of(above, fb_w, fb_h);
    assert_eq!(f.scale[1], 2.0);
    assert!(
        (f.apply(0.0, -1.0).1 + 1.0).abs() < 1e-6,
        "the bottom edge holds"
    );

    // Entirely outside: an empty viewport, which the caller skips.
    let gone = Rect::new(0.0, 700.0, 300.0, 80.0);
    assert_eq!(clamp_viewport(gone, fb_w, fb_h).3, 0.0);
}

/// **A marker is measured against the numbers it stands among.** It used
/// to be a fixed nine pixels drawn in the size a roll's OSC flags use,
/// which read as a speck beside the tick labels at one density and as a
/// blob at another. The rule is the proportion, so this asserts it at
/// every scale the metrics are generated at rather than a pixel count.
#[test]
fn a_markers_arrow_is_read_against_the_rulers_own_text() {
    for scale in [0.75, 1.0, 1.5, 2.0, 3.0] {
        let m = Metrics::default().at(scale);
        let w = draw::marker_w(&m);
        let cell = clausters_gui_draw::font::advance(m.caption_scale);
        assert!(
            w >= cell,
            "an arrow narrower than one character of the row it stands on \
                 is the speck this replaced (scale {scale})"
        );
        assert!(
            w <= m.ruler_h,
            "and one wider than the strip is tall would cover the numbers \
                 rather than point at them (scale {scale})"
        );
    }
}

fn editor(ruler: Ruler, ruler_y: RulerY) -> EditorProps {
    EditorProps {
        // A test fixture's axis has no air: what it checks is placement,
        // not how far a hand may open the vertical.
        y_headroom: 1.0,
        y_measure: crate::host::widget::YMeasure::Value,
        y_parked: None,
        ruler,
        ruler_strip: true,
        dir: crate::host::widget::RulerDir::Up,
        ruler_y,
        sample_rate: 0.0,
        bit_depth: 16,
        tempo: 1.0,
        tempo_map: None,
        beat_at: 0.0,
        quant: 4.0,
        bars: Default::default(),
        grid: 0.0,
        grid_snap: true,
        autofit: true,
        sel_start: 0.0,
        sel_len: 0.0,
        x_start: 0.0,
        x_len: 1.0,
        playhead_at: -1.0,
        playhead: -1.0,
        cursor: -1.0,
        playhead_loop_start: 0.0,
        playhead_loop_len: 0.0,
        y_start: 0.0,
        y_len: 1.0,
        sel_min: 0.0,
        sel_max: 0.0,
        link: None,
        offset: 0.0,
        markers: Vec::new(),
        naming: None,
    }
}

/// The two paint props reach the frame through one door, and each in its
/// own units: the opacity is already resolved (it composed down the tree at
/// the mutation point), while the radius is a **logical** length that the
/// placement's own table turns into pixels -- so a widget seen at a HiDPI
/// scale rounds by as much as it grew, and one that asked for neither draws
/// exactly what it always drew.
#[test]
fn the_ink_of_a_placement_carries_the_opacity_and_scales_the_radius() {
    use crate::host::guidef::GuiNode;
    use crate::host::widget::{Widget, resolve_style};

    let json = r#"{"type":"window","margin":0,"opacity":0.5,"children":[
            {"id":7,"type":"button","label":"go","radius":6},
            {"id":8,"type":"button","label":"plain"}]}"#;
    let mut tree = Widget::from_node(1, &GuiNode::parse(json.as_bytes()).unwrap(), &[]).unwrap();
    resolve_style(&mut tree, &Arc::new(Theme::default()));
    for (scale, want_radius) in [(1.0, 6.0), (2.0, 12.0)] {
        let m = Metrics::default().resolved(scale);
        let placed = layout::layout(Rect::new(0.0, 0.0, 400.0, 200.0), &tree, &m);
        let ink = |id: i32| {
            ink_of(
                placed
                    .iter()
                    .find(|p| p.widget.id == Some(id))
                    .expect("placed"),
            )
        };
        assert_eq!(ink(7).radius, want_radius);
        assert_eq!(ink(7).alpha, 0.5, "the window's fade reaches its buttons");
        assert_eq!(ink(8).radius, 0.0, "a widget that said nothing is square");
    }
}

#[test]
fn timeline_body_reserves_the_ruler_strip_and_the_group_gutter() {
    let rect = Rect::new(10.0, 10.0, 400.0, 200.0);
    let m = Metrics::default();
    // The x ruler takes the bottom strip; the gutter is the group's, so a
    // view alone with its value ruler indents by that ruler's width.
    let body = timeline_body(
        rect,
        &editor(Ruler::Time, RulerY::Norm),
        false,
        m.ruler_w,
        &m,
    );
    assert_eq!(body.h, 200.0 - m.ruler_h);
    assert_eq!(body.x, 10.0 + m.ruler_w);
    assert_eq!(body.w, 400.0 - m.ruler_w);
    // Each is independently optional.
    let x_only = timeline_body(rect, &editor(Ruler::Time, RulerY::Off), false, 0.0, &m);
    assert_eq!((x_only.x, x_only.w), (10.0, 400.0));
    assert_eq!(x_only.h, 200.0 - m.ruler_h);
    let y_only = timeline_body(rect, &editor(Ruler::Off, RulerY::Hz), false, m.ruler_w, &m);
    assert_eq!(y_only.h, 200.0);
    assert_eq!(y_only.x, 10.0 + m.ruler_w);
    assert_eq!(
        timeline_body(rect, &editor(Ruler::Off, RulerY::Off), false, 0.0, &m),
        rect
    );
    // A **labelled** view gives up the same strip a labelled control does:
    // the picture starts below the caption instead of under it. It stacks
    // with the ruler, since the two take opposite ends of the rect.
    let strip = crate::host::widget::size::label_strip(true, m.text_scale, &m) + m.pad;
    assert!(strip > 0.0, "a labelled widget reserves a strip");
    let titled = timeline_body(rect, &editor(Ruler::Off, RulerY::Off), true, 0.0, &m);
    assert_eq!(titled.y, 10.0 + strip);
    assert_eq!(titled.h, 200.0 - strip);
    let titled_ruled = timeline_body(rect, &editor(Ruler::Time, RulerY::Off), true, 0.0, &m);
    assert_eq!(titled_ruled.y, 10.0 + strip);
    assert_eq!(titled_ruled.h, 200.0 - strip - m.ruler_h);
    // The caption's own gap is the reason the strip is not just the text's
    // height: the picture starts a pad below the line, exactly as a
    // control's body does, and never against it.
    assert_eq!(
        titled.y,
        10.0 + crate::host::graphics::controls::label_height(rect.h, true, m.text_scale, &m)
            + m.pad,
        "the same formula controls::body_rect uses vertically"
    );

    // Sharing an axis with a lane, the same view starts its trace where the
    // lane starts its clips -- the indent is the axis', not the widget's.
    let shared = timeline_body(
        rect,
        &editor(Ruler::Off, RulerY::Norm),
        false,
        m.header_w,
        &m,
    );
    assert_eq!(shared.x, 10.0 + m.header_w);
}

/// **The note the draw pass leaves**: a zoom past the summary over a span
/// nothing covers is a fetch owed, and every other case is silence.
#[test]
fn only_a_zoom_past_the_summary_over_uncovered_samples_asks_for_a_span() {
    use crate::waveform::WaveformData;
    // Long enough that the whole of it, over 800 px, is coarser than a
    // bucket -- which is what "zoomed out" means for this question.
    let (bucket, frames) = (256usize, 256 * 4_000);
    let told = WaveformView::new(WaveformData::with_multi_pyramid(
        clausters_core::peaks::MultiPyramid::empty(frames, 1, bucket),
    ));
    let wide = View {
        start: 0.0,
        len: frames as f64,
    };
    assert_eq!(
        owed(&told, &wide, 800.0, None),
        None,
        "zoomed out, the summary is the answer"
    );
    let close = View {
        start: 1_000.0,
        len: 2_000.0,
    };
    assert_eq!(
        owed(&told, &close, 800.0, None),
        Some(Owed::Samples { a: 0, b: 4_000 }),
        "past the bucket over samples it cannot answer for: the view and \
             half a view past either edge"
    );

    // A run over the view alone is asked again: a pan would leave it
    // at once, so what is held reaches a quarter of a view past it.
    let mut data = WaveformData::with_multi_pyramid(clausters_core::peaks::MultiPyramid::empty(
        frames, 1, bucket,
    ));
    assert!(data.set_window(1_000, 1, &vec![0.0; 2_000]));
    let edge = WaveformView::new(data);
    assert_eq!(
        owed(&edge, &close, 800.0, None),
        Some(Owed::Samples { a: 0, b: 4_000 })
    );

    // The same view once the run asked has arrived: nothing more is owed.
    let mut data = WaveformData::with_multi_pyramid(clausters_core::peaks::MultiPyramid::empty(
        frames, 1, bucket,
    ));
    assert!(data.set_window(0, 1, &vec![0.0; 4_000]));
    let covered = WaveformView::new(data);
    assert_eq!(owed(&covered, &close, 800.0, None), None);

    // And a view that owns its samples never asks.
    let owned = WaveformView::new(WaveformData::from_interleaved(
        &vec![0.0; frames],
        1,
        bucket,
    ));
    assert_eq!(owed(&owned, &close, 800.0, None), None);
}

/// **A texture over a summary asks its samples at any zoom**: zoomed
/// out, where the trace's summary answers, the spectrogram still has
/// nothing to analyze. What is held covers it once it lands, a view
/// past [`TEXTURE_SPAN`] asks that much about its middle and is covered
/// by it, and a view that owns its samples never asks.
#[test]
fn a_texture_over_a_summary_asks_its_samples_at_any_zoom() {
    use crate::waveform::WaveformData;
    let (bucket, frames) = (256usize, 256 * 4_000usize);
    let summary = || {
        WaveformData::with_multi_pyramid(clausters_core::peaks::MultiPyramid::empty(
            frames, 1, bucket,
        ))
    };
    let wide = View {
        start: 0.0,
        len: frames as f64,
    };
    let told = WaveformView::new(summary());
    assert_eq!(owed(&told, &wide, 800.0, None), None, "the trace's answer");
    assert_eq!(
        texture_owed(&told, &wide, None),
        Some(Owed::Run { a: 0, b: frames }),
        "the texture's: the whole take, which the view shows"
    );
    let mut data = summary();
    assert!(data.set_window(0, 1, &vec![0.0; frames]));
    assert_eq!(texture_owed(&WaveformView::new(data), &wide, None), None);

    let long = 4 * TEXTURE_SPAN;
    let all = View {
        start: 0.0,
        len: long as f64,
    };
    let told = WaveformView::new(WaveformData::with_multi_pyramid(
        clausters_core::peaks::MultiPyramid::empty(long, 1, bucket),
    ));
    let middle = long / 2 - TEXTURE_SPAN / 2;
    assert_eq!(
        texture_owed(&told, &all, None),
        Some(Owed::Run {
            a: middle,
            b: middle + TEXTURE_SPAN
        })
    );
    let mut data = WaveformData::with_multi_pyramid(clausters_core::peaks::MultiPyramid::empty(
        long, 1, bucket,
    ));
    assert!(data.set_window(middle, 1, &vec![0.0; TEXTURE_SPAN]));
    assert_eq!(
        texture_owed(&WaveformView::new(data), &all, None),
        None,
        "the run asked is the run that answers: nothing asked again"
    );

    let owned = WaveformView::new(WaveformData::from_interleaved(
        &vec![0.0; frames],
        1,
        bucket,
    ));
    assert_eq!(texture_owed(&owned, &wide, None), None);
}

/// **A take being recorded is asked for what is behind its frontier.** The
/// span is clamped to `written` and never crosses it: a page zoomed past
/// its summary reads the samples that are final, and nothing over the zeros
/// the buffer is still holding.
#[test]
fn a_recording_asks_for_the_span_behind_its_frontier_and_no_further() {
    use crate::waveform::WaveformData;
    let (bucket, frames) = (256usize, 256 * 4_000);
    let told = WaveformView::new(WaveformData::with_multi_pyramid(
        clausters_core::peaks::MultiPyramid::empty(frames, 1, bucket),
    ));
    let close = View {
        start: 1_000.0,
        len: 2_000.0,
    };
    assert_eq!(
        owed(&told, &close, 800.0, Some(10_000)),
        Some(Owed::Samples { a: 0, b: 4_000 }),
        "wholly behind the frontier: the same span a finished take asks for"
    );
    assert_eq!(
        owed(&told, &close, 800.0, Some(2_000)),
        Some(Owed::Samples { a: 0, b: 2_000 }),
        "straddling it: only the settled half"
    );
    assert_eq!(
        owed(&told, &close, 800.0, Some(500)),
        None,
        "wholly past it: there is nothing written to read"
    );
    assert_eq!(
        owed(&told, &close, 800.0, Some(0)),
        None,
        "a take nothing has been written into yet"
    );
    // Zoomed out it is still the summary's answer, recording or not.
    let wide = View {
        start: 0.0,
        len: frames as f64,
    };
    assert_eq!(owed(&told, &wide, 800.0, Some(10_000)), None);
}

/// **What is owed is a finer summary, until a summary stops being worth
/// asking for.** The same view at three zooms answers three ways: the
/// summary it has, a grid it can be sent in one reply, and the samples.
#[test]
fn a_zoom_past_the_summary_asks_for_a_finer_grid_and_not_for_the_samples() {
    use crate::waveform::WaveformData;
    let (bucket, frames) = (256usize, 256 * 4_000);
    let told = WaveformView::new(WaveformData::with_multi_pyramid(
        clausters_core::peaks::MultiPyramid::empty(frames, 1, bucket),
    ));
    // 800 px over 256 000 frames: 320 samples a pixel, coarser than the
    // bucket, and the summary is already the answer.
    let out = View {
        start: 0.0,
        len: 256_000.0,
    };
    assert_eq!(owed(&told, &out, 800.0, None), None);

    // 800 px over 51 200 frames: 64 samples a pixel. What draws that is one
    // min/max pair per column, which is 800 pairs -- and asking for the
    // samples would be 51 200 of them, a few hundred kilobytes through a
    // 64 KiB carrier, to compute a few kilobytes' worth.
    let mid = View {
        start: 0.0,
        len: 51_200.0,
    };
    assert_eq!(
        owed(&told, &mid, 800.0, None),
        Some(Owed::Summary {
            a: 0,
            b: 76_800,
            bucket: 32,
            seen: (0, 51_200),
        }),
        "two buckets a column, finer than the view's own"
    );

    // 800 px over 3 200 frames: four samples a pixel. A grid there would be
    // two samples a bucket -- three floats to describe two -- so the
    // samples are both cheaper and exact.
    let deep = View {
        start: 0.0,
        len: 12_800.0,
    };
    assert_eq!(
        owed(&told, &deep, 800.0, None),
        Some(Owed::Samples { a: 0, b: 19_200 }),
    );
}

/// The grid a span is asked at: a power of two, two of them to a column,
/// coarsened until one reply holds the span, and `None` where the samples
/// are the better answer.
#[test]
fn the_detail_grid_holds_two_buckets_a_column_and_fits_one_reply() {
    let base = 256;
    for (per_px, want) in [
        (256.0, Some(128)),
        (64.0, Some(32)),
        (63.0, Some(16)),
        (32.0, Some(16)),
        (31.9, None),
        (8.0, None),
        (0.5, None),
    ] {
        assert_eq!(
            detail_bucket(per_px, 8_192, base),
            want,
            "at {per_px} samples a pixel"
        );
    }
    // Never as coarse as the view's own summary: there would be nothing to
    // gain, and the report belongs in the pyramid itself.
    assert_eq!(detail_bucket(512.0, 8_192, base), None);
    assert_eq!(detail_bucket(1_000.0, 8_192, base), None);
    // A span too long for one reply is summarized coarser rather than in
    // two, because a grid is replaced and not extended.
    let long = DETAIL_GRID_BUCKETS * 64;
    assert_eq!(detail_bucket(64.0, long, base), Some(64));
    assert_eq!(
        detail_bucket(64.0, long * 8, base),
        None,
        "not without passing the view's own"
    );
    assert_eq!(detail_bucket(64.0, 0, base), None);
}

#[test]
fn placed_nav_shifts_the_body_window_by_the_offset() {
    let nav = View {
        start: 100.0,
        len: 400.0,
    };
    // The un-placed default is the identity.
    assert_eq!(placed_nav(&nav, 0.0), nav);
    // A member placed at timeline sample 100 draws its data sample 0 there:
    // the local window starts one clip-length earlier.
    let placed = placed_nav(&nav, 100.0);
    assert_eq!((placed.start, placed.len), (0.0, 400.0));
    // Placing further right pushes the local window negative (data before
    // the visible origin) without changing the span.
    let placed = placed_nav(&nav, 250.0);
    assert_eq!((placed.start, placed.len), (-150.0, 400.0));
}

#[test]
fn channel_at_picks_the_channel_under_the_cursor() {
    let body = Rect::new(0.0, 0.0, 400.0, 300.0);
    assert_eq!(channel_at(body, 3, 50.0), 0);
    assert_eq!(channel_at(body, 3, 150.0), 1);
    assert_eq!(channel_at(body, 3, 299.0), 2);
    assert_eq!(channel_at(body, 3, 1000.0), 2, "clamped");
}

#[test]
fn channels_split_the_body_evenly_and_share_x() {
    let body = Rect::new(0.0, 0.0, 400.0, 300.0);
    let a = channel_rect(body, 3, 0);
    let b = channel_rect(body, 3, 1);
    let c = channel_rect(body, 3, 2);
    assert_eq!(a.h, 100.0);
    assert_eq!((a.x, a.w), (b.x, b.w));
    assert_eq!(b.y, 100.0);
    assert_eq!(c.y + c.h, 300.0);
}

#[test]
fn deinterleave_splits_frames_and_drops_the_partial_tail() {
    let flat = [1.0, -1.0, 2.0, -2.0, 3.0];
    let chans = deinterleave(&flat, 2);
    assert_eq!(chans, vec![vec![1.0, 2.0], vec![-1.0, -2.0]]);
    assert_eq!(deinterleave(&flat, 1).len(), 1);
    assert_eq!(deinterleave(&flat, 1)[0].len(), 5);
}

/// A channel is analyzed at the hop asked for, however many textures
/// wide that makes it: what bounds a transform is the memory it takes
/// (`spectrogram::hop_capped`), and the card is shown a window of it.
#[test]
fn stft_channels_keep_the_hop_past_a_textures_width() {
    let n = 200_000;
    let chan: Vec<f32> = (0..n).map(|i| (i as f32 * 0.01).sin()).collect();
    let stacks = stft_channels(vec![chan], (256, 8, 0), 48_000.0);
    assert_eq!(stacks.len(), 1);
    assert_eq!(stacks[0].hop(), 8);
    assert!(stacks[0].n_frames() > crate::spectrogram::MAX_FRAMES);
    assert!(stacks[0].n_frames() <= crate::spectrogram::max_frames(256));
    assert_eq!(stacks[0].total_samples(), n);
}
