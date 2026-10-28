//! **The renders an audio editor's edits are**, rendered: each score
//! `clausters_apps::audio::renders` answers, run through the offline render
//! a `--nrt` process runs, and the file and the peaks it writes read back.
//! Its renders run SynthDefs, so it needs the `synth` family.

#![cfg(feature = "synth")]

use clausters::server::render::{OscScore, RenderConfig, render_discarding, render_to_wav};
use clausters_apps::audio::renders::{self, GainPoint, List, Output};
use clausters_document::clipboard::SegmentPart;
use clausters_editing::apply::{Render, Step};

const RATE: f64 = 48_000.0;

/// A directory of this test's own, emptied first.
fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("renders-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A float file of `channels` channels, frame `f` channel `c` worth
/// `value(f, c)`; answers its path.
fn file(
    dir: &std::path::Path,
    name: &str,
    frames: usize,
    channels: usize,
    value: impl Fn(usize, usize) -> f32,
) -> String {
    let path = dir.join(name).to_string_lossy().into_owned();
    let mut w = clausters::dsp::pcm::Writer::create(&path, channels, RATE, "float").unwrap();
    let samples: Vec<f32> = (0..frames)
        .flat_map(|f| (0..channels).map(move |c| (f, c)))
        .map(|(f, c)| value(f, c))
        .collect();
    w.write(&samples).unwrap();
    w.finalize().unwrap();
    path
}

/// A part over the whole of a file.
fn whole(path: &str, frames: u64, channels: u32) -> SegmentPart {
    SegmentPart {
        path: path.into(),
        start: 0,
        frames,
        fade_in: 0,
        fade_out: 0,
        fade_in_shape: Default::default(),
        fade_out_shape: Default::default(),
        channels: None,
        source_channels: channels,
        source_frames: frames,
        source_rate: RATE,
    }
}

fn output(dir: &std::path::Path, name: &str) -> Output {
    Output {
        path: dir
            .join(format!("{name}.wav"))
            .to_string_lossy()
            .into_owned(),
        peaks: dir
            .join(format!("{name}.peaks"))
            .to_string_lossy()
            .into_owned(),
        format: "float".into(),
        label: String::new(),
    }
}

/// Renders a step as an endpoint does, and reads back what it wrote.
fn run(step: Step) -> Vec<f32> {
    let Step::Render(Render {
        score,
        path,
        rate,
        channels,
        format,
        ..
    }) = step
    else {
        panic!("not a render");
    };
    let score = OscScore::from_bytes(&score).unwrap();
    let cfg = RenderConfig {
        sample_rate: rate,
        channels,
        ..RenderConfig::default()
    };
    if path.is_empty() {
        render_discarding(&score, &cfg, |_, _| {}).unwrap();
        return Vec::new();
    }
    render_to_wav(&score, &cfg, &path, &format).unwrap();
    let layout = clausters::dsp::pcm::Layout::open(&path).unwrap();
    let mut file = std::fs::File::open(&path).unwrap();
    let (mut bytes, mut out) = (Vec::new(), Vec::new());
    clausters_core::pcm::read_frames(
        &mut file,
        &layout,
        0,
        layout.frames as usize,
        &mut bytes,
        &mut out,
    )
    .unwrap();
    out
}

fn peaks_written(path: &str) -> bool {
    std::fs::read(path).is_ok_and(|b| b.starts_with(b"CLPK"))
}

#[test]
fn a_file_opened_without_peaks_is_measured_and_nothing_else_is_written() {
    let dir = scratch("measure");
    let take = file(&dir, "take.wav", 30_000, 2, |f, c| {
        (f as f32 / 30_000.0) * if c == 0 { 1.0 } else { -1.0 }
    });
    let peaks = dir.join("take.peaks").to_string_lossy().into_owned();
    assert!(run(renders::measure(&whole(&take, 30_000, 2), &peaks, "")).is_empty());
    let pyramid = clausters_core::peaks::MultiPyramid::read_cache(&peaks)
        .unwrap()
        .expect("a peaks file");
    assert_eq!((pyramid.frames(), pyramid.num_channels()), (30_000, 2));
}

#[test]
fn a_gain_curve_scales_the_list_as_its_points_say() {
    let dir = scratch("gain");
    let take = file(&dir, "one.wav", 48_000, 1, |_, _| 1.0);
    let list = List {
        parts: vec![whole(&take, 48_000, 1)],
        channels: 1,
        rate: RATE,
    };
    let curve = [
        GainPoint {
            at: 0.25,
            value: 1.0,
            shape: 1.0,
            curve: 0.0,
        },
        GainPoint {
            at: 0.75,
            value: 0.0,
            shape: 1.0,
            curve: 0.0,
        },
    ];
    let out = output(&dir, "gained");
    let samples = run(renders::gain(&list, &curve, &out));
    assert_eq!(samples.len(), 48_000);
    assert!(
        (samples[100] - 1.0).abs() < 1e-6,
        "held before the first point"
    );
    assert!(
        (samples[24_000] - 0.5).abs() < 1e-3,
        "half way down the ramp: {}",
        samples[24_000]
    );
    assert!(samples[47_000].abs() < 1e-6, "held after the last");
    assert!(peaks_written(&out.peaks));
}

#[test]
fn a_span_is_reversed_sample_for_sample() {
    let dir = scratch("reverse");
    let take = file(&dir, "ramp.wav", 10_000, 2, |f, c| {
        f as f32 + c as f32 * 0.5
    });
    let mut part = whole(&take, 10_000, 2);
    part.start = 1_000;
    part.frames = 4_000;
    let list = List {
        parts: vec![part],
        channels: 2,
        rate: RATE,
    };
    let out = output(&dir, "reversed");
    let samples = run(renders::reverse(&list, &out));
    assert_eq!(samples.len(), 8_000);
    for f in 0..4_000 {
        assert_eq!(samples[2 * f], (4_999 - f) as f32, "frame {f}");
        assert_eq!(samples[2 * f + 1], (4_999 - f) as f32 + 0.5);
    }
    assert!(peaks_written(&out.peaks));
}

#[test]
fn a_list_and_a_block_are_mixed_into_a_span() {
    let dir = scratch("mix");
    let a = file(&dir, "a.wav", 6_000, 1, |_, _| 0.25);
    let b = file(&dir, "b.wav", 6_000, 1, |_, _| 0.5);
    let onto = List {
        parts: vec![whole(&a, 6_000, 1)],
        channels: 1,
        rate: RATE,
    };
    let added = List {
        parts: vec![whole(&b, 6_000, 1)],
        channels: 1,
        rate: RATE,
    };
    let out = output(&dir, "mixed");
    let samples = run(renders::mix_lists(&onto, &added, &out));
    assert!(samples.len() == 6_000 && samples.iter().all(|v| *v == 0.75));
    let block: Vec<f32> = (0..6_000).map(|f| f as f32 * 1e-4).collect();
    let out = output(&dir, "block_mixed");
    let samples = run(renders::mix_block(&onto, &block, &out));
    assert!(
        samples
            .iter()
            .enumerate()
            .all(|(f, v)| (v - (0.25 + f as f32 * 1e-4)).abs() < 1e-6)
    );
    assert!(peaks_written(&out.peaks));
}

#[test]
fn a_block_a_stroke_and_silence_are_files_of_their_own() {
    let dir = scratch("files");
    let values: Vec<f32> = (0..2_000).map(|i| (i % 7) as f32 * 0.1).collect();
    let out = output(&dir, "block");
    assert_eq!(run(renders::block_file(&values, 2, RATE, &out)), values);

    let take = file(&dir, "stereo.wav", 3_000, 2, |f, c| {
        if c == 0 { f as f32 } else { -1.0 }
    });
    let mut over = whole(&take, 3_000, 2);
    over.start = 100;
    over.frames = 500;
    let list = List {
        parts: vec![over],
        channels: 2,
        rate: RATE,
    };
    let drawn: Vec<f32> = (0..500).map(|i| i as f32 * 0.001).collect();
    let out = output(&dir, "stroke");
    let samples = run(renders::stroke(&list, 1, &drawn, &out));
    assert_eq!(samples.len(), 1_000);
    for f in 0..500 {
        assert_eq!(
            samples[2 * f],
            (100 + f) as f32,
            "the other channel is the list's"
        );
        assert_eq!(samples[2 * f + 1], drawn[f], "the drawn one is the stroke");
    }

    let out = output(&dir, "silence");
    let samples = run(renders::silence(1_234, 2, RATE, &out));
    assert!(samples.len() == 2_468 && samples.iter().all(|v| *v == 0.0));
    assert!(peaks_written(&out.peaks));
}

/// **An editor's gain apply, end to end**: a take opened from a file, a span
/// cut out of it and the curve applied render a file of the list through
/// the curve, the source file is never written, and an undo and a redo name
/// the files again without rendering anything.
#[test]
fn an_editor_applies_a_curve_by_rendering_the_list_and_undo_names_the_take_again() {
    use clausters_apps::audio::editor::{call_json, new_json};
    use clausters_editing::apply::steps_from_json;
    use serde_json::{Value, json};
    const FRAMES: usize = 1_000;
    let dir = scratch("editor-gain");
    let take = file(&dir, "take.wav", FRAMES, 2, |f, c| {
        (f as f32 + 1.0) * if c == 0 { 0.001 } else { -0.001 }
    });
    let original = std::fs::read(&take).unwrap();
    let work = dir.join("work").to_string_lossy().into_owned();
    std::fs::create_dir_all(&work).unwrap();
    let mut editor = new_json(
        &json!({"path": take, "workspace": work, "frames": FRAMES, "channels": 2,
                "rate": RATE})
        .to_string(),
    )
    .unwrap();
    call_json(
        &mut editor,
        &json!({"verb": "sync", "rate": RATE}).to_string(),
    );
    // carries out the renders a batch holds
    let walk = |steps: &Value| -> usize {
        steps_from_json(steps)
            .into_iter()
            .filter(|step| matches!(step, Step::Render(_)))
            .map(|step| {
                run(step);
            })
            .count()
    };
    walk(&editor.open().unwrap());
    // the list as data: the window is handed it by reference
    let parts = |editor: &mut clausters_apps::audio::editor::AudioEditor| -> Vec<Value> {
        let list: Value =
            serde_json::from_str(&call_json(editor, r#"{"verb":"segments"}"#)).unwrap();
        list["parts"].as_array().unwrap().clone()
    };
    // the window over the take, which the gestures are on
    call_json(&mut editor, r#"{"verb":"window","widget":10}"#);
    let cut = editor.event(
        &clausters_apps::turn::Event {
            addr: "/gui_event".into(),
            args: vec![
                json!(10),
                json!(1),
                json!(0),
                json!("cut"),
                json!(300),
                json!(200),
            ],
        },
        1,
    );
    assert_eq!(walk(&cut.steps.unwrap()), 0, "a cut renders nothing");
    let points = json!([0.0, 0.0, 1.0, 0.0, 800.0 / RATE, 1.0, 1.0, 0.0]);
    assert!(
        editor
            .act(&json!({"verb": "setautomation", "points": points}), 2)
            .changed
    );
    let applied = editor.act(&json!({"verb": "apply_envelope"}), 3);
    assert_eq!(walk(applied.steps.as_ref().unwrap()), 1, "one render");
    let now = parts(&mut editor);
    assert_eq!(now.len(), 1, "the list is the render");
    let rendered = now[0]["path"].as_str().unwrap().to_string();
    assert!(rendered.starts_with(&work));
    let layout = clausters::dsp::pcm::Layout::open(&rendered).unwrap();
    assert_eq!(layout.frames, 800);
    let mut f = std::fs::File::open(&rendered).unwrap();
    let (mut bytes, mut samples) = (Vec::new(), Vec::new());
    clausters_core::pcm::read_frames(&mut f, &layout, 0, 800, &mut bytes, &mut samples).unwrap();
    for frame in 0..800 {
        // the list's frame, past the cut, under a ramp from 0 to 1
        let source = if frame < 300 { frame } else { frame + 200 };
        let expected = (source as f32 + 1.0) * 0.001 * frame as f32 / 800.0;
        assert!(
            (samples[2 * frame] - expected).abs() < 1e-4,
            "frame {frame}: {} against {expected}",
            samples[2 * frame]
        );
    }
    assert_eq!(
        std::fs::read(&take).unwrap(),
        original,
        "the source is never written"
    );

    let leg = &applied.record.unwrap().legs[0];
    assert!(
        editor.apply(&leg.backward),
        "an undo, which renders nothing"
    );
    let undone = parts(&mut editor);
    assert_eq!(undone.len(), 2);
    assert!(undone.iter().all(|p| p["path"] == take.as_str()));
    assert!(
        editor.apply(&leg.forward["edit"]),
        "a redo names the render again"
    );
    assert_eq!(parts(&mut editor)[0]["path"], rendered.as_str());
}
