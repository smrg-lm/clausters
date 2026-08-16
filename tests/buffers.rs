//! M5 tests: the buffer pool, the NRT thread (alloc / WAV read / write /
//! zero / free, in submission order) and the `PlayBuf`/`BufRd` UGens, plus
//! the `/buffer_*` OSC round trip with a manually ticked engine.

#![cfg(feature = "synth")]

use std::sync::Arc;
use std::time::Duration;

use clausters::clausters_core::rng::SEED_STRIDE;
use clausters::dsp::buffer::Buffer;
use clausters::node::{AddAction, ROOT_NODE_ID};
use clausters::rosc::OscType;
use clausters::server::engine::{BLOCK_SIZE, Cmd, Engine, engine_pair};
use clausters::server::nrt::{NrtAction, NrtJob, NrtRequest, NrtThread};
use clausters::synthdef::SynthDefSpec;
use clausters::synthdef::instance::UGenSynth;
use serde_json::json;

const SR: f32 = 48_000.0;
const CHANNELS: usize = 2;
const NRT_DEADLINE: Duration = Duration::from_secs(10);

/// A throwaway path in the test temp dir.
fn tmp_wav(name: &str) -> String {
    tmp_path(name, "wav")
}

/// Same, with an arbitrary extension (used to force the non-WAV read path).
fn tmp_path(name: &str, ext: &str) -> String {
    let path = std::env::temp_dir().join(format!("clausters_{name}_{}.{ext}", std::process::id()));
    path.to_str().unwrap().to_string()
}

fn run_nrt(job: NrtJob) -> Result<NrtAction, String> {
    let nrt = NrtThread::spawn();
    nrt.submit(NrtRequest {
        cmd: "/b_test",
        index: 0,
        client: clausters::osc::ClientId::Udp("127.0.0.1:1".parse().unwrap()),
        chained: false,
        job,
    })
    .ok()
    .unwrap();
    nrt.recv_result_timeout(NRT_DEADLINE)
        .expect("NRT job must finish")
        .outcome
}

fn installed(action: Result<NrtAction, String>) -> Arc<Buffer> {
    match action.expect("job must succeed") {
        NrtAction::Install(buffer) => buffer,
        _ => panic!("expected an installed buffer"),
    }
}

/// `PlayBuf(bufnum, chan, rate, loop)` to the given output bus.
fn playbuf_spec(bufnum: f32, chan: f32, rate: f32, looping: f32, out_bus: f32) -> UGenSynth {
    spec_synth(json!({
        "name": "player",
        "ugens": [
            {"kind": "PlayBuf", "inputs": [
                {"const": bufnum}, {"const": chan}, {"const": rate}, {"const": looping}
            ]},
            {"kind": "Out", "inputs": [{"const": out_bus}, {"ugen": 0}]}
        ]
    }))
}

fn spec_synth(spec: serde_json::Value) -> UGenSynth {
    let spec: SynthDefSpec = serde_json::from_value(spec).unwrap();
    UGenSynth::new(
        Arc::new(clausters::synthdef::compile(spec).unwrap()),
        SR,
        SEED_STRIDE,
    )
}

fn add_synth(id: i32, synth: UGenSynth) -> Cmd {
    Cmd::AddSynth {
        id,
        target: ROOT_NODE_ID,
        action: AddAction::Tail,
        synth: Box::new(synth),
        usage: Default::default(),
    }
}

/// Renders `blocks` blocks and returns the chosen interleaved channel.
fn render_channel(engine: &mut Engine, blocks: usize, channel: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; BLOCK_SIZE * CHANNELS];
    let mut buf = Vec::with_capacity(blocks * BLOCK_SIZE);
    for _ in 0..blocks {
        engine.process_block(&mut out);
        buf.extend(out.iter().skip(channel).step_by(CHANNELS).copied());
    }
    buf
}

/// A mono buffer whose samples are a recognizable ramp `i / 1000`.
fn ramp_buffer(frames: usize) -> Arc<Buffer> {
    let data: Vec<f32> = (0..frames).map(|i| i as f32 / 1000.0).collect();
    Arc::new(Buffer::new(data, 1, frames, SR as f64))
}

// ---- engine + UGens ----

#[test]
fn playbuf_plays_a_buffer_once_then_goes_silent() {
    let frames = 100;
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    handle
        .send(Cmd::SetBuffer {
            index: 0,
            buffer: Some(ramp_buffer(frames)),
        })
        .ok()
        .unwrap();
    handle
        .send(add_synth(1000, playbuf_spec(0.0, 0.0, 1.0, 0.0, 0.0)))
        .ok()
        .unwrap();

    let left = render_channel(&mut engine, 4, 0); // 256 samples > 100 frames
    for (i, s) in left.iter().enumerate() {
        let expected = if i < frames { i as f32 / 1000.0 } else { 0.0 };
        assert_eq!(*s, expected, "sample {i}: rate-1 playback must be exact");
    }
}

#[test]
fn playbuf_loops_exactly() {
    let frames = 24;
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    handle
        .send(Cmd::SetBuffer {
            index: 3,
            buffer: Some(ramp_buffer(frames)),
        })
        .ok()
        .unwrap();
    handle
        .send(add_synth(1000, playbuf_spec(3.0, 0.0, 1.0, 1.0, 0.0)))
        .ok()
        .unwrap();

    let left = render_channel(&mut engine, 8, 0);
    for (i, s) in left.iter().enumerate() {
        assert_eq!(*s, (i % frames) as f32 / 1000.0, "sample {i}");
    }
}

#[test]
fn playbuf_reads_each_channel_of_an_interleaved_buffer() {
    // Stereo buffer: left = 0.25, right = -0.5 everywhere.
    let frames = 200;
    let data: Vec<f32> = (0..frames).flat_map(|_| [0.25, -0.5]).collect();
    let buffer = Arc::new(Buffer::new(data, 2, frames, SR as f64));

    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    handle
        .send(Cmd::SetBuffer {
            index: 0,
            buffer: Some(buffer),
        })
        .ok()
        .unwrap();
    handle
        .send(add_synth(1000, playbuf_spec(0.0, 0.0, 1.0, 0.0, 0.0)))
        .ok()
        .unwrap();
    handle
        .send(add_synth(1001, playbuf_spec(0.0, 1.0, 1.0, 0.0, 1.0)))
        .ok()
        .unwrap();

    let mut out = vec![0.0f32; BLOCK_SIZE * CHANNELS];
    engine.process_block(&mut out);
    assert!(out.chunks(2).all(|f| f == [0.25, -0.5]), "{:?}", &out[..4]);
}

#[test]
fn playbuf_with_no_buffer_is_silent() {
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    handle
        .send(add_synth(1000, playbuf_spec(7.0, 0.0, 1.0, 1.0, 0.0)))
        .ok()
        .unwrap();
    let left = render_channel(&mut engine, 4, 0);
    assert!(left.iter().all(|s| *s == 0.0));
}

