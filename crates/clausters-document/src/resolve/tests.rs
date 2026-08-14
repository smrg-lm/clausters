//! O9's acceptance: a selection made on a clip's body resolves to the right
//! span of the take underneath it, trim and offset included.

use super::*;
use crate::{Grouping, Lifetime, Opaque, SourceRef};

/// A beat is a second at 48 kHz here, which keeps the arithmetic readable.
const FPB: f64 = 48_000.0;

fn take(id: u64, source: u64, trim: Option<Range>) -> Node {
    Node::new(
        NodeId(id),
        Body::Buffer {
            source: SourceRef {
                source: SourceId(source),
                lifetime: Lifetime::External,
                generation: 2,
                range: trim,
            },
            config: Opaque::none(),
        },
    )
}

fn placed(offset: Beats, dur: Option<Beats>, node: Node) -> Member {
    Member { offset, dur, node }
}

fn set(members: Vec<Member>) -> Node {
    Node::new(
        NodeId(1),
        Body::Set {
            grouping: Grouping::Concrete,
            members,
        },
    )
}

/// One take placed at beat 2, four beats long, reading the source from frame
/// 480 000 (ten beats in) -- so placement and trim are different numbers and a
/// test cannot pass by confusing them.
fn one_clip() -> Document {
    Document::new(set(vec![placed(
        2.0,
        Some(4.0),
        take(
            2,
            100,
            Some(Range {
                start: 480_000,
                end: 480_000 + 4 * 48_000,
            }),
        ),
    )]))
}

#[test]
fn a_selection_on_a_clips_body_resolves_through_its_trim_and_its_offset() {
    // O9's acceptance. The selection covers the second beat *of the clip*,
    // which is beat 3 of the timeline and frame 528 000 of the take. Both terms
    // matter and getting either wrong is silent.
    let document = one_clip();
    let selection = Selection::span(3.0 * FPB, 1.0 * FPB);
    let resolved = resolve(&document, &selection, &Mapping::frames(FPB));

    assert_eq!(resolved.len(), 1);
    assert_eq!(
        resolved[0],
        Resolved {
            node: NodeId(2),
            source: SourceId(100),
            generation: 2,
            range: Range {
                start: 480_000 + 48_000,
                end: 480_000 + 96_000,
            },
            at: 0,
        }
    );
}

#[test]
fn the_same_selection_in_beats_lands_in_the_same_place() {
    // The unit is the reader's to declare, not the selection's: a view over
    // placements reports beats and a view over material reports frames, and
    // both mean the same span.
    let document = one_clip();
    let in_frames = resolve(
        &document,
        &Selection::span(3.0 * FPB, 1.0 * FPB),
        &Mapping::frames(FPB),
    );
    let in_beats = resolve(&document, &Selection::span(3.0, 1.0), &Mapping::beats(FPB));
    assert_eq!(in_frames, in_beats);
}

#[test]
fn a_selection_dragged_past_the_end_of_a_clip_resolves_to_what_the_clip_covers() {
    // Never past the end of a file. A span that reads beyond the trim is the
    // kind of thing an operation performs happily and a person hears as a click.
    let document = one_clip();
    let resolved = resolve(
        &document,
        &Selection::span(5.0 * FPB, 100.0 * FPB),
        &Mapping::frames(FPB),
    );
    assert_eq!(resolved.len(), 1);
    assert_eq!(
        resolved[0].range,
        Range {
            start: 480_000 + 3 * 48_000,
            end: 480_000 + 4 * 48_000,
        }
    );
}

#[test]
fn a_selection_that_starts_before_the_clip_resolves_from_the_clips_start() {
    let document = one_clip();
    let resolved = resolve(
        &document,
        &Selection::span(0.0, 3.0 * FPB),
        &Mapping::frames(FPB),
    );
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].range.start, 480_000, "the trim's own start");
    assert_eq!(resolved[0].range.end, 480_000 + 48_000);
    assert_eq!(
        resolved[0].at,
        2 * 48_000,
        "and it starts two beats into the selection"
    );
}

#[test]
fn a_selection_that_misses_the_clip_resolves_to_nothing() {
    let document = one_clip();
    assert!(
        resolve(
            &document,
            &Selection::span(20.0 * FPB, 1.0 * FPB),
            &Mapping::frames(FPB)
        )
        .is_empty()
    );
    assert!(
        resolve(
            &document,
            &Selection::cursor(3.0 * FPB),
            &Mapping::frames(FPB)
        )
        .is_empty(),
        "a cursor selects nothing to operate on"
    );
}

