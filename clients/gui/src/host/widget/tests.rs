//! The schema's own suite: every test here parses a `/gui_def` document with
//! [`Widget::from_node`] and then applies props to it, because that round trip
//! *is* what the model promises — a wire node in, a typed widget out, a
//! `/gui_set` landing on the part of it that the prop names.

use clausters_core::osc::OscType;

use super::super::signal::Presentation;
use super::element::SlotKind;
use super::*;
use crate::spectrogram::FreqScale;

fn node(json: &str) -> GuiNode {
    GuiNode::parse(json.as_bytes()).unwrap()
}

/// The traversal is pre-order and keeps each level's own order: a parent
/// before the children it contains, siblings left to right. Every pass that
/// reads the tree through it inherits that order, so it is pinned here
/// rather than in each of them.
#[test]
fn descendants_walk_parents_before_children_in_order() {
    let tree = Widget::from_node(
        1,
        &node(
            r#"{"id":1,"type":"window","children":[
                {"id":2,"type":"layout","children":[
                    {"id":3,"type":"label","text":"a"},
                    {"id":4,"type":"label","text":"b"}]},
                {"id":5,"type":"layout","children":[
                    {"id":6,"type":"label","text":"c"}]}]}"#,
        ),
        &[],
    )
    .unwrap();
    let ids: Vec<i32> = tree.descendants().filter_map(|w| w.id).collect();
    assert_eq!(ids, vec![1, 2, 3, 4, 5, 6]);
}