#[test]
fn bufrd_interpolates_wraps_and_clamps() {
    let frames = 8;
    let bufrd = |phase: f32, looping: f32, out_bus: f32| {
        spec_synth(json!({
            "name": "rd",
            "ugens": [
                {"kind": "BufRd", "inputs": [
                    {"const": 0.0}, {"const": 0.0}, {"const": phase}, {"const": looping}
                ]},
                {"kind": "Out", "inputs": [{"const": out_bus}, {"ugen": 0}]}
            ]
        }))
    };
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    handle
        .send(Cmd::SetBuffer {
            index: 0,
            buffer: Some(ramp_buffer(frames)), // data[i] = i/1000
        })
        .ok()
        .unwrap();
    // Linear interpolation between frames 5 and 6.
    handle
        .send(add_synth(1000, bufrd(5.25, 0.0, 0.0)))
        .ok()
        .unwrap();
    // Out-of-range phase: wraps when looping (9.0 → 1.0)...
    handle
        .send(add_synth(1001, bufrd(9.0, 1.0, 1.0)))
        .ok()
        .unwrap();
    let mut out = vec![0.0f32; BLOCK_SIZE * CHANNELS];
    engine.process_block(&mut out);
    assert_eq!(out[0], 5.25 / 1000.0);
    assert_eq!(out[1], 1.0 / 1000.0);

    // ...and clamps to the last frame when not looping.
    let (mut engine2, mut handle2) = engine_pair(SR, CHANNELS);
    handle2
        .send(Cmd::SetBuffer {
            index: 0,
            buffer: Some(ramp_buffer(frames)),
        })
        .ok()
        .unwrap();
    handle2
        .send(add_synth(1000, bufrd(100.0, 0.0, 0.0)))
        .ok()
        .unwrap();
    let mut out = vec![0.0f32; BLOCK_SIZE * CHANNELS];
    engine2.process_block(&mut out);
    assert_eq!(out[0], 7.0 / 1000.0);
}

#[test]
fn kr_buf_rate_scale_still_reports_the_hardware_ratio() {
    // The twin of `rates.rs::kr_samplerate_still_reports_the_engine_rate`, for
    // the other quantity that is a hardware fact rather than a time base: a
    // control-rate BufRateScale must report file_sr / engine_sr, not
    // file_sr / (engine_sr / BLOCK_SIZE). Getting this wrong is silent — the
    // ratio comes back as the block size and a PlayBuf driven by it races
    // through its buffer in a few milliseconds.
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    handle
        .send(Cmd::SetBuffer {
            index: 0,
            buffer: Some(Arc::new(Buffer::new(vec![0.0; 100], 1, 100, 24_000.0))),
        })
        .ok()
        .unwrap();
    handle
        .send(add_synth(
            1000,
            spec_synth(json!({
                "name": "scale_kr",
                "ugens": [
                    {"kind": "BufRateScale", "rate": "kr", "inputs": [{"const": 0.0}]},
                    {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 0}]}
                ]
            })),
        ))
        .ok()
        .unwrap();
    let mut out = vec![0.0f32; BLOCK_SIZE * CHANNELS];
    engine.process_block(&mut out);
    assert_eq!(
        out[0], 0.5,
        "BufRateScale.kr = 24000/48000, not the block size"
    );
}

#[test]
fn buf_info_ugens_report_shape_and_rate_scale() {
    // A 24 kHz, 100-frame mono buffer played on a 48 kHz server: BufRateScale
    // must report 0.5 (file_sr / server_sr) so PlayBuf can correct the pitch.
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    let data: Vec<f32> = vec![0.0; 100];
    handle
        .send(Cmd::SetBuffer {
            index: 0,
            buffer: Some(Arc::new(Buffer::new(data, 1, 100, 24_000.0))),
        })
        .ok()
        .unwrap();

    let info = |kind: &str, out_bus: f32| {
        spec_synth(json!({
            "name": "info",
            "ugens": [
                {"kind": kind, "inputs": [{"const": 0.0}]},
                {"kind": "Out", "inputs": [{"const": out_bus}, {"ugen": 0}]}
            ]
        }))
    };
    for (i, kind) in ["BufRateScale", "BufSampleRate", "BufFrames", "BufDur"]
        .iter()
        .enumerate()
    {
        // Out bus 0 -> left channel, 1 -> right; reuse two channels twice.
        handle
            .send(add_synth(1000 + i as i32, info(kind, (i % 2) as f32)))
            .ok()
            .unwrap();
        if i % 2 == 1 {
            let mut out = vec![0.0f32; BLOCK_SIZE * CHANNELS];
            engine.process_block(&mut out);
            // i-1 went to bus 0, i to bus 1.
            let prev = out[0];
            let cur = out[1];
            match i {
                1 => {
                    assert_eq!(prev, 0.5, "BufRateScale = 24000/48000");
                    assert_eq!(cur, 24_000.0, "BufSampleRate");
                }
                3 => {
                    assert_eq!(prev, 100.0, "BufFrames");
                    assert_eq!(cur, 100.0 / 24_000.0, "BufDur = frames/file_sr");
                }
                _ => unreachable!(),
            }
            // Clear the nodes before the next pair so the buses are clean.
            handle
                .send(Cmd::FreeNode {
                    id: 1000 + i as i32 - 1,
                })
                .ok()
                .unwrap();
            handle
                .send(Cmd::FreeNode {
                    id: 1000 + i as i32,
                })
                .ok()
                .unwrap();
            render_channel(&mut engine, 1, 0);
        }
    }

    // BufChannels on a stereo buffer.
    handle
        .send(Cmd::SetBuffer {
            index: 1,
            buffer: Some(Arc::new(Buffer::new(vec![0.0; 8], 2, 4, 48_000.0))),
        })
        .ok()
        .unwrap();
    handle
        .send(add_synth(
            2000,
            spec_synth(json!({
                "name": "ch",
                "ugens": [
                    {"kind": "BufChannels", "inputs": [{"const": 1.0}]},
                    {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 0}]}
                ]
            })),
        ))
        .ok()
        .unwrap();
    let mut out = vec![0.0f32; BLOCK_SIZE * CHANNELS];
    engine.process_block(&mut out);
    assert_eq!(out[0], 2.0, "BufChannels");
}

#[test]
fn replaced_buffer_leaves_through_the_garbage_fifo() {
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    handle
        .send(Cmd::SetBuffer {
            index: 0,
            buffer: Some(ramp_buffer(10)),
        })
        .ok()
        .unwrap();
    render_channel(&mut engine, 1, 0);
    assert_eq!(
        handle.collect_garbage(),
        0,
        "first install replaces nothing"
    );

    handle
        .send(Cmd::SetBuffer {
            index: 0,
            buffer: Some(ramp_buffer(20)),
        })
        .ok()
        .unwrap();
    handle
        .send(Cmd::SetBuffer {
            index: 0,
            buffer: None,
        })
        .ok()
        .unwrap();
    render_channel(&mut engine, 1, 0);
    assert_eq!(handle.collect_garbage(), 2, "replace + free ship both out");
}

// ---- The write half: /buffer_set and /buffer_setRange ----

/// Bulk samples ride as one little-endian f32 blob (docs/schemas.md).
fn blob(values: &[f32]) -> OscType {
    OscType::Blob(values.iter().flat_map(|v| v.to_le_bytes()).collect())
}

/// The parse takes a mirror pool; these tests build a one-slot one.
fn mirror_of(buffer: Arc<Buffer>) -> clausters::dsp::buffer::BufferPool {
    vec![Some(buffer)]
}

fn parse_set(
    addr: &str,
    args: Vec<OscType>,
    mirror: &clausters::dsp::buffer::BufferPool,
) -> Result<NrtJob, String> {
    clausters::osc::translate::parse_buffer_msg(addr, &args, mirror, SR as f64).map(|(_, job)| job)
}

