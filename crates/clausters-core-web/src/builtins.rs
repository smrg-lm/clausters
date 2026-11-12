//! Builtins and the pitch space.
//!

#[cfg(target_arch = "wasm32")]
use clausters_core::builtins;
#[cfg(target_arch = "wasm32")]
use clausters_core::measure;
#[cfg(target_arch = "wasm32")]
use clausters_core::peaks::MultiPyramid;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

/// JS face: one unary builtin by name (`"midicps"`, `"cpsmidi"`, `"dbamp"`,
/// ...), computed in `f32` exactly as the server's UGens compute it.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn unary(op: &str, x: f64) -> Result<f64, JsError> {
    let op = builtins::UnaryOp::from_name(op)
        .ok_or_else(|| JsError::new(&format!("unknown unary op '{op}'")))?;
    Ok(builtins::apply_unary(op, x as f32) as f64)
}

/// JS face: one binary builtin by name (`"add"`, `"pow"`, `"clip2"`, ...).
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn binary(op: &str, a: f64, b: f64) -> Result<f64, JsError> {
    let op = builtins::BinaryOp::from_name(op)
        .ok_or_else(|| JsError::new(&format!("unknown binary op '{op}'")))?;
    Ok(builtins::apply_binary(op, a as f32, b as f32) as f64)
}

/// JS face: one two-bound operator by name (`"clip"`, `"wrap"`, `"fold"`),
/// keeping `x` inside `lo..hi`.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn bound(op: &str, x: f64, lo: f64, hi: f64) -> Result<f64, JsError> {
    let op = builtins::BoundOp::from_name(op)
        .ok_or_else(|| JsError::new(&format!("unknown bound op '{op}'")))?;
    Ok(builtins::apply_bound(op, x as f32, lo as f32, hi as f32) as f64)
}

/// JS face: one list function by name (`"normalize"`, `"integrate"`,
/// `"resamp1"`, `"asrandomtable"`, ...) over `list` with its arguments.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn list(op: &str, list: &[f64], a: f64, b: f64) -> Result<Vec<f64>, JsError> {
    let o = clausters_core::numeric::ListOp::from_name(op)
        .ok_or_else(|| JsError::new(&format!("unknown list function '{op}'")))?;
    let mut out = vec![0.0; o.output_len(list.len(), a)];
    clausters_core::numeric::apply_list(o, list, a, b, &mut out);
    Ok(out)
}

/// JS face: `list` read at the fractional `index`, blended between the two
/// values around it; `method` (`"clip"`, `"wrap"`, `"fold"`) says what an index
/// past either end reads.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = blendAt)]
pub fn blend_at(list: &[f64], index: f64, method: &str) -> Result<f64, JsError> {
    let m = builtins::BoundOp::from_name(method)
        .ok_or_else(|| JsError::new(&format!("unknown blend method '{method}'")))?;
    Ok(clausters_core::numeric::blend_at(list, index, m))
}

/// JS face: one numeric function by name (`"degrad"`, `"erf"`, `"snap"`,
/// `"bilin"`, ...) over its arguments, in `f64`.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn calc(op: &str, args: &[f64]) -> Result<f64, JsError> {
    let c = clausters_core::numeric::Calc::from_name(op)
        .ok_or_else(|| JsError::new(&format!("unknown numeric function '{op}'")))?;
    Ok(clausters_core::numeric::apply_calc(c, args))
}

/// JS face: one scale function by name (`"degkey"`, `"keydeg"`,
/// `"nearestinscale"`, `"nearestinlist"`) of `x` over `list`, `steps` to the
/// octave.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = scaleCalc)]
pub fn scale_calc(op: &str, x: f64, list: &[f64], steps: f64) -> Result<f64, JsError> {
    let c = clausters_core::numeric::ScaleCalc::from_name(op)
        .ok_or_else(|| JsError::new(&format!("unknown scale function '{op}'")))?;
    Ok(clausters_core::numeric::apply_scale_calc(c, x, list, steps))
}

/// JS face: one range map by name (`"linlin"`, `"linexp"`, `"lincurve"`, ...),
/// with `clip` naming what an out-of-range input is trimmed to (`"minmax"`,
/// `"min"`, `"max"`, `"none"`). `curve` is read only by the bent pair and the
/// input bounds only by the maps that have an input range.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
#[allow(clippy::too_many_arguments)]
pub fn map(
    op: &str,
    clip: &str,
    x: f64,
    in_lo: f64,
    in_hi: f64,
    out_lo: f64,
    out_hi: f64,
    curve: f64,
) -> Result<f64, JsError> {
    let op = clausters_core::warp::MapOp::from_name(op)
        .ok_or_else(|| JsError::new(&format!("unknown range map '{op}'")))?;
    let clip = clausters_core::warp::Clip::from_name(clip)
        .ok_or_else(|| JsError::new(&format!("unknown clip mode '{clip}'")))?;
    Ok(clausters_core::warp::apply_map(
        op,
        x as f32,
        in_lo as f32,
        in_hi as f32,
        out_lo as f32,
        out_hi as f32,
        curve as f32,
        clip,
    ) as f64)
}

/// JS face: the stereo **correlation** (Pearson's r) of two equal-length
/// channels, in `[-1, 1]`. `undefined` when it is undefined -- a length
/// mismatch, an empty pair, or a constant channel.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn correlation(left: &[f32], right: &[f32]) -> Option<f32> {
    measure::correlation(left, right)
}

/// JS face: the **Lissajous / goniometer** projection of a stereo pair, as
/// interleaved `[x, y]` pairs (`x` = side, `y` = mid) -- one pair per input
/// frame. An empty array when the two channels differ in length.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn lissajous(left: &[f32], right: &[f32]) -> Vec<f32> {
    let mut out = vec![[0.0f32; 2]; left.len()];
    if !measure::lissajous_into(left, right, &mut out) {
        return Vec::new();
    }
    out.into_iter().flatten().collect()
}