/// A plane's zoom is nameable *and* clearable: a positive number is the
/// scale, and anything else — `0`, an empty string — puts it back to the
/// default, which is the only way the wire can ask for a default it has no
/// number for.
#[test]
fn a_plane_zoom_is_named_or_cleared() {
    let m = crate::host::metrics::Metrics::default().resolved(2.0);
    let view = |json: &str| match Widget::from_node(1, &node(json), &[]).unwrap().kind {
        WidgetKind::Scroll { view, .. } => view,
        other => panic!("not a scroll: {other:?}"),
    };
    assert_eq!(view(r#"{"type":"plane"}"#).view_zoom, None);
    assert_eq!(view(r#"{"type":"plane","view_zoom":0}"#).view_zoom, None);
    assert_eq!(
        view(r#"{"type":"plane","view_zoom":3}"#).view_zoom,
        Some(3.0)
    );

    let mut kind = Widget::from_node(1, &node(r#"{"type":"plane"}"#), &[])
        .unwrap()
        .kind;
    let zoom_of = |kind: &WidgetKind| match kind {
        WidgetKind::Scroll { view, .. } => view.zoom(&m),
        other => panic!("not a scroll: {other:?}"),
    };
    let set = |kind: &mut WidgetKind, v: Value| super::apply::apply_kind(kind, "view_zoom", &v);
    assert!(set(&mut kind, serde_json::json!(3.0)));
    assert_eq!(zoom_of(&kind), 3.0);
    for cleared in [serde_json::json!(0), serde_json::json!("")] {
        assert!(set(&mut kind, serde_json::json!(3.0)));
        assert!(set(&mut kind, cleared.clone()), "{cleared} is handled");
        assert_eq!(
            zoom_of(&kind),
            m.ui_scale as f64,
            "{cleared} restores the default"
        );
    }
}

#[test]
fn themes_resolve_recursively_at_the_mutation_point() {
    // A theme group on a container overlays its whole subtree; a nested
    // group overlays the *inherited* table; a `color` re-seeds one widget;
    // a widget with neither shares its parent's Arc.
    let n = node(
        r##"{"type":"window","children":[
          {"id":11,"type":"layout","theme":{"accent":"#ff0000"},"children":[
            {"id":12,"type":"label","text":"in the group"},
            {"id":13,"type":"slider","color":"#0000ff"},
            {"id":14,"type":"layout","theme":{"text":"#00ff00"},"children":[
              {"id":15,"type":"label","text":"nested"}]}]},
          {"id":16,"type":"label","text":"outside"}]}"##,
    );
    let mut w = Widget::from_node(1, &n, &[]).unwrap();
    let base = Arc::new(super::super::theme::Theme::default());
    resolve_themes(&mut w, &base);
    let theme_of = |id: i32| w.find(id).unwrap().theme.clone().unwrap();
    let red = [1.0, 0.0, 0.0, 1.0];
    assert_eq!(theme_of(12).accent, red, "the group reaches the subtree");
    assert_eq!(
        theme_of(13).accent,
        [0.0, 0.0, 1.0, 1.0],
        "color wins on its widget"
    );
    assert_eq!(
        theme_of(13).text,
        base.text,
        "color leaves the group's other roles"
    );
    let nested = theme_of(15);
    assert_eq!(
        nested.accent, red,
        "a nested group inherits the outer overlay"
    );
    assert_eq!(nested.text, [0.0, 1.0, 0.0, 1.0], "and adds its own");
    assert!(
        Arc::ptr_eq(&theme_of(16), &base),
        "outside any group the host theme is shared, not cloned"
    );
}

#[test]
fn style_props_set_live_and_clear() {
    let n = node(r#"{"type":"layout"}"#);
    let mut w = Widget::from_node(1, &n, &[]).unwrap();
    assert!(w.style_apply("color", &Value::from("#112233")));
    assert!(w.color.is_some());
    assert!(w.style_apply("color", &Value::from("")), "empty clears");
    assert!(w.color.is_none());
    // `theme` rides as a JSON object or its string carrier.
    assert!(w.style_apply("theme", &Value::from(r##"{"accent":"#ff0000"}"##)));
    assert!(w.theme_over.is_some());
    assert!(
        w.style_apply("theme", &Value::from("")),
        "empty clears the group"
    );
    assert!(w.theme_over.is_none());
    assert!(!w.style_apply("color", &Value::from("nonsense")));
    assert!(!w.style_apply("value", &Value::from(1)), "not a style key");
}

#[test]
fn text_size_sets_live_and_clamps() {
    let n = node(r#"{"type":"slider","label":"amp"}"#);
    let mut w = Widget::from_node(1, &n, &[]).unwrap();
    assert!(w.kind.apply("text_size", &Value::from(4.0)));
    // Out-of-range sizes clamp instead of degenerating the strip math: the
    // clamp is the parser's, so what this pass answers for is that the key
    // reached the element at all.
    assert!(w.kind.apply("text_size", &Value::from(0.0)));
    assert!(!w.kind.apply("text_size", &Value::from("big")));
    // A bad align value is rejected, the good ones apply.
    let n = node(r#"{"type":"label","text":"hi"}"#);
    let mut w = Widget::from_node(2, &n, &[]).unwrap();
    assert!(!w.kind.apply("align", &Value::from("sideways")));
    assert!(w.kind.apply("align", &Value::from("end")));
    assert!(w.kind.apply("wrap", &Value::from(1)));
}

#[test]
fn window_with_inline_waveform() {
    let n = node(
        r#"{"type":"window","title":"W","w":480,"h":240,"flow":"col",
            "children":[{"id":12,"type":"signal","view":"trace","data":[0.0,0.5,-0.5,1.0],"base_bucket":2}]}"#,
    );
    let w = Widget::from_node(1, &n, &[]).unwrap();
    assert_eq!(w.id, Some(1));
    match w.kind {
        WidgetKind::Window {
            title,
            width,
            height,
            layout,
            ..
        } => {
            assert_eq!(title.as_deref(), Some("W"));
            assert_eq!((width, height), (480, 240));
            assert_eq!(layout, Layout::Col);
        }
        other => panic!("expected window, got {other:?}"),
    }
    assert_eq!(w.children.len(), 1);
    let data = w.children[0]
        .signal()
        .and_then(|el| el.source.data())
        .expect("a waveform is a signal element over addressable samples");
    assert_eq!(&data.samples[..], &[0.0, 0.5, -0.5, 1.0]);
    assert_eq!(data.base_bucket, 2);
    assert_eq!(data.buffer, None);
}

#[test]
fn waveform_parses_its_placement_offset() {
    let n = node(
        r#"{"type":"window","children":[
            {"id":1,"type":"signal","view":"trace","data":[0.0,1.0],"offset":8.0},
            {"id":2,"type":"signal","view":"trace","data":[0.0,1.0],"offset":-3.0}
        ]}"#,
    );
    let w = Widget::from_node(9, &n, &[]).unwrap();
    assert_eq!(w.children[0].kind.editor().unwrap().offset, 8.0);
    // A negative placement clamps to 0 (no clip starts before the timeline).
    assert_eq!(w.children[1].kind.editor().unwrap().offset, 0.0);
    // The default is un-placed.
    let n = node(
        r#"{"type":"window","children":[{"id":3,"type":"signal","view":"trace","data":[0.0]}]}"#,
    );
    let w = Widget::from_node(9, &n, &[]).unwrap();
    assert_eq!(w.children[0].kind.editor().unwrap().offset, 0.0);
}

#[test]
fn track_carries_clips_with_their_placement() {
    let n = node(
        r#"{"type":"window","children":[
            {"id":1,"type":"field","label":"drums","children":[
                {"id":10,"type":"field","offset":0.0,"dur":100.0,"data":[0.0,1.0],"label":"a"},
                {"id":11,"type":"field","offset":-5.0,"dur":50.0}
            ]}
        ]}"#,
    );
    let w = Widget::from_node(9, &n, &[]).unwrap();
    let track = &w.children[0];
    match &track.kind {
        WidgetKind::Track { label, .. } => assert_eq!(label.as_deref(), Some("drums")),
        other => panic!("expected track, got {other:?}"),
    }
    assert_eq!(track.children.len(), 2, "a track carries its clips");
    let clip = &track.children[0];
    match &clip.kind {
        WidgetKind::Clip { offset, dur, label } => {
            assert_eq!((*offset, *dur), (0.0, 100.0));
            assert_eq!(label.as_deref(), Some("a"));
        }
        other => panic!("expected clip, got {other:?}"),
    }
    // The take is a **child** of the clip, and an ordinary signal element:
    // the clip is a container, so what it holds is elements.
    assert_eq!(clip.children.len(), 1, "one body: the take");
    let take = clip.signal_target().expect("the clip holds a take");
    assert_eq!(&take.source.data().unwrap().samples[..], &[0.0, 1.0]);
    assert!(
        !take.caps.navigable,
        "a body navigates nothing: the clip does"
    );
    assert!(take.source.data().unwrap().bulk, "a take is bulk");
    // A clip with no source at all holds no take: a body a clip does not
    // describe is simply absent.
    assert!(track.children[1].children.is_empty());
    // A negative offset clamps to 0 (no clip starts before the timeline).
    match &track.children[1].kind {
        WidgetKind::Clip { offset, .. } => assert_eq!(*offset, 0.0),
        other => panic!("expected clip, got {other:?}"),
    }
}

#[test]
fn a_lane_carries_the_ruler_and_playhead_chrome() {
    let n = node(
        r#"{"type":"window","children":[
            {"id":1,"type":"field","ruler":"beats","tempo":2.0,"playhead_at":480.0},
            {"id":2,"type":"field"}
        ]}"#,
    );
    let mut w = Widget::from_node(9, &n, &[]).unwrap();
    let lane = w.children[0].kind.editor().unwrap();
    assert_eq!(lane.ruler, Ruler::Beats);
    assert_eq!((lane.tempo, lane.playhead_at), (2.0, 480.0));
    // A lane asks for no ruler by default (it reserves no strip), and shows
    // no playhead until one is anchored.
    let plain = w.children[1].kind.editor().unwrap();
    assert_eq!(plain.ruler, Ruler::Off);
    assert!(plain.playhead_at < 0.0);
    // The chrome parses live too: `/gui_set` reaches these fields (what a
    // lane *draws* is its navigation group's playhead, which these seed).
    assert!(
        w.children[1]
            .kind
            .apply("playhead_at", &serde_json::json!(96000.0))
    );
    assert_eq!(w.children[1].kind.editor().unwrap().playhead_at, 96000.0);
}

#[test]
fn a_clip_parses_its_piano_roll_notes() {
    let n = node(
        r#"{"type":"window","children":[
            {"id":1,"type":"field","children":[
                {"id":10,"type":"field","offset":0.0,"dur":400.0,"min":48.0,"max":72.0,
                 "notes":[0.0,100.0,60.0, 100.0,100.0,67.0, 999.0]}
            ]}
        ]}"#,
    );
    let w = Widget::from_node(9, &n, &[]).unwrap();
    let clip = &w.children[0].children[0];
    let body = clip.children.first().expect("the clip grew a roll body");
    assert_eq!(
        body.kind.body_role(),
        Some(super::element::BodyRole::Notes),
        "{:?}",
        body.kind
    );
    // Two complete triples; the trailing lone number is dropped, so the roll
    // reaches the end of the second note and no further.
    assert_eq!(body.kind.content_span(), Some(200.0));
}

#[test]
fn waveform_by_server_buffer_starts_empty_with_the_buffer_number() {
    let n = node(
        r#"{"type":"window","children":[{"id":3,"type":"signal","view":"trace","buffer":7}]}"#,
    );
    let w = Widget::from_node(1, &n, &[]).unwrap();
    let data = w.children[0]
        .signal()
        .and_then(|el| el.source.data())
        .unwrap();
    assert!(
        data.samples.is_empty(),
        "no inline data yet — fetched later"
    );
    assert_eq!(data.buffer, Some(7));
}

#[test]
fn waveform_by_path_and_cache_defer_with_their_props() {
    let n = node(
        r#"{"type":"window","children":[
            {"id":1,"type":"signal","view":"trace","path":"/tmp/buf.f32","channels":2},
            {"id":2,"type":"signal","view":"trace","cache":"/tmp/buf.peaks"}
        ]}"#,
    );
    let w = Widget::from_node(9, &n, &[]).unwrap();
    let data = w.children[0]
        .signal()
        .and_then(|el| el.source.data())
        .unwrap();
    assert!(
        data.samples.is_empty(),
        "samples are mapped later, not inline"
    );
    assert_eq!(
        data.path.as_deref(),
        Some(std::path::Path::new("/tmp/buf.f32"))
    );
    assert_eq!(data.channels, 2);
    let data = w.children[1]
        .signal()
        .and_then(|el| el.source.data())
        .unwrap();
    assert_eq!(
        data.cache.as_deref(),
        Some(std::path::Path::new("/tmp/buf.peaks"))
    );
}

