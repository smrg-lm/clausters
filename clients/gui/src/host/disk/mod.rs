//! **A take that is a list of segments over files**, drawn by the host with no
//! server: what a `signal` named by `segments` shows.
//!
//! An application's takes live on disk (`docs/decisions.md`, "An
//! application's takes live on disk, and the server holds audio functions"):
//! a take is a list of parts, each a span of a file with its own fades and
//! channel map, and an edit is a new list over files that never change. So
//! the picture of a take is composed from its files' peaks, and the samples
//! under a zoom are read from the files, by the host itself -- natively from
//! the disk, in a page from its own storage (OPFS).
//!
//! **A list is a tree of shared parts** ([`List`]): what an application the
//! host holds hands its window by reference ([`receive`]), and a client's
//! list written out and read into one. A picture is composed from the nodes
//! each column holds whole, so it costs the columns and not the parts.
//!
//! **It goes through the fetch machine the host already has.** A list gets a
//! **take key** ([`is_list_key`]), a buffer number in a range no server
//! allocates, and the views over it ask for its shape, its summary at the
//! level they draw and the span under a zoom exactly as they ask a server for
//! a buffer's -- `/buffer_query`, `/buffer_peaks`, `/buffer_getRange` -- and
//! are answered in the server's own reply formats. What differs is only where
//! a message goes: a front sends a list key's to the host's [`DiskLink`]
//! instead of to a server, so asking at the drawn level, placing a picture
//! whole, keeping it across an edit and asking again after a lost reply are
//! the same code for a buffer and for a list.
//!
//! The answering is [`answer::Answerer`], which does no I/O: it says what it
//! needs read ([`answer::Need`]) and is handed the bytes, so one answerer
//! serves the native front, whose thread reads the disk ([`native`]), and a
//! page, which reads OPFS with promises.

use std::sync::Arc;

use std::collections::BTreeMap;

use clausters_core::osc::{OscMessage, OscType};
use clausters_core::segments::{Cut, FadeShape, PartSpan, rate_between};
use clausters_document::parts::tree::Placed;
use clausters_document::parts::{FileList, TakeFile, Tree};
use clausters_document::session::Part;
use clausters_document::{Lifetime, Range, SourceId, SourceRef};
use serde::Deserialize;

pub mod answer;
#[cfg(not(target_arch = "wasm32"))]
pub mod native;
#[cfg(test)]
mod tests;

/// The first take key, counting down: a buffer number below every one a
/// server allocates and below `-1`, which the protocol uses for absence.
pub const FIRST_LIST_KEY: i32 = -2;

/// Whether `n` names a list rather than a server buffer.
pub fn is_list_key(n: i32) -> bool {
    n <= FIRST_LIST_KEY
}

/// The take key a fetch-machine message names, when it names a list: the
/// message is the host's to answer and never goes to a server.
pub fn list_key_of(msg: &OscMessage) -> Option<i32> {
    if !msg.addr.starts_with("/buffer_") {
        return None;
    }
    match msg.args.first() {
        Some(OscType::Int(n)) if is_list_key(*n) => Some(*n),
        _ => None,
    }
}

/// One part as a `segments` prop names it: the document's segment part (a
/// span of a file by its path, its fades, its map and the file's shape) and
/// where that file's peaks are.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct WirePart {
    #[serde(flatten)]
    pub part: clausters_document::clipboard::SegmentPart,
    /// The file's peaks (the CLPK format a peaks UGen writes). Absent, the
    /// part is summarized from its samples as it is asked, which is correct
    /// and slow over a long file.
    #[serde(default)]
    pub peaks: Option<String>,
}

/// The `segments` prop: a list's channels and rate, and its parts in order.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct WireList {
    pub channels: usize,
    pub sample_rate: f64,
    pub parts: Vec<WirePart>,
}

/// One part of a list, placed.
#[derive(Debug, Clone)]
pub struct ListPart {
    /// The file it reads.
    pub path: Arc<str>,
    /// Where that file's peaks are, if anywhere.
    pub peaks: Option<Arc<str>>,
    /// The file's channels, as the list was told.
    pub source_channels: usize,
    pub span: PartSpan,
}

