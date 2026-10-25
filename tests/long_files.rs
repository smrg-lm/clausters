//! A file of any length, and a peaks file for it: W64 and RF64 written by an
//! offline render and read back wherever a WAV is read, and `PeaksOut` writing
//! the summary of the same signal in the same render.
//!
//! The take past 4 GB is `--ignored` (a few minutes and that much disk); the
//! rest runs on a minute or less of audio.
#![cfg(all(feature = "synth", not(target_arch = "wasm32")))]

use std::sync::Arc;

use clausters::clausters_core::peaks::MultiPyramid;
use clausters::server::nrt::read_audio;
use clausters::server::render::{OscScore, RenderConfig, render_to_wav};
use rosc::{OscMessage, OscType};

const SR: f64 = 48_000.0;

fn msg(addr: &str, args: Vec<OscType>) -> OscMessage {
    OscMessage {
        addr: addr.into(),
        args,
    }
}

/// A directory of this test's own, emptied first, on the disk cargo gives
/// integration tests (not `/tmp`, which may be memory).
fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("long-files-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A stereo score of `seconds`: two tones out, and `PeaksOut` on the same two
/// signals into `peaks`.
fn tones(seconds: f64, peaks: &str) -> OscScore {
    let def = serde_json::json!({
        "name": "tones",
        "ugens": [
            {"kind": "Sine", "inputs": [{"const": 440.0}]},
            {"kind": "Sine", "inputs": [{"const": 661.0}]},
            {"kind": "Mul", "inputs": [{"ugen": 1}, {"const": 0.25}]},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 0}]},
            {"kind": "Out", "inputs": [{"const": 1.0}, {"ugen": 2}]},
            {"kind": "PeaksOut", "inputs": [{"ugen": 0}, {"ugen": 2}], "path": peaks}
        ]
    });
    OscScore::new([
        (
            0.0,
            vec![
                msg(
                    "/def_send",
                    vec![
                        OscType::String("synth".into()),
                        OscType::Blob(serde_json::to_vec(&def).unwrap()),
                    ],
                ),
                msg(
                    "/synth_new",
                    vec![
                        OscType::String("tones".into()),
                        OscType::Int(1000),
                        OscType::Int(0),
                        OscType::Int(0),
                    ],
                ),
            ],
        ),
        (seconds, vec![msg("/node_free", vec![OscType::Int(1000)])]),
    ])
    .unwrap()
}

fn stereo() -> RenderConfig {
    RenderConfig {
        sample_rate: SR,
        channels: 2,
        ..RenderConfig::default()
    }
}

/// **The peaks file is the summary of the samples, bucket for bucket**: the
/// one `PeaksOut` wrote against the one built from the file read back.
fn assert_peaks_match(samples: &[f32], channels: usize, peaks: &str) {
    let built = MultiPyramid::build_interleaved(samples, channels, 256);
    let kept = MultiPyramid::read_cache(peaks)
        .expect("readable")
        .expect("a CLPK file");
    assert_eq!(kept.num_channels(), channels);
    assert_eq!(kept.frames(), built.frames(), "the frames summarized");
    let frames = samples.len() / channels;
    for ch in 0..channels {
        let (a, b) = (kept.channel(ch).unwrap(), built.channel(ch).unwrap());
        for bucket in 0..frames.div_ceil(256) {
            let (s0, s1) = (
                (bucket * 256) as f64,
                ((bucket + 1) * 256).min(frames) as f64,
            );
            assert_eq!(
                a.column(0, s0, s1),
                b.column(0, s0, s1),
                "ch {ch} bucket {bucket}"
            );
            let (ma, mb) = (
                a.column_ms(0, s0, s1).unwrap(),
                b.column_ms(0, s0, s1).unwrap(),
            );
            assert!(
                (ma - mb).abs() <= 1e-6 * mb.max(1e-9),
                "ch {ch} bucket {bucket}: mean square {ma} against {mb}"
            );
        }
    }
}

