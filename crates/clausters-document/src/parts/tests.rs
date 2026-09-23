use super::*;
use crate::{Lifetime, SourceRef};

fn part(source: u64, start: u64, end: u64) -> Part {
    Part {
        source: SourceRef {
            source: SourceId(source),
            lifetime: Lifetime::Temporary,
            generation: 0,
            range: Some(Range { start, end }),
        },
        fade_in: 0,
        fade_out: 0,
        channels: None,
    }
}

fn faded(mut part: Part, fade_in: u64, fade_out: u64) -> Part {
    part.fade_in = fade_in;
    part.fade_out = fade_out;
    part
}

/// Each part as `(source, start, end)`, which is what a test reads.
fn spans(parts: &[Part]) -> Vec<(u64, u64, u64)> {
    parts
        .iter()
        .map(|p| {
            let r = p.source.range.expect("ranged");
            (p.source.source.0, r.start, r.end)
        })
        .collect()
}

#[test]
fn a_list_is_as_long_as_its_parts_and_a_part_that_does_not_say_is_refused() {
    let list = [part(1, 0, 100), part(2, 50, 80)];
    assert_eq!(length(&list), Ok(130));
    let mut whole = part(3, 0, 0);
    whole.source.range = None;
    assert_eq!(length(&[whole.clone()]), Err(UNRANGED));
    assert_eq!(remove(&[whole], 0, 1), Err(UNRANGED));
}

#[test]
fn a_span_is_the_parts_it_crosses_each_cut_to_it() {
    let list = [part(1, 0, 100), part(2, 50, 80), part(3, 0, 10)];
    assert_eq!(
        spans(&span(&list, 90, 135).unwrap()),
        [(1, 90, 100), (2, 50, 80), (3, 0, 5)]
    );
    assert_eq!(
        spans(&span(&list, 135, 1000).unwrap()),
        [(3, 5, 10)],
        "cut at the end"
    );
    assert!(span(&list, 20, 20).unwrap().is_empty());
}

#[test]
fn a_fade_is_kept_only_where_its_end_of_the_part_is_kept() {
    let list = [faded(part(1, 0, 100), 8, 8)];
    let head = &span(&list, 0, 50).unwrap()[0];
    assert_eq!((head.fade_in, head.fade_out), (8, 0));
    let tail = &span(&list, 50, 100).unwrap()[0];
    assert_eq!((tail.fade_in, tail.fade_out), (0, 8));
}

#[test]
fn a_cut_pasted_back_where_it_was_is_the_list_it_started_from() {
    let list = vec![part(1, 0, 100)];
    let cut = span(&list, 20, 30).unwrap();
    let left = remove(&list, 20, 30).unwrap();
    assert_eq!(spans(&left), [(1, 0, 20), (1, 30, 100)]);
    assert_eq!(length(&left), Ok(90));
    assert_eq!(insert(&left, 20, &cut).unwrap(), list);
}

#[test]
fn a_take_put_over_a_span_replaces_those_frames_and_nothing_else() {
    let list = [part(1, 0, 100)];
    let drawn = [part(9, 0, 10)];
    let out = replace(&list, 40, 50, &drawn).unwrap();
    assert_eq!(spans(&out), [(1, 0, 40), (9, 0, 10), (1, 50, 100)]);
    assert_eq!(length(&out), Ok(100));
}

#[test]
fn a_position_past_the_end_is_the_end() {
    let list = [part(1, 0, 10)];
    let out = insert(&list, 99, &[part(2, 0, 5)]).unwrap();
    assert_eq!(spans(&out), [(1, 0, 10), (2, 0, 5)], "appended");
    assert_eq!(spans(&remove(&list, 5, 99).unwrap()), [(1, 0, 5)]);
}

#[test]
fn parts_that_read_on_from_each_other_are_one_unless_a_seam_says_otherwise() {
    let list = [part(1, 0, 10), part(2, 0, 10), part(1, 10, 20)];
    let out = remove(&list, 10, 20).unwrap();
    assert_eq!(spans(&out), [(1, 0, 20)]);

    let seamed = [faded(part(1, 0, 10), 0, 4), part(2, 0, 10), part(1, 10, 20)];
    let out = remove(&seamed, 10, 20).unwrap();
    assert_eq!(spans(&out), [(1, 0, 10), (1, 10, 20)], "a fade is a seam");

    let mut other = part(1, 10, 20);
    other.channels = Some(vec![1]);
    let out = remove(&[part(1, 0, 10), part(2, 0, 10), other], 10, 20).unwrap();
    assert_eq!(out.len(), 2, "another channel map is another reading");
}

#[test]
fn every_take_a_list_reads_is_named_once() {
    let list = [part(1, 0, 10), part(2, 0, 10), part(1, 30, 40)];
    assert_eq!(sources(&list), [SourceId(1), SourceId(2)]);
}

#[test]
fn a_list_over_joins_reads_through_to_the_takes() {
    // 5 is a join of 1 and 2; 6 is a join over 5 and 3.
    let of = |id: SourceId| match id.0 {
        5 => Some(vec![part(1, 0, 10), part(2, 0, 10)]),
        6 => Some(vec![part(5, 5, 15), part(3, 0, 4)]),
        _ => None,
    };
    let list = [part(6, 2, 12)];
    assert_eq!(
        spans(&flatten(&list, &of).unwrap()),
        [(1, 7, 10), (2, 0, 5), (3, 0, 2)]
    );
    let faded_list = [faded(part(5, 0, 20), 3, 3)];
    let flat = flatten(&faded_list, &of).unwrap();
    assert_eq!((flat[0].fade_in, flat[1].fade_out), (3, 3));
}