impl AsRef<PartSpan> for ListPart {
    fn as_ref(&self) -> &PartSpan {
        &self.span
    }
}

/// **A take as a list of segments over files**: its shape, its parts as a
/// tree whose nodes are shared ([`Tree`]), and the files they read.
///
/// What an application hands its window is its own take's tree, by
/// reference ([`receive`]), so a list of any number of parts is named and
/// compared in the same few steps, and a picture of it is composed from the
/// nodes a column holds whole ([`answer`]).
#[derive(Debug, Clone)]
pub struct List {
    pub channels: usize,
    pub sample_rate: f64,
    pub frames: usize,
    pub tree: Tree,
    pub files: Arc<BTreeMap<SourceId, TakeFile>>,
    /// What a client's list was parsed from, which is what says two of
    /// those are the same.
    wire: Option<WireList>,
}

impl PartialEq for List {
    fn eq(&self, other: &Self) -> bool {
        match (&self.wire, &other.wire) {
            (Some(a), Some(b)) => a == b,
            (None, None) => {
                self.tree.same(&other.tree)
                    && self.channels == other.channels
                    && self.sample_rate == other.sample_rate
                    && self.files == other.files
            }
            _ => false,
        }
    }
}

thread_local! {
    /// The lists an application handed over, by the number its JSON names
    /// each with: read where a `segments` prop names one.
    static RECEIVED: std::cell::RefCell<std::collections::VecDeque<(u64, Arc<List>)>> =
        const { std::cell::RefCell::new(std::collections::VecDeque::new()) };
}

/// How many handed-over lists are kept for a prop to name: an answer names
/// one per picture, read before the next answer comes.
const RECEIVED_AT_MOST: usize = 64;

/// **Lists an application hands its windows by reference**, under the
/// numbers what it answers names them by (`{"shared": n, ...}`): kept for the
/// `segments` props that follow on this thread.
pub fn receive(lists: impl IntoIterator<Item = (u64, FileList)>) {
    RECEIVED.with(|r| {
        let mut r = r.borrow_mut();
        for (id, list) in lists {
            r.push_back((id, Arc::new(List::over(list))));
        }
        while r.len() > RECEIVED_AT_MOST {
            r.pop_front();
        }
    });
}

impl List {
    /// Reads a `segments` prop, refusing what cannot be drawn: a part of no
    /// frames, a rate that is not a positive number, a map of the wrong
    /// width, or a span past the end of its file. A list of no parts is an
    /// empty take, drawn as one. One an application handed over names it by
    /// number ([`receive`]).
    ///
    /// A def carries the list as JSON; a `/gui_set` carries it as its JSON
    /// text, since an OSC key/value is a scalar.
    pub fn parse(value: &serde_json::Value) -> Result<Arc<Self>, String> {
        let value = match value {
            serde_json::Value::String(text) => {
                serde_json::from_str(text).map_err(|e| format!("segments: {e}"))?
            }
            other => other.clone(),
        };
        if let Some(id) = value.get("shared").and_then(serde_json::Value::as_u64) {
            return RECEIVED
                .with(|r| {
                    r.borrow()
                        .iter()
                        .find(|(said, _)| *said == id)
                        .map(|(_, list)| Arc::clone(list))
                })
                .ok_or_else(|| format!("segments: no list was handed over as {id}"));
        }
        let wire: WireList = serde_json::from_value(value).map_err(|e| format!("segments: {e}"))?;
        Self::from_wire(wire).map(Arc::new)
    }

    /// **The list an application holds**, as its window draws it.
    pub fn over(list: FileList) -> Self {
        Self {
            channels: list.channels.max(1) as usize,
            sample_rate: list.rate,
            frames: list.tree.frames() as usize,
            tree: list.tree,
            files: list.files,
            wire: None,
        }
    }