#[test]
fn meter_and_scope_parse_with_defaults_and_apply() {
    let n = node(
        r#"{"type":"window","children":[
            {"id":1,"type":"meter","bus":5,"max":2.0,"label":"out"},
            {"id":2,"type":"signal","view":"trace","bus":6}
        ]}"#,
    );
    let mut w = Widget::from_node(9, &n, &[]).unwrap();
    // The meter is an element: the schema resolves the name and the document's
    // props become its declaration, which is all this pass answers for. What it
    // does with them is its own file's suite.
    assert!(matches!(w.children[0].kind, WidgetKind::Custom(_)));
    assert_eq!(
        w.children[0].kind.needs().levels,
        vec![5],
        "a meter watches audio unless told"
    );
    // The scope is a signal element over a forward-only source, and
    // defaults to the bipolar [-1, 1] range.
    let el = w.children[1].signal().expect("a scope is a signal element");
    assert_eq!(el.source.bus().unwrap().bus, 6);
    assert_eq!((el.value.min, el.value.max), (Some(-1.0), Some(1.0)));
    // An audio-rate meter reads a published level, not a control bus.
    assert_eq!(w.children[0].kind.needs().buses.first().copied(), None);
    // A live `/gui_set` reaches the element and moves the declaration with it.
    let meter = w.find_mut(1).unwrap();
    assert!(meter.kind.apply("bus", &Value::from(8)));
    assert_eq!(meter.kind.needs().levels, vec![8]);
    assert!(meter.kind.apply("rate", &Value::from("control")));
    assert_eq!(meter.kind.needs().buses, vec![8]);
    assert!(meter.kind.needs().levels.is_empty());
}

#[test]
fn nodetree_and_plot_parse_with_defaults_and_apply() {
    let n = node(
        r#"{"type":"window","children":[
            {"id":1,"type":"nodes","group":2,"controls":0,"label":"tree"},
            {"id":2,"type":"signal","navigable":0,"data":[0.0,1.0,-1.0],"max":2.0,"label":"sig"}
        ]}"#,
    );
    let mut w = Widget::from_node(9, &n, &[]).unwrap();
    // The node-tree view is an element; its props are its own file's suite.
    assert!(matches!(w.children[0].kind, WidgetKind::Custom(_)));
    assert_eq!(w.children[0].kind.needs().node_groups, vec![2]);
    // A nodetree is non-interactive and reads no bus.
    assert_eq!(w.children[0].kind.event_value(), None);
    assert_eq!(w.children[0].kind.needs().buses.first().copied(), None);
    let el = w.children[1].signal().expect("a plot is a signal element");
    assert_eq!(&el.source.data().unwrap().samples[..], &[0.0, 1.0, -1.0]);
    // An explicit side is kept; the omitted one auto-fits.
    assert_eq!((el.value.min, el.value.max), (None, Some(2.0)));
    // A plot is the point of the product with every capability off.
    assert_eq!(el.caps, signal::Caps::default());
    // Live `/gui_set` retargets the tree's group and rescales the plot.
    assert!(w.find_mut(1).unwrap().kind.apply("group", &Value::from(0)));
    assert!(w.find_mut(2).unwrap().kind.apply("max", &Value::from(1.0)));
    assert_eq!(w.children[0].kind.needs().node_groups, vec![0]);
}

