//! A segment list whose parts name files, played from disk by
//! `TransportDiskIn`, against the same list over pool buffers played by
//! `TransportBufRd`: same logic, different location, so the two must hear
//! the same frames.
//!
//! The engine is paced in real time where a test is about keeping up (a
//! block, then the block's duration of sleep), since the reader's thread
//! races a real audio thread and not one that runs as fast as it can.
#![cfg(all(feature = "synth", not(target_arch = "wasm32")))]

use std::sync::Arc;
use std::time::Duration;

use clausters::clausters_core::rng::SEED_STRIDE;
use clausters::dsp::buffer::Buffer;
use clausters::node::{AddAction, ROOT_NODE_ID};
use clausters::server::engine::{BLOCK_SIZE, Cmd, Engine, EngineHandle, engine_pair};
use clausters::server::meters::Meters;
use clausters::server::nrt::{NrtAction, run_job};
use clausters::synthdef::instance::UGenSynth;
use clausters::synthdef::{SynthDefSpec, compile};
use rosc::OscType;

const SR: f64 = 48_000.0;
const QUARTER: usize = 12_000;
const BLOCK: Duration = Duration::from_micros(1_333);

/// A directory of this test's own, emptied first.
fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "clausters-transport-disk-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A mono float file whose every frame is told apart from every other: file
/// `k`'s frame `n` is `(k + 1) / 8 + n / 2^22`.
fn take(dir: &std::path::Path, k: usize, frames: usize) -> (String, Vec<f32>) {
    let values: Vec<f32> = (0..frames)
        .map(|n| (k + 1) as f32 / 8.0 + n as f32 / (1 << 22) as f32)
        .collect();
    let path = dir.join(format!("take{k}.wav"));
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SR as u32,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::create(&path, spec).unwrap();
    for &v in &values {
        w.write_sample(v).unwrap();
    }
    w.finalize().unwrap();
    (path.to_string_lossy().into_owned(), values)
}

/// One part's arguments: a source (a path or a buffer number), a span, fades
/// of `fade` frames and a mono map.
fn part(src: OscType, start: usize, frames: usize, fade: i32) -> Vec<OscType> {
    vec![
        src,
        OscType::Int(start as i32),
        OscType::Int(frames as i32),
        OscType::Int(fade),
        OscType::Int(fade),
        OscType::Int(1),
        OscType::Float(0.0),
        OscType::Int(1),
        OscType::Float(0.0),
        OscType::Int(0),
    ]
}

/// The buffer `/buffer_stitch` installs for `parts`, parsed against `pool`.
fn stitch(pool: &[Option<Arc<Buffer>>], parts: Vec<Vec<OscType>>) -> Arc<Buffer> {
    let mut args = vec![OscType::Int(9), OscType::Int(1), OscType::Float(SR as f32)];
    args.extend(parts.into_iter().flatten());
    // The target number has to be inside the pool.
    let mut mirror = pool.to_vec();
    mirror.resize(16, None);
    let (_, job) =
        clausters::osc::translate::parse_buffer_msg("/buffer_stitch", &args, &mirror, SR)
            .expect("parsed");
    match run_job(job).expect("stitched") {
        NrtAction::Install(b) => b,
        _ => panic!("a stitch installs a buffer"),
    }
}

/// The two readers: `TransportDiskIn` over buffer 0 to bus 0 and
/// `TransportBufRd` over buffer 1 to bus 1, both from frame 0 at transport 0.
fn readers(handle: &mut EngineHandle, meters: &Arc<Meters>, looping: bool) {
    let flag = if looping { 1.0 } else { 0.0 };
    let json = format!(
        r#"{{"name": "pair", "ugens": [
            {{"kind": "TransportDiskIn", "inputs": [{{"const": 0.0}}, {{"const": 0.0}},
                {{"const": 0.0}}, {{"const": 0.0}}, {{"const": {flag}}}]}},
            {{"kind": "Out", "inputs": [{{"const": 0.0}}, {{"ugen": 0}}]}},
            {{"kind": "TransportBufRd", "inputs": [{{"const": 1.0}}, {{"const": 0.0}},
                {{"const": 0.0}}, {{"const": 1.0}}, {{"const": 0.0}}, {{"const": {flag}}}]}},
            {{"kind": "Out", "inputs": [{{"const": 1.0}}, {{"ugen": 2}}]}}
        ]}}"#
    );
    let spec: SynthDefSpec = serde_json::from_str(&json).unwrap();
    let synth = UGenSynth::metered(
        Arc::new(compile(spec).unwrap()),
        SR as f32,
        SEED_STRIDE,
        Arc::clone(meters),
    );
    handle
        .send(Cmd::AddSynth {
            id: 1000,
            target: ROOT_NODE_ID,
            action: AddAction::Tail,
            synth: Box::new(synth),
            usage: Default::default(),
        })
        .ok()
        .unwrap();
}

