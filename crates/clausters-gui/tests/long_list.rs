//! **A take of an hour, drawn from its files**: what opening it, editing it
//! ten times, drawing its spectrogram and zooming into it read, measured over a real W64 of an hour of
//! stereo and its peaks.
//!
//! Ignored by default: it writes 1.4 GB, under `CARGO_TARGET_TMPDIR` on the
//! disk rather than a temporary directory that may be memory. Run it in
//! release:
//!
//! ```sh
//! cargo test --release --test long_list -- --ignored --nocapture
//! ```

use std::time::Instant;

use clausters_core::osc::{OscMessage, OscType};
use clausters_core::peaks::MultiPyramid;
use clausters_gui::host::disk::List;
use clausters_gui::host::disk::answer::{Answer, Answerer, Got, SAMPLE_BLOCK};
use clausters_gui::host::disk::native::Disk;
use serde_json::json;

const SR: usize = 48_000;
const FRAMES: usize = SR * 3600;
const CHANNELS: usize = 2;
const COLUMNS: usize = 760;

/// Writes an hour of stereo as a float W64, its peaks and the spectral
/// summary beside them, a minute at a time, so none is ever held whole.
fn write_take(path: &str, peaks: &str) {
    let mut w = clausters_core::pcm::Writer::create(path, CHANNELS, SR as f64, "float").unwrap();
    let mut stats = Vec::with_capacity(FRAMES.div_ceil(256) * CHANNELS * 3);
    let mut spectra = clausters_core::spectra::Accumulator::new(CHANNELS);
    spectra.set_channels(CHANNELS);
    let mut columns = Vec::new();
    let mut s = 1u32;
    let chunk = SR * 60;
    let mut block = vec![0.0f32; chunk * CHANNELS];
    for minute in 0..FRAMES / chunk {
        for (i, frame) in block.chunks_mut(CHANNELS).enumerate() {
            let t = (minute * chunk + i) as f32 / FRAMES as f32;
            for x in frame.iter_mut() {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                *x = ((s as f32 / u32::MAX as f32) * 2.0 - 1.0) * (0.1 + 0.8 * t);
            }
        }
        w.write(&block).unwrap();
        let mut done = |col: &[u8]| columns.extend_from_slice(col);
        for frame in block.as_chunks::<CHANNELS>().0 {
            spectra.push(|c| frame[c], &mut done);
        }
        for bucket in block.chunks(256 * CHANNELS) {
            let n = bucket.len() / CHANNELS;
            for ch in 0..CHANNELS {
                let v: Vec<f32> = (0..n).map(|f| bucket[f * CHANNELS + ch]).collect();
                let lo = v.iter().copied().fold(f32::INFINITY, f32::min);
                let hi = v.iter().copied().fold(f32::NEG_INFINITY, f32::max);
                let ms = v.iter().map(|x| f64::from(*x) * f64::from(*x)).sum::<f64>() / n as f64;
                stats.extend([lo, hi, ms as f32]);
            }
        }
    }
    w.finalize().unwrap();
    let mut pyramid = MultiPyramid::empty(FRAMES, CHANNELS, 256);
    assert!(pyramid.write_buckets(0, 256, &stats));
    pyramid.write_cache(peaks).unwrap();
    spectra.finish(&mut |col: &[u8]| columns.extend_from_slice(col));
    clausters_core::spectra::Spectra::from_columns(FRAMES, CHANNELS, &columns)
        .unwrap()
        .write(clausters_core::spectra::beside_peaks(peaks))
        .unwrap();
}

/// Bytes read and replies, over one conversation.
#[derive(Default, Debug)]
struct Cost {
    bytes: usize,
    replies: usize,
}

/// Asks `msg` through to its answer, counting what the disk read.
fn ask(answerer: &mut Answerer, disk: &mut Disk, msg: &OscMessage, cost: &mut Cost) -> OscMessage {
    for _ in 0..6 {
        match answerer.answer(msg) {
            Answer::Reply(reply) => {
                cost.replies += 1;
                return reply;
            }
            Answer::None => panic!("no answer to {msg:?}"),
            Answer::Need(needs) => {
                for need in needs {
                    let got = disk.read(&need);
                    if let Ok(Got::Bytes(b)) = &got {
                        cost.bytes += b.len();
                    }
                    answerer.deliver(&need, got);
                }
            }
        }
    }
    panic!("still reading");
}