#[test]
fn plot_parses_views_channels_and_applies_live() {
    let n = node(
        r#"{"type":"window","children":[
            {"id":1,"type":"signal","navigable":0,"data":[0.0,1.0,0.0,-1.0],"channels":2,
             "view":"spectrum","overlay":1,"sample_rate":48000.0,
             "fft_size":1024,"freq_scale":"mel","ruler":"time","ruler_y":"off"}
        ]}"#,
    );
    let mut w = Widget::from_node(9, &n, &[]).unwrap();
    let el = w.children[0].signal().unwrap();
    assert_eq!(el.channels(), 2);
    assert_eq!(el.presentation, Presentation::Spectrum);
    assert!(el.display.overlay);
    assert_eq!(el.editor.sample_rate, 48_000.0);
    assert_eq!(el.spectral.fft_size, 1024);
    assert_eq!(el.spectral.freq_scale, FreqScale::Mel);
    assert_eq!(el.editor.ruler, Ruler::Time);
    assert_eq!(el.editor.ruler_y, RulerY::Off);
    // The spectrum presentation analyzed its (inline) samples at parse.
    let spec = el.analysis.as_ref().expect("analysis cached at parse");
    assert_eq!(spec.curves.len(), 2);
    assert_eq!(spec.fft_size, 1024);
    // Live `/gui_set`: back to the signal view drops the analysis; a
    // numeric `min` pins that side and the string "auto" releases it.
    let kind = &mut w.find_mut(1).unwrap().kind;
    assert!(kind.apply("view", &Value::from("signal")));
    assert!(kind.apply("min", &Value::from(-2.0)));
    let el = kind.signal().unwrap();
    assert!(
        el.analysis.is_none(),
        "the signal presentation holds no analysis"
    );
    assert_eq!(el.value.min, Some(-2.0));
    assert!(kind.apply("min", &Value::from("auto")));
    assert!(kind.apply("view", &Value::from("spectrum")));
    let el = kind.signal().unwrap();
    assert_eq!(el.value.min, None);
    assert!(el.analysis.is_some(), "switching back re-analyzes");
    // An unknown view name is rejected (the prop keeps its value).
    assert!(!kind.apply("view", &Value::from("histogram")));
}

#[test]
fn canvas_parses_shader_params_buses_and_applies() {
    let n = node(
        r#"{"type":"window","children":[
            {"id":1,"type":"canvas","shader":"fn shade(){}","params":[0.5,0.25],"buses":[7]}
        ]}"#,
    );
    let mut w = Widget::from_node(9, &n, &[]).unwrap();
    // The canvas is an element: the schema resolves the name, and what the
    // document says about it becomes the slot it claims and the buses it
    // declares. Its props are its own file's suite.
    assert!(matches!(w.children[0].kind, WidgetKind::Custom(_)));
    let needs = w.children[0].kind.needs();
    assert_eq!(
        needs.slot,
        Some(SlotKind::Shader {
            source: "fn shade(){}".into()
        })
    );
    assert_eq!(needs.buses, vec![7], "an unset slot names no bus");
    // A canvas is non-interactive: it reads buses, it reports no value.
    assert_eq!(w.children[0].kind.event_value(), None);
    // Live `/gui_set` reaches the element and moves both with it.
    let c = w.find_mut(1).unwrap();
    assert!(c.kind.apply("bus0", &Value::from(9)));
    assert!(c.kind.apply("shader", &Value::from("fn shade2(){}")));
    assert!(
        !c.kind.apply("param9", &Value::from(1.0)),
        "out-of-range slot"
    );
    let needs = c.kind.needs();
    assert_eq!(needs.buses, vec![9]);
    assert_eq!(
        needs.slot,
        Some(SlotKind::Shader {
            source: "fn shade2(){}".into()
        })
    );
}

#[test]
fn canvas_without_a_shader_gets_the_default() {
    let n = node(r#"{"type":"window","children":[{"id":1,"type":"canvas"}]}"#);
    let w = Widget::from_node(9, &n, &[]).unwrap();
    match w.children[0].kind.needs().slot {
        Some(SlotKind::Shader { source }) => assert!(
            source.contains("fn shade"),
            "falls back to the default shader"
        ),
        other => panic!("expected the shader slot, got {other:?}"),
    }
}

#[test]
fn plot_by_path_defers_empty_with_its_props() {
    let n = node(
        r#"{"type":"window","children":[{"id":3,"type":"signal","navigable":0,"path":"/tmp/sig.f32","channels":2}]}"#,
    );
    let w = Widget::from_node(1, &n, &[]).unwrap();
    let data = w.children[0]
        .signal()
        .and_then(|el| el.source.data())
        .unwrap();
    assert!(data.samples.is_empty(), "mapped later, not inline");
    assert_eq!(
        data.path.as_deref(),
        Some(std::path::Path::new("/tmp/sig.f32"))
    );
    assert_eq!(data.channels, 2);
}

#[test]
fn waveform_from_blob() {
    let blob: Vec<u8> = [1.0f32, -1.0]
        .iter()
        .flat_map(|x| x.to_le_bytes())
        .collect();
    let n =
        node(r#"{"type":"window","children":[{"id":2,"type":"signal","view":"trace","blob":0}]}"#);
    let w = Widget::from_node(1, &n, &[blob]).unwrap();
    let data = w.children[0]
        .signal()
        .and_then(|el| el.source.data())
        .unwrap();
    assert_eq!(&data.samples[..], &[1.0, -1.0]);
}

#[test]
fn phasescope_and_spectrum_parse_with_defaults_and_apply() {
    let n = node(
        r#"{"type":"window","children":[
            {"id":1,"type":"signal","view":"phase","bus":2},
            {"id":2,"type":"signal","view":"spectrum","bus":0,"fft_size":1024,"log_freq":0}
        ]}"#,
    );
    let mut w = Widget::from_node(9, &n, &[]).unwrap();
    let el = w.children[0].signal().unwrap();
    assert_eq!(el.presentation, Presentation::Phase);
    let bus = el.source.bus().unwrap();
    assert_eq!(bus.bus, 2, "the right channel is the next bus");
    assert_eq!(bus.window_ms, 30.0);
    assert!(!bus.hold);
    // A phasescope reads both buses; it is not a single-bus widget.
    let mut buses = Vec::new();
    buses.extend(w.children[0].kind.needs().taps);
    assert_eq!(buses, vec![2, 3]);
    assert_eq!(w.children[0].kind.needs().buses.first().copied(), None);
    let el = w.children[1].signal().unwrap();
    assert_eq!(el.presentation, Presentation::Spectrum);
    assert_eq!(
        (el.source.bus().unwrap().bus, el.spectral.fft_size),
        (0, 1024)
    );
    assert_eq!((el.spectral.db_floor, el.spectral.db_ceil), (-100.0, 0.0));
    assert_eq!(
        el.spectral.freq_scale,
        FreqScale::Linear,
        "legacy log_freq: 0 reads as linear"
    );
    // Live `/gui_set`: retarget a tap, resize the FFT (only a supported size
    // takes), reshape the frequency axis, retune the phasescope window and
    // freeze it.
    assert!(
        w.find_mut(2)
            .unwrap()
            .kind
            .apply("fft_size", &Value::from(2048))
    );
    assert!(
        !w.find_mut(2)
            .unwrap()
            .kind
            .apply("fft_size", &Value::from(1000))
    );
    assert!(
        w.find_mut(2)
            .unwrap()
            .kind
            .apply("freq_scale", &Value::from("mel"))
    );
    assert!(
        !w.find_mut(2)
            .unwrap()
            .kind
            .apply("freq_scale", &Value::from("nope"))
    );
    assert!(w.find_mut(1).unwrap().kind.apply("hold", &Value::from(1)));
    let el = w.find_mut(2).unwrap().signal().unwrap();
    assert_eq!(el.spectral.fft_size, 2048);
    assert_eq!(el.spectral.freq_scale, FreqScale::Mel);
}