/// The refusal a malformed or out-of-bounds write parses into. (`NrtJob` is
/// not `Debug`, so the `Ok` side cannot be unwrapped away.)
fn set_error(
    addr: &str,
    args: Vec<OscType>,
    mirror: &clausters::dsp::buffer::BufferPool,
) -> String {
    match parse_set(addr, args, mirror) {
        Err(e) => e,
        Ok(_) => panic!("{addr} should have been refused"),
    }
}

#[test]
fn set_writes_runs_into_a_replacement_keeping_the_shape() {
    let mirror = mirror_of(Arc::new(Buffer::zeroed(8, 2, 44_100.0)));
    let job = parse_set(
        "/buffer_setRange",
        vec![
            OscType::Int(0),
            OscType::Int(4),
            blob(&[1.0, 2.0, 3.0]),
            // A second run in the same message, to prove they pack.
            OscType::Int(12),
            blob(&[9.0]),
        ],
        &mirror,
    )
    .expect("a well-formed setRange parses");

    let written = installed(run_nrt(job));
    assert_eq!(
        (written.frames(), written.channels()),
        (8, 2),
        "a write keeps the buffer's shape"
    );
    assert_eq!(
        written.sample_rate(),
        44_100.0,
        "and its sample rate: a write is not a re-allocation"
    );
    let mut expected = [0.0f32; 16];
    expected[4..7].copy_from_slice(&[1.0, 2.0, 3.0]);
    expected[12] = 9.0;
    assert_eq!(written.to_vec(), expected);
}

#[test]
fn set_writes_single_samples_by_flat_index() {
    let mirror = mirror_of(Arc::new(Buffer::zeroed(4, 2, SR as f64)));
    let job = parse_set(
        "/buffer_set",
        vec![
            OscType::Int(0),
            OscType::Int(1),
            OscType::Float(0.25),
            OscType::Int(6),
            OscType::Float(-0.5),
        ],
        &mirror,
    )
    .expect("a well-formed set parses");

    let written = installed(run_nrt(job));
    let mut expected = [0.0f32; 8];
    expected[1] = 0.25;
    expected[6] = -0.5;
    assert_eq!(
        written.to_vec(),
        &expected[..],
        "indices are flat across channels, as the reads are"
    );
}

/// **One channel of an interleaved buffer**, which no flat run can name: the
/// positions are frames of that channel, the samples land `channels` apart, and
/// the other channel is untouched.
#[test]
fn set_range_channel_writes_one_channel_and_leaves_the_other_alone() {
    let mirror = mirror_of(Arc::new(Buffer::new(
        vec![-1.0; 8], // both channels held, so an overwrite is visible
        2,
        4,
        SR as f64,
    )));
    let job = parse_set(
        "/buffer_setRangeChannel",
        vec![
            OscType::Int(0),
            OscType::Int(1), // the right channel
            OscType::Int(1), // from its second frame
            blob(&[1.0, 2.0, 3.0]),
        ],
        &mirror,
    )
    .expect("a well-formed setRangeChannel parses");

    let written = installed(run_nrt(job));
    assert_eq!(
        (written.frames(), written.channels()),
        (4, 2),
        "a channel write keeps the buffer's shape like every other write"
    );
    assert_eq!(
        written.to_vec(),
        &[-1.0, -1.0, -1.0, 1.0, -1.0, 2.0, -1.0, 3.0][..],
        "frames 1..4 of channel 1, strided; channel 0 as it was"
    );
}

/// The single-sample form of the same addressing.
#[test]
fn set_channel_writes_single_frames_of_one_channel() {
    let mirror = mirror_of(Arc::new(Buffer::zeroed(4, 2, SR as f64)));
    let job = parse_set(
        "/buffer_setChannel",
        vec![
            OscType::Int(0),
            OscType::Int(0),
            OscType::Int(0),
            OscType::Float(0.25),
            OscType::Int(3),
            OscType::Float(-0.5),
        ],
        &mirror,
    )
    .expect("a well-formed setChannel parses");

    let written = installed(run_nrt(job));
    let mut expected = [0.0f32; 8];
    expected[0] = 0.25; // frame 0, channel 0
    expected[6] = -0.5; // frame 3, channel 0
    assert_eq!(written.to_vec(), expected);
}

/// A channel the buffer does not have is a mistake worth hearing about — the
/// same posture as a channel a *file* does not have on the reading side.
#[test]
fn a_channel_write_names_a_channel_the_buffer_has() {
    let mirror = mirror_of(Arc::new(Buffer::zeroed(4, 2, SR as f64)));
    let err = set_error(
        "/buffer_setRangeChannel",
        vec![
            OscType::Int(0),
            OscType::Int(2),
            OscType::Int(0),
            blob(&[1.0]),
        ],
        &mirror,
    );
    assert!(err.contains("no channel 2"), "unexpected message: {err}");

    // And the bound is the channel's own frames, reported in frames: a run of
    // three from frame 2 of a four-frame buffer is past its end even though
    // the flat index it starts at is well inside the samples.
    let err = set_error(
        "/buffer_setRangeChannel",
        vec![
            OscType::Int(0),
            OscType::Int(1),
            OscType::Int(2),
            blob(&[1.0, 2.0, 3.0]),
        ],
        &mirror,
    );
    assert!(
        err.contains("frame range 2..5") && err.contains("4 frames"),
        "the refusal speaks the unit the caller wrote in: {err}"
    );
}

#[test]
fn a_write_past_the_end_fails_rather_than_being_clamped() {
    let mirror = mirror_of(Arc::new(Buffer::zeroed(4, 1, SR as f64)));
    // The read side clamps; the write side must not, or the caller believes it
    // stored samples the server dropped.
    let err = set_error(
        "/buffer_setRange",
        vec![OscType::Int(0), OscType::Int(2), blob(&[1.0; 4])],
        &mirror,
    );
    assert!(err.contains("past the end"), "unexpected message: {err}");

    let err = set_error(
        "/buffer_set",
        vec![OscType::Int(0), OscType::Int(4), OscType::Float(1.0)],
        &mirror,
    );
    assert!(err.contains("past the end"), "unexpected message: {err}");
}

#[test]
fn a_malformed_write_is_refused_before_it_reaches_the_queue() {
    let mirror = mirror_of(Arc::new(Buffer::zeroed(8, 1, SR as f64)));
    // A blob that is not a whole number of f32s: a partial sample would be
    // worse than a refusal.
    let err = set_error(
        "/buffer_setRange",
        vec![
            OscType::Int(0),
            OscType::Int(0),
            OscType::Blob(vec![0, 1, 2]),
        ],
        &mirror,
    );
    assert!(err.contains("multiple of 4"), "unexpected message: {err}");

    // Float arguments are the single-sample form's shape, not this one's.
    set_error(
        "/buffer_setRange",
        vec![OscType::Int(0), OscType::Int(0), OscType::Float(1.0)],
        &mirror,
    );

    // An odd tail on the pair form.
    set_error(
        "/buffer_set",
        vec![OscType::Int(0), OscType::Int(1)],
        &mirror,
    );

    // No values at all.
    // A write with nothing to write is a mistake, not a no-op.
    set_error("/buffer_set", vec![OscType::Int(0)], &mirror);

    // A negative index.
    // Sample indices are non-negative.
    set_error(
        "/buffer_set",
        vec![OscType::Int(0), OscType::Int(-1), OscType::Float(1.0)],
        &mirror,
    );
}