    pub fn from_wire(wire: WireList) -> Result<Self, String> {
        let channels = wire.channels;
        if channels == 0 {
            return Err("segments: a list has at least one channel".into());
        }
        // a list of no parts is an empty take: a file that is a header
        let mut files: BTreeMap<SourceId, TakeFile> = BTreeMap::new();
        let mut ids: std::collections::HashMap<&str, SourceId> = std::collections::HashMap::new();
        let mut counted = Vec::with_capacity(wire.parts.len());
        for (n, w) in wire.parts.iter().enumerate() {
            let p = &w.part;
            let rate = rate_between(p.source_rate, wire.sample_rate)
                .ok_or_else(|| format!("segments: part {n}: a rate of zero is no axis"))?;
            if p.frames == 0 {
                return Err(format!("segments: part {n} has no frames"));
            }
            let reads = p.start as f64 + p.frames as f64 * rate;
            if reads > p.source_frames as f64 + 1.0 {
                return Err(format!(
                    "segments: part {n} reads to frame {reads} of {}, which has {}",
                    p.path, p.source_frames
                ));
            }
            if let Some(map) = &p.channels
                && map.len() != channels
            {
                return Err(format!(
                    "segments: part {n}: the map has {} entries and the list {channels} channels",
                    map.len()
                ));
            }
            let next = SourceId(ids.len() as u64);
            let id = *ids.entry(p.path.as_str()).or_insert(next);
            files.entry(id).or_insert_with(|| TakeFile {
                path: p.path.as_str().into(),
                peaks: w.peaks.as_deref().map(Arc::from),
                channels: p.source_channels,
                frames: p.source_frames,
                rate: p.source_rate,
            });
            let part = Part {
                source: SourceRef {
                    source: id,
                    lifetime: Lifetime::Temporary,
                    generation: 0,
                    range: Some(Range {
                        start: p.start,
                        end: p.start + p.frames,
                    }),
                },
                fade_in: p.fade_in,
                fade_out: p.fade_out,
                fade_in_shape: p.fade_in_shape,
                fade_out_shape: p.fade_out_shape,
                channels: p.channels.clone(),
            };
            // the wire counts a part in the list's frames already
            counted.push((part, p.frames));
        }
        let tree = Tree::from_counted(counted);
        Ok(Self {
            channels,
            sample_rate: wire.sample_rate,
            frames: tree.frames() as usize,
            tree,
            files: Arc::new(files),
            wire: Some(wire),
        })
    }

    /// **One part where the list reads it**, as the views draw it: its span
    /// over its file at the list's rate, its channels by the map every reader
    /// of a part uses.
    pub fn placed(&self, placed: Placed) -> Option<ListPart> {
        let file = self.files.get(&placed.part.source.source)?;
        let part = placed.part;
        let range = part.source.range?;
        let map = clausters_core::segments::channel_map(
            part.channels.as_deref(),
            file.channels as usize,
            self.channels,
        );
        let shape = |s: &clausters_document::session::SeamShape| FadeShape {
            shape: s.shape,
            curve: s.curve as f32,
        };
        let cut = Cut {
            src_start: range.start as usize,
            frames: placed.frames as usize,
            fade_in: part.fade_in as usize,
            fade_out: part.fade_out as usize,
            fade_in_shape: shape(&part.fade_in_shape),
            fade_out_shape: shape(&part.fade_out_shape),
            map,
        };
        let rate = rate_between(file.rate, self.sample_rate).unwrap_or(1.0);
        Some(ListPart {
            path: Arc::clone(&file.path),
            peaks: file.peaks.clone(),
            source_channels: file.channels as usize,
            span: PartSpan::new(cut, file.frames as usize, rate, placed.start as usize),
        })
    }

    /// **The part under frame `frame`**, found by descending the tree.
    pub fn part_at(&self, frame: usize) -> Option<ListPart> {
        self.tree
            .iter_from(frame as u64)
            .next()
            .and_then(|p| self.placed(p))
    }

