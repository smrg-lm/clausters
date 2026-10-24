//! **The answers a list's views are given**, in the server's reply formats,
//! with no I/O of its own.
//!
//! [`Answerer::answer`] reads a fetch-machine message about a list and either
//! answers it or says what it needs read first ([`Need`]): a file's size and
//! age, its header, the head of its peaks file, a block of one level of its
//! peaks, a block of its samples. Whoever drives it reads those and hands them
//! over ([`Answerer::deliver`]), then asks again, and the second ask finds
//! them held. So the native front's thread, which reads the disk as it is
//! asked, and a page, which reads OPFS with promises, drive the same answerer.
//!
//! **What is held is blocks, not files.** A take of hours is never read whole:
//! a file's peaks are read a block of one level at a time, its samples a
//! block of frames at a time, and both are dropped oldest first past a bound
//! ([`HELD_BYTES`]). What a picture costs is the levels and the spans its
//! window resolves.
//!
//! **A summary at a coarse level is folded, not measured.** A list's bucket
//! over a file is the file's peaks folded over the frames it covers, through
//! the largest aligned blocks of the levels, as the server folds a join's.
//! Where a column's edge falls off the file's grid -- after a paste, all of
//! them do -- the server reads the partial bucket from the samples; here, for
//! a column of 32 base buckets or more, the edge is rounded to a level a
//! thirty-second of the column wide, so the picture of an edited take of
//! hours is drawn from peaks alone and no column reaches more than a
//! thirty-second past its own span. A part's **own** edges, a cut or the end
//! of a fade, are read exactly, since past a cut is material the list does not
//! hold; so are the fades, and every edge of a finer column.

use std::collections::HashMap;
use std::sync::Arc;

use clausters_core::osc::{OscMessage, OscType};
use clausters_core::pcm::Layout;
use clausters_core::peaks::SpanStats;
use clausters_core::peaks::compose::{Ends, PartSamples, list_span};

use super::{List, ListPart};

/// Frames per block of samples held: 4096, under a tenth of a second -- a
/// zoom's chunk, and small beside the fade a seam reads it for.
pub const SAMPLE_BLOCK: usize = 1 << 12;

/// Buckets per block of one level of a peaks file held: 2048, 8 KiB a
/// statistic.
pub const LEVEL_BLOCK: usize = 1 << 11;

/// **What the answerer holds at most**, in bytes of blocks: past it the
/// oldest are dropped. A bound on the work in hand, never on a take's length.
pub const HELD_BYTES: usize = 64 << 20;

/// Bytes of a file's head read to find its samples; a header that does not
/// fit is read again from a larger head, once.
const HEAD_BYTES: usize = 1 << 16;
const LARGE_HEAD_BYTES: usize = 1 << 20;

/// Statistics one `/buffer_peaks.reply` carries at most. A server's reply
/// is bounded by the carrier it crosses, 8 KiB; this answer crosses none --
/// it is the host's own, handed to the view in the same process -- so it
/// carries a whole grid a view asks for (two views of stereo at two buckets
/// a column) in one reply, where 8 KiB took two dozen round trips, one a
/// frame, and a pan outran them.
const MAX_REPLY_BYTES: usize = 256 * 1024;

/// Samples one answer reads at most to fold its fades and edges, as the
/// server's, after which it answers how far it got; at least one bucket
/// always comes back.
const READ_BUDGET: usize = 1 << 17;

/// A peaks file is trusted when it is no older than this, in seconds, before
/// its samples: the peaks UGen and the writer of the samples close their
/// files in either order at the end of one render.
const PEAKS_AGE_SLACK: f64 = 2.0;

/// A file's size and when it was last written, in seconds of whatever epoch
/// the platform's clock counts from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stat {
    pub len: u64,
    pub modified: Option<f64>,
}

/// **Something the answerer needs read** before it can answer.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Need {
    /// The size and age of the file at `path`.
    Stat { path: Arc<str> },
    /// `len` bytes of the file at `path` from `offset`, for `what`.
    Read {
        path: Arc<str>,
        offset: u64,
        len: usize,
        what: What,
    },
}

impl Need {
    /// The file it reads.
    pub fn path(&self) -> &str {
        match self {
            Need::Stat { path } | Need::Read { path, .. } => path,
        }
    }
}