#[test]
fn a_write_needs_an_allocated_buffer() {
    let empty: clausters::dsp::buffer::BufferPool = vec![None];
    let err = set_error(
        "/buffer_set",
        vec![OscType::Int(0), OscType::Int(0), OscType::Float(1.0)],
        &empty,
    );
    assert!(err.contains("no buffer allocated"), "unexpected: {err}");
}

// ---- NRT thread: WAV round trips ----

#[test]
fn nrt_allocates_zeroed_buffers() {
    let buffer = installed(run_nrt(NrtJob::Alloc {
        frames: 64,
        channels: 2,
        sample_rate: 44_100.0,
    }));
    assert_eq!((buffer.frames(), buffer.channels()), (64, 2));
    assert_eq!(buffer.sample_rate(), 44_100.0);
    assert!(buffer.to_vec().iter().all(|s| *s == 0.0));
}

#[test]
fn wav_write_then_alloc_read_round_trips_float_exactly() {
    let path = tmp_wav("roundtrip");
    let frames = 300;
    let data: Vec<f32> = (0..frames * 2).map(|i| (i as f32).sin() * 0.5).collect();
    let original = Arc::new(Buffer::new(data, 2, frames, 22_050.0));

    let outcome = run_nrt(NrtJob::Write {
        path: path.clone(),
        sample_format: "float".into(),
        buf_start: 0,
        num_frames: -1,
        buffer: Arc::clone(&original),
    });
    assert!(matches!(outcome, Ok(NrtAction::None)), "{outcome:?}");

    let read = installed(run_nrt(NrtJob::AllocRead {
        path: path.clone(),
        file_start: 0,
        num_frames: 0,
        channels: Vec::new(),
    }));
    assert_eq!(read.frames(), frames);
    assert_eq!(read.channels(), 2);
    assert_eq!(read.sample_rate(), 22_050.0);
    assert_eq!(
        read.to_vec(),
        original.to_vec(),
        "float WAV must be lossless"
    );
    std::fs::remove_file(&path).ok();
}

#[test]
fn alloc_read_slices_the_file() {
    let path = tmp_wav("slice");
    let data: Vec<f32> = (0..100).map(|i| i as f32 / 1000.0).collect();
    run_nrt(NrtJob::Write {
        path: path.clone(),
        sample_format: "float".into(),
        buf_start: 0,
        num_frames: -1,
        buffer: Arc::new(Buffer::new(data, 1, 100, 48_000.0)),
    })
    .unwrap();

    let read = installed(run_nrt(NrtJob::AllocRead {
        path: path.clone(),
        file_start: 10,
        num_frames: 5,
        channels: Vec::new(),
    }));
    assert_eq!(read.frames(), 5);
    let expected: Vec<f32> = (10..15).map(|i| i as f32 / 1000.0).collect();
    assert_eq!(read.to_vec(), expected);
    std::fs::remove_file(&path).ok();
}

#[test]
fn read_overlays_a_file_keeping_the_buffer_shape() {
    let path = tmp_wav("overlay");
    let data: Vec<f32> = vec![0.5; 10];
    run_nrt(NrtJob::Write {
        path: path.clone(),
        sample_format: "float".into(),
        buf_start: 0,
        num_frames: -1,
        buffer: Arc::new(Buffer::new(data, 1, 10, 48_000.0)),
    })
    .unwrap();

    let read = installed(run_nrt(NrtJob::Read {
        path: path.clone(),
        file_start: 0,
        num_frames: -1,
        buf_start: 5,
        current: Arc::new(Buffer::zeroed(20, 1, 48_000.0)),
        channels: Vec::new(),
    }));
    assert_eq!(read.frames(), 20, "/buffer_read keeps the buffer's shape");
    for (i, s) in read.to_vec().iter().enumerate() {
        let expected = if (5..15).contains(&i) { 0.5 } else { 0.0 };
        assert_eq!(*s, expected, "frame {i}");
    }
    std::fs::remove_file(&path).ok();
}

#[test]
fn read_rejects_channel_mismatch_and_missing_files() {
    let path = tmp_wav("mismatch");
    run_nrt(NrtJob::Write {
        path: path.clone(),
        sample_format: "float".into(),
        buf_start: 0,
        num_frames: -1,
        buffer: Arc::new(Buffer::zeroed(10, 2, 48_000.0)),
    })
    .unwrap();
    let err = run_nrt(NrtJob::Read {
        path: path.clone(),
        file_start: 0,
        num_frames: -1,
        buf_start: 0,
        current: Arc::new(Buffer::zeroed(10, 1, 48_000.0)),
        channels: Vec::new(),
    })
    .unwrap_err();
    assert!(err.contains("channel count mismatch"), "{err}");
    std::fs::remove_file(&path).ok();

    let err = run_nrt(NrtJob::AllocRead {
        path: "/nonexistent/clausters.wav".into(),
        file_start: 0,
        num_frames: 0,
        channels: Vec::new(),
    })
    .unwrap_err();
    assert!(err.contains("/nonexistent/clausters.wav"), "{err}");
}

#[test]
fn int16_write_quantizes_to_the_expected_grid() {
    let path = tmp_wav("int16");
    let original = Arc::new(Buffer::new(vec![0.5, -1.0, 1.0, 0.0], 1, 4, 48_000.0));
    run_nrt(NrtJob::Write {
        path: path.clone(),
        sample_format: "int16".into(),
        buf_start: 0,
        num_frames: -1,
        buffer: original,
    })
    .unwrap();
    let read = installed(run_nrt(NrtJob::AllocRead {
        path: path.clone(),
        file_start: 0,
        num_frames: 0,
        channels: Vec::new(),
    }));
    // Write scales by 32767, read by 1/32768.
    let expected = [
        (0.5f32 * 32767.0).round() / 32768.0,
        -32767.0 / 32768.0,
        32767.0 / 32768.0,
        0.0,
    ];
    assert_eq!(read.to_vec(), expected);
    std::fs::remove_file(&path).ok();
}

