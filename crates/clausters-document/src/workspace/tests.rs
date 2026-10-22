//! A render lives while something names it, a copy names its files by
//! location, and a save names no temporary file.

use super::*;
use crate::history::{Entry, Step};
use crate::session::SeamShape;
use crate::{Body, Document, Node, NodeId, Opaque};

const TAKE: SourceId = SourceId(1);
const ORIGINAL: SourceId = SourceId(2);

fn part(source: SourceId, start: u64, end: u64) -> Part {
    Part {
        source: SourceRef {
            source,
            lifetime: Lifetime::External,
            generation: 0,
            range: Some(Range { start, end }),
        },
        fade_in: 0,
        fade_out: 0,
        fade_in_shape: SeamShape::default(),
        fade_out_shape: SeamShape::default(),
        channels: None,
    }
}

/// A session whose document plays the take, a join over the user's file.
fn session(parts: Vec<Part>) -> Session {
    let root = Node::new(
        NodeId(1),
        Body::Vector {
            source: SourceRef {
                source: TAKE,
                lifetime: Lifetime::Session,
                generation: 0,
                range: None,
            },
            config: Opaque::default(),
        },
    );
    Session::new(Document::new(root))
        .with_source(
            ORIGINAL,
            Source::file("/user/take.wav", Lifetime::External).shaped(2, 48_000, 48_000.0),
        )
        .with_source(
            TAKE,
            Source {
                location: Location::Segments { parts },
                ..Source::volatile(Lifetime::Session)
            },
        )
}

/// The take's list, put back as an edit or its undo would.
fn play(session: &mut Session, parts: Vec<Part>) {
    session.sources.get_mut(&TAKE).unwrap().location = Location::Segments { parts };
}

fn entry(
    history: &mut History,
    structure: crate::history::StructureId,
    label: &str,
    forward: Vec<SourceId>,
    backward: Vec<SourceId>,
) {
    let e = Entry::new(
        label,
        structure,
        Step::Edit(Opaque::default()),
        Opaque::default(),
    )
    .holding(forward, backward);
    assert!(history.record(e));
}

/// **The history's walk frees exactly the renders nothing names**: through a
/// sequence of edits, an undo, a redo dropped by the next edit, a clipboard
/// that names one and a history cleared.
#[test]
fn a_render_goes_when_nothing_names_it() {
    let ws = Workspace::temporary("/tmp/editor-1");
    let whole = vec![part(ORIGINAL, 0, 48_000)];
    let mut s = session(whole.clone());
    let mut history = History::new();
    let take = history.register("take");

    // Two edits, each a render spliced into the list.
    let (r1, _) = s.add_render(&ws, 2, 100, 48_000.0, "w64");
    let one = vec![
        part(ORIGINAL, 0, 100),
        part(r1, 0, 100),
        part(ORIGINAL, 200, 48_000),
    ];
    play(&mut s, one.clone());
    entry(
        &mut history,
        take,
        "gain",
        vec![ORIGINAL, r1],
        vec![ORIGINAL],
    );
    let (r2, _) = s.add_render(&ws, 2, 50, 48_000.0, "w64");
    let two = vec![
        part(ORIGINAL, 0, 100),
        part(r1, 0, 50),
        part(r2, 0, 50),
        part(ORIGINAL, 200, 48_000),
    ];
    play(&mut s, two);
    entry(
        &mut history,
        take,
        "reverse",
        vec![ORIGINAL, r1, r2],
        vec![ORIGINAL, r1],
    );
    assert!(
        s.release_renders(&ws, &history, None).is_empty(),
        "both are named"
    );

    // Undone, the second render is held by the redo.
    history.undo();
    play(&mut s, one.clone());
    assert!(
        s.release_renders(&ws, &history, None).is_empty(),
        "a redo would read it"
    );

    // An edit after the undo drops the redo, and the render with it.
    let (r3, _) = s.add_render(&ws, 2, 10, 48_000.0, "w64");
    play(
        &mut s,
        vec![
            part(ORIGINAL, 0, 100),
            part(r1, 0, 100),
            part(r3, 0, 10),
            part(ORIGINAL, 210, 48_000),
        ],
    );
    entry(
        &mut history,
        take,
        "fade",
        vec![ORIGINAL, r1, r3],
        vec![ORIGINAL, r1],
    );
    assert_eq!(
        s.release_renders(&ws, &history, None),
        vec![ws.render_path(r2, "w64")],
        "exactly the render the dropped redo held"
    );
    assert!(s.source(r2).is_none(), "and it left the table");

    // A clipboard names the first render; the history is cleared and the take
    // is back to the whole file.
    let copied = s.copy_segments(&[part(r1, 0, 100)], &ws).unwrap();
    let clipboard = Clipboard::segments(2, 48_000.0, copied);
    play(&mut s, whole);
    let gone = s.release_renders(&ws, &History::new(), Some(&clipboard));
    assert_eq!(
        gone,
        vec![ws.render_path(r3, "w64")],
        "the copy keeps the first render"
    );
    let gone = s.release_renders(&ws, &History::new(), None);
    assert_eq!(
        gone,
        vec![ws.render_path(r1, "w64")],
        "and lets it go once it is gone"
    );
    assert!(s.release_renders(&ws, &History::new(), None).is_empty());
    assert!(
        s.source(ORIGINAL).is_some(),
        "the user's file is never a render"
    );
}

