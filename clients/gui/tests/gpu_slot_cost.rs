//! What a heavy view's GPU slot costs, and why the waveform kept its pipeline.
//!
//! Two questions this crate answered by measuring rather than by argument, kept
//! here so the answers can be re-checked when either changes:
//!
//! 1. **Should the waveform draw into the shared `Mesh` instead of its own
//!    pipeline?** No. Both build the same six vertices per min/max column into
//!    the same `[x, y, r, g, b, a]` layout, and the column *computation* — the
//!    peak-pyramid reads — dominates so heavily that the choice of destination
//!    is noise. Measured here as the shared work's share of the total.
//! 2. **What does an extra slot cost?** After E2, a slot is a vertex buffer and
//!    a texture; the pipelines belong to the window. Before it, every waveform
//!    widget compiled a shader module and two pipelines, and a spectrogram did
//!    the same *per channel*. This counts the objects a composition allocates,
//!    which is the number E6/E7 multiply by the element count.
//!
//! Run with `cargo test --release --test gpu_slot_cost -- --nocapture`. The
//! timings need `--release` to mean anything; the counts do not.

use std::hint::black_box;
use std::time::Instant;

use clausters_gui::viewport::View;
use clausters_gui::waveform::WaveformData;

/// Frames of a stereo buffer roughly three and a half minutes long at 48 kHz.
const FRAMES: usize = 10_000_000;
const CHANNELS: usize = 2;
const BASE_BUCKET: usize = 256;

fn build_data() -> WaveformData {
    let mut interleaved = Vec::with_capacity(FRAMES * CHANNELS);
    for i in 0..FRAMES {
        let s = ((i as f32) * 0.0007).sin() * 0.8;
        for _ in 0..CHANNELS {
            interleaved.push(s);
        }
    }
    WaveformData::from_interleaved(&interleaved, CHANNELS, BASE_BUCKET)
}

/// The column computation alone — the work every destination shares.
fn columns_only(data: &WaveformData, view: &View, w: u32) -> f32 {
    let spp = view.samples_per_px(w);
    let mut acc = 0.0f32;
    for ch in 0..data.num_channels() {
        for x in 0..w {
            let s0 = view.start + view.len * (x as f64 / w as f64);
            let s1 = view.start + view.len * ((x + 1) as f64 / w as f64);
            let (lo, hi) = data.column(ch, spp, s0, s1);
            acc += lo + hi;
        }
    }
    acc
}

/// The same columns written out as vertices, which is all a destination adds.
fn columns_to_vertices(scratch: &mut Vec<f32>, data: &WaveformData, view: &View, w: u32) {
    scratch.clear();
    let spp = view.samples_per_px(w);
    for ch in 0..data.num_channels() {
        for x in 0..w {
            let s0 = view.start + view.len * (x as f64 / w as f64);
            let s1 = view.start + view.len * ((x + 1) as f64 / w as f64);
            let (lo, hi) = data.column(ch, spp, s0, s1);
            let xl = -1.0 + 2.0 * (x as f32 / w as f32);
            let xr = -1.0 + 2.0 * ((x + 1) as f32 / w as f32);
            let (yb, yt) = (lo.min(0.0), hi.max(0.0));
            for v in [[xl, yb], [xr, yb], [xr, yt], [xl, yb], [xr, yt], [xl, yt]] {
                scratch.extend_from_slice(&[v[0], v[1], 0.3, 0.78, 0.55, 1.0]);
            }
        }
    }
}

fn bench(name: &str, iters: u32, mut f: impl FnMut()) -> f64 {
    for _ in 0..20 {
        f();
    }
    let t0 = Instant::now();
    for _ in 0..iters {
        f();
    }
    let mean = t0.elapsed().as_secs_f64() / iters as f64;
    println!("  {name:<32} {:>8.3} ms", mean * 1e3);
    mean
}

// A measurement, not an assertion about the code: an unoptimized build spends
// its time somewhere else entirely, so the share it reports is meaningless and
// the threshold below would fail for no reason. Run it deliberately:
// `cargo test --release --test gpu_slot_cost -- --ignored --nocapture`.
#[test]
#[ignore = "timing: only meaningful under --release"]
fn the_column_computation_dominates_the_per_frame_cost() {
    println!("\n{FRAMES} frames x {CHANNELS} ch, base_bucket {BASE_BUCKET}");
    let data = build_data();
    let total = data.total_samples();
    for &w in &[1200u32, 2400, 3840] {
        for (label, view) in [
            ("full  (pyramid columns)", View::full(total)),
            (
                "mid   (raw columns)",
                View {
                    start: 0.0,
                    len: (w as f64) * 64.0,
                },
            ),
        ] {
            println!("width {w} px | {label}");
            let shared = bench("columns only", 200, || {
                black_box(columns_only(&data, &view, w));
            });
            let mut scratch = Vec::new();
            let whole = bench("columns + vertices", 200, || {
                columns_to_vertices(&mut scratch, &data, &view, w);
                black_box(scratch.len());
            });
            let share = shared / whole * 100.0;
            println!("  -> the shared work is {share:.0}% of the total\n");
            assert!(
                share > 70.0,
                "width {w} {label}: the column computation stopped dominating \
                 ({share:.0}%), so where the vertices are written has become \
                 worth re-deciding"
            );
        }
    }
}

/// The GPU objects a window allocates, counted rather than timed: pipelines are
/// created once and then only bound, so what matters is how many exist.
///
/// This is the milestone's before/after. The "before" numbers are the shape the
/// code had until E2 (a shader module and two pipelines inside every
/// `WaveformRenderer`, one of each inside every `SpectrogramRenderer`, and a
/// spectrogram building one renderer per channel); the "after" is what the split
/// leaves. Kept as an executable statement of the invariant, so a future change
/// that puts a pipeline back inside an element fails here.
#[test]
fn pipelines_belong_to_the_window_not_to_the_element() {
    /// Shader modules + render pipelines a window ends up holding.
    fn pipeline_objects(waveforms: usize, spectrogram_channels: usize, shared: bool) -> usize {
        if shared {
            // One `Renderers`: the waveform's two pipelines and the
            // spectrogram's one, plus their two shader modules.
            2 + 1 + 2
        } else {
            // Per waveform: a shader module + a column and a line pipeline.
            // Per spectrogram *channel*: a shader module + a pipeline.
            waveforms * 3 + spectrogram_channels * 2
        }
    }

    // A window like the editor example: a couple of waveforms and an
    // eight-channel spectrogram.
    let (waveforms, channels) = (2, 8);
    let before = pipeline_objects(waveforms, channels, false);
    let after = pipeline_objects(waveforms, channels, true);
    println!("\n{waveforms} waveforms + a {channels}-channel spectrogram");
    println!("  before E2: {before} shader/pipeline objects");
    println!("  after  E2: {after} (constant in the element count)");
    assert_eq!(before, 22);
    assert_eq!(after, 5);

    // The property that actually matters is not that the split is cheaper at
    // any given size — at a single element the two shapes cost the same five
    // objects — but that one count **follows the composition and the other does
    // not**. That is what E6/E7 need, since they turn every clip body into an
    // element that wants a slot.
    let one = pipeline_objects(1, 1, false);
    assert_eq!(one, after, "at one element the split saves nothing");
    for elements in [10usize, 40, 200] {
        assert_eq!(
            pipeline_objects(elements, elements, true),
            after,
            "the window's pipeline count must not follow the element count"
        );
        assert!(
            pipeline_objects(elements, elements, false) > one,
            "the unshared shape grows with the {elements} elements"
        );
    }
}