/// What a read is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum What {
    /// A sound file's header.
    Head,
    /// A peaks file's header.
    PeaksHead,
    /// One array (`0` min, `1` max, `2` mean square) of a block of one level
    /// of one channel of a peaks file.
    Level {
        channel: usize,
        level: usize,
        block: usize,
        array: usize,
    },
    /// A block of a sound file's frames.
    Samples { block: usize },
}

/// What a read brought.
#[derive(Debug)]
pub enum Got {
    Stat(Stat),
    Bytes(Vec<u8>),
}

/// What an ask came to.
#[derive(Debug)]
pub enum Answer {
    /// The reply, in the server's format.
    Reply(OscMessage),
    /// What has to be read first; ask again once it is delivered.
    Need(Vec<Need>),
    /// Nothing to answer: not a message about a list this answerer has.
    None,
}

/// A held block and when it was last used.
struct Held<T> {
    value: T,
    used: u64,
}

/// One array of a level block, or all three.
#[derive(Default)]
struct LevelBlock {
    arrays: [Option<Vec<f32>>; 3],
}

/// A peaks file's header: what its levels are and where each one is.
#[derive(Clone, Debug)]
struct PeaksHead {
    base: usize,
    frames: usize,
    channels: usize,
    /// Buckets at each level.
    lens: Vec<usize>,
}

impl PeaksHead {
    /// Reads the header of a version-3 multichannel peaks file (the one a
    /// peaks UGen writes): its magic, its version, its base bucket, its
    /// frames, its channels and its level count. The levels' sizes follow
    /// from those, as the writer lays them out.
    fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() < 40 || &bytes[..4] != b"CLPK" {
            return Err("not a peaks file".into());
        }
        let u64_at = |i: usize| u64::from_le_bytes(bytes[i..i + 8].try_into().unwrap()) as usize;
        let version = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        if version != 3 {
            return Err(format!("a peaks file of version {version}, not 3"));
        }
        let (base, frames, channels, levels) = (u64_at(8), u64_at(16), u64_at(24), u64_at(32));
        if base == 0 || channels == 0 {
            return Err("a peaks file with no bucket or no channels".into());
        }
        let mut lens = Vec::with_capacity(levels);
        let mut n = frames.div_ceil(base);
        for _ in 0..levels {
            lens.push(n);
            n = n.div_ceil(2);
        }
        Ok(Self {
            base,
            frames,
            channels,
            lens,
        })
    }

    /// The byte size the header says the file has, which a file of another
    /// size does not match.
    fn size(&self) -> u64 {
        40 + (self.channels * self.lens.iter().map(|n| 16 + 12 * n).sum::<usize>()) as u64
    }

    /// Where array `array` of level `level` of channel `channel` begins.
    fn offset(&self, channel: usize, level: usize, array: usize) -> u64 {
        let per_channel: usize = self.lens.iter().map(|n| 16 + 12 * n).sum();
        let before: usize = self.lens[..level].iter().map(|n| 16 + 12 * n).sum();
        (40 + channel * per_channel + before + 16 + array * 4 * self.lens[level]) as u64
    }
}

/// A peaks file: its age, its header and the level blocks read of it.
#[derive(Default)]
struct Peaks {
    stat: Option<Result<Stat, String>>,
    head: Option<Result<PeaksHead, String>>,
    levels: HashMap<(usize, usize, usize), Held<LevelBlock>>,
}

/// A sound file: its age, where its samples are, and the blocks read of them.
#[derive(Default)]
struct File {
    stat: Option<Result<Stat, String>>,
    layout: Option<Result<Layout, String>>,
    /// Bytes of head the layout was last asked from.
    head_asked: usize,
    /// The peaks file the first part over it named.
    peaks: Option<Arc<str>>,
    samples: HashMap<usize, Held<Arc<[f32]>>>,
}

/// **The host's answers about lists.**
#[derive(Default)]
pub struct Answerer {
    lists: HashMap<i32, Arc<List>>,
    files: HashMap<Arc<str>, File>,
    peaks: HashMap<Arc<str>, Peaks>,
    clock: u64,
    held: usize,
    /// Samples read from what is held, over the answerer's life -- what a
    /// test reads to know an answer folded peaks rather than samples.
    pub samples_read: usize,
}

impl Answerer {
    /// Take key `key` names `list`.
    pub fn define(&mut self, key: i32, list: Arc<List>) {
        for part in &list.parts {
            let file = self.files.entry(Arc::clone(&part.path)).or_default();
            if file.peaks.is_none() {
                file.peaks.clone_from(&part.peaks);
            }
        }
        self.lists.insert(key, list);
    }