#[test]
fn multichannel_scope_and_spectrum_read_adjacent_buses() {
    let n = node(
        r#"{"type":"window","children":[
            {"id":1,"type":"signal","view":"trace","bus":4,"channels":2,"overlay":1,"ruler":"off"},
            {"id":2,"type":"signal","view":"spectrum","bus":6,"channels":3,"ruler_y":"off"}
        ]}"#,
    );
    let mut w = Widget::from_node(9, &n, &[]).unwrap();
    let el = w.children[0].signal().unwrap();
    assert_eq!(el.channels(), 2);
    assert!(el.display.overlay);
    assert_eq!(el.editor.ruler, Ruler::Off, "\"off\" hides the x strip");
    assert_eq!(
        el.editor.ruler_y,
        RulerY::Norm,
        "the y strip defaults on, in the presentation's own unit"
    );
    // Each consumer reads its whole adjacent run of buses.
    let mut buses = Vec::new();
    buses.extend(w.children[0].kind.needs().taps);
    assert_eq!(buses, vec![4, 5]);
    buses.clear();
    buses.extend(w.children[1].kind.needs().taps);
    assert_eq!(buses, vec![6, 7, 8]);
    // Live: grow the runs and toggle a strip back on.
    assert!(
        w.find_mut(1)
            .unwrap()
            .kind
            .apply("channels", &Value::from(4))
    );
    assert!(w.find_mut(1).unwrap().kind.apply("ruler", &Value::from(1)));
    buses.clear();
    buses.extend(w.find_mut(1).unwrap().kind.needs().taps);
    assert_eq!(buses, vec![4, 5, 6, 7]);
    let el = w.children[1].signal().unwrap();
    assert_eq!(el.channels(), 3);
    assert_eq!(el.editor.ruler_y, RulerY::Off);
}

#[test]
fn waveform_editor_props_parse_and_apply() {
    let n = node(
        r#"{"type":"window","children":[
            {"id":1,"type":"signal","view":"trace","data":[0.0,1.0],"channels":2,"overlay":1,
             "ruler":"samples","sample_rate":48000.0,"sel_start":100.0,"sel_len":50.0,
             "playhead_at":1000.0}
        ]}"#,
    );
    let mut w = Widget::from_node(9, &n, &[]).unwrap();
    let el = w.children[0].signal().unwrap();
    assert_eq!(el.channels(), 2);
    assert!(el.display.overlay);
    assert_eq!(el.editor.ruler, Ruler::Samples);
    assert_eq!(el.editor.sample_rate, 48_000.0);
    assert_eq!((el.editor.sel_start, el.editor.sel_len), (100.0, 50.0));
    assert_eq!(el.editor.playhead_at, 1000.0);
    assert!(w.children[0].is_timeline());
    // The vertical ruler defaults to the normalized amplitude axis.
    assert_eq!(w.children[0].kind.editor().unwrap().ruler_y, RulerY::Norm);
    assert_eq!(w.children[0].kind.editor().unwrap().bit_depth, 16);
    // Live `/gui_set`: retune the selection, clear the playhead, switch
    // the ruler off.
    let wf = w.find_mut(1).unwrap();
    assert!(wf.kind.apply("sel_start", &Value::from(0.0)));
    assert!(wf.kind.apply("sel_len", &Value::from(0.0)));
    assert!(wf.kind.apply("playhead_at", &Value::from(-1.0)));
    assert!(wf.kind.apply("ruler", &Value::from("off")));
    assert!(!wf.kind.apply("ruler", &Value::from("nonesuch")));
    let editor = wf.kind.editor().unwrap();
    assert_eq!(editor.sel_len, 0.0, "zero length clears it");
    assert!(editor.playhead_at < 0.0);
    assert_eq!(editor.ruler, Ruler::Off);
}

#[test]
fn editor_ruler_units_parse_and_apply() {
    let n = node(
        r#"{"type":"window","children":[
            {"id":1,"type":"signal","view":"trace","data":[0.0],"ruler":"beats",
             "sample_rate":48000.0,"tempo":2.0,"beat_at":8.0,"quant":3.0,
             "ruler_y":"db","bit_depth":24}
        ]}"#,
    );
    let mut w = Widget::from_node(9, &n, &[]).unwrap();
    let editor = w.children[0].kind.editor().unwrap();
    assert_eq!(editor.ruler, Ruler::Beats);
    assert_eq!(
        (editor.tempo, editor.beat_at, editor.quant),
        (2.0, 8.0, 3.0)
    );
    assert_eq!(editor.ruler_y, RulerY::Db);
    assert_eq!(editor.bit_depth, 24);
    // Every unit is live via `/gui_set` (the button-wiring path).
    let wf = w.find_mut(1).unwrap();
    assert!(wf.kind.apply("ruler_y", &Value::from("bits")));
    assert!(wf.kind.apply("bit_depth", &Value::from(8)));
    assert!(wf.kind.apply("tempo", &Value::from(1.5)));
    assert!(wf.kind.apply("quant", &Value::from(4.0)));
    assert!(wf.kind.apply("beat_at", &Value::from(0.0)));
    assert!(!wf.kind.apply("ruler_y", &Value::from("nonesuch")));
    let editor = wf.kind.editor().unwrap();
    assert_eq!(editor.ruler_y, RulerY::Bits);
    assert_eq!(editor.bit_depth, 8);
    assert_eq!((editor.tempo, editor.quant), (1.5, 4.0));
}