/// The whole summary at `bucket`, walked as the fetch machine walks it.
fn summary(
    answerer: &mut Answerer,
    disk: &mut Disk,
    key: i32,
    bucket: usize,
    cost: &mut Cost,
) -> usize {
    let mut next = 0i64;
    let mut buckets = 0;
    loop {
        let msg = OscMessage {
            addr: "/buffer_summary".into(),
            args: vec![
                OscType::Int(key),
                OscType::Int(bucket as i32),
                OscType::Long(next),
                OscType::Int(-1),
            ],
        };
        let reply = ask(answerer, disk, &msg, cost);
        let OscType::Blob(blob) = &reply.args[3] else {
            panic!("{reply:?}");
        };
        let n = blob.len() / 4 / (CHANNELS * 3);
        if n == 0 {
            return buckets;
        }
        buckets += n;
        next += (n * bucket) as i64;
    }
}

fn part(path: &str, peaks: &str, start: usize, frames: usize, fade: usize) -> serde_json::Value {
    json!({"path": path, "peaks": peaks, "start": start, "frames": frames,
           "fade_in": fade, "fade_out": fade, "source_channels": CHANNELS,
           "source_frames": FRAMES, "source_rate": SR})
}

/// The coarsest bucket that gives each of `COLUMNS` at least one, as the
/// fetch machine picks it.
fn level(frames: usize) -> usize {
    let mut bucket = 256;
    while bucket * 2 <= frames / COLUMNS {
        bucket *= 2;
    }
    bucket
}

/// How long a list of `parts` is.
fn answerer_frames(parts: &[serde_json::Value]) -> usize {
    parts
        .iter()
        .map(|p| p["frames"].as_u64().unwrap() as usize)
        .sum()
}