    /// Nothing names `key`. What was read of its files stays held, for the
    /// next list over them.
    pub fn forget(&mut self, key: i32) {
        self.lists.remove(&key);
    }

    /// Whether `key` names a list here.
    pub fn has(&self, key: i32) -> bool {
        self.lists.contains_key(&key)
    }

    /// **Answers `msg`, or says what to read first.**
    pub fn answer(&mut self, msg: &OscMessage) -> Answer {
        let Some(OscType::Int(key)) = msg.args.first() else {
            return Answer::None;
        };
        let key = *key;
        let Some(list) = self.lists.get(&key).cloned() else {
            // A list nobody named any more is answered as absent, as a
            // freed buffer is.
            return match msg.addr.as_str() {
                "/buffer_query" => Answer::Reply(query_reply(key, -1, 0, 0.0)),
                _ => Answer::None,
            };
        };
        self.clock += 1;
        let mut needs = Vec::new();
        self.shapes(&list, &mut needs);
        if !needs.is_empty() {
            return Answer::Need(dedup(needs));
        }
        match msg.addr.as_str() {
            "/buffer_query" => Answer::Reply(query_reply(
                key,
                list.frames as i64,
                list.channels,
                list.sample_rate,
            )),
            "/buffer_peaks" => self.peaks_reply(key, &list, &msg.args[1..]),
            "/buffer_getRange" => self.range_reply(key, &list, &msg.args[1..]),
            "/buffer_spectrum" => self.spectrum_reply(key, &list, &msg.args[1..]),
            _ => Answer::None,
        }
    }

    /// **What was read**, handed over for the next ask.
    pub fn deliver(&mut self, need: &Need, got: Result<Got, String>) {
        self.clock += 1;
        match need {
            Need::Stat { path } => {
                let stat = got.and_then(|g| match g {
                    Got::Stat(s) => Ok(s),
                    Got::Bytes(_) => Err("a stat answered with bytes".into()),
                });
                if let Some(file) = self.files.get_mut(path) {
                    file.stat = Some(stat.clone());
                }
                if let Some(peaks) = self.peaks.get_mut(path) {
                    peaks.stat = Some(stat);
                }
            }
            Need::Read { path, what, .. } => {
                let bytes = got.and_then(|g| match g {
                    Got::Bytes(b) => Ok(b),
                    Got::Stat(_) => Err("a read answered with a stat".into()),
                });
                self.take_bytes(path, *what, bytes);
            }
        }
        self.bound();
    }

    fn take_bytes(&mut self, path: &Arc<str>, what: What, bytes: Result<Vec<u8>, String>) {
        let clock = self.clock;
        match what {
            What::Head => {
                let Some(file) = self.files.get_mut(path) else {
                    return;
                };
                let len = file
                    .stat
                    .as_ref()
                    .and_then(|s| s.as_ref().ok())
                    .map(|s| s.len);
                file.layout = Some(bytes.and_then(|b| {
                    let parsed = Layout::parse_within(&mut std::io::Cursor::new(&b), path, len);
                    // A header past the head read is read again from a larger
                    // one, once, rather than refused.
                    if parsed.is_err()
                        && (b.len() as u64) < len.unwrap_or(0)
                        && file.head_asked < LARGE_HEAD_BYTES
                    {
                        return Err(String::new());
                    }
                    parsed
                }));
                if matches!(&file.layout, Some(Err(e)) if e.is_empty()) {
                    file.layout = None;
                    file.head_asked = LARGE_HEAD_BYTES;
                }
            }
            What::PeaksHead => {
                let Some(peaks) = self.peaks.get_mut(path) else {
                    return;
                };
                peaks.head = Some(bytes.and_then(|b| PeaksHead::parse(&b)));
            }
            What::Level {
                channel,
                level,
                block,
                array,
            } => {
                let Some(peaks) = self.peaks.get_mut(path) else {
                    return;
                };
                let values: Vec<f32> = match bytes {
                    Ok(b) => clausters_core::osc::blob_samples(&b).collect(),
                    Err(e) => {
                        crate::host::diag::warn!("peaks {path}: {e}");
                        peaks.head = Some(Err(e));
                        return;
                    }
                };
                self.held += values.len() * 4;
                let held = peaks
                    .levels
                    .entry((channel, level, block))
                    .or_insert_with(|| Held {
                        value: LevelBlock::default(),
                        used: clock,
                    });
                held.value.arrays[array] = Some(values);
                held.used = clock;
            }
            What::Samples { block } => {
                let Some(file) = self.files.get_mut(path) else {
                    return;
                };
                let Some(Ok(layout)) = &file.layout else {
                    return;
                };
                let values = match bytes {
                    Ok(b) => decode(layout, &b),
                    Err(e) => {
                        crate::host::diag::warn!("{path}: {e}");
                        Vec::new()
                    }
                };
                self.held += values.len() * 4;
                file.samples.insert(
                    block,
                    Held {
                        value: values.into(),
                        used: clock,
                    },
                );
            }
        }
    }

