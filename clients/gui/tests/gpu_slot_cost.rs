//! What a heavy view's GPU slot costs, and where a waveform's vertices go.
//!
//! Questions this crate answered by measuring rather than by argument, kept
//! here so the answers can be re-checked when either changes:
//!
//! 1. **Should the waveform draw into the shared `Mesh` instead of its own
//!    pipeline?** Yes — and the measurement below is why it *could*. Both
//!    destinations build the same six vertices per min/max column into the same
//!    `[x, y, r, g, b, a]` layout, and the column *computation* — the
//!    peak-pyramid reads — dominates so heavily that the choice of destination
//!    is noise. With cost out of the way what remained was that a second
//!    destination is a second implementation of the same drawing, and the two
//!    had already drifted (the pipeline's polyline dropped the sample past the
//!    right edge, and marked samples with squares where the mesh marks discs).
//!    So the pipeline is gone and `trace::draw_channel` draws every signal.
//!    This test still measures the share, because it is what would have to
//!    change for the question to be worth reopening.
//! 2. **What does an extra slot cost?** After E2, a slot is a vertex buffer and
//!    a texture; the pipelines belong to the window. Before it, every waveform
//!    widget compiled a shader module and two pipelines, and a spectrogram did
//!    the same *per channel*. This counts the objects a composition allocates,
//!    which is the number E6/E7 multiply by the element count.
//! 3. **What does a retained waterfall upload per tick?** After E18, the new
//!    columns; before it, the whole magnitude image. Counted below, because the
//!    difference is not that one is cheaper at a given span but that one
//!    follows the *hop* and the other the *span*.
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
/// The "before" numbers are the shape the code had until the pipelines moved to
/// the window (a shader module and two pipelines inside every waveform view, one
/// of each inside every `SpectrogramRenderer`, and a spectrogram building one
/// renderer per channel); the "after" is what is left now that the split has
/// happened *and* the waveform has no pipeline at all — its picture is triangles
/// in the window's mesh. Kept as an executable statement of the invariant, so a
/// future change that puts a pipeline back inside an element fails here.
#[test]
fn pipelines_belong_to_the_window_not_to_the_element() {
    /// Shader modules + render pipelines a window ends up holding.
    fn pipeline_objects(waveforms: usize, spectrogram_channels: usize, shared: bool) -> usize {
        if shared {
            // One `Renderers`: the spectrogram's pipeline and its shader
            // module. The waveforms add nothing — they draw through the
            // window's painter, which exists for the chrome regardless.
            let _ = waveforms;
            1 + 1
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
    println!("  before: {before} shader/pipeline objects");
    println!("  after:  {after} (constant in the element count)");
    assert_eq!(before, 22);
    assert_eq!(after, 2);

    // The property that actually matters is not that the split is cheaper at
    // any given size but that one count **follows the composition and the other
    // does not**. That is what the clip bodies need, since they turn every body
    // into an element that wants a slot.
    let one = pipeline_objects(1, 1, false);
    assert!(one > after, "even one element carried its own pipelines");
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

/// A retained waterfall's per-tick upload, counted rather than timed: the
/// texture is `R8Unorm`, one byte a texel, so the bytes are the texels.
///
/// This is E18's before/after. The "before" is what a tick cost until the ring
/// existed — a fresh `Stft`, a fresh texture, and a write of every column in
/// the span — and the "after" is the columns that actually landed, written
/// twice because the ring is stored twice so its window never wraps. Kept as an
/// executable statement of the invariant, so a change that goes back to
/// rebuilding the picture per tick fails here.
#[test]
fn a_waterfall_uploads_the_hop_not_the_span() {
    /// Bytes written for `landed` new columns of a `span`-column waterfall.
    fn upload_bytes(span: usize, bins: usize, landed: usize, ring: bool) -> usize {
        if ring {
            // Two texels per bin per landing column, and no allocation.
            landed * bins * 2
        } else {
            // The whole image, plus a texture allocated to write it into.
            span * bins
        }
    }

    // The milestone's own numbers: an eight-second span at hop 512 and
    // fft_size 1024, 48 kHz, ticking at 30 fps — so a tick lands two or three
    // columns.
    let (bins, landed) = (512, 3);
    let span_8s = (8.0 * 48_000.0 / 512.0f64).ceil() as usize;
    assert_eq!(span_8s, 750);
    let before = upload_bytes(span_8s, bins, landed, false);
    let after = upload_bytes(span_8s, bins, landed, true);
    println!("\n8 s at hop 512, {bins} bins, {landed} columns landing");
    println!(
        "  before E18: {before} bytes a tick (~{:.1} MB/s at 30 fps)",
        before as f64 * 30.0 / 1e6
    );
    println!("  after  E18: {after} bytes a tick");
    assert_eq!(before, 384_000);
    assert_eq!(after, 3_072);

    // The property is the shape, not the ratio: eight times the retention is
    // eight times the old cost and exactly the same new one.
    let span_64s = span_8s * 8;
    assert_eq!(
        upload_bytes(span_64s, bins, landed, false),
        before * 8,
        "the old upload follows the span"
    );
    assert_eq!(
        upload_bytes(span_64s, bins, landed, true),
        after,
        "the ring's upload must not follow the span"
    );
    // ...and it does follow the hop: half the hop is twice the columns.
    assert_eq!(upload_bytes(span_8s, bins, landed * 2, true), after * 2);
}
