//! **The application reads its take, and the engine plays what it reads**:
//! a take over three files, read by the application's own thread into the
//! rings an engine node consumes, played on an engine driven block by block.
//!
//! Every frame of every file says which file and which frame it is, so what
//! is heard says where it came from: a cued play is heard from its first
//! frame, a loop across the seams of the three files has no silent frame,
//! and a locate while it plays never plays a frame of where it was.

#![cfg(feature = "synth")]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use clausters::server::engine::{BLOCK_SIZE, Engine, engine_pair};
use clausters_app_engine::take::{Headers, ReadList, TakeFeed};
use clausters_app_engine::{AppEngine, EngineMedium};
use clausters_core::ids::{IdShare, IdSpaces, ServerShape};
use clausters_document::clipboard::SegmentPart;
use clausters_editing::audio_playback::AudioEditorPlayback;
use clausters_editing::pass::Pass;

const SR: f64 = 48_000.0;
const FILE: usize = 12_000;
/// The play's ramp, in samples: past it the take is heard as it is.
const RAMP: usize = 256;

/// **Frame `f` of file `k`**: a quarter per file and a step per frame, each
/// exact in an `f32`, so a value is read back as its file and its frame.
fn value(k: usize, f: usize) -> f32 {
    (k + 1) as f32 * 0.25 + f as f32 / (1 << 20) as f32
}

/// The file and the frame a value is.
fn frame_of(v: f32) -> Option<(usize, usize)> {
    let k = (v / 0.25).floor() as usize;
    if !(1..=3).contains(&k) {
        return None;
    }
    let f = ((v - k as f32 * 0.25) * (1 << 20) as f32).round();
    (f >= 0.0 && (f as usize) < FILE).then_some((k - 1, f as usize))
}

/// Three mono float files, each its frames told apart.
fn files(name: &str) -> Vec<String> {
    (0..3)
        .map(|k| {
            let path = std::env::temp_dir().join(format!(
                "app-engine-reading-{name}-{k}-{}.wav",
                std::process::id()
            ));
            let spec = hound::WavSpec {
                channels: 1,
                sample_rate: SR as u32,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            };
            let mut w = hound::WavWriter::create(&path, spec).unwrap();
            for f in 0..FILE {
                w.write_sample(value(k, f)).unwrap();
            }
            w.finalize().unwrap();
            path.to_string_lossy().into_owned()
        })
        .collect()
}

/// The take's parts: a span of each file, in turn, so two seams fall
/// between files.
const PARTS: [(usize, usize, usize); 3] = [(0, 1_000, 8_000), (1, 2_000, 8_000), (2, 0, 8_000)];

/// What frame `x` of the take is, as a value.
fn heard_at(x: usize) -> f32 {
    let mut at = 0;
    for (k, start, frames) in PARTS {
        if x < at + frames {
            return value(k, start + x - at);
        }
        at += frames;
    }
    0.0
}

const TAKE: usize = 24_000;

/// An engine an application holds, reading the three-file take into the
/// stream its playback plays.
struct Rig {
    engine: Engine,
    app: Arc<Mutex<AppEngine>>,
    playback: AudioEditorPlayback<EngineMedium>,
    _feed: TakeFeed,
    paths: Vec<String>,
}

impl Rig {
    fn new(name: &str) -> Rig {
        let paths = files(name);
        let parts: Vec<SegmentPart> = PARTS
            .iter()
            .map(|&(k, start, frames)| SegmentPart {
                path: paths[k].clone(),
                start: start as u64,
                frames: frames as u64,
                fade_in: 0,
                fade_out: 0,
                fade_in_shape: Default::default(),
                fade_out_shape: Default::default(),
                channels: None,
                source_channels: 1,
                source_frames: FILE as u64,
                source_rate: SR,
            })
            .collect();
        let (engine, handle) = engine_pair(SR as f32, 2);
        let app = Arc::new(Mutex::new(AppEngine::over(handle)));
        let mut ids = IdSpaces::new(ServerShape::DEFAULT, IdShare::WHOLE);
        let list = ReadList::of(&parts, 1, SR, &mut Headers::default()).expect("the files read");
        assert_eq!(list.frames(), TAKE as u64);
        let feed = TakeFeed::start(1);
        feed.set(Arc::new(list));
        let buffers = AppEngine::alloc_take(1, &mut ids).unwrap();
        assert!(
            app.lock()
                .unwrap()
                .install_take(&feed, TAKE as u64, &buffers)
        );
        let mut playback = AudioEditorPlayback::in_medium(EngineMedium::new(Arc::clone(&app)));
        playback
            .sync_streams(1, buffers, TAKE as u64, SR, SR, &mut ids)
            .unwrap();
        Rig {
            engine,
            app,
            playback,
            _feed: feed,
            paths,
        }
    }