    /// Drops the blocks used longest ago until what is held is under
    /// [`HELD_BYTES`] by a quarter, so a bound reached is not met again on
    /// the next delivery.
    fn bound(&mut self) {
        if self.held <= HELD_BYTES {
            return;
        }
        let mut ages: Vec<u64> = self
            .files
            .values()
            .flat_map(|f| f.samples.values().map(|h| h.used))
            .chain(
                self.peaks
                    .values()
                    .flat_map(|p| p.levels.values().map(|h| h.used)),
            )
            .collect();
        ages.sort_unstable();
        let cut = ages[ages.len() / 2];
        let mut held = 0;
        for file in self.files.values_mut() {
            file.samples.retain(|_, h| h.used > cut);
            held += file
                .samples
                .values()
                .map(|h| h.value.len() * 4)
                .sum::<usize>();
        }
        for peaks in self.peaks.values_mut() {
            peaks.levels.retain(|_, h| h.used > cut);
            held += peaks
                .levels
                .values()
                .flat_map(|h| h.value.arrays.iter().flatten())
                .map(|a| a.len() * 4)
                .sum::<usize>();
        }
        self.held = held;
    }

    /// What every file of `list` needs before anything about it can be
    /// answered: its size and age, its header, and its peaks file's.
    fn shapes(&mut self, list: &List, needs: &mut Vec<Need>) {
        for part in &list.parts {
            let file = self.files.entry(Arc::clone(&part.path)).or_default();
            match &file.stat {
                None => needs.push(Need::Stat {
                    path: Arc::clone(&part.path),
                }),
                Some(Err(_)) => {}
                Some(Ok(stat)) => {
                    if file.layout.is_none() {
                        let len = stat.len.min(file.head_asked.max(HEAD_BYTES) as u64) as usize;
                        file.head_asked = file.head_asked.max(HEAD_BYTES);
                        needs.push(Need::Read {
                            path: Arc::clone(&part.path),
                            offset: 0,
                            len,
                            what: What::Head,
                        });
                    }
                }
            }
            let Some(path) = file.peaks.clone() else {
                continue;
            };
            let peaks = self.peaks.entry(Arc::clone(&path)).or_default();
            match &peaks.stat {
                None => needs.push(Need::Stat { path }),
                Some(Err(_)) => {}
                Some(Ok(stat)) if peaks.head.is_none() => needs.push(Need::Read {
                    path,
                    offset: 0,
                    len: 40.min(stat.len as usize),
                    what: What::PeaksHead,
                }),
                Some(Ok(_)) => {}
            }
        }
    }

    /// `/buffer_peaks key [bucket] [start] [frames]`, answered as the server
    /// answers it: from `start` rounded down to a bucket, whole buckets only
    /// unless the span runs to the end, at most [`MAX_REPLY_BYTES`] of them.
    fn peaks_reply(&mut self, key: i32, list: &List, args: &[OscType]) -> Answer {
        let int = |i: usize, default: i64| args.get(i).and_then(as_i64).unwrap_or(default);
        let bucket = int(0, 256).max(1) as usize;
        let start = int(1, 0).max(0) as usize;
        let asked = int(2, -1);
        let frames = list.frames;
        let first = (start / bucket) * bucket;
        let end = if asked < 0 {
            frames
        } else {
            (start + asked as usize).min(frames)
        };
        let held = end.saturating_sub(first);
        let buckets = if end == frames {
            held.div_ceil(bucket)
        } else {
            held / bucket
        };
        let channels = list.channels.max(1);
        let buckets = buckets.min((MAX_REPLY_BYTES / (channels * 12)).max(1));
        let mut stats = Vec::with_capacity(buckets * channels * 3);
        let mut needs = Vec::new();
        let mut read = 0;
        {
            let mut samples = FileSamples {
                answerer: self,
                bucket,
                needs: &mut needs,
                hot: None,
                cold: None,
            };
            for b in 0..buckets {
                let at = first + b * bucket;
                for ch in 0..channels {
                    let s = list_span(&list.parts, &mut samples, ch, at, at + bucket, &mut read);
                    stats.extend_from_slice(&s.bucket());
                }
                if read >= READ_BUDGET {
                    stats.truncate((b + 1) * channels * 3);
                    break;
                }
            }
        }
        if !needs.is_empty() {
            return Answer::Need(dedup(needs));
        }
        self.samples_read += read;
        Answer::Reply(OscMessage {
            addr: "/buffer_peaks.reply".into(),
            args: vec![
                OscType::Int(key),
                OscType::Long(first as i64),
                OscType::Int(bucket as i32),
                OscType::Blob(clausters_core::osc::sample_blob(&stats)),
            ],
        })
    }

