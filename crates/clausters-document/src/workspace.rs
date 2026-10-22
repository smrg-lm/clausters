//! **Where a session's renders are written, and when one goes.**
//!
//! In an editor whose takes live on disk, an edit's processing is a render: an
//! offline server writes the result to a new file, and the edit makes a part
//! of the list name it. That file is a source of the session like any other,
//! with one difference -- nobody chose to keep it. So a render is a source of
//! [`Lifetime::Temporary`] in a **workspace**, the folder the session's
//! renders are written to:
//!
//! - **the audio editor's** is temporary: renders and peaks go there, nothing
//!   is written beside the user's file, and the folder goes when the editor
//!   closes ([`Workspace::temporary`]);
//! - **a multitrack's** is the session's own folder, which a save keeps
//!   ([`Workspace::session`]).
//!
//! A render goes **as soon as nothing names it**: not the document, not an
//! entry of the history (which an undo or a redo would read again), and not
//! the clipboard. [`Session::release_renders`] walks the three and hands back
//! the files to delete; deleting them is the caller's, since this crate does
//! no I/O. A save writes a session that names no temporary file
//! ([`Session::for_saving`]): a render the document still names is kept, in
//! the session's folder, and the ones it does not are not written down.
//!
//! **A clipboard holds segments by location.** A copy of a span of a take is
//! the parts it is made of, each naming its file by an absolute path
//! ([`crate::clipboard::SegmentPart`]), so a copy of an hour is a few parts
//! and a paste into another session is an edit of a list whose sources that
//! session adds by the same paths ([`Session::copy_segments`],
//! [`Session::paste_segments`]).

use std::path::{Path, PathBuf};

use crate::clipboard::{Clipboard, SegmentPart};
use crate::history::History;
use crate::session::{Location, Part, Session, Source};
use crate::{Lifetime, Range, SourceId, SourceRef};

/// The folder a session's renders are written to, and whether it outlives
/// the session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    /// The folder.
    pub folder: PathBuf,
    /// Whether it goes when the session closes: the audio editor's does, a
    /// multitrack's -- the session's own folder -- does not.
    pub temporary: bool,
}

impl Workspace {
    /// A folder that goes when the session closes: the audio editor's.
    pub fn temporary(folder: impl Into<PathBuf>) -> Self {
        Self {
            folder: folder.into(),
            temporary: true,
        }
    }

    /// The session's own folder, which a save keeps: a multitrack's. A
    /// relative path in the session resolves against it.
    pub fn session(folder: impl Into<PathBuf>) -> Self {
        Self {
            folder: folder.into(),
            temporary: false,
        }
    }

    /// Where `path` is: as written when absolute, and against the session's
    /// folder when relative and the workspace is that folder.
    pub fn resolve(&self, path: &str) -> PathBuf {
        let p = Path::new(path);
        if p.is_absolute() || self.temporary {
            p.to_path_buf()
        } else {
            self.folder.join(p)
        }
    }

    /// Whether `path` is inside this folder.
    pub fn holds(&self, path: &str) -> bool {
        self.resolve(path).starts_with(&self.folder)
    }

    /// The path a render with this id is written at.
    pub fn render_path(&self, id: SourceId, extension: &str) -> PathBuf {
        self.folder.join(format!("render-{}.{extension}", id.0))
    }
}

impl Session {
    /// An id no source of this session has.
    pub fn next_source_id(&self) -> SourceId {
        SourceId(self.sources.keys().map(|id| id.0 + 1).max().unwrap_or(1))
    }

    /// **Adds a render**: a temporary source in the workspace, of this shape,
    /// and the path the offline server is to write it at.
    pub fn add_render(
        &mut self,
        workspace: &Workspace,
        channels: u32,
        frames: u64,
        sample_rate: f64,
        extension: &str,
    ) -> (SourceId, PathBuf) {
        let id = self.next_source_id();
        let path = workspace.render_path(id, extension);
        self.sources.insert(
            id,
            Source::file(path.to_string_lossy(), Lifetime::Temporary).shaped(
                channels,
                frames,
                sample_rate,
            ),
        );
        (id, path)
    }

    /// **The sources the document names**: the regions' and the general
    /// tree's, and through a join every source its parts read.
    pub fn named(&self) -> Vec<SourceId> {
        let mut named = Vec::new();
        for region in self.multitrack.regions() {
            if let Some(source) = region.content.source() {
                named.push(source);
            }
        }
        self.document.walk(&mut |node| match &node.body {
            crate::Body::Vector { source, .. } => named.push(source.source),
            crate::Body::Segments { segments, .. } => named.extend(
                segments
                    .iter()
                    .filter_map(|s| s.source.samples())
                    .map(|s| s.source),
            ),
            _ => {}
        });
        self.reached(named)
    }

