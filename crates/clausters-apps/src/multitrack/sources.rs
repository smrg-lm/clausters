//! **A session's sources, resolved where the application reads them**: each
//! take's file and its header, each join's parts, and the peaks a box is
//! drawn from -- read through the core's file layer, which is the disk
//! natively and the page's own storage in a tab.
//!
//! A session's table says where each source is ([`Location`]). A file named
//! by a relative path is in the session's folder, which is what makes the
//! folder movable; an absolute one is the user's own, which a session never
//! copies or rewrites. **The peaks of a take are kept in the session's
//! folder** (`peaks/`), never beside the user's file: opening a session
//! again reads them and reads no sample, and one that is missing is measured
//! once, by a render, while the window is already open.

use std::collections::HashMap;

use clausters_document::SourceId;
use clausters_document::parts::TakeFile;
use clausters_document::session::{Location, Part, Session};
use clausters_editing::apply::Step;

use crate::editing::Header;

/// The folder a session's peaks are kept in, inside its own.
pub const PEAKS: &str = "peaks";

/// **A session's sources as the application reads them.**
#[derive(Debug, Default)]
pub struct Resolved {
    /// Each take's file, its header read; its peaks named when they are
    /// there to read.
    pub takes: HashMap<SourceId, TakeFile>,
    /// Each join's parts.
    pub segments: HashMap<SourceId, Vec<Part>>,
    /// Each sequence of events the session holds, which its boxes of notes
    /// read.
    pub sequences: HashMap<SourceId, clausters_document::events::EventSequence>,
    /// The renders that measure the peaks a take has not got yet, each with
    /// the source it is for and where it writes.
    pub measure: Vec<(SourceId, String, Step)>,
    /// The sources that are not there, and why.
    pub unresolved: Vec<(SourceId, String)>,
}

/// **Where `path` is**, as a session in `folder` names it: in the folder
/// when it is relative.
pub fn located(folder: &str, path: &str) -> String {
    if path.starts_with('/') || folder.is_empty() {
        path.to_string()
    } else {
        format!("{}/{path}", folder.trim_end_matches('/'))
    }
}

/// **How a session in `folder` names `path`**: relative when it is inside
/// the folder, as it is otherwise.
pub fn named(folder: &str, path: &str) -> String {
    let folder = format!("{}/", folder.trim_end_matches('/'));
    match path.strip_prefix(&folder) {
        Some(inside) if !folder.is_empty() && !inside.contains("..") => inside.to_string(),
        _ => path.to_string(),
    }
}

/// **Where the peaks of the take at `path` are kept**, for a session in
/// `folder`: its `peaks/` folder, named after the file and its path, so two
/// takes of one name are two files.
pub fn peaks_for(folder: &str, path: &str) -> String {
    crate::audio::editor::peaks_for_opened(&located(folder, PEAKS), path)
}

/// **The sources of `session`, in `folder`, resolved**: each file's header
/// read through `header`, its peaks found in the session's `peaks/` or a
/// render to measure them; each join's parts; a source only a running
/// system held, unresolved.
pub fn resolve(
    session: &Session,
    folder: &str,
    header: &mut dyn FnMut(&str) -> Result<Header, String>,
) -> Resolved {
    let mut resolved = Resolved::default();
    for (id, source) in &session.sources {
        match &source.location {
            Location::File { path } => {
                let path = located(folder, path);
                match header(&path) {
                    Ok(info) => {
                        let peaks = peaks_for(folder, &path);
                        let file = TakeFile {
                            path: path.as_str().into(),
                            peaks: clausters_core::files::exists(&peaks)
                                .then(|| peaks.as_str().into()),
                            channels: info.channels,
                            frames: info.frames,
                            rate: info.rate,
                        };
                        if file.peaks.is_none() {
                            let part = clausters_document::clipboard::SegmentPart {
                                path: path.clone(),
                                start: 0,
                                frames: info.frames,
                                fade_in: 0,
                                fade_out: 0,
                                fade_in_shape: Default::default(),
                                fade_out_shape: Default::default(),
                                channels: None,
                                source_channels: info.channels,
                                source_frames: info.frames,
                                source_rate: info.rate,
                            };
                            let label = crate::audio::renders::operation("open_file");
                            let step = crate::audio::renders::measure(&part, &peaks, &label);
                            resolved.measure.push((*id, peaks, step));
                        }
                        resolved.takes.insert(*id, file);
                    }
                    Err(why) => resolved.unresolved.push((*id, why)),
                }
            }
            Location::Segments { parts } => {
                resolved.segments.insert(*id, parts.clone());
            }
            Location::Events { sequence } => {
                resolved.sequences.insert(*id, (**sequence).clone());
            }
            _ => resolved
                .unresolved
                .push((*id, "its samples were never written down".into())),
        }
    }
    resolved
}

/// **`session` as a session in `folder` writes it**: every file inside the
/// folder named relative to it, so the folder can be moved.
pub fn written(mut session: Session, folder: &str) -> Session {
    for source in session.sources.values_mut() {
        if let Location::File { path } = &mut source.location {
            *path = named(folder, path);
        }
    }
    session
}

#[cfg(test)]
mod tests {
    use super::*;
    use clausters_document::session::Source;
    use clausters_document::{Document, Lifetime};

    #[test]
    fn a_path_inside_the_session_s_folder_is_written_relative() {
        assert_eq!(located("/s", "takes/a.wav"), "/s/takes/a.wav");
        assert_eq!(located("/s", "/home/u/a.wav"), "/home/u/a.wav");
        assert_eq!(named("/s", "/s/takes/a.wav"), "takes/a.wav");
        assert_eq!(named("/s", "/home/u/a.wav"), "/home/u/a.wav");
        assert_eq!(named("/s", "/s/../a.wav"), "/s/../a.wav");
        assert!(peaks_for("/s", "/s/takes/a.wav").starts_with("/s/peaks/a-"));
    }

    /// A file's header is read and its peaks, missing, are measured into
    /// the session's folder; a join is its parts; a volatile source is not
    /// there.
    #[test]
    fn a_session_s_sources_resolve_through_the_file_layer() {
        let session = Session::new(Document::empty())
            .with_source(SourceId(1), Source::file("takes/a.wav", Lifetime::Session))
            .with_source(SourceId(2), Source::volatile(Lifetime::Session));
        let resolved = resolve(&session, "/nowhere-s", &mut |path| {
            assert_eq!(path, "/nowhere-s/takes/a.wav");
            Ok(Header {
                frames: 480,
                channels: 2,
                rate: 48_000.0,
                codec: "wav".into(),
                sample_format: None,
            })
        });
        let take = &resolved.takes[&SourceId(1)];
        assert_eq!((take.channels, take.frames), (2, 480));
        assert!(take.peaks.is_none());
        assert_eq!(resolved.measure.len(), 1);
        assert!(resolved.measure[0].1.starts_with("/nowhere-s/peaks/"));
        assert_eq!(resolved.unresolved[0].0, SourceId(2));
    }
}