    /// `/buffer_getRange key start count`, flat and interleaved as the server
    /// answers it.
    fn range_reply(&mut self, key: i32, list: &List, args: &[OscType]) -> Answer {
        let channels = list.channels.max(1);
        let start = args.first().and_then(as_i64).unwrap_or(0).max(0) as usize;
        let count = args.get(1).and_then(as_i64).unwrap_or(0).max(0) as usize;
        let end = (start + count).min(list.frames * channels);
        let mut values = Vec::with_capacity(end.saturating_sub(start));
        let mut needs = Vec::new();
        {
            let mut samples = FileSamples {
                answerer: self,
                bucket: 1,
                needs: &mut needs,
                hot: None,
                cold: None,
            };
            let mut at = 0;
            for flat in start..end {
                let (frame, ch) = (flat / channels, flat % channels);
                while at + 1 < list.parts.len() && list.parts[at].span.end() <= frame {
                    at += 1;
                }
                while at > 0 && list.parts[at].span.start() > frame {
                    at -= 1;
                }
                let part = &list.parts[at];
                let inner = frame - part.span.start();
                values.push(
                    part.span
                        .read_with(inner, ch, |f, c| samples.frame(part, f, c)),
                );
            }
        }
        if !needs.is_empty() {
            return Answer::Need(dedup(needs));
        }
        self.samples_read += values.len();
        Answer::Reply(OscMessage {
            addr: "/buffer_getRange.reply".into(),
            args: vec![
                OscType::Int(key),
                crate::host::fetch::index_arg(start),
                OscType::Blob(clausters_core::osc::sample_blob(&values)),
            ],
        })
    }
}

