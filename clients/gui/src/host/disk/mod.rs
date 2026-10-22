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

use clausters_core::osc::{OscMessage, OscType};
use clausters_core::segments::{Cut, FadeShape, PartSpan, place, rate_between};
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

/// **A take as a list of segments over files**: its shape and its parts,
/// placed end to end.
#[derive(Debug, Clone)]
pub struct List {
    pub channels: usize,
    pub sample_rate: f64,
    pub frames: usize,
    pub parts: Vec<ListPart>,
    /// What it was parsed from, which is what says two lists are the same.
    wire: WireList,
}

impl PartialEq for List {
    fn eq(&self, other: &Self) -> bool {
        self.wire == other.wire
    }
}

impl List {
    /// Reads a `segments` prop, refusing what cannot be drawn: no parts, a
    /// part of no frames, a rate that is not a positive number, a map of the
    /// wrong width, or a span past the end of its file.
    ///
    /// A def carries the list as JSON; a `/gui_set` carries it as its JSON
    /// text, since an OSC key/value is a scalar.
    pub fn parse(value: &serde_json::Value) -> Result<Self, String> {
        let wire: WireList = match value {
            serde_json::Value::String(text) => serde_json::from_str(text),
            other => serde_json::from_value(other.clone()),
        }
        .map_err(|e| format!("segments: {e}"))?;
        Self::from_wire(wire)
    }

    pub fn from_wire(wire: WireList) -> Result<Self, String> {
        let channels = wire.channels;
        if channels == 0 {
            return Err("segments: a list has at least one channel".into());
        }
        if wire.parts.is_empty() {
            return Err("segments: a list has at least one part".into());
        }
        let mut cuts = Vec::with_capacity(wire.parts.len());
        for (n, w) in wire.parts.iter().enumerate() {
            let p = &w.part;
            let rate = rate_between(p.source_rate, wire.sample_rate)
                .ok_or_else(|| format!("segments: part {n}: a rate of zero is no axis"))?;
            let frames = usize::try_from(p.frames).map_err(|e| format!("part {n}: {e}"))?;
            if frames == 0 {
                return Err(format!("segments: part {n} has no frames"));
            }
            let start = usize::try_from(p.start).map_err(|e| format!("part {n}: {e}"))?;
            let source_frames =
                usize::try_from(p.source_frames).map_err(|e| format!("part {n}: {e}"))?;
            let reads = start as f64 + frames as f64 * rate;
            if reads > source_frames as f64 + 1.0 {
                return Err(format!(
                    "segments: part {n} reads to frame {reads} of {}, which has {source_frames}",
                    p.path
                ));
            }
            let source_channels = p.source_channels as usize;
            let map: Vec<i32> = match &p.channels {
                Some(map) if map.len() != channels => {
                    return Err(format!(
                        "segments: part {n}: the map has {} entries and the list {channels} channels",
                        map.len()
                    ));
                }
                Some(map) => map
                    .iter()
                    .map(|&c| if c >= source_channels as i32 { -1 } else { c })
                    .collect(),
                None => (0..channels as i32)
                    .map(|c| if c < source_channels as i32 { c } else { -1 })
                    .collect(),
            };
            let shape = |s: &clausters_document::session::SeamShape| FadeShape {
                shape: s.shape,
                curve: s.curve as f32,
            };
            let cut = Cut {
                src_start: start,
                frames,
                fade_in: p.fade_in as usize,
                fade_out: p.fade_out as usize,
                fade_in_shape: shape(&p.fade_in_shape),
                fade_out_shape: shape(&p.fade_out_shape),
                map,
            };
            cuts.push((cut, source_frames, rate));
        }
        let (spans, frames) = place(cuts);
        let parts = spans
            .into_iter()
            .zip(&wire.parts)
            .map(|(span, w)| ListPart {
                path: Arc::from(w.part.path.as_str()),
                peaks: w.peaks.as_deref().map(Arc::from),
                source_channels: w.part.source_channels as usize,
                span,
            })
            .collect();
        Ok(Self {
            channels,
            sample_rate: wire.sample_rate,
            frames,
            parts,
            wire,
        })
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
    }
}