#[test]
fn diskout_records_then_diskin_streams_it_back() {
    // End-to-end streaming I/O: play a buffer into DiskOut (a float WAV on
    // disk), then stream that file back through DiskIn and check the samples.
    // The signal `(i+1)/1000` starts non-zero, so DiskIn output is
    // distinguishable from underrun silence.
    let path = tmp_path("diskio", "wav");
    let frames = 300;
    let signal: Vec<f32> = (0..frames).map(|i| (i + 1) as f32 / 1000.0).collect();

    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    handle
        .send(Cmd::SetBuffer {
            index: 0,
            buffer: Some(Arc::new(Buffer::new(signal.clone(), 1, frames, SR as f64))),
        })
        .ok()
        .unwrap();

    // PlayBuf (rate 1, no loop) -> DiskOut(float). No Out: the synth is silent
    // on the buses, it only records.
    let recorder = spec_synth(json!({
        "name": "rec",
        "ugens": [
            {"kind": "PlayBuf", "inputs": [
                {"const": 0.0}, {"const": 0.0}, {"const": 1.0}, {"const": 0.0}
            ]},
            {"kind": "DiskOut", "inputs": [{"ugen": 0}], "path": path, "format": "float"}
        ]
    }));
    handle.send(add_synth(1000, recorder)).ok().unwrap();
    // Render enough blocks to push the whole buffer through the ring.
    let blocks = frames.div_ceil(BLOCK_SIZE) + 2;
    render_channel(&mut engine, blocks, 0);

    // Free the recorder; dropping its synth (on garbage collection) joins the
    // writer thread, which drains the ring and finalizes the WAV header.
    handle.send(Cmd::FreeNode { id: 1000 }).ok().unwrap();
    let deadline = std::time::Instant::now() + NRT_DEADLINE;
    while handle.collect_garbage() == 0 {
        render_channel(&mut engine, 1, 0);
        assert!(std::time::Instant::now() < deadline, "recorder never freed");
    }

    // Sanity: the file holds the signal (read it straight back).
    let read = installed(run_nrt(NrtJob::AllocRead {
        path: path.clone(),
        file_start: 0,
        num_frames: frames as i64,
        channels: Vec::new(),
    }));
    assert_eq!(read.frames(), frames);
    assert_eq!(read.to_vec(), signal, "DiskOut must write the signal");

    // Now stream the same file back through DiskIn -> Out(bus 0) and look for
    // the signal in the left channel. The disk thread fills the ring
    // asynchronously, so poll (with a tiny yield) until data arrives.
    let player = spec_synth(json!({
        "name": "play",
        "ugens": [
            {"kind": "DiskIn", "inputs": [{"const": 0.0}], "path": path},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 0}]}
        ]
    }));
    handle.send(add_synth(2000, player)).ok().unwrap();

    let mut collected: Vec<f32> = Vec::new();
    let deadline = std::time::Instant::now() + NRT_DEADLINE;
    let found = loop {
        collected.extend(render_channel(&mut engine, 1, 0));
        // The stream emits the file in order; find where signal[0] (0.001) is
        // followed by signal[1], signal[2], ... to confirm ordered streaming.
        if let Some(p) = collected
            .windows(8)
            .position(|w| (0..8).all(|k| (w[k] - signal[k]).abs() < 1e-9))
        {
            break Some(p);
        }
        if std::time::Instant::now() >= deadline {
            break None;
        }
        std::thread::sleep(Duration::from_millis(1));
    };
    let start = found.expect("DiskIn never produced the streamed signal");
    // Verify a longer ordered run from that point.
    for (k, expected) in signal.iter().enumerate().take(64) {
        assert_eq!(
            collected[start + k],
            *expected,
            "DiskIn frame {k} must stream in order"
        );
    }

    handle.send(Cmd::FreeNode { id: 2000 }).ok().unwrap();
    render_channel(&mut engine, 2, 0);
    while handle.collect_garbage() > 0 {}
    std::fs::remove_file(&path).ok();
}

#[test]
fn non_wav_extensions_decode_through_symphonia() {
    // Lossless float PCM written to a `.dat` file: `read_audio` routes it
    // through symphonia (not hound), which detects the container by content,
    // not the extension. Exercises the decode + interleave + slice path that
    // every compressed format (FLAC, OGG, MP3, ...) shares.
    let path = tmp_path("symphonia", "dat");
    let frames = 200;
    let data: Vec<f32> = (0..frames * 2)
        .map(|i| (i as f32 * 0.013).sin() * 0.5)
        .collect();
    let original = Arc::new(Buffer::new(data, 2, frames, 44_100.0));
    run_nrt(NrtJob::Write {
        path: path.clone(),
        sample_format: "float".into(),
        buf_start: 0,
        num_frames: -1,
        buffer: Arc::clone(&original),
    })
    .unwrap();

    let read = installed(run_nrt(NrtJob::AllocRead {
        path: path.clone(),
        file_start: 0,
        num_frames: 0,
        channels: Vec::new(),
    }));
    assert_eq!((read.frames(), read.channels()), (frames, 2));
    assert_eq!(read.sample_rate(), 44_100.0);
    assert_eq!(
        read.to_vec(),
        original.to_vec(),
        "float PCM via symphonia must be lossless"
    );

    // The slice window (file_start / num_frames) applies on this path too.
    let sliced = installed(run_nrt(NrtJob::AllocRead {
        path: path.clone(),
        file_start: 10,
        num_frames: 5,
        channels: Vec::new(),
    }));
    assert_eq!(sliced.frames(), 5);
    assert_eq!(sliced.to_vec(), original.to_vec()[10 * 2..15 * 2].to_vec());
    std::fs::remove_file(&path).ok();
}

// ---- OSC round trip: /buffer_* against a live server, engine ticked manually ----

mod osc {
    use super::*;
    use std::net::UdpSocket;

    use clausters::osc::server::{OscServer, ServerInfo};
    use clausters::rosc::{OscMessage, OscPacket, OscType, decoder, encoder};

