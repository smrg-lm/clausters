//! **What answering a take's overview costs**, out of the buffer's kept
//! summary and, for comparison, out of the samples -- the two ways
//! `/buffer_peaks` has answered.
//!
//! A ten-minute stereo take, asked the way a view asks: the whole of it at the
//! base bucket and at the bucket a 760-column window draws, the first time and
//! again, then a paste's join over it (the head, a second pasted from
//! elsewhere with fades, the tail shifted off the grid) at the window's bucket
//! and, zoomed in, at the base bucket over one window's width.
//!
//! Ignored by default, because it is a measurement and not an assertion:
//! `cargo test --release --test summary_cost -- --ignored --nocapture`. The
//! engine in a page runs the same code as wasm; both sets of numbers are in
//! `docs/decisions.md`, "A take's overview is answered from a summary kept on
//! the buffer".

use std::sync::Arc;
use std::time::Instant;

use clausters::clausters_core::peaks::{Pyramid, overview};
use clausters::dsp::buffer::Buffer;
use clausters::dsp::stitch::{FadeShape, PartSpec, Stitch};

const SR: f64 = 48_000.0;
const FRAMES: usize = 10 * 60 * 48_000;
const COLUMNS: usize = 760;

fn take() -> Arc<Buffer> {
    let mut s = 12_345u32;
    let samples = (0..FRAMES * 2)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            (s as f32 / u32::MAX as f32) * 2.0 - 1.0
        })
        .collect();
    Arc::new(Buffer::new(samples, 2, FRAMES, SR))
}

fn part(src: &Arc<Buffer>, src_start: usize, frames: usize, fade: usize) -> PartSpec {
    PartSpec {
        src: Arc::clone(src),
        src_index: 0,
        src_start,
        frames,
        fade_in: fade,
        fade_out: fade,
        fade_in_shape: FadeShape::LINEAR,
        fade_out_shape: FadeShape::LINEAR,
        map: vec![0, 1],
    }
}

/// Every bucket of `bucket` frames over `a..b`, out of the summary: the time
/// and the samples it read.
fn kept(buffer: &Buffer, bucket: usize, a: usize, b: usize) -> (f64, usize) {
    let t = Instant::now();
    let mut read = 0;
    let mut at = a;
    while at + bucket <= b {
        for ch in 0..2 {
            std::hint::black_box(buffer.summary_span(ch, at, at + bucket, &mut read));
        }
        at += bucket;
    }
    (t.elapsed().as_secs_f64() * 1e3, read)
}

/// The same, out of the samples, as the command answered before.
fn samples(buffer: &Buffer, bucket: usize, a: usize, b: usize) -> f64 {
    let t = Instant::now();
    let channels: Vec<_> = (0..2).map(|ch| buffer.channel(ch)).collect();
    let sources: Vec<_> = channels.iter().collect();
    std::hint::black_box(overview(&sources, a, bucket, (b - a) / bucket));
    t.elapsed().as_secs_f64() * 1e3
}

#[test]
#[ignore = "a measurement; run with --release --ignored --nocapture"]
fn what_an_overview_costs() {
    let src = take();
    let coarse = 1usize << (FRAMES / COLUMNS).ilog2();
    let row = |what: &str, (ms, read): (f64, usize), before: f64| {
        println!(
            "{what:<44} {ms:>8.1} ms {read:>11} samples read   (from samples: {before:.1} ms)"
        );
    };

    let fresh = take();
    row(
        &format!("open at {coarse}, whole"),
        kept(&fresh, coarse, 0, FRAMES),
        samples(&fresh, coarse, 0, FRAMES),
    );
    row(
        "open at 256, whole",
        kept(&src, 256, 0, FRAMES),
        samples(&src, 256, 0, FRAMES),
    );
    row(
        "again at 256, whole",
        kept(&src, 256, 0, FRAMES),
        samples(&src, 256, 0, FRAMES),
    );
    row(
        &format!("again at {coarse}, whole"),
        kept(&src, coarse, 0, FRAMES),
        samples(&src, coarse, 0, FRAMES),
    );

    let (cut, from, len) = (1_234_567, 3_000_001, 48_001);
    let t = Instant::now();
    let join = Buffer::stitched(
        Stitch::new(
            vec![
                part(&src, 0, cut, 0),
                part(&src, from, len, 64),
                part(&src, cut, FRAMES - cut, 0),
            ],
            2,
            SR,
        )
        .unwrap(),
        2,
        SR,
    );
    println!(
        "{:<44} {:>8.3} ms",
        "the paste's join, built",
        t.elapsed().as_secs_f64() * 1e3
    );
    let frames = join.frames();
    // What a server with a segment pays besides: it copies the join into a
    // region a peer maps and builds that region's overview from it -- once
    // through the join, sample by sample, as it did, and once part by part
    // into the cells, then from the cells, as it does.
    let t = Instant::now();
    let copy = join.to_vec();
    let file: Vec<_> = (0..2)
        .map(|ch| Pyramid::build_from(&join.channel(ch), 256))
        .collect();
    std::hint::black_box((&copy, &file));
    let before = t.elapsed().as_secs_f64() * 1e3;
    let t = Instant::now();
    let region = Buffer::zeroed(frames, 2, SR);
    join.stitch().unwrap().copy_into(region.cells().unwrap(), 2);
    let file: Vec<_> = (0..2)
        .map(|ch| Pyramid::build_from(&region.channel(ch), 256))
        .collect();
    std::hint::black_box(&file);
    println!(
        "{:<44} {:>8.1} ms                           (sample by sample: {before:.1} ms)",
        "the join's region copy and its overview",
        t.elapsed().as_secs_f64() * 1e3
    );
    row(
        &format!("join at {coarse}, whole"),
        kept(&join, coarse, 0, frames),
        samples(&join, coarse, 0, frames),
    );
    let view = (cut - COLUMNS * 128, cut + COLUMNS * 128);
    row(
        "join at 256, one window at the cut",
        kept(&join, 256, view.0, view.1),
        samples(&join, 256, view.0, view.1),
    );
    row(
        "join at 256, whole",
        kept(&join, 256, 0, frames),
        samples(&join, 256, 0, frames),
    );
}