#[test]
#[ignore = "writes 1.4 GB; run in release with --ignored"]
fn an_hour_is_opened_edited_and_zoomed_by_what_its_window_resolves() {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("long_list");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("hour.w64").to_string_lossy().into_owned();
    let peaks = dir.join("hour.peaks").to_string_lossy().into_owned();
    let spectra = clausters_core::spectra::beside_peaks(&peaks);
    if !std::path::Path::new(&spectra).exists() {
        let t = Instant::now();
        write_take(&path, &peaks);
        println!(
            "wrote an hour of stereo, its peaks and its spectral summary in {:.1} s",
            t.elapsed().as_secs_f64()
        );
    }
    let mut answerer = Answerer::default();
    let mut disk = Disk::default();

    // **Open**: the list is the file whole.
    let whole = json!({"channels": CHANNELS, "sample_rate": SR,
                       "parts": [part(&path, &peaks, 0, FRAMES, 0)]});
    answerer.define(-2, List::parse(&whole).unwrap());
    let mut open = Cost::default();
    let t = Instant::now();
    let query = OscMessage {
        addr: "/buffer_query".into(),
        args: vec![OscType::Int(-2)],
    };
    ask(&mut answerer, &mut disk, &query, &mut open);
    let bucket = level(FRAMES);
    let buckets = summary(&mut answerer, &mut disk, -2, bucket, &mut open);
    let opened = t.elapsed();
    println!(
        "open: {buckets} buckets of {bucket} in {:.1} ms, {} replies, {} kB read, {} samples read",
        opened.as_secs_f64() * 1e3,
        open.replies,
        open.bytes / 1000,
        answerer.samples_read
    );
    assert_eq!(answerer.samples_read, 0, "opening reads no samples");
    assert!(open.bytes < 2_000_000, "opening read {} bytes", open.bytes);

    // **Ten edits**: each a paste of half an hour at a place off the grid,
    // with 10 ms fades -- a new list over the same file, drawn at its level.
    let mut parts = vec![part(&path, &peaks, 0, FRAMES, 0)];
    let fade = SR / 100;
    let mut edits = Cost::default();
    let read_before = answerer.samples_read;
    // What a pass may read from samples: each part's two fades and its two
    // cut edges, a base bucket each at most, on every channel.
    let mut bound = 0;
    let t = Instant::now();
    for n in 0..10 {
        let at = (n * 7_919_017 + 123_457) % FRAMES;
        let mut flat = Vec::new();
        let mut placed = 0;
        let mut done = false;
        for p in parts.drain(..) {
            let frames = p["frames"].as_u64().unwrap() as usize;
            let start = p["start"].as_u64().unwrap() as usize;
            if !done && placed + frames > at {
                let cut = at - placed;
                if cut > 0 {
                    flat.push(part(&path, &peaks, start, cut, fade));
                }
                flat.push(part(&path, &peaks, 1_000_003 * (n + 1), SR * 1800, fade));
                flat.push(part(&path, &peaks, start + cut, frames - cut, fade));
                done = true;
            } else {
                flat.push(p);
            }
            placed += frames;
        }
        parts = flat;
        let list = json!({"channels": CHANNELS, "sample_rate": SR, "parts": parts.clone()});
        let list = List::parse(&list).unwrap();
        let frames = list.frames;
        bound += parts.len() * 2 * (fade + 256) * CHANNELS;
        answerer.define(-2, list);
        summary(&mut answerer, &mut disk, -2, level(frames), &mut edits);
    }
    let edited = t.elapsed();
    let read = answerer.samples_read - read_before;
    println!(
        "ten edits: {:.1} ms, {} replies, {} kB read, {read} samples read (fades and cuts), {} parts",
        edited.as_secs_f64() * 1e3,
        edits.replies,
        edits.bytes / 1000,
        parts.len()
    );
    assert!(
        read <= bound,
        "{read} samples read over ten edits, {bound} allowed"
    );
    // From the disk, only the blocks under the new seams: the peaks the
    // window resolves were read when it opened. An edit makes three parts
    // with two ends each, and an end's fade may straddle two sample blocks.
    let seams = 10 * 6 * 2 * SAMPLE_BLOCK * CHANNELS * 4;
    assert!(edits.bytes < seams + 2_000_000, "{} bytes", edits.bytes);

    // **Its spectrogram**: 760 columns over the whole of the last list, as
    // the view zoomed out asks them, each the largest of what happened in
    // the frames it stands for: folded from the files' spectral summaries,
    // so what is read is the summaries' columns and no sample, whatever the
    // take's length.
    let mut spectrum = Cost::default();
    let read_before = answerer.samples_read;
    let t = Instant::now();
    let total = answerer_frames(&parts);
    let hop = total / COLUMNS;
    let mut columns = 0;
    while columns < COLUMNS {
        let msg = OscMessage {
            addr: "/buffer_spectrum".into(),
            args: vec![
                OscType::Int(-2),
                OscType::Long((columns * hop) as i64),
                OscType::Int(hop as i32),
                OscType::Int((COLUMNS - columns) as i32),
                OscType::Int(clausters_core::spectra::WINDOW as i32),
            ],
        };
        let reply = ask(&mut answerer, &mut disk, &msg, &mut spectrum);
        let OscType::Blob(blob) = &reply.args[5] else {
            panic!("{reply:?}");
        };
        let came = blob.len() / (CHANNELS * clausters_core::spectra::BINS);
        assert!(came > 0, "an answer of no columns");
        columns += came;
    }
    println!(
        "spectrogram: {COLUMNS} columns in {:.1} ms, {} replies, {} kB read",
        t.elapsed().as_secs_f64() * 1e3,
        spectrum.replies,
        spectrum.bytes / 1000
    );
    assert_eq!(answerer.samples_read - read_before, 0, "no sample read");
    assert!(answerer.columns_summarized >= COLUMNS);
    // The summaries' columns, a level a column at most, a block of them at a
    // time.
    let summary_bytes = std::fs::metadata(&spectra).unwrap().len() as usize;
    assert!(
        spectrum.bytes <= summary_bytes,
        "{} bytes of {summary_bytes}",
        spectrum.bytes
    );

    // **Zoom to the samples**: 4096 frames in the middle of the last list.
    let mut zoom = Cost::default();
    let t = Instant::now();
    let range = OscMessage {
        addr: "/buffer_getRange".into(),
        args: vec![
            OscType::Int(-2),
            OscType::Int((FRAMES * CHANNELS) as i32),
            OscType::Int(4096),
        ],
    };
    ask(&mut answerer, &mut disk, &range, &mut zoom);
    println!(
        "zoom: 4096 samples in {:.2} ms, {} kB read",
        t.elapsed().as_secs_f64() * 1e3,
        zoom.bytes / 1000
    );
    assert!(
        zoom.bytes <= 2 * 16_384 * CHANNELS * 4,
        "a zoom read {} bytes",
        zoom.bytes
    );
}