    /// `n` blocks processed, the left channel heard, the application's
    /// side pumped between them and the time a block takes let pass.
    fn blocks(&mut self, n: usize) -> Vec<f32> {
        let mut out = vec![0.0f32; BLOCK_SIZE * 2];
        let mut left = Vec::new();
        for _ in 0..n {
            self.engine.process_block(&mut out);
            self.app.lock().unwrap().pump();
            left.extend(out.as_chunks::<2>().0.iter().map(|f| f[0]));
            std::thread::sleep(Duration::from_micros(1_333));
        }
        left
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        for path in &self.paths {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// **Where the play rolled**: the block its first sound is in. Nothing
/// before it is heard.
fn rolled(heard: &[f32]) -> usize {
    let first = heard
        .iter()
        .position(|&s| s != 0.0)
        .expect("the take was heard");
    assert!(first < 12_000, "the cue took {first} samples");
    first / BLOCK_SIZE * BLOCK_SIZE
}

/// **A cued play is heard from its first frame**: the block the transport
/// rolls in is the frame the play starts on, under the play's ramp -- each
/// sample the take's own frame at a gain that only rises -- and from the
/// ramp's end every frame is the take's, across the seam into the next file.
#[test]
fn a_cued_play_is_heard_from_its_first_frame() {
    let mut rig = Rig::new("cued");
    let from = 7_000;
    rig.playback.play(
        1,
        from as u64,
        Pass::Until {
            end: Some(TAKE as u64),
            back: 0,
        },
    );
    let heard = rig.blocks(240);
    let start = rolled(&heard);
    let mut gain = 0.0f32;
    for (i, &s) in heard.iter().enumerate().skip(start) {
        let x = from + i - start;
        let want = heard_at(x);
        if i < start + RAMP {
            let g = s / want;
            assert!(
                (gain - 1e-6..=1.0 + 1e-6).contains(&g),
                "sample {i}: frame {x} at a gain of {g}, after {gain}"
            );
            gain = g;
        } else {
            assert_eq!(s, want, "sample {i}: frame {x}");
        }
    }
    assert!(
        from + heard.len() - start > 8_000 + 500,
        "the seam into the second file was heard"
    );
}

/// **A loop across the seams of three files has no silent frame**: from the
/// first file to the last and back, over and over, every frame past the
/// ramp is the loop's, and none is silence.
#[test]
fn a_loop_across_the_seams_of_three_files_has_no_silent_frame() {
    let mut rig = Rig::new("loop");
    let (from, to) = (6_000usize, 20_000usize);
    rig.playback.play(
        1,
        from as u64,
        Pass::Loop {
            from: from as u64,
            to: to as u64,
        },
    );
    let heard = rig.blocks(1_200);
    let start = rolled(&heard);
    let span = to - from;
    assert!(heard.len() - start > 4 * span, "four times round the loop");
    for (i, &s) in heard.iter().enumerate().skip(start + RAMP) {
        let x = from + (i - start) % span;
        assert!(s != 0.0, "sample {i}: silence at frame {x}");
        assert_eq!(s, heard_at(x), "sample {i}: frame {x}");
    }
}

/// **A locate while it plays never plays a frame of where it was**: the
/// take plays on from where it was until the locate lands, then is silent
/// while the frames it was moved to are read, and plays on from there --
/// each frame heard either the next of where it was, before, or the next
/// of where it went, after, and nothing else.
#[test]
fn a_locate_never_plays_a_stale_frame() {
    let mut rig = Rig::new("locate");
    rig.playback.play(
        1,
        0,
        Pass::Until {
            end: Some(TAKE as u64),
            back: 0,
        },
    );
    let before = rig.blocks(60);
    let start = rolled(&before);
    let mut last = before.len() - 1 - start;
    assert_eq!(*before.last().unwrap(), heard_at(last));
    let to = 17_000usize;
    rig.playback.locate(to as u64);
    // short of the take's end, whose stop ramps it out
    let after = rig.blocks(80);
    // where it was, until a frame of where it went is heard
    let mut went: Option<usize> = None;
    for (i, &s) in after.iter().enumerate() {
        if s == 0.0 {
            assert!(
                went.is_none(),
                "sample {i}: a silence after the locate landed"
            );
            continue;
        }
        let Some((k, f)) = frame_of(s) else {
            panic!("sample {i}: {s} is no frame of the take");
        };
        let x = PARTS
            .iter()
            .scan(0, |at, &(pk, ps, pf)| {
                let here = *at;
                *at += pf;
                Some((here, pk, ps, pf))
            })
            .find(|&(_, pk, ps, pf)| pk == k && (ps..ps + pf).contains(&f))
            .map(|(at, _, ps, _)| at + f - ps)
            .expect("a frame the take reads");
        match went {
            Some(w) => assert_eq!(x, w + 1, "sample {i}: frame {x} after {w}"),
            None if x == last + 1 => last = x,
            None => {
                assert_eq!(
                    x, to,
                    "sample {i}: frame {x} is neither where it was nor where it went"
                );
                went = Some(x);
                continue;
            }
        }
        if let Some(w) = went.as_mut() {
            *w = x;
        }
    }
    assert!(went.is_some(), "the take was heard from where it went");
}