    #[test]
    fn buffer_lifecycle_over_osc() {
        let (mut engine, engine_handle) = engine_pair(SR, CHANNELS);
        let info = ServerInfo {
            nominal_sample_rate: SR as f64,
            actual_sample_rate: SR as f64,
        };
        let mut server = OscServer::bind(("127.0.0.1", 0), info, engine_handle).unwrap();
        let addr = server.local_addr().unwrap();
        let server_thread = std::thread::spawn(move || server.run());
        let client = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        client.set_read_timeout(Some(NRT_DEADLINE)).unwrap();

        let send = |addr_str: &str, args: Vec<OscType>| {
            let packet = OscPacket::Message(OscMessage {
                addr: addr_str.into(),
                args,
            });
            client
                .send_to(&encoder::encode(&packet).unwrap(), addr)
                .unwrap();
        };
        let recv_until = |want: &str| -> OscMessage {
            let mut buf = [0u8; 65536];
            for _ in 0..100 {
                let (len, _) = client.recv_from(&mut buf).expect("reply timed out");
                if let (_, OscPacket::Message(msg)) = decoder::decode_udp(&buf[..len]).unwrap()
                    && msg.addr == want
                {
                    return msg;
                }
            }
            panic!("never received {want}");
        };

        // A 50-frame mono WAV with a known ramp.
        let path = tmp_wav("osc");
        let data: Vec<f32> = (0..50).map(|i| i as f32 / 1000.0).collect();
        run_nrt(NrtJob::Write {
            path: path.clone(),
            sample_format: "float".into(),
            buf_start: 0,
            num_frames: -1,
            buffer: Arc::new(Buffer::new(data.clone(), 1, 50, SR as f64)),
        })
        .unwrap();

        // /buffer_allocRead → /done /buffer_allocRead 0, then /buffer_query → /buffer_query.reply.
        send(
            "/buffer_allocRead",
            vec![OscType::Int(0), OscType::String(path.clone())],
        );
        let done = recv_until("/done");
        assert_eq!(done.args[0], OscType::String("/buffer_allocRead".into()));
        assert_eq!(done.args[1], OscType::Int(0));

        send("/buffer_query", vec![OscType::Int(0)]);
        let infos = recv_until("/buffer_query.reply");
        assert_eq!(
            infos.args,
            vec![
                OscType::Int(0),
                OscType::Int(50),
                OscType::Int(1),
                OscType::Float(SR),
            ]
        );

        // M30: with no argument, /buffer_query lists the allocated buffers in the
        // same four-arg shape — how a patcher discovers buffers it never
        // allocated itself (the pool outlives any one client).
        send("/buffer_query", vec![]);
        let listed = recv_until("/buffer_query.reply");
        assert_eq!(
            listed.args,
            vec![
                OscType::Int(0),
                OscType::Int(50),
                OscType::Int(1),
                OscType::Float(SR),
            ],
            "only the allocated slot is listed"
        );

        // Play it through a /def_send synth'd PlayBuf def, looping; the engine is
        // ticked from here, so the output is deterministic.
        let def = json!({
            "name": "player",
            "ugens": [
                {"kind": "PlayBuf", "inputs": [
                    {"const": 0.0}, {"const": 0.0}, {"const": 1.0}, {"const": 1.0}
                ]},
                {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 0}]}
            ]
        })
        .to_string();
        send(
            "/def_send",
            vec![
                OscType::String("synth".into()),
                OscType::Blob(def.into_bytes()),
            ],
        );
        recv_until("/done");
        send(
            "/synth_new",
            vec![
                OscType::String("player".into()),
                OscType::Int(1000),
                OscType::Int(0),
                OscType::Int(0),
            ],
        );
        // The /synth_new and the buffer install race our ticking: poll until the
        // loop comes through.
        let mut ok = false;
        for _ in 0..100 {
            let left = render_channel(&mut engine, 2, 0);
            ok = left.iter().enumerate().all(|(i, s)| *s == data[i % 50]);
            if ok && left[1] != 0.0 {
                break;
            }
        }
        assert!(ok, "looped playback must reproduce the file exactly");

        // /buffer_zero keeps the shape but silences the playback.
        send("/buffer_zero", vec![OscType::Int(0)]);
        let done = recv_until("/done");
        assert_eq!(done.args[0], OscType::String("/buffer_zero".into()));
        let mut silent = false;
        for _ in 0..100 {
            silent = render_channel(&mut engine, 2, 0).iter().all(|s| *s == 0.0);
            if silent {
                break;
            }
        }
        assert!(silent, "zeroed buffer must play silence");
        send("/buffer_query", vec![OscType::Int(0)]);
        let infos = recv_until("/buffer_query.reply");
        assert_eq!(
            infos.args[1],
            OscType::Int(50),
            "shape survives /buffer_zero"
        );

        // /buffer_free empties the slot: /buffer_query answers an absent record, which
        // is `frames = -1` (the shape of a buffer that is not there, told in
        // the record rather than as a /fail).
        send("/node_free", vec![OscType::Int(1000)]);
        send("/buffer_free", vec![OscType::Int(0)]);
        let done = recv_until("/done");
        assert_eq!(done.args[0], OscType::String("/buffer_free".into()));
        send("/buffer_query", vec![OscType::Int(0)]);
        let infos = recv_until("/buffer_query.reply");
        assert_eq!(infos.args[1], OscType::Int(-1), "absent buffer");
        // ...and the freed slot drops out of the listing form entirely.
        send("/buffer_query", vec![]);
        assert!(
            recv_until("/buffer_query.reply").args.is_empty(),
            "a freed buffer is not listed"
        );

        // Errors come back as /fail: unallocated read, bad index.
        send(
            "/buffer_read",
            vec![OscType::Int(0), OscType::String(path.clone())],
        );
        let fail = recv_until("/fail");
        assert_eq!(fail.args[0], OscType::String("/buffer_read".into()));
        send("/buffer_alloc", vec![OscType::Int(-1), OscType::Int(10)]);
        recv_until("/fail");

        // Keep ticking so freed nodes/buffers drain, then shut down.
        render_channel(&mut engine, 2, 0);
        send("/server_quit", vec![]);
        recv_until("/done");
        server_thread.join().unwrap().unwrap();
        std::fs::remove_file(&path).ok();
    }

    /// A batch of writes to one buffer, submitted before any of them completes.
    /// Each job's parse snapshots the buffer from the network-side mirror, and
    /// the mirror is behind until results are drained — so without the queue
    /// chaining them every chunk would rebuild the *pre-batch* contents and the
    /// last one installed would erase the rest. This is the shape a client's
    /// chunked `set_samples` sends, so it is the regression that matters most.
    #[test]
    fn a_batch_of_writes_does_not_erase_itself() {
        // The engine is never ticked here: this is about what the queue
        // installs, which the mirror answers for.
        let (_engine, engine_handle) = engine_pair(SR, CHANNELS);
        let info = ServerInfo {
            nominal_sample_rate: SR as f64,
            actual_sample_rate: SR as f64,
        };
        let mut server = OscServer::bind(("127.0.0.1", 0), info, engine_handle).unwrap();
        let addr = server.local_addr().unwrap();
        let server_thread = std::thread::spawn(move || server.run());
        let client = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        client.set_read_timeout(Some(NRT_DEADLINE)).unwrap();

        let send = |addr_str: &str, args: Vec<OscType>| {
            let packet = OscPacket::Message(OscMessage {
                addr: addr_str.into(),
                args,
            });
            client
                .send_to(&encoder::encode(&packet).unwrap(), addr)
                .unwrap();
        };
        let recv_until = |want: &str| -> OscMessage {
            let mut buf = [0u8; 65536];
            for _ in 0..200 {
                let (len, _) = client.recv_from(&mut buf).expect("reply timed out");
                if let (_, OscPacket::Message(msg)) = decoder::decode_udp(&buf[..len]).unwrap()
                    && msg.addr == want
                {
                    return msg;
                }
            }
            panic!("never received {want}");
        };

        send("/buffer_alloc", vec![OscType::Int(1), OscType::Int(9)]);
        recv_until("/done");

        // Three writes fired back to back, no barrier between them, each on a
        // different third of the buffer.
        for (i, run) in [[1.0f32, 2.0, 3.0], [4.0, 5.0, 6.0], [7.0, 8.0, 9.0]]
            .iter()
            .enumerate()
        {
            send(
                "/buffer_setRange",
                vec![OscType::Int(1), OscType::Int(i as i32 * 3), blob(run)],
            );
        }
        // One barrier for the batch, the way a chunked client write closes.
        send("/server_sync", vec![OscType::Int(1)]);
        recv_until("/server_sync.reply");

        send(
            "/buffer_getRange",
            vec![OscType::Int(1), OscType::Int(0), OscType::Int(9)],
        );
        let read = recv_until("/buffer_getRange.reply");
        assert_eq!(
            read.args[2],
            blob(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0]),
            "every chunk of the batch survived"
        );

        send("/server_quit", vec![]);
        recv_until("/done");
        server_thread.join().unwrap().unwrap();
    }

    /// M31(a): the read → edit → write cycle an editor view needs. What a
    /// client writes with `/buffer_set`/`/buffer_setRange` is exactly what
    /// `/buffer_getRange` reads back, and the engine plays the edited samples.
    #[test]
    fn written_samples_read_back_and_sound() {
        let (mut engine, engine_handle) = engine_pair(SR, CHANNELS);
        let info = ServerInfo {
            nominal_sample_rate: SR as f64,
            actual_sample_rate: SR as f64,
        };
        let mut server = OscServer::bind(("127.0.0.1", 0), info, engine_handle).unwrap();
        let addr = server.local_addr().unwrap();
        let server_thread = std::thread::spawn(move || server.run());
        let client = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        client.set_read_timeout(Some(NRT_DEADLINE)).unwrap();

        let send = |addr_str: &str, args: Vec<OscType>| {
            let packet = OscPacket::Message(OscMessage {
                addr: addr_str.into(),
                args,
            });
            client
                .send_to(&encoder::encode(&packet).unwrap(), addr)
                .unwrap();
        };
        let recv_until = |want: &str| -> OscMessage {
            let mut buf = [0u8; 65536];
            for _ in 0..100 {
                let (len, _) = client.recv_from(&mut buf).expect("reply timed out");
                if let (_, OscPacket::Message(msg)) = decoder::decode_udp(&buf[..len]).unwrap()
                    && msg.addr == want
                {
                    return msg;
                }
            }
            panic!("never received {want}");
        };

        // A write reads the shape from the mirror, so the alloc has to have
        // completed: the /done is that barrier.
        send("/buffer_alloc", vec![OscType::Int(2), OscType::Int(8)]);
        assert_eq!(
            recv_until("/done").args[0],
            OscType::String("/buffer_alloc".into())
        );

        // Two runs in one message, plus two single samples.
        send(
            "/buffer_setRange",
            vec![
                OscType::Int(2),
                OscType::Int(0),
                blob(&[0.1, 0.2, 0.3]),
                OscType::Int(6),
                blob(&[0.7, 0.8]),
            ],
        );
        assert_eq!(
            recv_until("/done").args[0],
            OscType::String("/buffer_setRange".into())
        );
        send(
            "/buffer_set",
            vec![
                OscType::Int(2),
                OscType::Int(4),
                OscType::Float(0.5),
                OscType::Int(5),
                OscType::Float(0.6),
            ],
        );
        assert_eq!(
            recv_until("/done").args[0],
            OscType::String("/buffer_set".into())
        );

        // The read half sees every write, and the untouched slot stayed zero.
        send(
            "/buffer_getRange",
            vec![OscType::Int(2), OscType::Int(0), OscType::Int(8)],
        );
        let read = recv_until("/buffer_getRange.reply");
        // The reply carries each range as one little-endian f32 blob.
        let values = match &read.args[2] {
            OscType::Blob(bytes) => bytes
                .chunks_exact(4)
                .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                .collect::<Vec<f32>>(),
            other => panic!("expected a blob, got {other:?}"),
        };
        assert_eq!(
            values,
            vec![0.1, 0.2, 0.3, 0.0, 0.5, 0.6, 0.7, 0.8],
            "what was written is what is read back"
        );

        // A write past the end is refused rather than clamped, and refusing it
        // leaves the buffer as it was.
        send(
            "/buffer_setRange",
            vec![OscType::Int(2), OscType::Int(7), blob(&[1.0; 4])],
        );
        assert_eq!(
            recv_until("/fail").args[0],
            OscType::String("/buffer_setRange".into())
        );
        send(
            "/buffer_getRange",
            vec![OscType::Int(2), OscType::Int(7), OscType::Int(1)],
        );
        assert_eq!(
            recv_until("/buffer_getRange.reply").args[2],
            blob(&[0.8]),
            "a refused write changes nothing"
        );

        // And the engine plays the edited buffer: the write reached the audio
        // side, not only the network-side mirror.
        let def = serde_json::to_string(&json!({
            "name": "player",
            "ugens": [
                {"kind": "PlayBuf", "inputs": [
                    {"const": 2.0}, {"const": 0.0}, {"const": 1.0}, {"const": 1.0}
                ]},
                {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 0}]}
            ]
        }))
        .unwrap();
        send(
            "/def_send",
            vec![
                OscType::String("synth".into()),
                OscType::Blob(def.into_bytes()),
            ],
        );
        recv_until("/done");
        send(
            "/synth_new",
            vec![
                OscType::String("player".into()),
                OscType::Int(1000),
                OscType::Int(0),
                OscType::Int(0),
            ],
        );
        let expected = [0.1f32, 0.2, 0.3, 0.0, 0.5, 0.6, 0.7, 0.8];
        let mut heard = false;
        for _ in 0..100 {
            let left = render_channel(&mut engine, 2, 0);
            heard = left.iter().enumerate().all(|(i, s)| *s == expected[i % 8]);
            if heard && left[1] != 0.0 {
                break;
            }
        }
        assert!(heard, "the engine must play the written samples");

        send("/node_free", vec![OscType::Int(1000)]);
        render_channel(&mut engine, 2, 0);
        send("/server_quit", vec![]);
        recv_until("/done");
        server_thread.join().unwrap().unwrap();
    }
}

