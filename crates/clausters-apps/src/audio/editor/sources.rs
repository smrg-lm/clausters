//! **The files a take is made of**: the table an open file's list reads, and
//! what is made of it -- the list a window draws, the join the server plays,
//! the list a render reads, a copy by location.
//!
//! A take lives on disk: its list's parts are spans of files that never
//! change, the one it was opened from and the renders its edits wrote into
//! the editor's working folder. A source's id is a hash of its path, so a
//! file keeps its id across the files an editor holds, the history that
//! names it and the clipboard a copy of it crosses.

use std::collections::BTreeMap;

use clausters_core::osc::{OscMessage, OscType};
use clausters_document::SourceId;
use clausters_document::clipboard::SegmentPart;
use clausters_document::session::Part;
use serde_json::{Value, json};

use crate::audio::renders;

/// **One file a list reads**: where it is, where its peaks are, its shape,
/// and whether the editor wrote it (a render, which goes when nothing names
/// it).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FileSource {
    pub path: String,
    pub peaks: String,
    pub frames: u64,
    pub channels: u32,
    pub rate: f64,
    pub render: bool,
}

/// The table of a file's sources, by id.
pub(crate) type Sources = BTreeMap<SourceId, FileSource>;

/// **A file as it was on disk** when the editor last read it: when it was
/// last modified, in milliseconds since the epoch, and its size in bytes --
/// what a client's `stat` answers, natively and in a page alike.
#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub(crate) struct DiskStamp {
    pub modified: f64,
    pub size: u64,
}

/// **The id of the file at `path`**: a hash of the path (FNV-1a, 64 bits),
/// the same wherever the file is named.
pub(crate) fn id_of(path: &str) -> SourceId {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in path.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    // Clear of the small numbers a part over a buffer was named by.
    SourceId(hash | (1 << 62))
}

/// **Where the peaks of a file the editor opened are kept**: in its working
/// folder, named after the file and a hash of its path, so two files of one
/// name do not share them and nothing is written beside the user's file.
/// They go with the file's renders when it closes.
pub(crate) fn peaks_for_opened(workspace: &str, path: &str) -> String {
    let name = std::path::Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("take");
    format!("{workspace}/{name}-{:016x}.peaks", id_of(path).0)
}

/// Where the peaks of a render are: beside it.
pub(crate) fn peaks_beside(path: &str) -> String {
    match path.rfind('.') {
        Some(dot) if dot > path.rfind('/').map_or(0, |s| s + 1) => {
            format!("{}.peaks", &path[..dot])
        }
        _ => format!("{path}.peaks"),
    }
}

/// **The list's parts by location**: each part a span of its file, its
/// frames the file's, with what the file is -- what a copy carries. A part over a source the table does not hold is left
/// out, since nothing could read it.
pub(crate) fn segment_parts(parts: &[Part], sources: &Sources) -> Vec<SegmentPart> {
    parts
        .iter()
        .filter_map(|part| {
            let source = sources.get(&part.source.source)?;
            let range = part.source.range?;
            Some(SegmentPart {
                path: source.path.clone(),
                start: range.start,
                frames: range.len(),
                fade_in: part.fade_in,
                fade_out: part.fade_out,
                fade_in_shape: part.fade_in_shape,
                fade_out_shape: part.fade_out_shape,
                channels: part.channels.clone(),
                source_channels: source.channels,
                source_frames: source.frames,
                source_rate: source.rate,
            })
        })
        .collect()
}

/// **The list a render reads**: the parts over files, at `rate`, `channels`
/// wide; each part's contribution at the list's rate.
pub(crate) fn list(parts: &[Part], sources: &Sources, channels: u32, rate: f64) -> renders::List {
    let mut segments = segment_parts(parts, sources);
    for segment in &mut segments {
        segment.frames =
            clausters_document::parts::contributed(segment.frames, segment.source_rate, rate);
    }
    renders::List {
        parts: segments,
        channels,
        rate,
    }
}

/// **The list a window draws** (`segments`): the parts by location, each
/// with its file's peaks.
pub(crate) fn wire_list(parts: &[Part], sources: &Sources, channels: u32, rate: f64) -> Value {
    let list = list(parts, sources, channels, rate);
    let parts: Vec<Value> = list
        .parts
        .iter()
        .map(|part| {
            let mut value = serde_json::to_value(part).unwrap_or(Value::Null);
            if let Some(source) = sources.values().find(|s| s.path == part.path) {
                value["peaks"] = json!(source.peaks);
            }
            value
        })
        .collect();
    json!({"channels": channels, "sample_rate": rate, "parts": parts})
}

/// **The join the server plays**: `/buffer_stitch` of the list into buffer
/// `display`, at `rate`, each part over its file. Played by
/// `TransportDiskIn`, which reads it from the files ahead of the playhead.
pub(crate) fn stitch(display: i32, list: &renders::List) -> OscMessage {
    let frames = clausters_editing::apply::count_arg;
    let mut args = vec![
        OscType::Int(display),
        OscType::Int(list.channels as i32),
        OscType::Float(list.rate as f32),
    ];
    for part in &list.parts {
        args.push(OscType::String(part.path.clone()));
        args.push(frames(part.start));
        args.push(frames(part.frames));
        args.push(OscType::Int(part.fade_in as i32));
        args.push(OscType::Int(part.fade_out as i32));
        args.push(OscType::Int(part.fade_in_shape.shape));
        args.push(OscType::Float(part.fade_in_shape.curve as f32));
        args.push(OscType::Int(part.fade_out_shape.shape));
        args.push(OscType::Float(part.fade_out_shape.curve as f32));
        for c in 0..list.channels as i32 {
            let picked = match &part.channels {
                Some(map) => map.get(c as usize).copied().unwrap_or(-1),
                None => c % part.source_channels.max(1) as i32,
            };
            args.push(OscType::Int(picked));
        }
    }
    OscMessage {
        addr: "/buffer_stitch".into(),
        args,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_has_one_id_wherever_it_is_named_and_its_peaks_have_a_place() {
        assert_eq!(id_of("/a/take.wav"), id_of("/a/take.wav"));
        assert_ne!(id_of("/a/take.wav"), id_of("/b/take.wav"));
        assert!(id_of("x").0 >= 1 << 62);
        let peaks = peaks_for_opened("/work", "/music/take.wav");
        assert!(peaks.starts_with("/work/take-") && peaks.ends_with(".peaks"));
        assert_eq!(peaks_beside("/work/r1.w64"), "/work/r1.peaks");
        assert_eq!(peaks_beside("/work.d/r1"), "/work.d/r1.peaks");
    }
}
