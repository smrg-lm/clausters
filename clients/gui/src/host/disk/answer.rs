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

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use clausters_core::osc::{OscMessage, OscType};
use clausters_core::pcm::Layout;
use clausters_core::peaks::compose::{Ends, PartSamples, list_span};
use clausters_core::peaks::{FileHead, SpanStats, aligned_blocks};
use clausters_core::spectra;

use clausters_document::parts::tree::{Node, Placed};

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
    FileHead,
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
    /// A spectral summary's head.
    SpectraHead,
    /// A block of columns of one level of one channel of a spectral summary.
    Spectra {
        channel: usize,
        level: usize,
        block: usize,
    },
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

/// A peaks file: its age, its header and the level blocks read of it.
#[derive(Default)]
struct Peaks {
    stat: Option<Result<Stat, String>>,
    head: Option<Result<FileHead, String>>,
    levels: HashMap<(usize, usize, usize), Held<LevelBlock>>,
    /// The clock its size and age were last asked at.
    checked: u64,
}

/// **The spectral summary beside a file's peaks** (`clausters_core::spectra`):
/// its age, its head and the blocks of columns read of it.
#[derive(Default)]
struct SpectraFile {
    stat: Option<Result<Stat, String>>,
    head: Option<Result<spectra::Head, String>>,
    blocks: HashMap<ColumnBlock, Held<Arc<[u8]>>>,
    checked: u64,
}

/// A block of a spectral summary: its channel, its level and its index.
type ColumnBlock = (usize, usize, usize);

/// Columns per block of one level of a spectral summary held: 64, 32 KiB.
pub const SPECTRA_BLOCK: usize = 64;

/// **Whether a spectral summary describes `file`**, once both are known: as
/// [`usable`] asks of its peaks.
fn usable_spectra(file: &File, summary: &SpectraFile) -> Option<bool> {
    let stat = match summary.stat.as_ref()? {
        Ok(stat) => stat,
        Err(_) => return Some(false),
    };
    let head = match summary.head.as_ref()? {
        Ok(head) => head,
        Err(_) => return Some(false),
    };
    let layout = file.layout.as_ref()?.as_ref().ok()?;
    let sound = file.stat.as_ref()?.as_ref().ok()?;
    let fresh = match (stat.modified, sound.modified) {
        (Some(p), Some(f)) => p + PEAKS_AGE_SLACK >= f,
        _ => true,
    };
    Some(
        fresh
            && head.frames as u64 == layout.frames
            && head.channels == layout.channels
            && head.size() == stat.len,
    )
}

/// **How many turns of the clock a peaks file found unusable waits** before
/// it is looked at again ([`usable`]).
const RECHECK: u64 = 32;

/// **Whether `peaks` describe `file`**, once both are known: there, whole
/// (the size its header says), no older than the samples, and of the same
/// shape. A file being written -- the measure an open starts, which a list
/// names at once -- is none of those yet, and is looked at again, every
/// [`RECHECK`] turns, rather than held unusable for the session: a take
/// named before its peaks were whole was read from its samples for good,
/// a reply a bucket long over a take of hours.
fn usable(file: &File, peaks: &Peaks) -> Option<bool> {
    let stat = match peaks.stat.as_ref()? {
        Ok(stat) => stat,
        Err(_) => return Some(false),
    };
    let head = match peaks.head.as_ref()? {
        Ok(head) => head,
        Err(_) => return Some(false),
    };
    let layout = file.layout.as_ref()?.as_ref().ok()?;
    let sound = file.stat.as_ref()?.as_ref().ok()?;
    let fresh = match (stat.modified, sound.modified) {
        (Some(p), Some(f)) => p + PEAKS_AGE_SLACK >= f,
        _ => true,
    };
    Some(
        fresh
            && head.frames as u64 == layout.frames
            && head.channels == layout.channels
            && head.size() == stat.len,
    )
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
    /// **What each node of a list's tree comes to**, by its identity and the
    /// list's channel: kept once a node's every part was answered from what
    /// is held, and found again wherever the node is shared -- in the next
    /// list an edit makes, in a part pasted many times. So a column that
    /// holds thousands of parts whole is the few nodes over them.
    nodes: HashMap<(u64, usize), SpanStats>,
    /// Samples read from what is held, over the answerer's life -- what a
    /// test reads to know an answer folded peaks rather than samples.
    pub samples_read: usize,
    /// Parts summarized from their file, over the answerer's life -- what a
    /// test reads to know a picture was composed from the nodes it holds.
    pub parts_read: usize,
    /// **A spectrogram's columns of each file**, as the peaks are a file's:
    /// one window of a source channel at a point of a hop grid laid over the
    /// file's own frames, so a list's columns inside a part are found again
    /// wherever an edit moves that part ([`Self::spectrum_reply`]).
    columns: HashMap<ColumnKey, Arc<[u8]>>,
    /// The columns kept, oldest first, and the bytes they hold.
    column_order: std::collections::VecDeque<ColumnKey>,
    column_bytes: usize,
    /// Columns analyzed from a file, and found again, over the answerer's
    /// life: what a test reads to know an edit's spectrogram was composed
    /// from what was already analyzed.
    pub columns_analyzed: usize,
    pub columns_reused: usize,
    /// Windows analyzed through a list, over a seam, a fade or a resampled
    /// part, over the answerer's life.
    pub list_windows: usize,
    /// **The spectral summaries beside the files' peaks**, by path.
    spectra: HashMap<Arc<str>, SpectraFile>,
    /// Columns composed from a spectral summary, over the answerer's life:
    /// what a test reads to know a coarse picture read no samples.
    pub columns_summarized: usize,
}

