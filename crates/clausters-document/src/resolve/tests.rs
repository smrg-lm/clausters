//! A selection made on a clip's body resolves to the right
//! span of the take underneath it, trim and offset included.

use super::*;
use crate::{Grouping, Lifetime, Opaque, SegmentRef, SegmentSource, SourceRef};

/// The sample rate.
const FPS: f64 = 48_000.0;
/// How many frames a beat is under [`steady`], for the selections written in
/// frames.
const FPB: f64 = 48_000.0;

/// A beat is a second here, which keeps the arithmetic readable: a beat is
/// 48 000 frames. The two come apart in
/// `a_takes_length_is_seconds_and_the_tempo_does_not_move_it`, and the tempo
/// moves in the tests after it.
fn steady() -> TempoMap {
    TempoMap::new(1.0)
}

fn take(id: u64, source: u64, trim: Option<Range>) -> Node {
    Node::new(
        NodeId(id),
        Body::Vector {
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

fn placed(offset: Beats, dur: Option<f64>, node: Node) -> Member {
    Member { offset, dur, node }
}

fn aggregate(members: Vec<Member>) -> Node {
    Node::new(
        NodeId(1),
        Body::Aggregate {
            grouping: Grouping::Concrete,
            members,
            config: Opaque::none(),
        },
    )
}

/// One take placed at beat 2, four beats long, reading the source from frame
/// 480 000 (ten beats in) -- so placement and trim are different numbers and a
/// test cannot pass by confusing them.
fn one_clip() -> Document {
    Document::new(aggregate(vec![placed(
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
    // The selection covers the second beat *of the clip*,
    // which is beat 3 of the timeline and frame 528 000 of the take. Both terms
    // matter and getting either wrong is silent.
    let document = one_clip();
    let selection = Selection::span(3.0 * FPB, 1.0 * FPB);
    let resolved = resolve(&document, &selection, &Mapping::frames(&steady(), FPS));

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
    // placements reports beats and a view over samples reports frames, and
    // both mean the same span.
    let document = one_clip();
    let in_frames = resolve(
        &document,
        &Selection::span(3.0 * FPB, 1.0 * FPB),
        &Mapping::frames(&steady(), FPS),
    );
    let in_beats = resolve(
        &document,
        &Selection::span(3.0, 1.0),
        &Mapping::beats(&steady(), FPS),
    );
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
        &Mapping::frames(&steady(), FPS),
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
        &Mapping::frames(&steady(), FPS),
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
            &Mapping::frames(&steady(), FPS)
        )
        .is_empty()
    );
    assert!(
        resolve(
            &document,
            &Selection::cursor(3.0 * FPB),
            &Mapping::frames(&steady(), FPS)
        )
        .is_empty(),
        "a cursor selects nothing to operate on"
    );
}

// ---- more than one element ----

/// Two takes, in an aggregate placed at beat 10 -- so a nested base has to be
/// accumulated or the whole thing lands ten beats early.
fn nested() -> Document {
    let inner = Node::new(
        NodeId(10),
        Body::Aggregate {
            grouping: Grouping::Concrete,
            members: vec![
                placed(0.0, Some(2.0), take(11, 100, None)),
                placed(2.0, Some(2.0), take(12, 101, None)),
            ],
            config: Opaque::none(),
        },
    );
    Document::new(aggregate(vec![placed(10.0, None, inner)]))
}

#[test]
fn a_nested_placement_accumulates_its_base() {
    let document = nested();
    let resolved = resolve(
        &document,
        &Selection::span(11.0 * FPB, 2.0 * FPB),
        &Mapping::frames(&steady(), FPS),
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
        "and each multitrack says where it sits inside the selection"
    );
}

#[test]
fn a_selection_that_named_its_elements_resolves_only_those() {
    let document = nested();
    let selection = Selection::span(11.0 * FPB, 2.0 * FPB).of([NodeId(12)]);
    let resolved = resolve(&document, &selection, &Mapping::frames(&steady(), FPS));
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].node, NodeId(12));
}

#[test]
fn asking_about_one_element_gives_that_elements_span() {
    let document = nested();
    let selection = Selection::span(11.0 * FPB, 2.0 * FPB);
    let one = resolve_node(
        &document,
        NodeId(11),
        &selection,
        &Mapping::frames(&steady(), FPS),
    );
    assert_eq!(one.unwrap().node, NodeId(11));
    assert!(
        resolve_node(
            &document,
            NodeId(99),
            &selection,
            &Mapping::frames(&steady(), FPS)
        )
        .is_none()
    );
}

// ---- what has no span to give ----

#[test]
fn an_element_with_no_source_is_skipped_rather_than_reported() {
    // An aggregate and a generator are in the way of the selection, not underneath
    // it. The caller asked what is underneath.
    let document = Document::new(aggregate(vec![
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
        &Mapping::frames(&steady(), FPS),
    );
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].node, NodeId(3));
}

#[test]
fn a_placement_with_no_length_takes_it_from_the_trim() {
    // A clip dropped without an explicit length reads what its trim says, which
    // is the only other thing that knows how long it is.
    let document = Document::new(aggregate(vec![placed(
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
        &Mapping::frames(&steady(), FPS),
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
    // would be an operation reading samples the document never used.
    let document = Document::new(aggregate(vec![placed(0.0, None, take(2, 100, None))]));
    assert!(
        resolve(
            &document,
            &Selection::span(0.0, 4.0 * FPB),
            &Mapping::frames(&steady(), FPS)
        )
        .is_empty()
    );
}

#[test]
fn the_generation_travels_with_the_span() {
    // An operation reads samples, and a read taken against an older generation
    // is exactly the case the two counters exist for -- so it is part of the
    // answer rather than something the caller looks up afterwards.
    let document = one_clip();
    let resolved = resolve(
        &document,
        &Selection::span(2.0 * FPB, 1.0 * FPB),
        &Mapping::frames(&steady(), FPS),
    );
    assert_eq!(resolved[0].generation, 2);
}

#[test]
fn a_takes_length_is_seconds_and_the_tempo_does_not_move_it() {
    // The beat and the second come apart: two beats a second, so a beat is
    // 24 000 frames at 48 kHz. The clip is placed at beat 2 and lasts four *seconds*, so it
    // covers beats 2..10 of the arrangement -- a selection on beat 9 is still
    // inside it, which is what a length in beats would have got wrong (it would
    // have ended the clip at beat 6, halfway through the recording).
    let document = Document::new(aggregate(vec![placed(2.0, Some(4.0), take(2, 100, None))]));
    let tempo = TempoMap::new(2.0);
    let mapping = Mapping::beats(&tempo, 48_000.0);
    let resolved = resolve(&document, &Selection::span(9.0, 0.5), &mapping);

    assert_eq!(resolved.len(), 1);
    // Seven beats into the clip is three and a half seconds into the take.
    assert_eq!(
        resolved[0].range,
        Range {
            start: (3.5 * 48_000.0) as u64,
            end: (3.75 * 48_000.0) as u64,
        }
    );
}

/// One beat a second up to beat 4, two beats a second from there on: the
/// change falls on second 4, and beat 8 on second 6.
fn doubling_at_four() -> TempoMap {
    let mut tempo = TempoMap::new(1.0);
    tempo.push(4.0, 2.0).unwrap();
    tempo
}

/// **A selection crosses a tempo change and lands on the right frames at both
/// edges.** The take starts at beat 2 and is long enough to run through the
/// change at beat 4. A selection of beats 3..6 starts one second into the take
/// and lasts two seconds -- one before the change, and two beats at twice the
/// speed after it -- where one ratio would have said three seconds, or a
/// second and a half, depending on which side it was read from.
#[test]
fn a_selection_across_a_tempo_change_lands_on_the_right_frames_at_both_edges() {
    let document = Document::new(aggregate(vec![placed(2.0, Some(10.0), take(2, 100, None))]));
    let tempo = doubling_at_four();
    let resolved = resolve(
        &document,
        &Selection::span(3.0, 3.0),
        &Mapping::beats(&tempo, FPS),
    );
    assert_eq!(resolved.len(), 1);
    assert_eq!(
        resolved[0].range,
        Range {
            start: 48_000,
            end: 3 * 48_000,
        }
    );

    // The same stretch after the change is half as many frames: beats 6..9
    // are a second and a half, starting three seconds into the take.
    let after = resolve(
        &document,
        &Selection::span(6.0, 3.0),
        &Mapping::beats(&tempo, FPS),
    );
    assert_eq!(
        after[0].range,
        Range {
            start: 3 * 48_000,
            end: 3 * 48_000 + 72_000,
        }
    );
}

/// The same selection **in frames of the shared axis** names the same span:
/// frame 3 x 48 000 is second 3, which is beat 3 before the change, and the
/// selection's two seconds end on second 5, which is beat 6 after it.
#[test]
fn a_selection_in_frames_reads_its_beats_off_the_map() {
    let document = Document::new(aggregate(vec![placed(2.0, Some(10.0), take(2, 100, None))]));
    let tempo = doubling_at_four();
    let in_frames = resolve(
        &document,
        &Selection::span(3.0 * FPS, 2.0 * FPS),
        &Mapping::frames(&tempo, FPS),
    );
    let in_beats = resolve(
        &document,
        &Selection::span(3.0, 3.0),
        &Mapping::beats(&tempo, FPS),
    );
    assert_eq!(in_frames, in_beats);
}

/// **A take's length reaches a different beat depending on where it starts.**
/// Two takes of four seconds each: the one at beat 0 ends on the change, at
/// beat 4, and the one at beat 4 runs to beat 12 -- so a selection on beat 10
/// is under the second and would be past the end of both under either single
/// tempo read as the piece's.
#[test]
fn a_takes_end_is_read_off_the_map_from_where_it_starts() {
    let document = Document::new(aggregate(vec![
        placed(0.0, Some(4.0), take(2, 100, None)),
        placed(4.0, Some(4.0), take(3, 200, None)),
    ]));
    let tempo = doubling_at_four();
    let resolved = resolve(
        &document,
        &Selection::span(10.0, 1.0),
        &Mapping::beats(&tempo, FPS),
    );
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].node, NodeId(3));
    // Beat 10 is six beats into the second take, three seconds of it.
    assert_eq!(
        resolved[0].range,
        Range {
            start: 3 * 48_000,
            end: 3 * 48_000 + 24_000,
        }
    );
}

/// **Where a span starts inside the selection is in frames of time too**: a
/// selection from beat 2 that reaches a take starting at beat 6 finds it three
/// seconds in -- two before the change and one after -- and not four.
#[test]
fn where_a_span_starts_inside_the_selection_crosses_the_change_too() {
    let document = Document::new(aggregate(vec![placed(6.0, Some(4.0), take(2, 100, None))]));
    let tempo = doubling_at_four();
    let resolved = resolve(
        &document,
        &Selection::span(2.0, 8.0),
        &Mapping::beats(&tempo, FPS),
    );
    assert_eq!(resolved[0].at, 3 * 48_000);
}

/// A rate that measures nothing resolves nothing, rather than every span to
/// frame zero.
#[test]
fn a_degenerate_rate_resolves_nothing() {
    let document = one_clip();
    for rate in [0.0, -48_000.0, f64::NAN] {
        assert!(
            resolve(
                &document,
                &Selection::span(0.0, 100.0),
                &Mapping::beats(&steady(), rate)
            )
            .is_empty()
        );
    }
}

/// A window of `seconds` onto `source`, opening at second `start` of it.
fn window(source: u64, start: f64, seconds: f64) -> SegmentRef {
    SegmentRef {
        source: SegmentSource::Samples(SourceRef {
            source: SourceId(source),
            lifetime: Lifetime::External,
            generation: 1,
            range: None,
        }),
        start,
        duration: seconds,
    }
}

/// **Assembled windows are laid one after another in time, across a tempo
/// change.** Two windows of two seconds each from beat 2: the first ends on
/// the change at beat 4, and the second covers beats 4..8 at twice the speed.
/// A selection of beats 5..7 is under the second alone, half a second into
/// it and a second long -- where one ratio would have ended the run at beat
/// 6 or at beat 10.
#[test]
fn assembled_windows_are_laid_in_time_across_a_tempo_change() {
    let run = Node::new(
        NodeId(2),
        Body::Segments {
            segments: vec![window(100, 0.0, 2.0), window(200, 0.25, 2.0)],
            config: Opaque::none(),
        },
    );
    let document = Document::new(aggregate(vec![placed(2.0, None, run)]));
    let tempo = doubling_at_four();
    let resolved = resolve(
        &document,
        &Selection::span(5.0, 2.0),
        &Mapping::beats(&tempo, FPS),
    );
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].source, SourceId(200));
    assert_eq!(
        resolved[0].range,
        Range {
            // The window opens a quarter of a second into its source.
            start: 12_000 + 24_000,
            end: 12_000 + 24_000 + 48_000,
        }
    );

    // And a selection over the whole of it finds both, the second where the
    // first ended in time: two seconds into the selection's own frames.
    let both = resolve(
        &document,
        &Selection::span(2.0, 6.0),
        &Mapping::beats(&tempo, FPS),
    );
    assert_eq!(both.len(), 2);
    assert_eq!((both[0].at, both[1].at), (0, 2 * 48_000));
    assert_eq!(both[1].range.len(), 2 * 48_000);
}