#[test]
fn editor_y_view_parses_clamps_and_applies() {
    let n = node(
        r#"{"type":"window","children":[
            {"id":1,"type":"signal","view":"trace","data":[0.0],"y_start":0.8,"y_len":0.5},
            {"id":2,"type":"signal","view":"spectrogram","data":[0.0]}
        ]}"#,
    );
    let mut w = Widget::from_node(9, &n, &[]).unwrap();
    // The read-time window clamps inside the axis: 0.8 + 0.5 spills, so
    // the start pulls back to 0.5.
    let editor = w.children[0].kind.editor().unwrap();
    assert_eq!(editor.y_view(), (0.5, 0.5));
    // The default is the full axis.
    let editor = w.children[1].kind.editor().unwrap();
    assert_eq!(editor.y_view(), (0.0, 1.0));
    // Live `/gui_set` zooms and pans; a non-positive length resets.
    let wf = w.find_mut(1).unwrap();
    assert!(wf.kind.apply("y_len", &Value::from(0.25)));
    assert!(wf.kind.apply("y_start", &Value::from(0.7)));
    let editor = wf.kind.editor().unwrap();
    assert_eq!(editor.y_view(), (0.7, 0.25));
    // One set carrying both keys must not depend on key order: applying
    // y_start before y_len used to clamp it against the old full-axis
    // length and silently zero it (the "zoom lands in the wrong half"
    // regression).
    assert!(wf.kind.apply("y_start", &Value::from(0.5)));
    assert!(wf.kind.apply("y_len", &Value::from(0.5)));
    let editor = wf.kind.editor().unwrap();
    assert_eq!(editor.y_view(), (0.5, 0.5));
    assert!(wf.kind.apply("y_len", &Value::from(0.0)));
    let editor = wf.kind.editor().unwrap();
    assert_eq!(editor.y_view(), (0.0, 1.0));
}

/// A navigable spectrum's frequency window is the x sibling of `y_view`: the
/// same normalized reading, the same clamp, the same order-independence — and
/// it arrives under the x axis' own `view_start`/`view_len`, which on a
/// timeline member the group model takes instead.
#[test]
fn editor_x_view_parses_clamps_and_applies() {
    let n = node(
        r#"{"type":"window","children":[
            {"id":1,"type":"signal","view":"spectrum","bus":0,"navigable":1,
             "axes":{"x":{"start":0.8,"len":0.5}}},
            {"id":2,"type":"signal","view":"spectrum","bus":0,"navigable":1}
        ]}"#,
    );
    let mut n = n;
    super::axes::flatten_tree(&mut n); // the pass a def makes on the way in
    let mut w = Widget::from_node(9, &n, &[]).unwrap();
    // The nested axis spelling reaches it, and the read-time window clamps
    // inside the axis: 0.8 + 0.5 spills, so the start pulls back.
    assert_eq!(w.children[0].kind.editor().unwrap().x_view(), (0.5, 0.5));
    // The default is the whole frequency axis.
    assert_eq!(w.children[1].kind.editor().unwrap().x_view(), (0.0, 1.0));
    let sp = w.find_mut(2).unwrap();
    assert!(sp.kind.apply("view_start", &Value::from(0.25)));
    assert!(sp.kind.apply("view_len", &Value::from(0.5)));
    assert_eq!(sp.kind.editor().unwrap().x_view(), (0.25, 0.5));
    // Either key order lands on the same window, and a non-positive length
    // resets to the whole axis.
    assert!(sp.kind.apply("view_start", &Value::from(0.5)));
    assert!(sp.kind.apply("view_len", &Value::from(0.5)));
    assert_eq!(sp.kind.editor().unwrap().x_view(), (0.5, 0.5));
    assert!(sp.kind.apply("view_len", &Value::from(0.0)));
    assert_eq!(sp.kind.editor().unwrap().x_view(), (0.0, 1.0));
}

#[test]
fn spectrogram_parses_with_defaults_and_applies() {
    let n = node(
        r#"{"type":"window","children":[
            {"id":1,"type":"signal","view":"spectrogram","path":"/tmp/a.f32","channels":2,
             "sample_rate":44100.0},
            {"id":2,"type":"signal","view":"spectrogram","buffer":3,"window_size":333}
        ]}"#,
    );
    let mut w = Widget::from_node(9, &n, &[]).unwrap();
    let el = w.children[0].signal().unwrap();
    assert_eq!(el.presentation, Presentation::TimeFrequency);
    let data = el.source.data().unwrap();
    assert_eq!(
        data.path.as_deref(),
        Some(std::path::Path::new("/tmp/a.f32"))
    );
    assert_eq!(
        (data.channels, el.spectral.fft_size, el.spectral.hop),
        (2, 1024, 512)
    );
    assert_eq!(el.editor.sample_rate, 44_100.0);
    assert_eq!((el.spectral.db_floor, el.spectral.db_ceil), (-90.0, 0.0));
    assert_eq!(
        el.spectral.freq_scale,
        FreqScale::Log,
        "log is the default scale"
    );
    assert_eq!(el.spectral.colormap, 0);
    assert_eq!(el.editor.ruler_y, RulerY::Hz, "the Hz ruler defaults on");
    // An unsupported window size degrades to the default.
    let el = w.children[1].signal().unwrap();
    assert_eq!(el.source.data().unwrap().buffer, Some(3));
    assert_eq!(
        el.spectral.fft_size, 1024,
        "333 is not a supported FFT size"
    );
    // Live `/gui_set`: the display uniforms retune with zero recompute.
    let sg = w.find_mut(1).unwrap();
    assert!(sg.kind.apply("db_floor", &Value::from(-60.0)));
    assert!(sg.kind.apply("log_freq", &Value::from(0)), "legacy alias");
    assert!(sg.kind.apply("colormap", &Value::from(1)));
    assert!(sg.kind.apply("sel_start", &Value::from(10.0)));
    let el = sg.signal().unwrap();
    assert_eq!(el.spectral.db_floor, -60.0);
    assert_eq!(
        el.spectral.freq_scale,
        FreqScale::Linear,
        "log_freq 0 -> linear"
    );
    assert_eq!(el.spectral.colormap, 1);
    assert_eq!(el.editor.sel_start, 10.0);
    // The four-scale prop wins over the legacy alias and applies live.
    assert!(sg.kind.apply("freq_scale", &Value::from("mel")));
    assert!(!sg.kind.apply("freq_scale", &Value::from("nonesuch")));
    assert!(sg.kind.apply("ruler_y", &Value::from("off")));
    let el = sg.signal().unwrap();
    assert_eq!(el.spectral.freq_scale, FreqScale::Mel);
    assert_eq!(el.editor.ruler_y, RulerY::Off);
}

