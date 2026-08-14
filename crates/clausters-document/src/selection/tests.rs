//! O6's acceptance: a selection survives a round trip in every variant, and the
//! two-number form scripts already read keeps working.

use super::*;

fn round_trip(selection: &Selection) -> Selection {
    let json = serde_json::to_string(selection).unwrap();
    serde_json::from_str(&json).unwrap()
}

#[test]
fn a_plain_span_is_on_the_wire_exactly_the_two_numbers_it_always_was() {
    // The compatibility half of the acceptance. Every narrowing field is
    // omitted when absent, so a script reading the old payload sees the old
    // payload -- and what a query gives back is what a set takes.
    let selection = Selection::span(1024.0, 4096.0);
    let json = serde_json::to_value(&selection).unwrap();
    assert_eq!(json, serde_json::json!({"start": 1024.0, "len": 4096.0}));
    assert!(selection.is_plain());
}

#[test]
fn the_two_numbers_parse_into_a_selection() {
    // The other direction: what a host has been sending all along is a valid
    // selection with nothing else claimed.
    let selection: Selection =
        serde_json::from_value(serde_json::json!({"start": 0, "len": 512})).unwrap();
    assert_eq!(selection, Selection::span(0.0, 512.0));
    assert!(selection.is_plain());
}

#[test]
fn a_selection_survives_the_round_trip_in_every_variant() {
    let node = NodeId(7);
    let mut mask = Mask::new(4, 3);
    mask.set(0, 0, true);
    mask.set(3, 2, true);

    let variants = [
        Selection::span(0.0, 100.0),
        Selection::cursor(48.5),
        Selection::span(0.0, 100.0).of([node]),
        Selection::span(0.0, 100.0).with_value(ValueRange::new(-0.5, 0.5)),
        Selection::span(0.0, 100.0).with_bins(BinRange::new(12, 96)),
        Selection::span(0.0, 100.0).with_mask(mask.clone()),
        Selection::span(2.0, 6.0)
            .of([node, NodeId(9)])
            .with_value(ValueRange::new(0.0, 1.0))
            .with_bins(BinRange::new(0, 512))
            .with_mask(mask),
    ];
    for selection in variants {
        assert_eq!(round_trip(&selection), selection);
    }
}

#[test]
fn a_narrowed_selection_says_so() {
    // A script that only understands spans must be able to tell: treating a
    // spectral region as the whole band is the quiet kind of wrong.
    assert!(Selection::span(0.0, 1.0).is_plain());
    assert!(
        !Selection::span(0.0, 1.0)
            .with_bins(BinRange::new(0, 4))
            .is_plain()
    );
    assert!(!Selection::span(0.0, 1.0).of([NodeId(1)]).is_plain());
}

#[test]
fn a_cursor_is_a_position_and_not_a_selection() {
    let cursor = Selection::cursor(500.0);
    assert!(cursor.is_empty());
    assert_eq!(cursor.end(), 500.0);
    assert!(!cursor.contains(500.0), "no extent holds nothing");
}

#[test]
fn the_span_is_half_open() {
    let selection = Selection::span(10.0, 5.0);
    assert!(selection.contains(10.0));
    assert!(selection.contains(14.999));
    assert!(!selection.contains(15.0), "one past the end is outside");
    assert_eq!(selection.end(), 15.0);
}

#[test]
fn the_narrowing_axes_do_not_narrow_the_time_axis() {
    // They restrict *what* is selected, not *when* -- so a spectral region
    // still spans its frames, and a reader asking about time gets one answer.
    let selection = Selection::span(0.0, 100.0)
        .with_bins(BinRange::new(40, 50))
        .with_value(ValueRange::new(0.0, 0.1));
    assert!(selection.contains(99.0));
}

// ---- the mask ----

#[test]
fn a_mask_holds_the_cells_it_was_given() {
    let mut mask = Mask::new(10, 4);
    assert!(mask.is_empty() && mask.is_well_formed());
    mask.set(0, 0, true);
    mask.set(9, 3, true);
    mask.set(5, 2, true);
    assert!(mask.get(0, 0) && mask.get(9, 3) && mask.get(5, 2));
    assert!(!mask.get(1, 0));
    assert_eq!(mask.count(), 3);

    mask.set(5, 2, false);
    assert_eq!(mask.count(), 2);
}

#[test]
fn a_mask_answers_out_of_range_rather_than_panicking() {
    // It arrives over a wire, so a size that does not match what a reader
    // expects has to be an answer and not a crash.
    let mut mask = Mask::new(4, 4);
    mask.set(99, 0, true);
    assert!(!mask.get(99, 0));
    assert!(!mask.get(0, 99));
    assert_eq!(mask.count(), 0);

    let short = Mask {
        cols: 64,
        rows: 64,
        bits: vec![0xff; 2],
    };
    assert!(!short.is_well_formed());
    assert!(short.get(0, 0), "what did arrive still reads");
    assert!(!short.get(63, 63), "and what did not reads as out");
}

#[test]
fn a_lasso_that_closed_on_nothing_selected_nothing() {
    // Whatever rectangle bounds it. An empty mask inside a real span is the
    // case a paste has to decline rather than paste the bounding box.
    let selection = Selection::span(0.0, 100.0).with_mask(Mask::new(8, 8));
    assert!(selection.is_empty());

    let mut some = Mask::new(8, 8);
    some.set(3, 3, true);
    assert!(!Selection::span(0.0, 100.0).with_mask(some).is_empty());
}

#[test]
fn a_range_puts_its_edges_in_order() {
    // A drag runs both ways, and the gesture reports where it started and where
    // it ended -- so the type, not every caller, is what makes min the min.
    assert_eq!(ValueRange::new(0.8, 0.2), ValueRange::new(0.2, 0.8));
    assert!(ValueRange::new(0.8, 0.2).contains(0.5));
    assert_eq!(BinRange::new(90, 10), BinRange::new(10, 90));
    assert_eq!(BinRange::new(90, 10).len(), 80);
    assert!(BinRange::new(7, 7).is_empty());
}

#[test]
fn a_selection_names_what_it_is_of() {
    // What a copy reads to know what it may honestly copy.
    let selection = Selection::span(0.0, 10.0).of([NodeId(3), NodeId(4)]);
    assert!(selection.is_of(NodeId(3)) && selection.is_of(NodeId(4)));
    assert!(!selection.is_of(NodeId(5)));
    assert!(
        Selection::span(0.0, 10.0).nodes.is_empty(),
        "empty is the shared axis, not a selection of nothing"
    );
}