/// **A copy keeps its sources by location across two sessions**: a part over
/// a file the first session names relative to its folder is pasted into
/// another as that file, by its absolute path, and a second paste finds it.
#[test]
fn a_copy_across_two_sessions_keeps_its_sources_by_location() {
    let here = Workspace::session("/projects/a");
    let mut a = session(vec![]);
    a.sources.insert(
        SourceId(7),
        Source::file("audio/drums.wav", Lifetime::Session).shaped(1, 96_000, 48_000.0),
    );
    let mut cut = part(SourceId(7), 1_000, 5_000);
    cut.fade_in = 32;
    let copied = a.copy_segments(&[cut], &here).unwrap();
    assert_eq!(copied[0].path, "/projects/a/audio/drums.wav");
    assert_eq!(
        (copied[0].start, copied[0].frames, copied[0].fade_in),
        (1_000, 4_000, 32)
    );

    // Across the clipboard as text.
    let clipboard = Clipboard::parse(&Clipboard::segments(1, 48_000.0, copied).to_json());
    assert_eq!(clipboard.kind(), "segments");
    assert_eq!(clipboard.blobs(), 0, "no sample travels");
    let crate::clipboard::Content::Segments { parts, frames, .. } = &clipboard.content else {
        panic!("a clipboard of segments");
    };
    assert_eq!(*frames, 4_000);

    let there = Workspace::session("/projects/b");
    let mut b = session(vec![]);
    let pasted = b.paste_segments(parts, &there);
    let id = pasted[0].source.source;
    let source = b.source(id).unwrap();
    assert_eq!(
        source.location,
        Location::File {
            path: "/projects/a/audio/drums.wav".into()
        }
    );
    assert_eq!(
        source.lifetime,
        Lifetime::External,
        "another session's file is not ours"
    );
    assert_eq!((source.channels, source.frames), (Some(1), Some(96_000)));
    assert_eq!(
        pasted[0].source.range,
        Some(Range {
            start: 1_000,
            end: 5_000
        })
    );
    assert_eq!(pasted[0].fade_in, 32);
    let again = b.paste_segments(parts, &there);
    assert_eq!(
        again[0].source.source, id,
        "a second paste finds the file it added"
    );
}

/// A part over a join is copied as the files the join is spans of, cut at
/// the copy's edges.
#[test]
fn a_copy_of_a_join_names_the_files_under_it() {
    let ws = Workspace::temporary("/tmp/editor-2");
    let mut s = session(vec![part(ORIGINAL, 0, 100), part(ORIGINAL, 300, 400)]);
    let (r, _) = s.add_render(&ws, 2, 100, 48_000.0, "w64");
    play(
        &mut s,
        vec![
            part(ORIGINAL, 0, 100),
            part(r, 0, 100),
            part(ORIGINAL, 300, 400),
        ],
    );
    let mut over = part(TAKE, 50, 250);
    over.source.lifetime = Lifetime::Session;
    over.fade_out = 8;
    let copied = s.copy_segments(&[over], &ws).unwrap();
    let spans: Vec<(&str, u64, u64)> = copied
        .iter()
        .map(|p| (p.path.as_str(), p.start, p.frames))
        .collect();
    let render = ws.render_path(r, "w64").to_string_lossy().into_owned();
    assert_eq!(
        spans,
        vec![
            ("/user/take.wav", 50, 50),
            (render.as_str(), 0, 100),
            ("/user/take.wav", 300, 50)
        ]
    );
    assert_eq!(copied[2].fade_out, 8, "the copy's own edge keeps its fade");
}

/// **A saved session names no temporary file**: a render the document names
/// is kept in the session's folder, relative to it; one it does not is not
/// written down; and a workspace that goes cannot keep one.
#[test]
fn a_saved_session_names_no_temporary_file() {
    let ws = Workspace::session("/projects/c");
    let mut s = session(vec![]);
    let (kept, _) = s.add_render(&ws, 2, 100, 48_000.0, "w64");
    let (dropped, _) = s.add_render(&ws, 2, 100, 48_000.0, "w64");
    play(&mut s, vec![part(ORIGINAL, 0, 100), part(kept, 0, 100)]);
    let saved = s.for_saving(&ws).unwrap();
    assert!(
        saved
            .sources
            .values()
            .all(|s| s.lifetime != Lifetime::Temporary),
        "no temporary source is written down"
    );
    assert!(saved.source(dropped).is_none(), "one nothing names is not");
    let kept = saved.source(kept).unwrap();
    assert_eq!(kept.lifetime, Lifetime::Session);
    assert_eq!(
        kept.location,
        Location::File {
            path: "render-3.w64".into()
        }
    );

    let editor = Workspace::temporary("/tmp/editor-3");
    let mut e = session(vec![]);
    let (r, _) = e.add_render(&editor, 2, 100, 48_000.0, "w64");
    play(&mut e, vec![part(r, 0, 100)]);
    let refused = e.for_saving(&editor).unwrap_err();
    assert!(refused.contains("temporary"), "{refused}");
}
