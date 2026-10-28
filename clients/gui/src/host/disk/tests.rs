//! The answerer over real files: a list's shape, its summary at a coarse
//! level from peaks alone, an edit's list, a zoom's samples, and files with no
//! peaks or stale ones.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use clausters_core::osc::{OscMessage, OscType};
use clausters_core::peaks::{MultiPyramid, SpanStats};
use serde_json::json;

use super::answer::Answerer;
use super::native::{Disk, answer};
use super::{List, is_list_key, list_key_of};

fn noise(n: usize, seed: u32) -> Vec<f32> {
    let mut s = seed.max(1);
    (0..n)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            (s as f32 / u32::MAX as f32) * 1.6 - 0.8
        })
        .collect()
}

/// A directory of this test's own, emptied first.
fn scratch(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("clausters-gui-disk-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Writes `samples` (interleaved) as a float W64 and, when asked, its peaks
/// at the base bucket of 256. Answers the two paths.
fn take(dir: &Path, name: &str, samples: &[f32], channels: usize, peaks: bool) -> (String, String) {
    let path = dir
        .join(format!("{name}.w64"))
        .to_string_lossy()
        .into_owned();
    let mut w = clausters_core::pcm::Writer::create(&path, channels, 48_000.0, "float").unwrap();
    w.write(samples).unwrap();
    w.finalize().unwrap();
    let peaks_path = dir
        .join(format!("{name}.peaks"))
        .to_string_lossy()
        .into_owned();
    if peaks {
        MultiPyramid::build_interleaved(samples, channels, 256)
            .write_cache(&peaks_path)
            .unwrap();
    }
    (path, peaks_path)
}

fn part(
    path: &str,
    peaks: Option<&str>,
    start: usize,
    frames: usize,
    source: (usize, usize),
) -> serde_json::Value {
    let mut p = json!({
        "path": path, "start": start, "frames": frames,
        "source_channels": source.0, "source_frames": source.1, "source_rate": 48000.0,
    });
    if let Some(peaks) = peaks {
        p["peaks"] = json!(peaks);
    }
    p
}

fn list(channels: usize, parts: Vec<serde_json::Value>) -> Arc<List> {
    List::parse(&json!({"channels": channels, "sample_rate": 48000.0, "parts": parts})).unwrap()
}

fn ask(answerer: &mut Answerer, io: &mut Disk, addr: &str, args: Vec<OscType>) -> OscMessage {
    answer(
        answerer,
        io,
        &OscMessage {
            addr: addr.into(),
            args,
        },
    )
    .expect("an answer")
}

/// The whole grid at `bucket`, walked reply by reply as the fetch machine
/// walks it.
fn summary(
    answerer: &mut Answerer,
    io: &mut Disk,
    key: i32,
    bucket: usize,
    channels: usize,
) -> Vec<f32> {
    let mut stats = Vec::new();
    let mut next = 0i64;
    loop {
        let reply = ask(
            answerer,
            io,
            "/buffer_peaks",
            vec![
                OscType::Int(key),
                OscType::Int(bucket as i32),
                OscType::Long(next),
                OscType::Int(-1),
            ],
        );
        let [_, OscType::Long(start), _, OscType::Blob(blob)] = reply.args.as_slice() else {
            panic!("{reply:?}");
        };
        assert_eq!(*start, next);
        let got: Vec<f32> = clausters_core::osc::blob_samples(blob).collect();
        if got.is_empty() {
            return stats;
        }
        next += (got.len() / (channels * 3) * bucket) as i64;
        stats.extend(got);
    }
}

/// What a list's samples hold, for one channel and span, read directly.
fn direct(
    list: &List,
    data: &[&[f32]],
    channels: usize,
    ch: usize,
    a: usize,
    b: usize,
) -> SpanStats {
    let parts = list.parts();
    let paths: Vec<_> = parts.iter().map(|p| p.path.clone()).collect();
    let values: Vec<f32> = (a..b.min(list.frames))
        .map(|f| {
            let i = clausters_core::segments::part_at(&parts, f).unwrap();
            let p = &parts[i];
            let src = paths.iter().position(|q| *q == p.path).unwrap();
            p.span.read_with(f - p.span.start(), ch, |sf, sc| {
                data[src.min(data.len() - 1)][sf * channels + sc]
            })
        })
        .collect();
    SpanStats::of(&values)
}

#[test]
fn a_list_key_is_below_every_buffer_and_absence() {
    assert!(is_list_key(-2) && !is_list_key(-1) && !is_list_key(0));
    let msg = OscMessage {
        addr: "/buffer_peaks".into(),
        args: vec![OscType::Int(-7)],
    };
    assert_eq!(list_key_of(&msg), Some(-7));
    let msg = OscMessage {
        addr: "/buffer_query".into(),
        args: vec![OscType::Int(3)],
    };
    assert_eq!(list_key_of(&msg), None);
}

/// **A take opened as it is reads its peaks and no sample**: its shape is
/// the file's, and its summary at a coarse bucket is the file's own level.
#[test]
fn a_file_opened_whole_is_drawn_from_its_peaks_alone() {
    let dir = scratch("whole");
    let frames = 1 << 20;
    let data = noise(frames * 2, 3);
    let (path, peaks) = take(&dir, "a", &data, 2, true);
    let l = list(2, vec![part(&path, Some(&peaks), 0, frames, (2, frames))]);
    let mut answerer = Answerer::default();
    let mut io = Disk::default();
    answerer.define(-2, l.clone());
    let q = ask(
        &mut answerer,
        &mut io,
        "/buffer_query",
        vec![OscType::Int(-2)],
    );
    assert_eq!(
        q.args[..3],
        [
            OscType::Int(-2),
            OscType::Int(frames as i32),
            OscType::Int(2)
        ]
    );
    let bucket = 1 << 13;
    let stats = summary(&mut answerer, &mut io, -2, bucket, 2);
    assert_eq!(stats.len(), frames / bucket * 6);
    assert_eq!(answerer.samples_read, 0, "no sample was read");
    for i in 0..frames / bucket {
        for ch in 0..2 {
            let want = direct(&l, &[&data], 2, ch, i * bucket, (i + 1) * bucket);
            let got = &stats[(i * 2 + ch) * 3..][..3];
            assert_eq!(
                (got[0], got[1]),
                (want.min, want.max),
                "bucket {i} channel {ch}"
            );
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// **A mono file under a stereo list is drawn on both channels**, as every
/// reader of a part plays it: the drawing and the playing read one map.
#[test]
fn a_mono_file_under_a_stereo_list_is_drawn_on_both_channels() {
    let dir = scratch("mono");
    let frames = 1 << 16;
    let data = noise(frames, 7);
    let (path, peaks) = take(&dir, "m", &data, 1, true);
    let l = list(2, vec![part(&path, Some(&peaks), 0, frames, (1, frames))]);
    let mut answerer = Answerer::default();
    let mut io = Disk::default();
    answerer.define(-2, l);
    let bucket = 1 << 12;
    let stats = summary(&mut answerer, &mut io, -2, bucket, 2);
    assert_eq!(stats.len(), frames / bucket * 6);
    for column in stats.as_chunks::<6>().0 {
        assert_eq!(column[..3], column[3..], "both channels are the file's");
        assert!(column[1] > column[0], "and neither is silence");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// **An edit is drawn from the peaks already held**: a paste shifts the tail
/// off the file's grid, and at a coarse bucket only the fades and the cuts
/// are read from samples. Each column holds what its samples hold and
/// reaches at most a thirty-second of itself past its span.
#[test]
fn a_paste_is_drawn_from_peaks_and_its_seams() {
    let dir = scratch("paste");
    let frames = 1 << 20;
    let data = noise(frames, 5);
    let (path, peaks) = take(&dir, "a", &data, 1, true);
    let mut pasted = part(&path, Some(&peaks), 500_003, 48_001, (1, frames));
    pasted["fade_in"] = json!(64);
    pasted["fade_out"] = json!(64);
    let l = list(
        1,
        vec![
            part(&path, Some(&peaks), 0, 300_017, (1, frames)),
            pasted,
            part(&path, Some(&peaks), 300_017, frames - 300_017, (1, frames)),
        ],
    );
    let mut answerer = Answerer::default();
    let mut io = Disk::default();
    answerer.define(-3, l.clone());
    let bucket = 1 << 14;
    let stats = summary(&mut answerer, &mut io, -3, bucket, 1);
    // Two fades of 64 and the cut edges of four bodies, a base bucket each
    // at most.
    assert!(
        answerer.samples_read <= 2 * 64 + 4 * 256,
        "{} samples read",
        answerer.samples_read
    );
    let slack = bucket / 32;
    for i in 0..l.frames / bucket {
        let (a, b) = (i * bucket, (i + 1) * bucket);
        let inside = direct(&l, &[&data], 1, 0, a, b);
        let around = direct(&l, &[&data], 1, 0, a.saturating_sub(slack), b + slack);
        let got = &stats[i * 3..][..3];
        // The column's own samples are inside what it draws, and what it
        // draws is inside what the list holds a thirty-second either side:
        // a column's edge is rounded within its part, and a cut is exact.
        assert!(got[0] <= inside.min && got[1] >= inside.max, "bucket {i}");
        assert!(got[0] >= around.min && got[1] <= around.max, "bucket {i}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// **A zoom reads the span it shows**: the samples come back as the list's,
/// fades and map included, and nothing past the span is read.
#[test]
fn a_zoom_reads_only_its_span() {
    let dir = scratch("zoom");
    let frames = 200_000;
    let a = noise(frames * 2, 7);
    let b = noise(frames, 9);
    let (pa, _) = take(&dir, "a", &a, 2, false);
    let (pb, _) = take(&dir, "b", &b, 1, false);
    let mut first = part(&pa, None, 1_000, 50_000, (2, frames));
    first["channels"] = json!([1, -1]);
    first["fade_out"] = json!(300);
    let l = list(2, vec![first, part(&pb, None, 10, 60_000, (1, frames))]);
    let mut answerer = Answerer::default();
    let mut io = Disk::default();
    answerer.define(-4, l.clone());
    let (start, count) = (49_800 * 2, 4_096);
    let reply = ask(
        &mut answerer,
        &mut io,
        "/buffer_getRange",
        vec![
            OscType::Int(-4),
            OscType::Int(start as i32),
            OscType::Int(count),
        ],
    );
    let OscType::Blob(blob) = &reply.args[2] else {
        panic!("{reply:?}");
    };
    let got: Vec<f32> = clausters_core::osc::blob_samples(blob).collect();
    assert_eq!(got.len(), 4_096);
    for (i, v) in got.iter().enumerate() {
        let flat = start + i;
        let (f, ch) = (flat / 2, flat % 2);
        let p = l.part_at(f).unwrap();
        let want = if f < 50_000 {
            p.span.read_with(f, ch, |sf, sc| a[sf * 2 + sc])
        } else {
            p.span.read_with(f - 50_000, ch, |sf, _| b[sf])
        };
        assert_eq!(*v, want, "frame {f} channel {ch}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A file with no peaks, or with peaks of another take, is summarized from
/// its samples and exactly.
#[test]
fn no_peaks_or_stale_ones_fall_back_to_the_samples() {
    let dir = scratch("stale");
    let frames = 100_000;
    let data = noise(frames, 11);
    let (path, peaks) = take(&dir, "a", &data, 1, false);
    // Peaks of a shorter take, under the name this one's would have.
    MultiPyramid::build_interleaved(&data[..90_000], 1, 256)
        .write_cache(&peaks)
        .unwrap();
    let l = list(1, vec![part(&path, Some(&peaks), 0, frames, (1, frames))]);
    let mut answerer = Answerer::default();
    let mut io = Disk::default();
    answerer.define(-5, l.clone());
    let stats = summary(&mut answerer, &mut io, -5, 1 << 12, 1);
    assert!(answerer.samples_read >= frames / 2, "read its samples");
    for i in 0..frames >> 12 {
        let want = direct(&l, &[&data], 1, 0, i << 12, (i + 1) << 12);
        assert_eq!(
            (stats[i * 3], stats[i * 3 + 1]),
            (want.min, want.max),
            "bucket {i}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// **Peaks named before they were whole are looked at again**: a list named
/// while the measure an open starts is still writing its peaks reads its
/// samples, and once the peaks are whole it folds them -- where it held them
/// unusable for the session, every reply over a take of hours read a bucket
/// of samples and no more.
#[test]
fn peaks_named_before_they_are_whole_are_looked_at_again() {
    let dir = scratch("late");
    let frames = 1 << 20;
    let data = noise(frames, 5);
    let (path, peaks) = take(&dir, "a", &data, 1, false);
    // half the peaks file, as a measure under way leaves it
    MultiPyramid::build_interleaved(&data, 1, 256)
        .write_cache(&peaks)
        .unwrap();
    let whole = std::fs::read(&peaks).unwrap();
    std::fs::write(&peaks, &whole[..whole.len() / 2]).unwrap();
    let l = list(1, vec![part(&path, Some(&peaks), 0, frames, (1, frames))]);
    let mut answerer = Answerer::default();
    let mut io = Disk::default();
    answerer.define(-6, l);
    let bucket = 1 << 14;
    summary(&mut answerer, &mut io, -6, bucket, 1);
    assert!(answerer.samples_read >= frames / 2, "read its samples");
    std::fs::write(&peaks, &whole).unwrap();
    for _ in 0..64 {
        ask(
            &mut answerer,
            &mut io,
            "/buffer_query",
            vec![OscType::Int(-6)],
        );
    }
    let before = answerer.samples_read;
    let stats = summary(&mut answerer, &mut io, -6, bucket, 1);
    assert_eq!(stats.len(), frames / bucket * 3);
    assert_eq!(answerer.samples_read, before, "folded from its peaks");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A key nobody names is absent, as a freed buffer is.
#[test]
fn a_forgotten_list_is_absent() {
    let mut answerer = Answerer::default();
    let mut io = Disk::default();
    let q = ask(
        &mut answerer,
        &mut io,
        "/buffer_query",
        vec![OscType::Int(-9)],
    );
    assert_eq!(q.args[1], OscType::Int(-1));
}

/// Every column of a span, walked reply by reply as a view asks for them:
/// each column's channels in turn, `window / 2` bytes each.
fn spectrum(
    answerer: &mut Answerer,
    io: &mut Disk,
    key: i32,
    (start, hop, columns, window): (usize, usize, usize, usize),
    channels: usize,
) -> Vec<u8> {
    let mut got = Vec::new();
    let mut done = 0;
    while done < columns {
        let reply = ask(
            answerer,
            io,
            "/buffer_spectrum",
            vec![
                OscType::Int(key),
                OscType::Long((start + done * hop) as i64),
                OscType::Int(hop as i32),
                OscType::Int((columns - done) as i32),
                OscType::Int(window as i32),
            ],
        );
        let Some(OscType::Blob(blob)) = reply.args.last() else {
            panic!("{reply:?}");
        };
        let answered = blob.len() / (channels * window / 2);
        assert!(answered > 0, "a reply answers a column at least");
        done += answered;
        got.extend_from_slice(blob);
    }
    got
}

/// **A list's spectrogram is composed from its files' columns**, as its
/// summary is from their peaks: a column inside a part is the window its
/// file holds at the nearest point of a grid of the hop laid over the file,
/// so after a cut the columns of what was there before are found again and
/// only the seam's are analyzed.
#[test]
fn an_edit_s_spectrogram_is_composed_from_the_columns_analyzed_before() {
    let dir = scratch("columns");
    let frames = 1 << 17;
    let data = noise(frames, 11);
    let (path, _) = take(&dir, "a", &data, 1, false);
    let (hop, window) = (512, 1024);
    let mut answerer = Answerer::default();
    let mut io = Disk::default();
    answerer.define(-2, list(1, vec![part(&path, None, 0, frames, (1, frames))]));
    let columns = (frames - window) / hop;
    let whole = spectrum(&mut answerer, &mut io, -2, (0, hop, columns, window), 1);
    let analyzed = answerer.columns_analyzed;
    assert_eq!(analyzed, columns, "each column analyzed once");
    // a column is the file's window where the grid puts it
    let (hann, gain) = clausters_core::stft::analysis_window(window);
    let column_at = |at: usize| -> Vec<u8> {
        let mut out = vec![0.0f32; window / 2];
        clausters_core::stft::column_into(
            &data[at..at + window],
            &hann,
            gain,
            &mut vec![0.0; window],
            &mut vec![0.0; window / 2],
            &mut out,
        );
        out.iter()
            .map(|m| clausters_core::stft::quantize(*m))
            .collect()
    };
    assert_eq!(&whole[7 * 512..8 * 512], column_at(7 * hop).as_slice());
    // A cut of 10_001 frames, off the grid: what follows it moves by that
    // much, and its columns are the ones already analyzed.
    let cut = (40_000, 10_001);
    let edited = list(
        1,
        vec![
            part(&path, None, 0, cut.0, (1, frames)),
            part(
                &path,
                None,
                cut.0 + cut.1,
                frames - cut.0 - cut.1,
                (1, frames),
            ),
        ],
    );
    answerer.define(-3, edited.clone());
    let columns = (edited.frames - window) / hop;
    let after = spectrum(&mut answerer, &mut io, -3, (0, hop, columns, window), 1);
    let fresh = answerer.columns_analyzed - analyzed;
    assert!(
        fresh <= 4,
        "only the windows over the seam are analyzed: {fresh} of {columns}"
    );
    assert!(answerer.columns_reused >= columns - 4);
    // past the cut, a column at list frame f is the file's at the grid point
    // nearest f + the cut
    let c = 100;
    let f = c * hop;
    assert!(f > cut.0 + window);
    let at = (f + cut.1 + hop / 2) / hop * hop;
    assert_eq!(&after[c * 512..(c + 1) * 512], column_at(at).as_slice());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Writes the spectral summary of `samples` (interleaved) beside the peaks at
/// `peaks`, as the UGen that writes the peaks writes it, and answers it.
fn summary_beside(
    peaks: &str,
    samples: &[f32],
    channels: usize,
) -> clausters_core::spectra::Spectra {
    use clausters_core::spectra::{Accumulator, Spectra, beside_peaks};
    let mut acc = Accumulator::new(channels);
    acc.set_channels(channels);
    let mut columns = Vec::new();
    let mut done = |col: &[u8]| columns.extend_from_slice(col);
    for frame in samples.chunks_exact(channels) {
        acc.push(|c| frame[c], &mut done);
    }
    acc.finish(&mut done);
    let spectra = Spectra::from_columns(samples.len() / channels, channels, &columns).unwrap();
    spectra.write(beside_peaks(peaks)).unwrap();
    spectra
}

/// **A zoomed-out spectrogram of a list is read from its files' spectral
/// summaries, with no sample**: a column of the view is, for each part it
/// crosses, the largest of the summary's columns whose middle falls in what
/// the part plays there, a fade lowering what its part gives; and where a
/// file has no summary the same columns are analyzed from the samples.
#[test]
fn a_zoomed_out_spectrogram_is_read_from_the_spectral_summaries() {
    use clausters_core::spectra::{BINS, COLUMN_FRAMES, HOP, WINDOW};
    let dir = scratch("spectra");
    let frames = 12 * COLUMN_FRAMES + 333;
    let channels = 2;
    let data = noise(frames * channels, 5);
    let (path, peaks) = take(&dir, "a", &data, channels, true);
    let kept = summary_beside(&peaks, &data, channels);
    // a cut off every grid, and a fade on the second part
    let cut = (3 * COLUMN_FRAMES + 1_001, 2 * COLUMN_FRAMES + 77);
    let mut second = part(
        &path,
        Some(&peaks),
        cut.0 + cut.1,
        frames - cut.0 - cut.1,
        (2, frames),
    );
    second["fade_in"] = json!(COLUMN_FRAMES);
    let edited = list(
        channels,
        vec![part(&path, Some(&peaks), 0, cut.0, (2, frames)), second],
    );
    let mut answerer = Answerer::default();
    let mut io = Disk::default();
    answerer.define(-2, edited.clone());
    let hop = 2 * COLUMN_FRAMES;
    let columns = edited.frames.div_ceil(hop);
    let got = spectrum(
        &mut answerer,
        &mut io,
        -2,
        (0, hop, columns, WINDOW),
        channels,
    );
    assert_eq!(answerer.samples_read, 0, "no sample read");
    assert_eq!(answerer.columns_summarized, columns);
    // the same rule, over the summary held in memory
    let parts = edited.parts();
    for c in 0..columns {
        let (a, b) = (c * hop, ((c + 1) * hop).min(edited.frames));
        for ch in 0..channels {
            let mut want = vec![0u8; BINS];
            for p in &parts {
                let s = &p.span;
                let (pa, pb) = (a.max(s.start()), b.min(s.end()));
                if pa >= pb {
                    continue;
                }
                let from = s.src_start + pa - s.start();
                let to = from + (pb - pa);
                let n = kept.head.lens[0];
                let first = |f: usize| {
                    f.saturating_sub(COLUMN_FRAMES / 2)
                        .div_ceil(COLUMN_FRAMES)
                        .min(n)
                };
                let (mut k0, mut k1) = (first(from), first(to));
                if k0 >= k1 {
                    k0 = (from / COLUMN_FRAMES).min(n - 1);
                    k1 = k0 + 1;
                }
                let (i0, i1) = (pa - s.start(), pb - s.start());
                let shift = if i1 > s.fade_in {
                    0
                } else {
                    let g = s.gain(i0).max(s.gain(i1 - 1));
                    (20.0 * g.log10() / clausters_core::stft::REF_FLOOR * 255.0).round() as u8
                };
                for k in k0..k1 {
                    for (w, m) in want.iter_mut().zip(kept.column(ch, 0, k)) {
                        *w = (*w).max(m.saturating_sub(shift));
                    }
                }
            }
            let at = (c * channels + ch) * BINS;
            assert_eq!(
                &got[at..at + BINS],
                want.as_slice(),
                "column {c} channel {ch}"
            );
        }
    }
    // A file with no summary: the same view is analyzed, every window of it
    // on the grid of the hop.
    let (bare, _) = take(&dir, "b", &data, channels, false);
    let plain = list(channels, vec![part(&bare, None, 0, frames, (2, frames))]);
    let mut answerer = Answerer::default();
    answerer.define(-3, plain);
    let got = spectrum(
        &mut answerer,
        &mut io,
        -3,
        (0, hop, frames.div_ceil(hop), WINDOW),
        channels,
    );
    assert_eq!(answerer.columns_summarized, 0);
    // inside the file each channel's window is its file's; the last ones,
    // reaching past the end, are read through the list
    let windows = answerer.columns_analyzed + channels * answerer.list_windows;
    assert!(
        windows >= channels * (frames / HOP),
        "every window analyzed: {windows}"
    );
    assert_eq!(got.len(), frames.div_ceil(hop) * channels * BINS);
    let _ = std::fs::remove_dir_all(&dir);
}

/// **The window's shape is the view's**: a list's columns analyzed through a
/// Blackman window are that window's analysis of the file, and kept apart
/// from the Hann ones of the same file.
#[test]
fn a_lists_columns_are_analyzed_through_the_window_asked_for() {
    let dir = scratch("wintype");
    let frames = 1 << 15;
    let data = noise(frames, 3);
    let (path, _) = take(&dir, "a", &data, 1, false);
    let mut answerer = Answerer::default();
    let mut io = Disk::default();
    answerer.define(-2, list(1, vec![part(&path, None, 0, frames, (1, frames))]));
    let column = |answerer: &mut Answerer, io: &mut Disk, wintype: i32| -> Vec<u8> {
        let reply = ask(
            answerer,
            io,
            "/buffer_spectrum",
            vec![
                OscType::Int(-2),
                OscType::Long(4096),
                OscType::Int(512),
                OscType::Int(1),
                OscType::Int(1024),
                OscType::Int(512),
                OscType::Int(wintype),
            ],
        );
        let Some(OscType::Blob(blob)) = reply.args.last() else {
            panic!("{reply:?}");
        };
        blob.clone()
    };
    let want = |wintype: i32| -> Vec<u8> {
        let (window, gain) = clausters_core::stft::analysis_window_of(1024, wintype);
        let mut out = vec![0.0f32; 512];
        clausters_core::stft::column_into(
            &data[4096..4096 + 1024],
            &window,
            gain,
            &mut vec![0.0; 1024],
            &mut vec![0.0; 512],
            &mut out,
        );
        out.iter()
            .map(|m| clausters_core::stft::quantize(*m))
            .collect()
    };
    let hann = column(&mut answerer, &mut io, 0);
    let blackman = column(&mut answerer, &mut io, 4);
    assert_eq!(hann, want(0));
    assert_eq!(blackman, want(4));
    assert_ne!(hann, blackman, "two windows, two pictures");
    let _ = std::fs::remove_dir_all(&dir);
}