/// A column of one file's channel: the window, the hop of the grid and the
/// point of it the window starts at.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ColumnKey {
    path: Arc<str>,
    channel: usize,
    window: usize,
    wintype: i32,
    hop: usize,
    at: usize,
}

/// What [`Answerer::composed`] makes of one column of a list.
enum Composed {
    /// The column of each of the list's channels, from its file's.
    Columns(Vec<Arc<[u8]>>),
    /// Blocks of samples it has to read first, asked for.
    Need,
    /// The window is not inside one part as its file holds it: a seam, a
    /// fade, another rate. It is analyzed through the list.
    No,
}

/// Bytes of columns kept at most: past it the oldest are let go of and
/// analyzed again when asked. A column is half a window, a byte a bin.
const COLUMN_BYTES_HELD: usize = 32 << 20;

/// Nodes whose summary is kept at most; past it they are let go of and
/// found again. A node's summary is 32 bytes.
const NODES_HELD: usize = 1 << 18;

impl Answerer {
    /// Take key `key` names `list`.
    pub fn define(&mut self, key: i32, list: Arc<List>) {
        for taken in list.files.values() {
            let file = self.files.entry(Arc::clone(&taken.path)).or_default();
            if file.peaks.is_none() {
                file.peaks.clone_from(&taken.peaks);
            }
        }
        self.lists.insert(key, list);
    }

    /// Nothing names `key`. What was read of a file another list still names
    /// stays held, for the next list over it; **a file no list names any more
    /// is forgotten with the last**, so a read of it still on its way lands
    /// on nothing and is dropped -- the files of a closed editor's working
    /// folder are deleted with it, and a late read of one is no warning.
    pub fn forget(&mut self, key: i32) {
        if self.lists.remove(&key).is_none() {
            return;
        }
        let (mut files, mut peaks, mut spectra) = (HashSet::new(), HashSet::new(), HashSet::new());
        for taken in self.lists.values().flat_map(|l| l.files.values()) {
            files.insert(Arc::clone(&taken.path));
            if let Some(p) = &taken.peaks {
                spectra.insert(Arc::<str>::from(spectra::beside_peaks(p)));
                peaks.insert(Arc::clone(p));
            }
        }
        self.files.retain(|path, _| files.contains(path));
        self.peaks.retain(|path, _| peaks.contains(path));
        self.spectra.retain(|path, _| spectra.contains(path));
        self.held = self.counted();
    }