fn install(handle: &mut EngineHandle, index: usize, buffer: Arc<Buffer>) {
    handle
        .send(Cmd::SetBuffer {
            index,
            buffer: Some(buffer),
        })
        .ok()
        .unwrap();
}

fn roll(handle: &mut EngineHandle, position: Option<u64>) {
    if let Some(position) = position {
        handle
            .send(Cmd::TransportLocate {
                transport: 0,
                position,
            })
            .ok()
            .unwrap();
    }
    handle
        .send(Cmd::TransportRun {
            transport: 0,
            rolling: true,
        })
        .ok()
        .unwrap();
}

/// `blocks` blocks of (disk, reference), paced in real time when `paced`.
fn play(engine: &mut Engine, blocks: usize, paced: bool) -> (Vec<f32>, Vec<f32>) {
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    let (mut disk, mut reference) = (Vec::new(), Vec::new());
    for _ in 0..blocks {
        engine.process_block(&mut out);
        for f in out.as_chunks::<2>().0 {
            disk.push(f[0]);
            reference.push(f[1]);
        }
        if paced {
            std::thread::sleep(BLOCK);
        }
    }
    (disk, reference)
}

/// Where the disk reader first sounds, and that from there on it plays
/// exactly what the reference plays.
fn agrees_from_its_first_frame(disk: &[f32], reference: &[f32]) -> usize {
    let first = disk
        .iter()
        .position(|&s| s != 0.0)
        .expect("the reader never sounded");
    for i in first..disk.len() {
        assert_eq!(
            disk[i], reference[i],
            "sample {i}: the disk reader plays {} and the list in memory {}",
            disk[i], reference[i]
        );
    }
    first
}

/// The same list twice: over three files, and over three pool buffers
/// holding the same samples.
fn quarter_list(dir: &std::path::Path, fade: i32) -> (Arc<Buffer>, Arc<Buffer>) {
    let takes: Vec<(String, Vec<f32>)> = (0..3).map(|k| take(dir, k, 4 * QUARTER)).collect();
    let pool: Vec<Option<Arc<Buffer>>> = takes
        .iter()
        .map(|(_, v)| Some(Arc::new(Buffer::new(v.clone(), 1, v.len(), SR))))
        .collect();
    // Eight quarter-second parts, from the three takes in turn and from
    // different places in each.
    let order = [
        (0, 1),
        (1, 0),
        (2, 3),
        (0, 2),
        (1, 3),
        (2, 0),
        (0, 0),
        (1, 1),
    ];
    let on_disk = stitch(
        &pool,
        order
            .iter()
            .map(|&(k, q)| {
                part(
                    OscType::String(takes[k].0.clone()),
                    q * QUARTER,
                    QUARTER,
                    fade,
                )
            })
            .collect(),
    );
    let in_memory = stitch(
        &pool,
        order
            .iter()
            .map(|&(k, q)| part(OscType::Int(k as i32), q * QUARTER, QUARTER, fade))
            .collect(),
    );
    assert!(on_disk.reads_from_disk() && !in_memory.reads_from_disk());
    (on_disk, in_memory)
}