/// **The milestone's own acceptance**: one synth records into a buffer while
/// another plays it, and what comes out is what went in — one buffer, two
/// nodes, no copy between them.
#[test]
fn a_synth_records_into_a_buffer_while_another_plays_it() {
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    // 128 frames, mono: two blocks of material, so the recording fills while
    // the reader is still inside the first pass.
    let buffer = Arc::new(Buffer::zeroed(BLOCK_SIZE * 2, 1, SR as f64));
    handle
        .send(Cmd::SetBuffer {
            index: 3,
            buffer: Some(Arc::clone(&buffer)),
        })
        .ok()
        .unwrap();

    // The recorder is added **first**, so it runs first in the block and the
    // player reads, in the same block, the frames it has just written. That
    // ordering is the node tree's own (depth-first, earlier nodes first) and is
    // the whole reason a shared buffer is worth having: no copy passes between
    // them, only the buffer.
    handle
        .send(add_synth(
            10,
            spec_synth(json!({
                "name": "recorder",
                "ugens": [
                    {"kind": "Line", "inputs": [
                        {"const": 1.0}, {"const": 1.0}, {"const": 100.0}, {"const": 0.0}
                    ]},
                    {"kind": "RecordBuf", "inputs": [
                        {"const": 3.0},   // bufnum
                        {"const": 0.0},   // chan
                        {"ugen": 0},      // in: a constant 1
                        {"const": 0.0},   // offset
                        {"const": 1.0},   // rec_level
                        {"const": 0.0},   // pre_level: overwrite
                        {"const": 1.0},   // run
                        {"const": 0.0},   // loop
                        {"const": 0.0},   // trigger
                        {"const": 0.0}    // done_action
                    ]}
                ]
            })),
        ))
        .ok()
        .unwrap();
    handle
        .send(add_synth(11, playbuf_spec(3.0, 0.0, 1.0, 0.0, 1.0)))
        .ok()
        .unwrap();

    let heard = render_channel(&mut engine, 3, 1);
    assert!(
        buffer
            .to_vec()
            .iter()
            .take(BLOCK_SIZE * 2)
            .all(|s| *s == 1.0),
        "the recorder filled the buffer it was given"
    );
    // Every frame the player read had been written that same block, so the
    // whole two passes are the recorded signal and not a single zero.
    assert!(
        heard[..BLOCK_SIZE * 2].iter().all(|s| *s == 1.0),
        "the player read samples the recorder wrote into the same buffer: {:?}",
        &heard[..4]
    );
}

/// `RecordBuf`'s `pre_level` is what makes it a looper rather than a tape head:
/// a second pass over the same span **adds** to what is there.
#[test]
fn recording_twice_over_a_span_overdubs_it() {
    let recorder = |pre: f32, trigger: f32| {
        json!({
            "name": "overdub",
            "ugens": [
                {"kind": "RecordBuf", "inputs": [
                    {"const": 0.0}, {"const": 0.0}, {"const": 0.25},
                    {"const": 0.0}, {"const": 1.0}, {"const": pre},
                    {"const": 1.0}, {"const": 1.0}, {"const": trigger},
                    {"const": 0.0}
                ]}
            ]
        })
    };
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    let buffer = Arc::new(Buffer::zeroed(BLOCK_SIZE, 1, SR as f64));
    handle
        .send(Cmd::SetBuffer {
            index: 0,
            buffer: Some(Arc::clone(&buffer)),
        })
        .ok()
        .unwrap();
    handle
        .send(add_synth(20, spec_synth(recorder(1.0, 0.0))))
        .ok()
        .unwrap();

    // Two blocks over a one-block buffer, looping: every frame is written
    // twice, the second time onto the first.
    render_channel(&mut engine, 2, 0);
    let held = buffer.to_vec();
    assert!(
        held.iter().all(|s| (*s - 0.5).abs() < 1e-6),
        "two passes of 0.25 added: {:?}",
        &held[..4]
    );
}