impl Answerer {
    /// `/buffer_spectrum key start hop columns window`: **the columns of a
    /// spectrogram of the list over a span**, one analysis window of
    /// `window` frames starting at `start` and every `hop` frames after it,
    /// answered as `/buffer_spectrum.reply key start hop window channels
    /// blob` -- each column's channels in turn, `window / 2` magnitudes
    /// each, a byte apiece as a texture stores them.
    ///
    /// A list is never read whole, so its spectrogram is never analyzed
    /// whole: a view asks for the span it shows, at a hop of about a column
    /// of its own, and asks again when it moves. Each window is read through
    /// the parts, fades and channel maps included, so what is analyzed is
    /// what plays. An answer reads at most [`SPECTRUM_BLOCKS`] blocks it does
    /// not hold; past them it answers the columns it could, at least one,
    /// and the view asks for the rest from where it ended.
    fn spectrum_reply(&mut self, key: i32, list: &List, args: &[OscType]) -> Answer {
        let int = |i: usize| args.get(i).and_then(as_i64).unwrap_or(0).max(0) as usize;
        let (start, hop, columns, window) = (int(0), int(1).max(1), int(2), int(3));
        if !clausters_core::fft::supports(window) || columns == 0 {
            return Answer::None;
        }
        let channels = list.channels.max(1);
        let bins = window / 2;
        let (hann, gain) = clausters_core::stft::analysis_window(window);
        let mut windowed = vec![0.0f32; window];
        let mut spectrum = vec![0.0f32; bins];
        let mut column = vec![0.0f32; bins];
        let mut frame = vec![0.0f32; window];
        let mut out: Vec<u8> = Vec::with_capacity(columns * channels * bins);
        let mut needs = Vec::new();
        let mut answered = 0;
        let mut read = 0;
        for c in 0..columns {
            let first = start + c * hop;
            let before = needs.len();
            let mut values = vec![0.0f32; window * channels];
            {
                let mut samples = FileSamples {
                    answerer: self,
                    bucket: 1,
                    needs: &mut needs,
                    hot: None,
                    cold: None,
                };
                let mut at = 0;
                for i in 0..window {
                    let f = first + i;
                    if f >= list.frames {
                        break;
                    }
                    while at + 1 < list.parts.len() && list.parts[at].span.end() <= f {
                        at += 1;
                    }
                    let part = &list.parts[at];
                    let inner = f - part.span.start();
                    for ch in 0..channels {
                        values[i * channels + ch] = part
                            .span
                            .read_with(inner, ch, |f, c| samples.frame(part, f, c));
                    }
                }
            }
            // Every frame of a block not held asks for it again: what counts
            // is the blocks.
            needs = dedup(std::mem::take(&mut needs));
            if needs.len() > before {
                // Answer what is here, or gather the reads of a few more
                // columns before asking for any of them.
                if answered > 0 {
                    needs.truncate(before);
                    break;
                }
                if needs.len() >= SPECTRUM_BLOCKS {
                    break;
                }
                continue;
            }
            if !needs.is_empty() {
                continue;
            }
            read += window * channels;
            for ch in 0..channels {
                for (i, x) in frame.iter_mut().enumerate() {
                    *x = values[i * channels + ch];
                }
                clausters_core::stft::column_into(
                    &frame,
                    &hann,
                    gain,
                    &mut windowed,
                    &mut spectrum,
                    &mut column,
                );
                out.extend(column.iter().map(|m| clausters_core::stft::quantize(*m)));
            }
            answered += 1;
        }
        if answered == 0 && !needs.is_empty() {
            return Answer::Need(dedup(needs));
        }
        self.samples_read += read;
        Answer::Reply(OscMessage {
            addr: "/buffer_spectrum.reply".into(),
            args: vec![
                OscType::Int(key),
                OscType::Long(start as i64),
                OscType::Int(hop as i32),
                OscType::Int(window as i32),
                OscType::Int(channels as i32),
                OscType::Blob(out),
            ],
        })
    }
}

/// Blocks of samples one spectrum answer reads at most before it answers:
/// a window straddles two at most, so this is about a hundred columns of a
/// zoomed-out view, and a few megabytes of a stereo float file.
pub const SPECTRUM_BLOCKS: usize = 192;

/// A list's parts read through the answerer, at one bucket, noting what is
/// not held yet.
struct FileSamples<'a> {
    answerer: &'a mut Answerer,
    bucket: usize,
    needs: &'a mut Vec<Need>,
    /// The block the last frame was read from: a read runs along a block,
    /// and finding the file and the block again for every frame of it was
    /// what a spectrum's windows cost.
    hot: Option<Hot>,
    /// The block the last frame found missing, already asked for.
    cold: Option<(Arc<str>, usize)>,
}

/// A held block of one file's samples, at hand.
struct Hot {
    path: Arc<str>,
    block: usize,
    channels: usize,
    values: Arc<[f32]>,
}