// ---- more than one element ----

/// Two takes, in a group placed at beat 10 -- so a nested base has to be
/// accumulated or the whole thing lands ten beats early.
fn nested() -> Document {
    let inner = Node::new(
        NodeId(10),
        Body::Set {
            grouping: Grouping::Concrete,
            members: vec![
                placed(0.0, Some(2.0), take(11, 100, None)),
                placed(2.0, Some(2.0), take(12, 101, None)),
            ],
        },
    );
    Document::new(set(vec![placed(10.0, None, inner)]))
}

#[test]
fn a_nested_placement_accumulates_its_base() {
    let document = nested();
    let resolved = resolve(
        &document,
        &Selection::span(11.0 * FPB, 2.0 * FPB),
        &Mapping::frames(FPB),
    );
    assert_eq!(resolved.len(), 2, "the selection crosses both takes");
    assert_eq!(resolved[0].node, NodeId(11));
    assert_eq!(
        resolved[0].range,
        Range {
            start: 48_000,
            end: 96_000
        }
    );
    assert_eq!(resolved[0].at, 0);
    assert_eq!(resolved[1].node, NodeId(12));
    assert_eq!(
        resolved[1].range,
        Range {
            start: 0,
            end: 48_000
        }
    );
    assert_eq!(
        resolved[1].at, 48_000,
        "and each piece says where it sits inside the selection"
    );
}

#[test]
fn a_selection_that_named_its_elements_resolves_only_those() {
    let document = nested();
    let selection = Selection::span(11.0 * FPB, 2.0 * FPB).of([NodeId(12)]);
    let resolved = resolve(&document, &selection, &Mapping::frames(FPB));
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].node, NodeId(12));
}

#[test]
fn asking_about_one_element_gives_that_elements_span() {
    let document = nested();
    let selection = Selection::span(11.0 * FPB, 2.0 * FPB);
    let one = resolve_node(&document, NodeId(11), &selection, &Mapping::frames(FPB));
    assert_eq!(one.unwrap().node, NodeId(11));
    assert!(resolve_node(&document, NodeId(99), &selection, &Mapping::frames(FPB)).is_none());
}

// ---- what has no span to give ----

#[test]
fn an_element_with_no_material_is_skipped_rather_than_reported() {
    // A group and a generator are in the way of the selection, not underneath
    // it. The caller asked what is underneath.
    let document = Document::new(set(vec![
        placed(
            0.0,
            Some(4.0),
            Node::new(
                NodeId(2),
                Body::Generator {
                    config: Opaque::none(),
                    rendered: None,
                },
            ),
        ),
        placed(0.0, Some(4.0), take(3, 100, None)),
    ]));
    let resolved = resolve(
        &document,
        &Selection::span(0.0, 4.0 * FPB),
        &Mapping::frames(FPB),
    );
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].node, NodeId(3));
}

#[test]
fn a_placement_with_no_length_takes_it_from_the_trim() {
    // A clip dropped without an explicit length reads what its trim says, which
    // is the only other thing that knows how long it is.
    let document = Document::new(set(vec![placed(
        0.0,
        None,
        take(
            2,
            100,
            Some(Range {
                start: 1_000,
                end: 1_000 + 96_000,
            }),
        ),
    )]));
    let resolved = resolve(
        &document,
        &Selection::span(0.0, 100.0 * FPB),
        &Mapping::frames(FPB),
    );
    assert_eq!(
        resolved[0].range,
        Range {
            start: 1_000,
            end: 1_000 + 96_000
        },
        "the whole of the two beats the trim covers, and no more"
    );
}

#[test]
fn a_placement_with_neither_a_length_nor_a_trim_gives_no_span() {
    // There is nothing to bound the read with, and guessing "the whole file"
    // would be an operation reading material the composition never used.
    let document = Document::new(set(vec![placed(0.0, None, take(2, 100, None))]));
    assert!(
        resolve(
            &document,
            &Selection::span(0.0, 4.0 * FPB),
            &Mapping::frames(FPB)
        )
        .is_empty()
    );
}

#[test]
fn the_generation_travels_with_the_span() {
    // An operation reads material, and a read taken against an older generation
    // is exactly the case the two counters exist for -- so it is part of the
    // answer rather than something the caller looks up afterwards.
    let document = one_clip();
    let resolved = resolve(
        &document,
        &Selection::span(2.0 * FPB, 1.0 * FPB),
        &Mapping::frames(FPB),
    );
    assert_eq!(resolved[0].generation, 2);
}