    /// Whether anything read of `path` (a file, its peaks or its spectral
    /// summary) is held.
    #[cfg(test)]
    pub fn knows(&self, path: &str) -> bool {
        self.files.contains_key(path)
            || self.peaks.contains_key(path)
            || self.spectra.contains_key(path)
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
                    peaks.stat = Some(stat.clone());
                    peaks.checked = self.clock;
                }
                if let Some(summary) = self.spectra.get_mut(path) {
                    summary.stat = Some(stat);
                    summary.checked = self.clock;
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
            What::FileHead => {
                let Some(peaks) = self.peaks.get_mut(path) else {
                    return;
                };
                peaks.head = Some(bytes.and_then(|b| FileHead::parse(&b)));
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
            What::SpectraHead => {
                let Some(summary) = self.spectra.get_mut(path) else {
                    return;
                };
                summary.head = Some(bytes.and_then(|b| spectra::Head::parse(&b)));
            }
            What::Spectra {
                channel,
                level,
                block,
            } => {
                let Some(summary) = self.spectra.get_mut(path) else {
                    return;
                };
                let value: Arc<[u8]> = match bytes {
                    Ok(b) => b.into(),
                    Err(e) => {
                        crate::host::diag::warn!("spectra {path}: {e}");
                        summary.head = Some(Err(e));
                        return;
                    }
                };
                self.held += value.len();
                summary
                    .blocks
                    .insert((channel, level, block), Held { value, used: clock });
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
            .chain(
                self.spectra
                    .values()
                    .flat_map(|s| s.blocks.values().map(|h| h.used)),
            )
            .collect();
        ages.sort_unstable();
        let cut = ages[ages.len() / 2];
        for file in self.files.values_mut() {
            file.samples.retain(|_, h| h.used > cut);
        }
        for peaks in self.peaks.values_mut() {
            peaks.levels.retain(|_, h| h.used > cut);
        }
        for summary in self.spectra.values_mut() {
            summary.blocks.retain(|_, h| h.used > cut);
        }
        self.held = self.counted();
    }

    /// The bytes of samples, levels and spectra held.
    fn counted(&self) -> usize {
        let samples: usize = self
            .files
            .values()
            .flat_map(|f| f.samples.values())
            .map(|h| h.value.len() * 4)
            .sum();
        let levels: usize = self
            .peaks
            .values()
            .flat_map(|p| p.levels.values())
            .flat_map(|h| h.value.arrays.iter().flatten())
            .map(|a| a.len() * 4)
            .sum();
        let spectra: usize = self
            .spectra
            .values()
            .flat_map(|s| s.blocks.values())
            .map(|h| h.value.len())
            .sum();
        samples + levels + spectra
    }

    /// What every file of `list` needs before anything about it can be
    /// answered: its size and age, its header, and its peaks file's.
    fn shapes(&mut self, list: &List, needs: &mut Vec<Need>) {
        for taken in list.files.values() {
            let file = self.files.entry(Arc::clone(&taken.path)).or_default();
            match &file.stat {
                None => needs.push(Need::Stat {
                    path: Arc::clone(&taken.path),
                }),
                Some(Err(_)) => {}
                Some(Ok(stat)) => {
                    if file.layout.is_none() {
                        let len = stat.len.min(file.head_asked.max(HEAD_BYTES) as u64) as usize;
                        file.head_asked = file.head_asked.max(HEAD_BYTES);
                        needs.push(Need::Read {
                            path: Arc::clone(&taken.path),
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
            let clock = self.clock;
            let peaks = self.peaks.entry(Arc::clone(&path)).or_default();
            if usable(file, peaks) == Some(false) && clock >= peaks.checked + RECHECK {
                *peaks = Peaks {
                    checked: clock,
                    ..Peaks::default()
                };
            }
            match &peaks.stat {
                None => needs.push(Need::Stat { path }),
                Some(Err(_)) => {}
                Some(Ok(stat)) if peaks.head.is_none() => needs.push(Need::Read {
                    path,
                    offset: 0,
                    len: FileHead::BYTES.min(stat.len as usize),
                    what: What::FileHead,
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
                    let s = match list.tree.root() {
                        Some(root) => {
                            samples.node_span(list, root, 0, ch, (at, at + bucket), &mut read)
                        }
                        None => SpanStats::EMPTY,
                    };
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
            let mut at: Option<ListPart> = None;
            for flat in start..end {
                let (frame, ch) = (flat / channels, flat % channels);
                if !at
                    .as_ref()
                    .is_some_and(|p| p.span.start() <= frame && frame < p.span.end())
                {
                    at = list.part_at(frame);
                }
                let Some(part) = &at else {
                    values.push(0.0);
                    continue;
                };
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
    /// `/buffer_spectrum key start hop columns window [analysis [wintype]]`:
    /// **the
    /// columns of a spectrogram of the list over a span**, column `c`
    /// standing for the `hop` frames from `start + c * hop`, answered as
    /// `/buffer_spectrum.reply key start hop window channels blob`: each
    /// column's channels in turn, `window / 2` magnitudes each, a byte
    /// apiece as a texture stores them.
    ///
    /// A list is never read whole, so its spectrogram is never analyzed
    /// whole: a view asks for the span it shows at about a column of its
    /// own, and asks again when it moves. **A column is what happened in the
    /// frames it stands for**, each bin the largest it was there, as a pixel
    /// of a waveform is the extremes of its samples:
    ///
    /// - one at least a column of the file's spectral summary wide is the
    ///   summary's, folded over the levels and read with no sample
    ///   ([`Self::summarized`]), when the view analyzes as the summary does;
    /// - one wider than the view's own hop (`analysis`, half the window when
    ///   not said) is the largest of the windows on the grid of `analysis`
    ///   frames inside it, a quarter of a window apart at the closest and at
    ///   most [`COLUMN_WINDOWS`] of them;
    /// - one narrower is the one window at its start.
    ///
    /// A window inside one part is its file's, kept and found again after
    /// an edit ([`Self::composed`]); any other is read through the parts,
    /// fades and channel maps included, so what is analyzed is what plays.
    /// An answer reads at most [`SPECTRUM_BLOCKS`] blocks it does not hold
    /// and analyzes at most [`SPECTRUM_WINDOWS`] windows; past them it
    /// answers the columns it could, at least one, and the view asks for the
    /// rest from where it ended.
    fn spectrum_reply(&mut self, key: i32, list: &List, args: &[OscType]) -> Answer {
        let int = |i: usize| args.get(i).and_then(as_i64).unwrap_or(0).max(0) as usize;
        let (start, hop, columns, window) = (int(0), int(1).max(1), int(2), int(3));
        let analysis = match int(4) {
            0 => window / 2,
            h => h,
        }
        .max(1);
        let wintype = args.get(5).and_then(as_i64).unwrap_or(0) as i32;
        if !clausters_core::fft::supports(window) || columns == 0 {
            return Answer::None;
        }
        let channels = list.channels.max(1);
        let mut needs = Vec::new();
        let stored = window == spectra::WINDOW
            && analysis == spectra::HOP
            && wintype == 0
            && hop >= spectra::COLUMN_FRAMES;
        if stored {
            self.spectra_shapes(list, &mut needs);
            if !needs.is_empty() {
                return Answer::Need(dedup(needs));
            }
        }
        let mut out: Vec<u8> = Vec::with_capacity(columns * channels * window / 2);
        let mut answered = 0;
        let spent = |a: &Self| a.columns_analyzed + a.list_windows;
        let first_spent = spent(self);
        for c in 0..columns {
            let first = start + c * hop;
            let before = needs.len();
            let column = self.column_of(
                list,
                (first, hop),
                (window, analysis, wintype),
                stored,
                &mut needs,
            );
            needs = dedup(std::mem::take(&mut needs));
            match column {
                // In order: a column is answered only after every one before it.
                Some(column) if needs.is_empty() => {
                    for channel in column {
                        out.extend_from_slice(&channel);
                    }
                    answered += 1;
                }
                Some(_) => {}
                None => {
                    // Answer what is here, or gather the reads of a few more
                    // columns before asking for any of them.
                    if answered > 0 {
                        needs.truncate(before);
                        break;
                    }
                    if needs.len() >= SPECTRUM_BLOCKS {
                        break;
                    }
                }
            }
            if answered > 0 && spent(self) - first_spent >= SPECTRUM_WINDOWS {
                break;
            }
        }
        if answered == 0 && !needs.is_empty() {
            return Answer::Need(dedup(needs));
        }
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

    /// **One column of the list**, each of its channels' bins, or `None`
    /// with what it needs read noted (see [`Self::spectrum_reply`]).
    fn column_of(
        &mut self,
        list: &List,
        (first, hop): (usize, usize),
        (window, analysis, wintype): (usize, usize, i32),
        stored: bool,
        needs: &mut Vec<Need>,
    ) -> Option<Vec<Arc<[u8]>>> {
        let channels = list.channels.max(1);
        if stored {
            let end = (first + hop).min(list.frames);
            match self.summarized(list, (first, end), hop, channels, needs) {
                Composed::Columns(columns) => {
                    self.columns_summarized += 1;
                    return Some(columns);
                }
                Composed::Need => return None,
                Composed::No => {}
            }
        }
        let shape = (window, wintype);
        if hop <= analysis {
            return self.one_window(list, first, shape, hop, channels, needs);
        }
        // The largest of the windows the column stands for, each on the
        // grid of the view's own hop, which an edit or a zoom finds again.
        // They are taken a quarter of a window apart at the closest, still on
        // that grid: a window's taper weighs a frame between two of them at
        // 85% or more, so a closer one measures nothing a pixel can show,
        // and at a hop of a few frames a column of a few hundred analyzed a
        // window every frame. A column so wide it would still analyze more
        // than a few dozen is spread further.
        let stride = (window / 4).max(hop / COLUMN_WINDOWS);
        let analysis = stride.div_ceil(analysis).max(1) * analysis;
        let bins = window / 2;
        let mut largest = vec![vec![0u8; bins]; channels];
        let mut whole = true;
        let end = (first + hop).min(list.frames).max(first + 1);
        let mut at = first;
        while at < end {
            match self.one_window(list, at, shape, analysis, channels, needs) {
                Some(column) => {
                    for (into, channel) in largest.iter_mut().zip(column) {
                        for (o, m) in into.iter_mut().zip(channel.iter()) {
                            *o = (*o).max(*m);
                        }
                    }
                }
                None => whole = false,
            }
            at += analysis;
        }
        whole.then(|| largest.into_iter().map(Arc::from).collect())
    }

    /// **The window of `window` frames at list frame `first`**: its file's
    /// on the grid of `grid` frames when it lies inside one part
    /// ([`Self::composed`]), and read through the list and analyzed
    /// otherwise; `None` with what it needs read noted.
    fn one_window(
        &mut self,
        list: &List,
        first: usize,
        (window, wintype): (usize, i32),
        grid: usize,
        channels: usize,
        needs: &mut Vec<Need>,
    ) -> Option<Vec<Arc<[u8]>>> {
        let before = needs.len();
        match self.composed(list, first, (window, wintype), grid, channels, needs) {
            Composed::Columns(columns) => return (needs.len() == before).then_some(columns),
            Composed::Need => return None,
            Composed::No => {}
        }
        let mut values = vec![0.0f32; window * channels];
        {
            let mut samples = FileSamples {
                answerer: self,
                bucket: 1,
                needs,
                hot: None,
                cold: None,
            };
            let mut at: Option<ListPart> = None;
            for i in 0..window {
                let f = first + i;
                if f >= list.frames {
                    break;
                }
                if !at
                    .as_ref()
                    .is_some_and(|p| p.span.start() <= f && f < p.span.end())
                {
                    at = list.part_at(f);
                }
                let Some(part) = &at else {
                    continue;
                };
                let inner = f - part.span.start();
                for ch in 0..channels {
                    values[i * channels + ch] = part
                        .span
                        .read_with(inner, ch, |f, c| samples.frame(part, f, c));
                }
            }
        }
        if needs.len() > before {
            return None;
        }
        let bins = window / 2;
        let (hann, gain) = clausters_core::stft::analysis_window_of(window, wintype);
        let (mut windowed, mut spectrum) = (vec![0.0f32; window], vec![0.0f32; bins]);
        let (mut frame, mut column) = (vec![0.0f32; window], vec![0.0f32; bins]);
        let mut out = Vec::with_capacity(channels);
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
            out.push(
                column
                    .iter()
                    .map(|m| clausters_core::stft::quantize(*m))
                    .collect(),
            );
        }
        self.samples_read += window * channels;
        self.list_windows += 1;
        Some(out)
    }
}

impl Answerer {
    /// What every file of `list` needs before its spectral summary can be
    /// read: the summary's size and age, and its head. A file with no peaks
    /// has none.
    fn spectra_shapes(&mut self, list: &List, needs: &mut Vec<Need>) {
        let clock = self.clock;
        for taken in list.files.values() {
            let Some(peaks) = &taken.peaks else {
                continue;
            };
            let name: Arc<str> = spectra::beside_peaks(peaks).into();
            let file = self.files.entry(Arc::clone(&taken.path)).or_default();
            let summary = self.spectra.entry(Arc::clone(&name)).or_default();
            if usable_spectra(file, summary) == Some(false) && clock >= summary.checked + RECHECK {
                *summary = SpectraFile {
                    checked: clock,
                    ..SpectraFile::default()
                };
            }
            match &summary.stat {
                None => needs.push(Need::Stat { path: name }),
                Some(Ok(stat)) if summary.head.is_none() => needs.push(Need::Read {
                    path: name,
                    offset: 0,
                    len: spectra::Head::BYTES.min(stat.len as usize),
                    what: What::SpectraHead,
                }),
                Some(_) => {}
            }
        }
    }

    /// **A column of the list over frames `a..b` from its files' spectral
    /// summaries**: for each part the span crosses, the columns of its
    /// file's finest level whose middle falls in what the part plays there,
    /// folded through the largest aligned blocks of the levels, so a column
    /// costs the logarithm of what it stands for and reads no sample.
    /// A fade lowers what a part gives by the largest gain it has in the
    /// span; a part read at another rate, or a file with no usable summary,
    /// is not in any summary, and the column is analyzed from the samples.
    fn summarized(
        &mut self,
        list: &List,
        (a, b): (usize, usize),
        hop: usize,
        channels: usize,
        needs: &mut Vec<Need>,
    ) -> Composed {
        // Edges on the grid of a level an eighth of the column wide, so a
        // column reads the levels it is drawn at, as a coarse column of
        // peaks does.
        let mut grid = 1;
        while grid * 2 <= hop / spectra::COLUMN_FRAMES / 8 {
            grid *= 2;
        }
        if b <= a {
            return Composed::No;
        }
        let mut out = vec![vec![0u8; spectra::BINS]; channels];
        let mut missing = false;
        let placed: Vec<_> = list
            .tree
            .iter_from(a as u64)
            .take_while(|p| (p.start as usize) < b)
            .filter_map(|p| list.placed(p))
            .collect();
        for part in placed {
            let span = &part.span;
            if span.resampled() {
                return Composed::No;
            }
            let Some(peaks) = &part.peaks else {
                return Composed::No;
            };
            let name: Arc<str> = spectra::beside_peaks(peaks).into();
            let Some(head) = self.summary_head(&part.path, &name) else {
                return Composed::No;
            };
            let (pa, pb) = (a.max(span.start()), b.min(span.end()));
            if pa >= pb {
                continue;
            }
            let shift = fade_shift(span, pa - span.start(), pb - span.start());
            if shift == u8::MAX {
                continue;
            }
            let from = span.src_start + (pa - span.start());
            let (k0, k1) = middles(from, from + (pb - pa), head.lens[0], grid);
            for (ch, into) in out.iter_mut().enumerate() {
                let Some(source) = span
                    .map
                    .get(ch)
                    .and_then(|&s| usize::try_from(s).ok())
                    .filter(|&s| s < head.channels)
                else {
                    continue;
                };
                for (level, k) in aligned_blocks(head.lens.len(), k0, k1) {
                    match self.summary_column(&name, &head, source, level, k, needs) {
                        Some((block, at)) => {
                            for (o, m) in into.iter_mut().zip(&block[at..at + spectra::BINS]) {
                                *o = (*o).max(m.saturating_sub(shift));
                            }
                        }
                        None => missing = true,
                    }
                }
            }
        }
        if missing {
            return Composed::Need;
        }
        Composed::Columns(out.into_iter().map(Arc::from).collect())
    }

    /// The head of the spectral summary `name` of the file at `path`, when
    /// it is there and describes the file.
    fn summary_head(&self, path: &str, name: &str) -> Option<spectra::Head> {
        let file = self.files.get(path)?;
        let summary = self.spectra.get(name)?;
        if usable_spectra(file, summary) != Some(true) {
            return None;
        }
        summary.head.as_ref()?.as_ref().ok().cloned()
    }

    /// Column `k` of level `level` of channel `channel` of the summary
    /// `name`: the block it is in and where in it, or a need and `None`.
    fn summary_column(
        &mut self,
        name: &Arc<str>,
        head: &spectra::Head,
        channel: usize,
        level: usize,
        k: usize,
        needs: &mut Vec<Need>,
    ) -> Option<(Arc<[u8]>, usize)> {
        let block = k / SPECTRA_BLOCK;
        let clock = self.clock;
        let summary = self.spectra.get_mut(name)?;
        if let Some(held) = summary.blocks.get_mut(&(channel, level, block)) {
            held.used = clock;
            return Some((
                Arc::clone(&held.value),
                (k - block * SPECTRA_BLOCK) * spectra::BINS,
            ));
        }
        let first = block * SPECTRA_BLOCK;
        let n = SPECTRA_BLOCK.min(head.lens[level] - first);
        needs.push(Need::Read {
            path: Arc::clone(name),
            offset: head.offset(channel, level, first),
            len: n * spectra::BINS,
            what: What::Spectra {
                channel,
                level,
                block,
            },
        });
        None
    }
}

/// **The columns of a summary's finest level whose middle falls in source
/// frames `from..to`**, as a range of them: each column is in exactly one of
/// a run of spans laid end to end, off by at most half its width. Each edge
/// is then rounded to a multiple of `grid` columns, which keeps that: what a
/// view column a few dozen summary columns wide gains is reading the levels
/// it is drawn at and not the finest, off by at most half a grid. A span
/// narrower than a column takes the column it starts in. `n` is how many the
/// level has.
fn middles(from: usize, to: usize, n: usize, grid: usize) -> (usize, usize) {
    let (width, half) = (spectra::COLUMN_FRAMES, spectra::COLUMN_FRAMES / 2);
    let first = |f: usize| (f.saturating_sub(half).div_ceil(width) + grid / 2) / grid * grid;
    let (k0, k1) = (first(from).min(n), first(to).min(n));
    if k0 < k1 {
        return (k0, k1);
    }
    let k = (from / width).min(n.saturating_sub(1));
    (k, k + 1)
}

/// **How far a part's fades lower what it gives over frames `i0..i1` of it**,
/// in steps of a stored magnitude: by its largest gain there, which is one
/// where any of the span is outside both fades. [`u8::MAX`] where the gain is
/// nothing.
fn fade_shift(span: &clausters_core::segments::PartSpan, i0: usize, i1: usize) -> u8 {
    if !span.faded() || i1 <= i0 {
        return 0;
    }
    let level = span.frames.saturating_sub(span.fade_out);
    if i0 < level && i1 > span.fade_in {
        return 0;
    }
    let gain = span.gain(i0).max(span.gain(i1 - 1));
    if gain <= 0.0 {
        return u8::MAX;
    }
    let db = 20.0 * gain.log10();
    let steps = (db / clausters_core::stft::REF_FLOOR * 255.0).round();
    steps.clamp(0.0, 254.0) as u8
}

impl Answerer {
    /// **One column of a list from its file's columns**, when its window lies
    /// inside one part, clear of the part's fades and at the list's rate: the
    /// window the file holds there, at the nearest point of a grid of `hop`
    /// frames laid over the file's own frames, so the same column serves the
    /// part wherever an edit places it. A column off the grid by up to half a
    /// hop is one the view draws a column wide anyway.
    fn composed(
        &mut self,
        list: &List,
        first: usize,
        (window, wintype): (usize, i32),
        hop: usize,
        channels: usize,
        needs: &mut Vec<Need>,
    ) -> Composed {
        if first + window > list.frames {
            return Composed::No;
        }
        let Some(part) = list.part_at(first) else {
            return Composed::No;
        };
        let span = &part.span;
        let inside = first >= span.start() + span.fade_in
            && first + window <= span.end().saturating_sub(span.fade_out);
        if span.resampled() || !inside {
            return Composed::No;
        }
        let at = (span.src_start + (first - span.start()) + hop / 2) / hop;
        if at * hop + window > span.src_frames() {
            return Composed::No;
        }
        let bins = window / 2;
        let mut out = Vec::with_capacity(channels);
        for ch in 0..channels {
            let source = span.map.get(ch).copied().unwrap_or(-1);
            let Ok(source) = usize::try_from(source) else {
                // a channel mapped to nothing is silence, which analyzes to
                // the floor
                out.push(Arc::from(vec![0u8; bins]));
                continue;
            };
            let key = ColumnKey {
                path: Arc::clone(&part.path),
                channel: source,
                window,
                wintype,
                hop,
                at,
            };
            if let Some(column) = self.columns.get(&key) {
                self.columns_reused += 1;
                out.push(Arc::clone(column));
                continue;
            }
            let before = needs.len();
            let mut frame = vec![0.0f32; window];
            {
                let mut samples = FileSamples {
                    answerer: self,
                    bucket: 1,
                    needs,
                    hot: None,
                    cold: None,
                };
                for (i, x) in frame.iter_mut().enumerate() {
                    *x = samples.frame(&part, at * hop + i, source);
                }
            }
            if needs.len() > before {
                return Composed::Need;
            }
            let (hann, gain) = clausters_core::stft::analysis_window_of(window, wintype);
            let mut windowed = vec![0.0f32; window];
            let mut spectrum = vec![0.0f32; bins];
            let mut magnitudes = vec![0.0f32; bins];
            clausters_core::stft::column_into(
                &frame,
                &hann,
                gain,
                &mut windowed,
                &mut spectrum,
                &mut magnitudes,
            );
            let column: Arc<[u8]> = magnitudes
                .iter()
                .map(|m| clausters_core::stft::quantize(*m))
                .collect();
            self.samples_read += window;
            self.columns_analyzed += 1;
            self.keep_column(key, Arc::clone(&column));
            out.push(column);
        }
        Composed::Columns(out)
    }

    /// Keeps `column`, letting go of the oldest past [`COLUMN_BYTES_HELD`].
    fn keep_column(&mut self, key: ColumnKey, column: Arc<[u8]>) {
        self.column_bytes += column.len();
        self.column_order.push_back(key.clone());
        self.columns.insert(key, column);
        while self.column_bytes > COLUMN_BYTES_HELD {
            let Some(oldest) = self.column_order.pop_front() else {
                break;
            };
            if let Some(gone) = self.columns.remove(&oldest) {
                self.column_bytes -= gone.len();
            }
        }
    }
}

/// Blocks of samples one spectrum answer reads at most before it answers:
/// a window straddles two at most, so this is about a hundred columns of a
/// zoomed-out view, and a few megabytes of a stereo float file.
pub const SPECTRUM_BLOCKS: usize = 192;

/// **Windows one column is the largest of, at most**: past it the windows
/// are spread further apart, since a pixel cannot show what a finer hop adds
/// and a column of seconds at a hop of a few frames would analyze thousands.
pub const COLUMN_WINDOWS: usize = 64;

/// Windows one spectrum answer analyzes at most once it has a column to
/// answer: about six milliseconds natively, so a view of minutes pooled from
/// its windows comes over many short answers rather than a few long ones. A
/// page answers on its own thread and gives itself a turn before each
/// (`web::disk`), so none of them holds a frame back; the total is the same.
/// Measured over a four-minute stereo take at a thousand columns of a
/// hundred seconds: 4096 windows an answer took 47 ms at the longest, 128
/// take 6, both about 300 ms in all.
pub const SPECTRUM_WINDOWS: usize = 128;

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
    fn peaks_of(&self, path: &str) -> Option<(Arc<str>, FileHead)> {
        let file = self.answerer.files.get(path)?;
        let name = file.peaks.clone()?;
        let peaks = self.answerer.peaks.get(&name)?;
        if usable(file, peaks) != Some(true) {
            return None;
        }
        let head = peaks.head.as_ref()?.as_ref().ok()?;
        Some((name, head.clone()))
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
        head: &FileHead,
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
        (name, head): (&Arc<str>, &FileHead),
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
        for (level, index) in aligned_blocks(head.lens.len(), first, last) {
            let count = head.count(level, index);
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

impl FileSamples<'_> {
    /// **The list's `channel` over frames `a..b`, from `node`**, which
    /// starts at frame `start`: a node the span holds whole is its kept
    /// summary, or its children's folded and then kept; one it crosses is
    /// its children's, down to the parts it cuts, which are read as a list
    /// of one.
    fn node_span(
        &mut self,
        list: &List,
        node: &Node,
        start: usize,
        channel: usize,
        (a, b): (usize, usize),
        read: &mut usize,
    ) -> SpanStats {
        let end = start + node.frames() as usize;
        if end <= a || start >= b {
            return SpanStats::EMPTY;
        }
        let whole = a <= start && end <= b;
        let key = (node.id(), channel);
        if whole && let Some(kept) = self.answerer.nodes.get(&key) {
            return *kept;
        }
        let asked = self.needs.len();
        let stats = match node.children() {
            Some((l, r)) => {
                let mid = start + l.frames() as usize;
                let mut stats = self.node_span(list, l, start, channel, (a, b), read);
                stats.merge(self.node_span(list, r, mid, channel, (a, b), read));
                stats
            }
            None => {
                let Some(part) = node.part().and_then(|part| {
                    list.placed(Placed {
                        start: start as u64,
                        frames: node.frames(),
                        part,
                    })
                }) else {
                    return SpanStats::silence(end.min(b) - start.max(a));
                };
                self.answerer.parts_read += 1;
                list_span(std::slice::from_ref(&part), self, channel, a, b, read)
            }
        };
        // kept only once every part under it answered from what is held
        if whole && self.needs.len() == asked {
            if self.answerer.nodes.len() >= NODES_HELD {
                self.answerer.nodes.clear();
            }
            self.answerer.nodes.insert(key, stats);
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