/// **A list of quarter-second parts from different files plays without a
/// click**: from its first sounding frame on, every sample is the one the same
/// list in memory plays -- seams and fades included -- and nothing ran out of
/// lead.
#[test]
fn a_list_of_files_plays_what_the_same_list_in_memory_plays() {
    let dir = scratch("quarters");
    let (on_disk, in_memory) = quarter_list(&dir, 64);
    let (mut engine, mut handle) = engine_pair(SR as f32, 2);
    let meters = Meters::detached();
    install(&mut handle, 0, on_disk);
    install(&mut handle, 1, in_memory);
    readers(&mut handle, &meters, false);
    roll(&mut handle, None);
    // Two seconds: all eight parts and past the end.
    let (disk, reference) = play(&mut engine, 2 * 48_000 / BLOCK_SIZE + 40, true);
    // Past its end the list is silent, where `TransportBufRd` holds its last
    // frame.
    let end = 8 * QUARTER;
    assert!(disk[end..].iter().all(|&s| s == 0.0), "silent past the end");
    let first = agrees_from_its_first_frame(&disk[..end], &reference[..end]);
    assert!(
        first <= 4 * BLOCK_SIZE,
        "the first refill took {first} samples"
    );
    assert_eq!(meters.starved(), 0, "nothing ran out of lead");
    let _ = std::fs::remove_dir_all(&dir);
}

/// **A locate under a moving playhead lands on silence and never on a stale
/// frame**: every sample after it is either silence or the frame the list in
/// memory plays there, the silence is short, and it ends in agreement.
#[test]
fn a_locate_lands_on_silence_and_never_on_a_stale_frame() {
    let dir = scratch("locate");
    let (on_disk, in_memory) = quarter_list(&dir, 0);
    let (mut engine, mut handle) = engine_pair(SR as f32, 2);
    let meters = Meters::detached();
    install(&mut handle, 0, on_disk);
    install(&mut handle, 1, in_memory);
    readers(&mut handle, &meters, false);
    roll(&mut handle, None);
    play(&mut engine, 100, true);
    let mut gaps = Vec::new();
    for (n, to) in [5 * QUARTER as u64 + 777, 1_000, 3 * QUARTER as u64 - 5]
        .into_iter()
        .enumerate()
    {
        roll(&mut handle, Some(to));
        let (disk, reference) = play(&mut engine, 80, true);
        for (i, (&d, &r)) in disk.iter().zip(&reference).enumerate() {
            assert!(
                d == 0.0 || d == r,
                "locate {n}, sample {i}: {d} is a stale frame (the list plays {r})"
            );
        }
        gaps.push(agrees_from_its_first_frame(&disk, &reference));
    }
    let worst = *gaps.iter().max().unwrap();
    assert!(
        worst <= 4 * BLOCK_SIZE,
        "a locate was silent for {worst} samples ({gaps:?})"
    );
    assert_eq!(meters.starved(), 0, "a locate is not running out of lead");
    let _ = std::fs::remove_dir_all(&dir);
}

/// **Running out of lead is silence, counted**: an engine that runs as fast
/// as it can outruns any disk, and what it cannot read is silence -- never a
/// stale frame -- and the server says how much there was.
#[test]
fn running_out_of_lead_is_counted_silence() {
    let dir = scratch("starved");
    let (on_disk, in_memory) = quarter_list(&dir, 0);
    let (mut engine, mut handle) = engine_pair(SR as f32, 2);
    let meters = Meters::detached();
    install(&mut handle, 0, on_disk);
    install(&mut handle, 1, in_memory);
    readers(&mut handle, &meters, true);
    roll(&mut handle, None);
    let (disk, reference) = play(&mut engine, 20_000, false);
    for (i, (&d, &r)) in disk.iter().zip(&reference).enumerate() {
        assert!(
            d == 0.0 || d == r,
            "sample {i}: {d} where the list plays {r}"
        );
    }
    assert!(meters.starved() > 0, "an unpaced engine starves the reader");
    let _ = std::fs::remove_dir_all(&dir);
}