/// **The spectral summary beside the peaks is the summary of the samples**,
/// column for column: the one `PeaksOut` folded as it played against the one
/// folded from the file read back.
fn assert_spectra_match(samples: &[f32], channels: usize, peaks: &str) {
    use clausters::clausters_core::spectra::{Accumulator, Spectra, beside_peaks};
    let mut acc = Accumulator::new(channels);
    acc.set_channels(channels);
    let mut columns = Vec::new();
    let mut done = |col: &[u8]| columns.extend_from_slice(col);
    for frame in samples.chunks_exact(channels) {
        acc.push(|c| frame[c], &mut done);
    }
    acc.finish(&mut done);
    let frames = samples.len() / channels;
    let built = Spectra::from_columns(frames, channels, &columns).expect("the columns");
    let kept = Spectra::from_bytes(&std::fs::read(beside_peaks(peaks)).expect("written beside"))
        .expect("a spectral summary");
    assert_eq!(kept.head, built.head);
    assert!(kept == built, "the summary of the samples written");
}

/// **A render writes a W64 and an RF64 and both read back sample for
/// sample**, the same as the WAV of the same render, and the peaks file
/// written beside each is the summary of its samples.
#[test]
fn a_render_writes_every_container_and_its_peaks() {
    let dir = scratch("containers");
    let mut takes = Vec::new();
    for name in ["take.wav", "take.w64", "take.rf64"] {
        let path = dir.join(name);
        let peaks = dir.join(format!("{name}.peaks"));
        let peaks = peaks.to_str().unwrap();
        render_to_wav(&tones(1.3, peaks), &stereo(), &path, "float").unwrap();
        let back = read_audio(path.to_str().unwrap(), 0, 0).unwrap();
        assert_eq!((back.channels(), back.frames()), (2, 62_400), "{name}");
        let samples = back.to_vec();
        assert_peaks_match(&samples, 2, peaks);
        assert_spectra_match(&samples, 2, peaks);
        takes.push(samples);
    }
    assert_eq!(takes[0], takes[1], "the W64 holds the WAV's samples");
    assert_eq!(takes[0], takes[2], "the RF64 holds the WAV's samples");
    let _ = std::fs::remove_dir_all(&dir);
}

/// **A file imported without peaks is measured by a render of `DiskIn` into
/// `PeaksOut` alone**: no command, the same summary.
#[test]
fn a_file_is_measured_by_a_render_of_disk_in_into_peaks_out() {
    let dir = scratch("measure");
    let take = dir.join("take.w64");
    let first = dir.join("first.peaks");
    render_to_wav(
        &tones(2.0, first.to_str().unwrap()),
        &stereo(),
        &take,
        "int24",
    )
    .unwrap();
    let samples = read_audio(take.to_str().unwrap(), 0, 0).unwrap().to_vec();
    let measured = dir.join("measured.peaks");
    let def = serde_json::json!({
        "name": "measure",
        "ugens": [
            {"kind": "DiskIn", "inputs": [{"const": 0.0}], "path": take.to_str().unwrap()},
            {"kind": "DiskIn", "inputs": [{"const": 1.0}], "path": take.to_str().unwrap()},
            {"kind": "PeaksOut", "inputs": [{"ugen": 0}, {"ugen": 1}],
             "path": measured.to_str().unwrap()}
        ]
    });
    let score = OscScore::new([
        (
            0.0,
            vec![
                msg(
                    "/def_send",
                    vec![
                        OscType::String("synth".into()),
                        OscType::Blob(serde_json::to_vec(&def).unwrap()),
                    ],
                ),
                msg(
                    "/synth_new",
                    vec![
                        OscType::String("measure".into()),
                        OscType::Int(1000),
                        OscType::Int(0),
                        OscType::Int(0),
                    ],
                ),
            ],
        ),
        (2.0, vec![msg("/node_free", vec![OscType::Int(1000)])]),
    ])
    .unwrap();
    // Offline a `DiskIn` waits for its reads, so a render faster than the
    // disk measures the file and not the silence of an underrun.
    clausters::server::render::render_to_vec(&score, &stereo()).unwrap();
    assert_peaks_match(&samples, 2, measured.to_str().unwrap());
    assert_spectra_match(&samples, 2, measured.to_str().unwrap());
    let _ = std::fs::remove_dir_all(&dir);
}

