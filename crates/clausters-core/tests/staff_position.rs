//! The staff-position reading, against the vector the GUI host reads too.
//!
//! `DisplayList::staff_position` and the host's `ScoreData::staff_position`
//! measure the same thing off the same drawing, from two crates in two
//! workspaces that cannot link to each other: the host names the position a
//! dragged note reaches and a client resolves it against the page it engraved,
//! so a disagreement puts the note somewhere nobody asked for. Neither side can
//! call the other, so both assert against one file — this test and
//! `the_host_reads_the_same_staff_positions_as_the_core` in `clients/gui`.

#![cfg(feature = "notation")]

use clausters_core::notation::DisplayList;

#[test]
fn the_vector_positions_are_what_the_core_reads() {
    let raw = include_str!("staff_position_vector.json");
    let vector: serde_json::Value = serde_json::from_str(raw).expect("the vector parses");
    let page: DisplayList =
        serde_json::from_value(vector["page"].clone()).expect("the page parses");

    let expected = vector["positions"].as_object().expect("a position table");
    assert!(!expected.is_empty(), "the vector asserts something");
    for (id, want) in expected {
        let want = want.as_i64().map(|n| n as i32);
        assert_eq!(page.staff_position(id), want, "staff position of {id}");
    }
}

/// The rule that the vector's awkward geometry exists to pin: a staff line is
/// long relative to the page's other horizontal strokes, not to its viewBox.
#[test]
fn a_short_system_on_a_wide_sheet_still_has_staves() {
    let raw = include_str!("staff_position_vector.json");
    let vector: serde_json::Value = serde_json::from_str(raw).expect("the vector parses");
    let page: DisplayList =
        serde_json::from_value(vector["page"].clone()).expect("the page parses");

    let staves = page.staves();
    assert_eq!(
        staves.len(),
        2,
        "two systems, and the ledger and beam are not"
    );
    assert_eq!((staves[0].y0, staves[0].y1), (1040.0, 1760.0));
    assert_eq!((staves[1].y0, staves[1].y1), (2300.0, 2660.0));
    assert!(
        page.vb[0] > 6.0 * (3338.0 - 500.0),
        "the sheet is far wider than the system, which is the case at issue"
    );
}