/// **An hour of list is a few numbers**: a list of 3600 one-second parts of
/// one file plays from deep inside it, where every frame index is past what
/// an `f32` counts, and what it plays is the frame the list holds there.
#[test]
fn an_hour_of_list_plays_from_anywhere_in_it() {
    let dir = scratch("hour");
    let (path, values) = take(&dir, 0, 48_000);
    let parts = (0..3600)
        .map(|_| part(OscType::String(path.clone()), 0, 48_000, 0))
        .collect();
    let list = stitch(&[], parts);
    assert_eq!(list.frames(), 3600 * 48_000);
    let (mut engine, mut handle) = engine_pair(SR as f32, 2);
    let meters = Meters::detached();
    install(&mut handle, 0, list);
    readers(&mut handle, &meters, false);
    let deep = 3_500 * 48_000 + 12_345;
    roll(&mut handle, Some(deep));
    let (disk, _) = play(&mut engine, 60, true);
    let first = disk.iter().position(|&s| s != 0.0).expect("it sounded");
    for (i, &d) in disk.iter().enumerate().skip(first) {
        let frame = (deep as usize + i) % 48_000;
        assert_eq!(d, values[frame], "sample {i}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// **A list that reads a file is refused by every random-access read**, and
/// is not the source of another list.
#[test]
fn a_list_with_a_file_part_is_refused_where_it_would_be_read_at_random() {
    let dir = scratch("refused");
    let (path, values) = take(&dir, 0, 1_000);
    let list = stitch(&[], vec![part(OscType::String(path), 0, 1_000, 0)]);
    let mut pool = vec![
        Some(list),
        Some(Arc::new(Buffer::new(values.clone(), 1, 1_000, SR))),
    ];
    pool.resize(16, None);
    let nested = clausters::osc::translate::parse_buffer_msg(
        "/buffer_stitch",
        &[
            vec![OscType::Int(2), OscType::Int(1), OscType::Float(SR as f32)],
            part(OscType::Int(0), 0, 10, 0),
        ]
        .concat(),
        &pool,
        SR,
    )
    .and_then(|(_, job)| run_job(job).map(|_| ()));
    assert!(
        nested.is_err_and(|e| e.contains("not a source of another list")),
        "a list that reads a file is not nested"
    );
    let copy = clausters::osc::translate::parse_buffer_gen(
        &[
            OscType::Int(1),
            OscType::String("copy".into()),
            OscType::Int(0),
            OscType::Int(0),
            OscType::Int(0),
            OscType::Int(10),
        ],
        &pool,
        SR,
    );
    assert!(
        copy.is_err_and(|e| e.contains("reads a file")),
        "a copy out of it is refused by name"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// **The two numbers the reader states, measured**: how long a locate is
/// silent, and what opening a part's file and reading its first span costs --
/// the cost the seam lead has to cover. `--ignored`, since it prints rather
/// than asserts.
#[test]
#[ignore = "a measurement: prints the locate silence and the cost of a seam"]
fn measure_the_locate_silence_and_the_cost_of_a_seam() {
    let dir = scratch("measure");
    let (on_disk, in_memory) = quarter_list(&dir, 0);
    let (mut engine, mut handle) = engine_pair(SR as f32, 2);
    let meters = Meters::detached();
    install(&mut handle, 0, on_disk);
    install(&mut handle, 1, in_memory);
    readers(&mut handle, &meters, true);
    roll(&mut handle, None);
    play(&mut engine, 100, true);
    let mut gaps = Vec::new();
    for n in 0..60u64 {
        roll(&mut handle, Some((n * 7_919) % (8 * QUARTER as u64)));
        let (disk, reference) = play(&mut engine, 30, true);
        gaps.push(agrees_from_its_first_frame(&disk, &reference));
    }
    gaps.sort_unstable();
    println!(
        "locate silence in samples: min {} median {} max {} (a block is {BLOCK_SIZE})",
        gaps[0],
        gaps[gaps.len() / 2],
        gaps[gaps.len() - 1]
    );
    let (path, _) = take(&dir, 5, 4 * QUARTER);
    let mut costs = Vec::new();
    for _ in 0..200 {
        let t = std::time::Instant::now();
        let layout = clausters::dsp::pcm::Layout::open(&path).unwrap();
        let mut reader = clausters::dsp::pcm::Reader::open(&layout).unwrap();
        let mut out = Vec::new();
        reader.read(QUARTER as u64, 4096, &mut out).unwrap();
        costs.push(t.elapsed().as_micros());
    }
    costs.sort_unstable();
    println!(
        "open and read 4096 frames: median {} us, max {} us; starved {}",
        costs[costs.len() / 2],
        costs[costs.len() - 1],
        meters.starved()
    );
    let _ = std::fs::remove_dir_all(&dir);
}
