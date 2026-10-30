//! **An edit's render reads its takes through the application**: the lists
//! a render names are fed by the application's own reader of takes, and the
//! engine reads no file -- checked sample for sample over a list of two
//! files, forwards through a gain and backwards for a reverse, with the
//! peaks of what was written measured beside it.

use clausters_apps::audio::renders::{self, GainPoint, List, Output};
use clausters_document::clipboard::SegmentPart;

const RATE: f64 = 48_000.0;

/// A mono float file of `samples`, at `RATE`.
fn file(dir: &std::path::Path, name: &str, samples: &[f32]) -> String {
    let path = dir.join(name).to_string_lossy().into_owned();
    let mut w = clausters_core::pcm::Writer::create(&path, 1, RATE, "float").unwrap();
    w.write(samples).unwrap();
    w.finalize().unwrap();
    path
}

/// A part reading `frames` of `path` from `start`, with no fades.
fn part(path: &str, start: u64, frames: u64, source_frames: u64) -> SegmentPart {
    serde_json::from_value(serde_json::json!({
        "path": path, "start": start, "frames": frames,
        "fade_in": 0, "fade_out": 0,
        "source_channels": 1, "source_frames": source_frames, "source_rate": RATE,
    }))
    .unwrap()
}

/// The samples of the mono file at `path`.
fn read(path: &str) -> Vec<f32> {
    let layout = clausters_core::pcm::Layout::open(path).unwrap();
    let mut reader = clausters_core::pcm::Reader::open(&layout).unwrap();
    let mut out = Vec::new();
    reader.read(0, layout.frames as usize, &mut out).unwrap();
    out
}

#[test]
fn a_render_reads_its_list_through_the_application_forwards_and_backwards() {
    let dir = std::env::temp_dir().join(format!("clausters-renders-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let a: Vec<f32> = (0..3000).map(|i| i as f32 / 10_000.0).collect();
    let b: Vec<f32> = (0..2000).map(|i| -(i as f32) / 10_000.0).collect();
    let (pa, pb) = (file(&dir, "a.wav", &a), file(&dir, "b.wav", &b));
    // 1000..3000 of a, then 0..1500 of b
    let list = List {
        parts: vec![part(&pa, 1000, 2000, 3000), part(&pb, 0, 1500, 2000)],
        channels: 1,
        rate: RATE,
    };
    let expected: Vec<f32> = a[1000..3000].iter().chain(&b[..1500]).copied().collect();
    let out = |name: &str| Output {
        path: dir
            .join(format!("{name}.w64"))
            .to_string_lossy()
            .into_owned(),
        peaks: dir
            .join(format!("{name}.peaks"))
            .to_string_lossy()
            .into_owned(),
        format: "float".into(),
        label: String::new(),
    };

    let halved = out("halved");
    let curve = [GainPoint {
        at: 0.0,
        value: 0.5,
        shape: 1.0,
        curve: 0.0,
    }];
    clausters_app_engine::steps::carry(&[renders::gain(&list, &curve, &halved)], |_, _, _| {})
        .unwrap();
    let got = read(&halved.path);
    assert_eq!(got.len(), expected.len());
    for (i, (g, e)) in got.iter().zip(&expected).enumerate() {
        assert!(
            (g - e * 0.5).abs() < 1e-6,
            "frame {i}: {g} against {}",
            e * 0.5
        );
    }
    assert!(
        std::path::Path::new(&halved.peaks).exists(),
        "measured beside"
    );

    let reversed = out("reversed");
    clausters_app_engine::steps::carry(&[renders::reverse(&list, &reversed)], |_, _, _| {})
        .unwrap();
    let got = read(&reversed.path);
    let backwards: Vec<f32> = expected.iter().rev().copied().collect();
    assert_eq!(got, backwards, "the list from its last frame to its first");

    let _ = std::fs::remove_dir_all(&dir);
}