    /// `roots` and every source a join among them reads, once each.
    fn reached(&self, roots: Vec<SourceId>) -> Vec<SourceId> {
        let mut out: Vec<SourceId> = Vec::new();
        let mut todo = roots;
        while let Some(id) = todo.pop() {
            if out.contains(&id) {
                continue;
            }
            out.push(id);
            if let Some(Source {
                location: Location::Segments { parts },
                ..
            }) = self.sources.get(&id)
            {
                todo.extend(parts.iter().map(|p| p.source.source));
            }
        }
        out
    }

    /// **The parts of a copy, by location**: `parts` read as spans of files,
    /// a part over a join read through to the files it is spans of. Refuses
    /// a part whose samples are not in a file of known shape -- those are
    /// copied as samples, not as segments.
    pub fn copy_segments(
        &self,
        parts: &[Part],
        workspace: &Workspace,
    ) -> Result<Vec<SegmentPart>, String> {
        let mut out = Vec::new();
        for part in parts {
            self.copy_part(part, workspace, 0, &mut out)?;
        }
        Ok(out)
    }

    fn copy_part(
        &self,
        part: &Part,
        workspace: &Workspace,
        depth: usize,
        out: &mut Vec<SegmentPart>,
    ) -> Result<(), String> {
        let id = part.source.source;
        let source = self
            .sources
            .get(&id)
            .ok_or_else(|| format!("source {} is not in the session", id.0))?;
        let frames = source.frames;
        let range = part.source.range.unwrap_or(Range {
            start: 0,
            end: frames.unwrap_or(0),
        });
        match &source.location {
            Location::File { path } => {
                let (Some(channels), Some(frames), Some(rate)) =
                    (source.channels, source.frames, source.sample_rate)
                else {
                    return Err(format!(
                        "source {}: a file whose shape is not known is not copied as segments",
                        id.0
                    ));
                };
                out.push(SegmentPart {
                    path: workspace.resolve(path).to_string_lossy().into_owned(),
                    start: range.start,
                    frames: range.len(),
                    fade_in: part.fade_in,
                    fade_out: part.fade_out,
                    fade_in_shape: part.fade_in_shape,
                    fade_out_shape: part.fade_out_shape,
                    channels: part.channels.clone(),
                    source_channels: channels,
                    source_frames: frames,
                    source_rate: rate,
                });
                Ok(())
            }
            Location::Segments { parts } if depth < 8 => {
                // The span of the join this part reads, cut out of the parts
                // it is made of: a piece cut at an edge takes this part's
                // fade there.
                let mut at = 0u64;
                let first = out.len();
                for inner in parts {
                    let len = self.part_frames(inner);
                    let (lo, hi) = (at.max(range.start), (at + len).min(range.end));
                    if lo < hi {
                        let mut piece = inner.clone();
                        let base = inner.source.range.map_or(0, |r| r.start);
                        piece.source.range = Some(Range {
                            start: base + (lo - at),
                            end: base + (hi - at),
                        });
                        if lo > at {
                            piece.fade_in = 0;
                        }
                        if hi < at + len {
                            piece.fade_out = 0;
                        }
                        self.copy_part(&piece, workspace, depth + 1, out)?;
                    }
                    at += len;
                }
                if let Some(head) = out.get_mut(first)
                    && part.fade_in > 0
                {
                    head.fade_in = part.fade_in;
                    head.fade_in_shape = part.fade_in_shape;
                }
                if out.len() > first
                    && let Some(tail) = out.last_mut()
                    && part.fade_out > 0
                {
                    tail.fade_out = part.fade_out;
                    tail.fade_out_shape = part.fade_out_shape;
                }
                Ok(())
            }
            _ => Err(format!(
                "source {}: only spans of files are copied as segments",
                id.0
            )),
        }
    }

    /// How many frames a part contributes: its range, or its whole source.
    fn part_frames(&self, part: &Part) -> u64 {
        match part.source.range {
            Some(r) => r.len(),
            None => self
                .sources
                .get(&part.source.source)
                .and_then(|s| s.frames)
                .unwrap_or(0),
        }
    }