/// **Two offline renders at once write what each writes alone**: each file
/// and its peaks, byte for byte.
#[test]
fn two_renders_at_once_write_what_each_writes_alone() {
    let dir = scratch("parallel");
    let job = |name: &str, seconds: f64| {
        let path = dir.join(format!("{name}.w64"));
        let peaks = dir.join(format!("{name}.peaks"));
        let score = tones(seconds, peaks.to_str().unwrap());
        render_to_wav(&score, &stereo(), &path, "float").unwrap();
        (
            std::fs::read(&path).unwrap(),
            std::fs::read(&peaks).unwrap(),
        )
    };
    let alone = (job("a-alone", 3.0), job("b-alone", 2.0));
    let together = std::thread::scope(|s| {
        let a = s.spawn(|| job("a-together", 3.0));
        let b = s.spawn(|| job("b-together", 2.0));
        (a.join().unwrap(), b.join().unwrap())
    });
    assert!(together.0 == alone.0, "the first render's file and peaks");
    assert!(together.1 == alone.1, "the second render's file and peaks");
    let _ = std::fs::remove_dir_all(&dir);
}

/// **A W64 past WAV's 4 GB, written by a render, reads back sample for
/// sample** -- spot-checked at its start, around the 4 GB mark and at its
/// end against the signal the render made -- and its peaks file covers all of
/// it.
#[test]
#[ignore = "writes 4.4 GB to disk and takes minutes"]
fn a_render_past_four_gigabytes_reads_back() {
    use clausters::dsp::pcm::{Layout, Reader};
    let dir = scratch("huge");
    let path = dir.join("long.w64");
    let peaks = dir.join("long.peaks");
    // 3.2 hours of stereo floats is 4.4 GB.
    let seconds = 3.2 * 3600.0;
    let t = std::time::Instant::now();
    render_to_wav(
        &tones(seconds, peaks.to_str().unwrap()),
        &stereo(),
        &path,
        "float",
    )
    .unwrap();
    let rendered = t.elapsed();
    let layout = Layout::open(path.to_str().unwrap()).unwrap();
    let bytes = std::fs::metadata(&path).unwrap().len();
    assert!(bytes > 4 << 30, "{bytes} bytes");
    let frames = (seconds * SR).round() as u64;
    assert_eq!(layout.frames, frames);
    // The same score rendered again, keeping only the spans it checks: the
    // file has to hold what the render made, wherever in it.
    let past = (4u64 << 30) / 8;
    let windows = [0, past - 1_000, frames - 4_096];
    let mut kept_spans: Vec<Vec<f32>> = vec![Vec::new(); windows.len()];
    let mut at = 0u64;
    let again = dir.join("again.peaks");
    clausters::server::render::render(
        &tones(seconds, again.to_str().unwrap()),
        &stereo(),
        |chunk| {
            let n = (chunk.len() / 2) as u64;
            for (w, &start) in windows.iter().enumerate() {
                let (lo, hi) = (start.max(at), (start + 4_096).min(at + n));
                if lo < hi {
                    let from = ((lo - at) * 2) as usize;
                    let to = ((hi - at) * 2) as usize;
                    kept_spans[w].extend_from_slice(&chunk[from..to]);
                }
            }
            at += n;
            Ok(())
        },
    )
    .unwrap();
    let mut reader = Reader::open(&layout).unwrap();
    let mut span = Vec::new();
    for (w, &start) in windows.iter().enumerate() {
        reader.read(start, 4_096, &mut span).unwrap();
        assert!(span == kept_spans[w], "the span at frame {start}");
    }
    let kept = MultiPyramid::read_cache(&peaks).unwrap().unwrap();
    assert_eq!(kept.frames() as u64, frames);
    println!(
        "rendered {:.1} GB in {:.1} s; peaks file {:.1} MB",
        bytes as f64 / 1e9,
        rendered.as_secs_f64(),
        std::fs::metadata(&peaks).unwrap().len() as f64 / 1e6
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// **`/buffer_write` writes the container its header argument or its path
/// names**, and `/buffer_read` reads each back.
#[test]
fn buffer_write_writes_every_container() {
    use clausters::dsp::buffer::Buffer;
    use clausters::dsp::pcm::{Container, Layout};
    use clausters::osc::translate::parse_buffer_msg;
    use clausters::server::nrt::run_job;
    let dir = scratch("write");
    let values: Vec<f32> = (0..2_000).map(|i| (i as f32 / 2_000.0) - 0.5).collect();
    let mut pool = vec![Some(Arc::new(Buffer::new(values.clone(), 2, 1_000, SR)))];
    pool.resize(4, None);
    for (name, header, container) in [
        ("a.w64", "wav", Container::W64),
        ("b.wav", "rf64", Container::Rf64),
        ("c.rf64", "wav", Container::Rf64),
        ("d.wav", "wav", Container::Wav),
    ] {
        let path = dir.join(name);
        let path = path.to_str().unwrap();
        let (_, job) = parse_buffer_msg(
            "/buffer_write",
            &[
                OscType::Int(0),
                OscType::String(path.into()),
                OscType::String(header.into()),
                OscType::String("float".into()),
            ],
            &pool,
            SR,
        )
        .unwrap();
        run_job(job).unwrap();
        assert_eq!(Layout::open(path).unwrap().container, container, "{name}");
        assert_eq!(read_audio(path, 0, 0).unwrap().to_vec(), values, "{name}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// **Measuring an hour of stereo costs about one read of it, off the audio
/// thread, and an RT server plays on without running out of lead**: the
/// measure is an offline render of `DiskIn` into `PeaksOut` on a thread of
/// its own while an engine paced in real time streams a list from disk.
#[test]
#[ignore = "writes an hour of stereo to disk and measures it; prints what it costs"]
fn measuring_an_hour_leaves_a_playing_server_alone() {
    use clausters::clausters_core::rng::SEED_STRIDE;
    use clausters::node::{AddAction, ROOT_NODE_ID};
    use clausters::server::engine::{BLOCK_SIZE, Cmd, engine_pair};
    use clausters::server::meters::Meters;
    use clausters::synthdef::instance::UGenSynth;
    use clausters::synthdef::{SynthDefSpec, compile};
    let dir = scratch("hour");
    let take = dir.join("hour.w64");
    let first = dir.join("hour.peaks");
    let seconds = 3600.0;
    // Floats, so the file holds the very values the render's own peaks
    // summarized and the measure has to find the same file.
    render_to_wav(
        &tones(seconds, first.to_str().unwrap()),
        &stereo(),
        &take,
        "float",
    )
    .unwrap();
    let bytes = std::fs::metadata(&take).unwrap().len();

    // What one read of it costs, from where it is now.
    let t = std::time::Instant::now();
    let mut file = std::fs::File::open(&take).unwrap();
    let mut chunk = vec![0u8; 1 << 22];
    while std::io::Read::read(&mut file, &mut chunk).unwrap() > 0 {}
    let one_read = t.elapsed();

    // The server playing: a list over a second file, from disk, paced.
    let other = dir.join("other.w64");
    let other_peaks = dir.join("other.peaks");
    render_to_wav(
        &tones(30.0, other_peaks.to_str().unwrap()),
        &stereo(),
        &other,
        "float",
    )
    .unwrap();
    let mut args = vec![OscType::Int(0), OscType::Int(1), OscType::Float(SR as f32)];
    for q in 0..40 {
        args.extend([
            OscType::String(other.to_str().unwrap().into()),
            OscType::Int((q * 7_919) % 1_000_000),
            OscType::Int(12_000),
            OscType::Int(32),
            OscType::Int(32),
            OscType::Int(1),
            OscType::Float(0.0),
            OscType::Int(1),
            OscType::Float(0.0),
            OscType::Int(0),
        ]);
    }
    let mirror = vec![None; 4];
    let (_, job) =
        clausters::osc::translate::parse_buffer_msg("/buffer_stitch", &args, &mirror, SR).unwrap();
    let list = match clausters::server::nrt::run_job(job).unwrap() {
        clausters::server::nrt::NrtAction::Install(b) => b,
        _ => unreachable!(),
    };
    let (mut engine, mut handle) = engine_pair(SR as f32, 2);
    let meters = Meters::detached();
    handle
        .send(Cmd::SetBuffer {
            index: 0,
            buffer: Some(list),
        })
        .ok()
        .unwrap();
    let spec: SynthDefSpec = serde_json::from_str(
        r#"{"name": "play", "ugens": [
            {"kind": "TransportDiskIn", "inputs": [{"const": 0.0}, {"const": 0.0},
                {"const": 0.0}, {"const": 0.0}, {"const": 1.0}]},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 0}]}
        ]}"#,
    )
    .unwrap();
    handle
        .send(Cmd::AddSynth {
            id: 1000,
            target: ROOT_NODE_ID,
            action: AddAction::Tail,
            synth: Box::new(UGenSynth::metered(
                Arc::new(compile(spec).unwrap()),
                SR as f32,
                SEED_STRIDE,
                Arc::clone(&meters),
            )),
            usage: Default::default(),
        })
        .ok()
        .unwrap();
    handle
        .send(Cmd::TransportRun {
            transport: 0,
            rolling: true,
        })
        .ok()
        .unwrap();

    // The measure, on a thread of its own.
    let measured = dir.join("measured.peaks");
    let def = serde_json::json!({
        "name": "measure",
        "ugens": [
            {"kind": "DiskIn", "inputs": [{"const": 0.0}], "path": take.to_str().unwrap()},
            {"kind": "DiskIn", "inputs": [{"const": 1.0}], "path": take.to_str().unwrap()},
            {"kind": "PeaksOut", "inputs": [{"ugen": 0}, {"ugen": 1}],
             "path": measured.to_str().unwrap()}
        ]
    });
    let score = OscScore::new([
        (
            0.0,
            vec![
                msg(
                    "/def_send",
                    vec![
                        OscType::String("synth".into()),
                        OscType::Blob(serde_json::to_vec(&def).unwrap()),
                    ],
                ),
                msg(
                    "/synth_new",
                    vec![
                        OscType::String("measure".into()),
                        OscType::Int(1000),
                        OscType::Int(0),
                        OscType::Int(0),
                    ],
                ),
            ],
        ),
        (seconds, vec![msg("/node_free", vec![OscType::Int(1000)])]),
    ])
    .unwrap();
    let t = std::time::Instant::now();
    let measuring = std::thread::spawn(move || {
        let cfg = RenderConfig {
            sample_rate: SR,
            channels: 1,
            ..RenderConfig::default()
        };
        clausters::server::render::render(&score, &cfg, |_| Ok(())).unwrap();
    });
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    let mut blocks = 0u64;
    while !measuring.is_finished() {
        engine.process_block(&mut out);
        blocks += 1;
        std::thread::sleep(std::time::Duration::from_micros(1_333));
    }
    measuring.join().unwrap();
    let measure = t.elapsed();
    let kept = MultiPyramid::read_cache(&measured).unwrap().unwrap();
    let first = MultiPyramid::read_cache(&first).unwrap().unwrap();
    assert!(
        kept.to_bytes() == first.to_bytes(),
        "the measure is the render's own peaks"
    );
    println!(
        "an hour of stereo floats ({:.2} GB): one read {:.2} s, the measure {:.2} s; \
         the playing server ran {blocks} blocks and starved {} samples",
        bytes as f64 / 1e9,
        one_read.as_secs_f64(),
        measure.as_secs_f64(),
        meters.starved()
    );
    assert_eq!(
        meters.starved(),
        0,
        "the playing server never ran out of lead"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