impl FileSamples<'_> {
    /// The layout of `path`, when it was read and parsed.
    fn layout(&self, path: &str) -> Option<&Layout> {
        self.answerer
            .files
            .get(path)?
            .layout
            .as_ref()?
            .as_ref()
            .ok()
    }

    /// The peaks of `path`, when they are there and describe it.
    fn peaks_of(&self, path: &str) -> Option<(Arc<str>, PeaksHead)> {
        let file = self.answerer.files.get(path)?;
        let layout = file.layout.as_ref()?.as_ref().ok()?;
        let name = file.peaks.clone()?;
        let peaks = self.answerer.peaks.get(&name)?;
        let head = peaks.head.as_ref()?.as_ref().ok()?;
        let stat = peaks.stat.as_ref()?.as_ref().ok()?;
        let fresh = match (stat.modified, file.stat.as_ref()?.as_ref().ok()?.modified) {
            (Some(p), Some(f)) => p + PEAKS_AGE_SLACK >= f,
            _ => true,
        };
        (fresh
            && head.frames as u64 == layout.frames
            && head.channels == layout.channels
            && head.size() == stat.len)
            .then(|| (name, head.clone()))
    }

    /// **One frame of `path` on channel `ch`**, out of a held block of its
    /// samples, or a need and a zero.
    fn sample(&mut self, path: &Arc<str>, frame: usize, ch: usize) -> f32 {
        let block = frame / SAMPLE_BLOCK;
        if let Some(hot) = &self.hot
            && hot.block == block
            && Arc::ptr_eq(&hot.path, path)
        {
            if ch >= hot.channels {
                return 0.0;
            }
            let i = (frame - block * SAMPLE_BLOCK) * hot.channels + ch;
            return hot.values.get(i).copied().unwrap_or(0.0);
        }
        if let Some((cold, at)) = &self.cold
            && *at == block
            && Arc::ptr_eq(cold, path)
        {
            return 0.0;
        }
        let Some(layout) = self.layout(path) else {
            return 0.0;
        };
        let channels = layout.channels;
        if frame as u64 >= layout.frames || ch >= channels {
            return 0.0;
        }
        let clock = self.answerer.clock;
        let file = self.answerer.files.get_mut(path).expect("a laid-out file");
        if let Some(held) = file.samples.get_mut(&block) {
            held.used = clock;
            let i = (frame - block * SAMPLE_BLOCK) * channels + ch;
            let value = held.value.get(i).copied().unwrap_or(0.0);
            self.hot = Some(Hot {
                path: Arc::clone(path),
                block,
                channels,
                values: Arc::clone(&held.value),
            });
            return value;
        }
        let layout = file.layout.as_ref().unwrap().as_ref().unwrap();
        let first = (block * SAMPLE_BLOCK) as u64;
        let frames = (SAMPLE_BLOCK as u64).min(layout.frames - first);
        let width = layout.frame_bytes() as u64;
        self.needs.push(Need::Read {
            path: Arc::clone(path),
            offset: layout.data_offset + first * width,
            len: (frames * width) as usize,
            what: What::Samples { block },
        });
        self.cold = Some((Arc::clone(path), block));
        0.0
    }

    /// The statistics of frames `a..b` of `path` on channel `ch`, read from
    /// its samples.
    fn sample_stats(
        &mut self,
        path: &Arc<str>,
        ch: usize,
        a: usize,
        b: usize,
        read: &mut usize,
    ) -> SpanStats {
        if a >= b {
            return SpanStats::EMPTY;
        }
        let values: Vec<f32> = (a..b).map(|f| self.sample(path, f, ch)).collect();
        *read += b - a;
        SpanStats::of(&values)
    }

    /// One bucket of one level of `path`'s peaks, or a need and `None`.
    fn level(
        &mut self,
        name: &Arc<str>,
        head: &PeaksHead,
        ch: usize,
        level: usize,
        index: usize,
    ) -> Option<[f32; 3]> {
        let block = index / LEVEL_BLOCK;
        let clock = self.answerer.clock;
        let peaks = self.answerer.peaks.get_mut(name)?;
        let held = peaks.levels.get_mut(&(ch, level, block));
        if let Some(held) = held
            && held.value.arrays.iter().all(Option::is_some)
        {
            held.used = clock;
            let i = index - block * LEVEL_BLOCK;
            let at = |a: usize| held.value.arrays[a].as_ref().unwrap().get(i).copied();
            return Some([at(0)?, at(1)?, at(2)?]);
        }
        let have = peaks
            .levels
            .get(&(ch, level, block))
            .map(|h| h.value.arrays.each_ref().map(Option::is_some))
            .unwrap_or_default();
        let len = head.lens[level];
        let first = block * LEVEL_BLOCK;
        let n = LEVEL_BLOCK.min(len - first);
        for (array, had) in have.into_iter().enumerate() {
            if !had {
                self.needs.push(Need::Read {
                    path: Arc::clone(name),
                    offset: head.offset(ch, level, array) + (first * 4) as u64,
                    len: n * 4,
                    what: What::Level {
                        channel: ch,
                        level,
                        block,
                        array,
                    },
                });
            }
        }
        None
    }

    /// **Frames `a..b` of `path` on channel `ch`, folded from its peaks**:
    /// the largest aligned block of the levels at each position, with what
    /// is left at the edges read from the samples. An edge that is not one
    /// of the part's own is first rounded to the grid of a level a
    /// thirty-second of the bucket wide, when the bucket is that coarse.
    fn fold(
        &mut self,
        path: &Arc<str>,
        (name, head): (&Arc<str>, &PeaksHead),
        ch: usize,
        (a, b): (usize, usize),
        ends: Ends,
        read: &mut usize,
    ) -> SpanStats {
        let base = head.base;
        let frames = head.frames;
        let coarse = self.bucket >= 32 * base;
        let mut fine = base;
        while coarse && fine * 2 <= self.bucket / 32 {
            fine *= 2;
        }
        let a = if ends.start || !coarse {
            a
        } else {
            a / fine * fine
        };
        let b = if ends.end || !coarse {
            b
        } else {
            (b.div_ceil(fine) * fine).min(frames)
        };
        let buckets = head.lens.first().copied().unwrap_or(0);
        let first = a.div_ceil(base);
        let last = if b == frames {
            buckets
        } else {
            (b / base).min(buckets)
        };
        let mut stats = SpanStats::EMPTY;
        if first >= last {
            return self.sample_stats(path, ch, a, b, read);
        }
        let mut missing = false;
        let mut i = first;
        while i < last {
            let mut level = 0;
            while level + 1 < head.lens.len()
                && (i >> level).is_multiple_of(2)
                && i + (2usize << level) <= last
            {
                level += 1;
            }
            let index = i >> level;
            let lvl_bucket = base << level;
            let count = lvl_bucket.min(frames.saturating_sub(index * lvl_bucket));
            match self.level(name, head, ch, level, index) {
                Some([min, max, ms]) if count > 0 => stats.merge(SpanStats {
                    min,
                    max,
                    sum_sq: ms as f64 * count as f64,
                    count,
                }),
                Some(_) => {}
                None => missing = true,
            }
            i += 1usize << level;
        }
        let inner = (first * base, (last * base).min(frames));
        stats.merge(self.sample_stats(path, ch, a, inner.0.max(a).min(b), read));
        stats.merge(self.sample_stats(path, ch, inner.1.max(a), b, read));
        if missing {
            return SpanStats::EMPTY;
        }
        stats
    }
}