    /// **A paste of segments**: the parts as this session's, each file found
    /// in the table by its path or added to it -- a render of this
    /// workspace's as temporary, anyone else's file as the user's own.
    pub fn paste_segments(&mut self, parts: &[SegmentPart], workspace: &Workspace) -> Vec<Part> {
        parts
            .iter()
            .map(|clip| {
                let path = workspace.resolve(&clip.path);
                let found = self.sources.iter().find_map(|(id, s)| match &s.location {
                    Location::File { path: p } if workspace.resolve(p) == path => {
                        Some((*id, s.lifetime, s.generation))
                    }
                    _ => None,
                });
                let (id, lifetime, generation) = match found {
                    Some(found) => found,
                    None => {
                        let id = self.next_source_id();
                        let lifetime = if workspace.holds(&clip.path) {
                            Lifetime::Temporary
                        } else {
                            Lifetime::External
                        };
                        self.sources.insert(
                            id,
                            Source::file(clip.path.clone(), lifetime).shaped(
                                clip.source_channels,
                                clip.source_frames,
                                clip.source_rate,
                            ),
                        );
                        (id, lifetime, 0)
                    }
                };
                Part {
                    source: SourceRef {
                        source: id,
                        lifetime,
                        generation,
                        range: Some(Range {
                            start: clip.start,
                            end: clip.start + clip.frames,
                        }),
                    },
                    fade_in: clip.fade_in,
                    fade_out: clip.fade_out,
                    fade_in_shape: clip.fade_in_shape,
                    fade_out_shape: clip.fade_out_shape,
                    channels: clip.channels.clone(),
                }
            })
            .collect()
    }

    /// Whether `id` is a render of `workspace`: a temporary file inside it.
    fn is_render(&self, id: SourceId, workspace: &Workspace) -> bool {
        matches!(
            self.sources.get(&id),
            Some(Source {
                location: Location::File { path },
                lifetime: Lifetime::Temporary,
                ..
            }) if workspace.holds(path)
        )
    }

    /// **Lets go of the renders nothing names**: neither the document, nor an
    /// entry of `history` an undo or a redo would read, nor `clipboard`. They
    /// leave the source table, and their files are handed back for the caller
    /// to delete.
    pub fn release_renders(
        &mut self,
        workspace: &Workspace,
        history: &History,
        clipboard: Option<&Clipboard>,
    ) -> Vec<PathBuf> {
        let mut roots = self.named();
        roots.extend(self.reached(history.held_sources()));
        let copied: Vec<PathBuf> = clipboard
            .map(|c| {
                c.files()
                    .into_iter()
                    .map(|f| workspace.resolve(f))
                    .collect()
            })
            .unwrap_or_default();
        let gone: Vec<SourceId> = self
            .sources
            .iter()
            .filter(|(id, s)| {
                self.is_render(**id, workspace)
                    && !roots.contains(id)
                    && !matches!(&s.location, Location::File { path }
                        if copied.contains(&workspace.resolve(path)))
            })
            .map(|(id, _)| *id)
            .collect();
        gone.into_iter()
            .filter_map(|id| match self.sources.remove(&id) {
                Some(Source {
                    location: Location::File { path },
                    ..
                }) => Some(workspace.resolve(&path)),
                _ => None,
            })
            .collect()
    }

    /// **The session as a save writes it**, naming no temporary file: a render
    /// the document names is kept, as a source of the session's own folder,
    /// written relative to it; one it does not name is not written down. A
    /// workspace that goes when the session closes cannot keep one, and a
    /// render the document names there is refused, by name.
    pub fn for_saving(&self, workspace: &Workspace) -> Result<Session, String> {
        let named = self.named();
        let mut saved = self.clone();
        saved.sources.retain(|id, s| {
            s.lifetime != Lifetime::Temporary || named.contains(id) || s.editing.is_some()
        });
        for (id, source) in saved.sources.iter_mut() {
            if source.lifetime != Lifetime::Temporary || source.editing.is_some() {
                continue;
            }
            let Location::File { path } = &source.location else {
                continue;
            };
            if workspace.temporary || !workspace.holds(path) {
                return Err(format!(
                    "source {} is a temporary file ({path}) the document names: a saved \
                     session names none, so render it into the session's folder first",
                    id.0
                ));
            }
            let full = workspace.resolve(path);
            let relative = full
                .strip_prefix(&workspace.folder)
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|_| path.clone());
            source.location = Location::File { path: relative };
            source.lifetime = Lifetime::Session;
        }
        Ok(saved)
    }
}

#[cfg(test)]
mod tests;