    /// The parts in order, placed: what a test reads of a list.
    pub fn parts(&self) -> Vec<ListPart> {
        self.tree.iter().filter_map(|p| self.placed(p)).collect()
    }
}

/// **Where a front sends what concerns lists**: the host's own answerer,
/// behind whatever carries it -- a thread natively, the page's event loop in
/// a tab. Taken by `&self`, as a front's way to the server is.
pub trait DiskLink {
    /// Take key `key` now names `list` (a new one, or the list after an edit).
    fn define(&self, key: i32, list: Arc<List>);
    /// Nothing names `key` any more.
    fn forget(&self, key: i32);
    /// A fetch-machine message about a list, answered in the server's reply
    /// format through the front's way in from the server.
    fn ask(&self, msg: OscMessage);
}

impl crate::host::Host {
    /// **Gives the host its way to the answerer**, and tells it every list
    /// the windows already name.
    pub fn set_disk_link(&mut self, link: Box<dyn DiskLink>) {
        for (key, list) in &self.lists {
            link.define(*key, Arc::clone(list));
        }
        self.disk = Some(link);
    }

    /// **The take keys whose list changed** since the last call: a front
    /// drops what it asked about them before (`BufferFetches::forget`), since
    /// the answer is the new list's from wherever it had reached -- a summary
    /// walked on to the old length, the rest of the take drawn as silence.
    pub(crate) fn take_relisted(&mut self) -> Vec<i32> {
        std::mem::take(&mut self.relisted)
    }

    /// **Sends a fetch-machine message about a list to the answerer**, which
    /// answers it the way a server answers about a buffer.
    pub fn ask_disk(&self, msg: OscMessage) {
        match &self.disk {
            Some(disk) => disk.ask(msg),
            None => crate::host::diag::warn!(
                "{}: a list of segments is drawn here and this front reads no files",
                msg.addr
            ),
        }
    }

    /// **Brings the lists the windows name in step with the answerer**: a
    /// list with no take key yet is given one, a key whose list changed is
    /// told the new one, and a key nothing names any more is forgotten. Run
    /// wherever a tree or a list can have changed -- a def, a free, a set of
    /// `segments` -- before the front asks about any of them.
    pub(crate) fn sync_lists(&mut self) {
        let mut named: std::collections::HashMap<i32, Arc<List>> = std::collections::HashMap::new();
        let mut next = self.next_list_key;
        for tree in self.window_defs.values_mut() {
            super::takes::samples_views(tree, &mut |el| {
                let Some(list) = el.segments() else {
                    return false;
                };
                let key = match el.source_buffer() {
                    Some(key) if is_list_key(key) && !named.contains_key(&key) => key,
                    _ => {
                        let key = next;
                        next -= 1;
                        el.name_list(key);
                        key
                    }
                };
                named.insert(key, list);
                true
            });
        }
        self.next_list_key = next;
        for (key, list) in &named {
            if self.lists.get(key).is_some_and(|was| **was != **list) {
                self.relisted.push(*key);
            }
        }
        if let Some(disk) = &self.disk {
            for (key, list) in &named {
                if self.lists.get(key).is_none_or(|was| **was != **list) {
                    disk.define(*key, Arc::clone(list));
                }
            }
            for key in self.lists.keys().filter(|k| !named.contains_key(k)) {
                disk.forget(*key);
            }
        }
        self.lists = named;
        // **A list's length is known as it is named**, before its picture is
        // read: its axis spans it from the start, and a view set in the same
        // answer (a paste shown whole) stands against the take it is about.
        fn lengths(w: &super::widget::Widget, out: &mut Vec<(i32, usize)>) {
            if let (Some(id), Some(list)) = (w.id, w.kind.as_samples().and_then(|s| s.segments())) {
                out.push((id, list.frames));
            }
            for c in &w.children {
                lengths(c, out);
            }
        }
        let mut found = Vec::new();
        for tree in self.window_defs.values() {
            lengths(tree, &mut found);
        }
        for (id, frames) in found {
            self.set_take_total(id, frames);
        }
    }
}