/// The six wire names land on six configurations of one element. This is
/// the parse-level half of the preset table's own test: what the wire says
/// and what the model holds, in one place, so a name cannot quietly change
/// what it configures.
#[test]
fn the_six_names_parse_to_their_point_of_the_product() {
    let n = node(
        r#"{"type":"window","children":[
            {"id":1,"type":"signal","view":"trace","data":[0.0,1.0]},
            {"id":2,"type":"signal","view":"spectrogram","data":[0.0,1.0]},
            {"id":3,"type":"signal","navigable":0,"data":[0.0,1.0]},
            {"id":4,"type":"signal","view":"trace","bus":0},
            {"id":5,"type":"signal","view":"spectrum","bus":0},
            {"id":6,"type":"signal","view":"phase","bus":0},
            {"id":7,"type":"signal","view":"spectrum","bus":0,"navigable":1}
        ]}"#,
    );
    let w = Widget::from_node(9, &n, &[]).unwrap();
    let point = |i: usize| {
        let el = w.children[i].signal().expect("a signal element");
        (el.presentation, el.is_live(), el.caps.navigable)
    };
    assert_eq!(point(0), (Presentation::Signal, false, true));
    assert_eq!(point(1), (Presentation::TimeFrequency, false, true));
    assert_eq!(point(2), (Presentation::Signal, false, false));
    assert_eq!(point(3), (Presentation::Signal, true, false));
    assert_eq!(point(4), (Presentation::Spectrum, true, false));
    assert_eq!(point(5), (Presentation::Phase, true, false));
    // The seventh point the six names never had: a spectrum that navigates.
    // It is opt-in — a bare `spectrum` is the spectroscope above — and it
    // joins **no** time axis, because the axis it navigates is frequency.
    assert_eq!(point(6), (Presentation::Spectrum, true, true));
    assert!(w.children[6].kind.navigates_freq());
    assert!(!w.children[6].is_nav_signal(), "frequency is not time");
    // Only the views navigating *time* join the window's time axis.
    let timelines: Vec<bool> = (0..7).map(|i| w.children[i].is_timeline()).collect();
    assert_eq!(timelines, [true, true, false, false, false, false, false]);
}

/// A `/gui_set` key lands on the part of the model it names, and is refused
/// where that part does not exist — the source keys on a stored element,
/// the analysis size under either of its two wire names.
#[test]
fn a_set_lands_on_the_part_of_the_model_it_names() {
    let n = node(
        r#"{"type":"window","children":[
            {"id":1,"type":"signal","view":"trace","data":[0.0,1.0]},
            {"id":2,"type":"signal","view":"trace","bus":3}
        ]}"#,
    );
    let mut w = Widget::from_node(9, &n, &[]).unwrap();
    // A source key means nothing to an element that reads samples.
    assert!(!w.find_mut(1).unwrap().kind.apply("bus", &Value::from(2)));
    assert!(!w.find_mut(1).unwrap().kind.apply("hold", &Value::from(1)));
    // It means everything to one that reads a bus.
    assert!(w.find_mut(2).unwrap().kind.apply("bus", &Value::from(9)));
    assert_eq!(w.children[1].signal().unwrap().source.bus().unwrap().bus, 9);
    // One analysis size, under both of the names the wire has for it.
    assert!(
        w.find_mut(1)
            .unwrap()
            .kind
            .apply("window_size", &Value::from(512))
    );
    assert_eq!(w.children[0].signal().unwrap().spectral.fft_size, 512);
    assert!(
        w.find_mut(1)
            .unwrap()
            .kind
            .apply("fft_size", &Value::from(256))
    );
    assert_eq!(w.children[0].signal().unwrap().spectral.fft_size, 256);
}

#[test]
fn spectrogram_freq_scale_prop_parses_all_four() {
    let n = node(
        r#"{"type":"window","children":[
            {"id":1,"type":"signal","view":"spectrogram","data":[0.0],"freq_scale":"bark"},
            {"id":2,"type":"signal","view":"spectrogram","data":[0.0],"freq_scale":"linear","log_freq":1}
        ]}"#,
    );
    let w = Widget::from_node(9, &n, &[]).unwrap();
    assert_eq!(
        w.children[0].signal().unwrap().spectral.freq_scale,
        FreqScale::Bark
    );
    // freq_scale wins over the legacy log_freq when both are present.
    assert_eq!(
        w.children[1].signal().unwrap().spectral.freq_scale,
        FreqScale::Linear
    );
}

/// The `curve` wire name resolves to the **element**, which is what a clip
/// then recognizes as its automation body. What the element does with the
/// props is its own suite (`elements::curve`); what the schema owes is that the
/// name builds one, that it is no timeline view and no scalar control, and that
/// a `/gui_set` reaches it.
#[test]
fn curve_builds_an_element_filling_the_curve_body_role() {
    let n = node(
        r#"{"type":"window","children":[
            {"id":1,"type":"curve","points":[0.0,0.0,1,0.0, 0.1,1.0,-4.0,0.0, 1.0,0.0,1,0.0],
             "label":"env"},
            {"id":2,"type":"curve","min":20.0,"max":20000.0,"exp":1,"duration":4.0}
        ]}"#,
    );
    let mut w = Widget::from_node(9, &n, &[]).unwrap();
    for child in &w.children {
        assert!(
            matches!(child.kind, WidgetKind::Custom(_)),
            "{:?}",
            child.kind
        );
        assert_eq!(
            child.kind.body_role(),
            Some(element::BodyRole::Curve),
            "so a clip recognizes it as its automation body"
        );
        // Neither a timeline view nor a scalar-value control: its edit-back
        // carries the flat breakpoint list instead.
        assert!(!child.is_timeline());
        assert_eq!(child.kind.event_value(), None);
    }
    // Live `/gui_set`: the whole breakpoint list (array or its JSON-string
    // carrier) and the domain land on the element; a malformed value is
    // refused rather than swallowed, and so is a key it does not own.
    let b = w.find_mut(1).unwrap();
    assert!(
        b.kind
            .apply("points", &Value::from("[0.0,0.5,1,0.0, 2.0,0.25,3,0.0]"))
    );
    assert!(b.kind.apply("duration", &Value::from(3.0)));
    assert!(!b.kind.apply("points", &Value::from("nonesuch")));
    assert!(!b.kind.apply("sideways", &Value::from(1)));
}