/// **The spectrogram cache of a mono take**: `samples` analyzed with a Hann
/// window of `windowSize` every `hop` samples, at `sampleRate` for the
/// frequency axis -- the bytes a `spectrogram`'s `cache` names, and the
/// transform the GUI host computes from samples it holds. `undefined` for a
/// window the FFT has no size for (a power of two from 256 to 4096) or a hop
/// of 0. The C ABI's `clausters_core_stft_build`.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = stftCache)]
pub fn stft_cache(
    samples: &[f32],
    window_size: usize,
    hop: usize,
    sample_rate: f32,
) -> Option<Vec<u8>> {
    clausters_core::stft::Stft::analyze(samples, window_size, hop, sample_rate)
        .map(|stft| stft.to_bytes())
}

/// A built min/max peak pyramid, the JS face of
/// [`clausters_core::peaks::MultiPyramid`] -- the summary a waveform view is
/// drawn from, so the drawing costs the width of the window rather than the
/// length of the buffer. Built (or filled from `/buffer_stream` reports) here
/// and handed to the GUI host, which draws it; the readers below answer **what
/// the cache is** -- length, channels, bucket, levels -- and never what it says,
/// which is a drawing's question.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = Pyramid)]
pub struct JsPyramid(MultiPyramid);

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_class = Pyramid)]
impl JsPyramid {
    /// Builds one pyramid per channel from interleaved `samples`.
    /// `base_bucket` is the level-0 bucket size (256 is the usual choice:
    /// ~0.8% of the source in cache for a floor of 256 samples per column).
    pub fn build(samples: &[f32], channels: u32, base_bucket: usize) -> JsPyramid {
        JsPyramid(MultiPyramid::build_interleaved(
            samples,
            channels.max(1) as usize,
            base_bucket.max(1),
        ))
    }

    /// **An empty pyramid of a given length** -- the picture of a take that has
    /// been allocated and not yet recorded into, ready to be filled by
    /// [`Self::write_buckets`] as the reports arrive.
    ///
    /// Building one out of a buffer of silence instead would allocate the take
    /// (230 MB for ten minutes of stereo) to summarize samples nobody wrote.
    pub fn empty(frames: usize, channels: u32, base_bucket: usize) -> JsPyramid {
        JsPyramid(MultiPyramid::empty(
            frames,
            channels.max(1) as usize,
            base_bucket.max(1),
        ))
    }

    /// Reads back a serialized cache (`toBytes`, or the file the GUI host maps
    /// and the Python client writes). `undefined` when the bytes are not one.
    #[wasm_bindgen(js_name = fromBytes)]
    pub fn from_bytes(data: &[u8]) -> Option<JsPyramid> {
        MultiPyramid::from_bytes(data).map(JsPyramid)
    }

    /// Rewrites the part of the cache a **frame span** touches, from the
    /// interleaved buffer as it now stands -- what keeps an editor's overview
    /// true after an edit without re-summarizing the take.
    ///
    /// `samples` is the whole buffer, not the span: a bucket at either edge of
    /// it holds untouched samples too. Returns whether it applied -- `false`,
    /// changing nothing, when the buffer is not the one this cache describes,
    /// which is an edit that changed the *length* and therefore a rebuild.
    #[wasm_bindgen(js_name = updateRange)]
    pub fn update_range(&mut self, samples: &[f32], start: usize, frames: usize) -> bool {
        self.0.update_range(samples, start, frames)
    }

    /// Folds a run of **already-summarized buckets** into this pyramid -- the
    /// receiving half of `/buffer_stream`, which is how a page follows a
    /// recording it cannot map: the server sends the overview of what was
    /// written (2 kB/s where the audio is 190) and this puts it in the
    /// picture.
    ///
    /// `stats` is the reply's blob read as `f32`s, **bucket-major and
    /// channel-minor**: for each bucket of `bucket` frames in order, for each
    /// channel, `min`, `max` and mean square. `startFrame` is where the report
    /// begins on the buffer's own sample axis. Returns whether it applied --
    /// `false`, changing nothing, when the report is on another grid than this
    /// cache (another bucket size, a start off a bucket boundary, or a run
    /// that does not fit).
    #[wasm_bindgen(js_name = writeBuckets)]
    pub fn write_buckets(&mut self, start_frame: usize, bucket: usize, stats: &[f32]) -> bool {
        self.0.write_buckets(start_frame, bucket, stats)
    }

    /// The cache's bytes, in the format every client reads: the mono layout
    /// for a single channel and the multichannel one above it -- the choice
    /// the Python client's door makes, so the same samples serialize to the
    /// same bytes whichever client reduced them. Both are read back by
    /// `fromBytes` and by the GUI host.
    #[wasm_bindgen(js_name = toBytes)]
    pub fn to_bytes(&self) -> Vec<u8> {
        match self.0.channel(0) {
            Some(mono) if self.0.num_channels() == 1 => mono.to_bytes(),
            _ => self.0.to_bytes(),
        }
    }

    /// Samples per channel -- the length a view of this cache spans.
    #[wasm_bindgen(getter)]
    pub fn frames(&self) -> usize {
        self.0.frames()
    }

    #[wasm_bindgen(getter)]
    pub fn channels(&self) -> usize {
        self.0.num_channels()
    }

    #[wasm_bindgen(getter, js_name = baseBucket)]
    pub fn base_bucket(&self) -> usize {
        self.0.base_bucket()
    }

    #[wasm_bindgen(getter, js_name = numLevels)]
    pub fn num_levels(&self) -> usize {
        self.0.channel(0).map_or(0, |p| p.num_levels())
    }
}