/// A non-looping recorder **stops at the end** and reports its done action, so
/// a one-shot capture frees its own node.
#[test]
fn a_recording_that_fills_the_buffer_stops_and_is_done() {
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    let buffer = Arc::new(Buffer::zeroed(BLOCK_SIZE / 2, 1, SR as f64));
    handle
        .send(Cmd::SetBuffer {
            index: 0,
            buffer: Some(Arc::clone(&buffer)),
        })
        .ok()
        .unwrap();
    handle
        .send(add_synth(
            30,
            spec_synth(json!({
                "name": "oneshot",
                "ugens": [
                    {"kind": "RecordBuf", "inputs": [
                        {"const": 0.0}, {"const": 0.0}, {"const": 1.0},
                        {"const": 0.0}, {"const": 1.0}, {"const": 0.0},
                        {"const": 1.0}, {"const": 0.0}, {"const": 0.0},
                        {"const": 2.0}
                    ]}
                ]
            })),
        ))
        .ok()
        .unwrap();

    // One block is twice the buffer: it fills, stops, and the done action (2 =
    // free this synth) takes the node with it.
    render_channel(&mut engine, 2, 0);
    let held = buffer.to_vec();
    assert!(
        held.iter().all(|s| *s == 1.0),
        "every frame was written exactly once"
    );
    handle.send(Cmd::FreeNode { id: 30 }).ok().unwrap();
    render_channel(&mut engine, 1, 0);
    assert!(
        held.iter().all(|s| *s == 1.0),
        "and nothing wrote past the end afterwards"
    );
}

/// `BufWr` writes where its phase says and passes the signal through, which is
/// what lets a chain go on using what it just recorded.
#[test]
fn bufwr_writes_at_its_phase_and_passes_the_signal_on() {
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    let buffer = Arc::new(Buffer::zeroed(8, 1, SR as f64));
    handle
        .send(Cmd::SetBuffer {
            index: 0,
            buffer: Some(Arc::clone(&buffer)),
        })
        .ok()
        .unwrap();
    handle
        .send(add_synth(
            40,
            spec_synth(json!({
                "name": "writer",
                "ugens": [
                    {"kind": "BufWr", "inputs": [
                        {"const": 0.0},  // bufnum
                        {"const": 0.0},  // chan
                        {"const": 3.0},  // phase: frame 3, held
                        {"const": 0.0},  // loop
                        {"const": 0.75}  // in
                    ]},
                    {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 0}]}
                ]
            })),
        ))
        .ok()
        .unwrap();

    let out = render_channel(&mut engine, 1, 0);
    let held = buffer.to_vec();
    assert_eq!(held[3], 0.75, "the frame the phase named");
    assert!(
        held.iter().enumerate().all(|(i, s)| i == 3 || *s == 0.0),
        "and no other: {held:?}"
    );
    assert!(
        out.iter().all(|s| (*s - 0.75).abs() < 1e-6),
        "the signal came out too"
    );
}

/// **A delay over a pool buffer**, the other half of S14's acceptance: the same
/// circular-line arithmetic the private family uses, over samples somebody else
/// can also read.
#[test]
fn a_buffer_delay_places_an_impulse_on_an_exact_frame() {
    let frames = 16usize;
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    let buffer = Arc::new(Buffer::zeroed(1024, 1, SR as f64));
    handle
        .send(Cmd::SetBuffer {
            index: 0,
            buffer: Some(Arc::clone(&buffer)),
        })
        .ok()
        .unwrap();
    handle
        .send(add_synth(
            50,
            spec_synth(json!({
                "name": "bufdelay",
                "ugens": [
                    // Frequency 0: one impulse, then silence.
                    {"kind": "Impulse", "inputs": [{"const": 0.0}]},
                    {"kind": "BufDelayN", "inputs": [
                        {"const": 0.0},                      // bufnum
                        {"const": 0.0},                      // chan
                        {"ugen": 0},                         // signal
                        {"const": frames as f32 / SR}        // delaytime
                    ]},
                    {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 1}]}
                ]
            })),
        ))
        .ok()
        .unwrap();

    let y = render_channel(&mut engine, 1, 0);
    let hit = y.iter().position(|v| *v != 0.0);
    assert_eq!(hit, Some(frames), "the impulse landed at {hit:?}");
    assert!((y[frames] - 1.0).abs() < 1e-6, "amplitude {}", y[frames]);
    // And the line is **in the buffer**, which is the whole difference from the
    // private family: what the delay is holding can be read, saved or played by
    // anything else.
    let held = buffer.to_vec();
    assert!(
        held.iter().any(|s| (*s - 1.0).abs() < 1e-6),
        "the impulse is in the buffer the line lives in"
    );
}

/// A `Buf*` delay with nowhere to write plays silence rather than guessing —
/// the same answer every other buffer UGen gives a missing buffer.
#[test]
fn a_buffer_delay_without_a_buffer_is_silent() {
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    handle
        .send(add_synth(
            60,
            spec_synth(json!({
                "name": "nowhere",
                "ugens": [
                    {"kind": "Impulse", "inputs": [{"const": 0.0}]},
                    {"kind": "BufDelayN", "inputs": [
                        {"const": 41.0}, {"const": 0.0}, {"ugen": 0}, {"const": 0.001}
                    ]},
                    {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 1}]}
                ]
            })),
        ))
        .ok()
        .unwrap();
    let y = render_channel(&mut engine, 2, 0);
    assert!(y.iter().all(|s| *s == 0.0), "silence, not garbage");
}

/// `BufCombN` over a pool line decays the way the private one does: the
/// feedback path is the same code, and only the storage differs.
#[test]
fn a_buffer_comb_repeats_and_decays() {
    let frames = 32usize;
    let (mut engine, mut handle) = engine_pair(SR, CHANNELS);
    let buffer = Arc::new(Buffer::zeroed(1024, 1, SR as f64));
    handle
        .send(Cmd::SetBuffer {
            index: 0,
            buffer: Some(Arc::clone(&buffer)),
        })
        .ok()
        .unwrap();
    handle
        .send(add_synth(
            70,
            spec_synth(json!({
                "name": "bufcomb",
                "ugens": [
                    {"kind": "Impulse", "inputs": [{"const": 0.0}]},
                    {"kind": "BufCombN", "inputs": [
                        {"const": 0.0}, {"const": 0.0}, {"ugen": 0},
                        {"const": frames as f32 / SR},
                        {"const": 1.0}
                    ]},
                    {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 1}]}
                ]
            })),
        ))
        .ok()
        .unwrap();

    // Two blocks: the second repeat lands past the first block's 64 frames.
    let y = render_channel(&mut engine, 2, 0);
    let (first, second) = (y[frames], y[frames * 2]);
    assert!(first > 0.9, "the first repeat is the impulse: {first}");
    assert!(
        second > 0.0 && second < first,
        "and the second is quieter: {second} vs {first}"
    );
}