impl PartSamples<ListPart> for FileSamples<'_> {
    fn summary(
        &mut self,
        part: &ListPart,
        src_ch: usize,
        a: usize,
        b: usize,
        ends: Ends,
        read: &mut usize,
    ) -> SpanStats {
        let Some(layout) = self.layout(&part.path) else {
            // A file that could not be read is drawn as silence, which its
            // reader has already said why.
            return SpanStats::silence(b.saturating_sub(a));
        };
        let b = b.min(layout.frames as usize);
        if src_ch >= layout.channels || a >= b {
            return SpanStats::silence(b.saturating_sub(a));
        }
        match self.peaks_of(&part.path) {
            Some((name, head)) => self.fold(&part.path, (&name, &head), src_ch, (a, b), ends, read),
            None => self.sample_stats(&part.path, src_ch, a, b, read),
        }
    }

    fn frame(&mut self, part: &ListPart, frame: usize, src_ch: usize) -> f32 {
        self.sample(&part.path, frame, src_ch)
    }
}

/// Samples of `bytes` as `layout` encodes them, interleaved.
fn decode(layout: &Layout, bytes: &[u8]) -> Vec<f32> {
    let width = layout.encoding.bytes();
    bytes
        .chunks_exact(width)
        .map(|b| layout.encoding.decode(b))
        .collect()
}

/// `/buffer_query.reply key frames channels rate`. A length past what an
/// `int32` counts goes as a long, as the server's longer replies do.
fn query_reply(key: i32, frames: i64, channels: usize, rate: f64) -> OscMessage {
    let frames = match i32::try_from(frames) {
        Ok(f) => OscType::Int(f),
        Err(_) => OscType::Long(frames),
    };
    OscMessage {
        addr: "/buffer_query.reply".into(),
        args: vec![
            OscType::Int(key),
            frames,
            OscType::Int(channels as i32),
            OscType::Float(rate as f32),
        ],
    }
}

fn as_i64(v: &OscType) -> Option<i64> {
    match v {
        OscType::Int(n) => Some(i64::from(*n)),
        OscType::Long(n) => Some(*n),
        _ => None,
    }
}

/// Each need once, in the order first asked.
fn dedup(needs: Vec<Need>) -> Vec<Need> {
    let mut seen = std::collections::HashSet::new();
    needs
        .into_iter()
        .filter(|n| seen.insert(n.clone()))
        .collect()
}