#[test]
fn place_props_parse_and_apply() {
    let n = node(
        r#"{"type":"window","flow":"row","children":[
        {"id":7,"type":"label","text":"a","w":100.5,"weight":2,"x":4,"y":8}]}"#,
    );
    let w = Widget::from_node(1, &n, &[]).unwrap();
    let child = &w.children[0];
    assert_eq!(child.place.w, Some(100.5));
    assert_eq!(child.place.h, None);
    assert_eq!(child.place.weight, Some(2.0));
    assert_eq!((child.place.x, child.place.y), (Some(4.0), Some(8.0)));
    // Live /gui_set: numbers set, a non-number releases the prop.
    let mut place = child.place;
    assert!(place.apply("h", &serde_json::json!(20)));
    assert_eq!(place.h, Some(20.0));
    assert!(place.apply("w", &serde_json::json!("auto")));
    assert_eq!(place.w, None, "a non-number releases a place prop");
    assert!(
        !place.apply("min", &serde_json::json!(1)),
        "not a place key"
    );
}

#[test]
fn flow_props_parse_and_apply() {
    let n = node(r#"{"type":"window","flow":"grid","margin":0,"gap":2,"cols":3}"#);
    let w = Widget::from_node(1, &n, &[]).unwrap();
    let WidgetKind::Window { mut flow, .. } = w.kind else {
        unreachable!()
    };
    assert_eq!(
        (flow.margin, flow.gap, flow.cols),
        (Some(0.0), Some(2.0), Some(3))
    );
    assert!(flow.apply("cols", &serde_json::json!(0)));
    assert_eq!(flow.cols, Some(1), "cols clamps to at least 1");
    // The container arm of the kind apply routes layout and flow keys.
    let mut kind = WidgetKind::Panel {
        layout: Layout::Col,
        flow: Flow::default(),
    };
    assert!(kind.apply("flow", &serde_json::json!("row")));
    assert!(kind.apply("gap", &serde_json::json!(10)));
    assert!(matches!(
        kind,
        WidgetKind::Panel {
            layout: Layout::Row,
            flow: Flow { gap: Some(g), .. }
        } if g == 10.0
    ));
}

#[test]
fn defaults_and_unknown_type() {
    // An unrecognized type is laid out but kept as `Unknown`, never
    // rejected (the protocol's forward-compatibility rule).
    let n = node(r#"{"type":"window","children":[{"id":7,"type":"no_such_widget"}]}"#);
    let w = Widget::from_node(1, &n, &[]).unwrap();
    // Window size defaults when w/h are omitted.
    match w.kind {
        WidgetKind::Window {
            width,
            height,
            layout,
            ..
        } => {
            assert_eq!((width, height), DEFAULT_WINDOW);
            assert_eq!(layout, Layout::Col);
        }
        _ => unreachable!(),
    }
    match &w.children[0].kind {
        WidgetKind::Unknown(t) => assert_eq!(t, "no_such_widget"),
        other => panic!("expected unknown, got {other:?}"),
    }
}

#[test]
fn bad_blob_index_is_an_error() {
    let n =
        node(r#"{"type":"window","children":[{"id":2,"type":"signal","view":"trace","blob":3}]}"#);
    assert!(Widget::from_node(1, &n, &[]).is_err());
}

#[test]
fn parses_controls_and_clamps_value() {
    let n = node(
        r#"{"type":"window","children":[
            {"id":1,"type":"slider","min":20.0,"max":2000.0,"value":5000.0,"label":"cut"},
            {"id":2,"type":"toggle","value":1},
            {"id":3,"type":"menu","options":["a","b","c"],"index":1}
        ]}"#,
    );
    let w = Widget::from_node(9, &n, &[]).unwrap();
    // The slider is an element; that its value clamped into the range is its
    // own file's suite, and what this pass answers for is that the document
    // resolved to one that reports a value.
    assert!(matches!(w.children[0].kind, WidgetKind::Custom(_)));
    assert_eq!(
        w.children[0].kind.event_value(),
        Some(OscType::Float(2000.0)),
        "value clamps into the range"
    );
    assert_eq!(
        w.children[1].kind.event_value(),
        Some(OscType::Int(1)),
        "the toggle parsed its state"
    );
    assert_eq!(
        w.children[2].kind.event_value(),
        Some(OscType::Int(1)),
        "the menu parsed its index"
    );
}

#[test]
fn apply_updates_value_and_event_value_reports_it() {
    let n = node(r#"{"type":"window","children":[{"id":5,"type":"knob","min":0.0,"max":10.0}]}"#);
    let mut w = Widget::from_node(9, &n, &[]).unwrap();
    let knob = w.find_mut(5).unwrap();
    assert!(knob.kind.apply("value", &Value::from(4.0)));
    assert_eq!(knob.kind.event_value(), Some(OscType::Float(4.0)));
    // An unknown key is a no-op.
    assert!(!knob.kind.apply("nonesuch", &Value::from(1.0)));
}

#[test]
fn a_free_standing_ruler_changes_its_unit_live() {
    let mut w = Widget::from_node(
        1,
        &node(r#"{"id":9,"type":"field","h":22,"ruler":"beats"}"#),
        &[],
    )
    .unwrap();
    let unit = |w: &Widget| match &w.kind {
        WidgetKind::TimeRuler { editor } => editor.ruler,
        other => panic!("not a ruler: {other:?}"),
    };
    assert_eq!(unit(&w), Ruler::Beats);
    assert!(apply_widget(&mut w, "ruler", &serde_json::json!("samples")));
    assert_eq!(unit(&w), Ruler::Samples);
    assert!(apply_widget(&mut w, "ruler", &serde_json::json!("time")));
    assert_eq!(unit(&w), Ruler::Time);
}
