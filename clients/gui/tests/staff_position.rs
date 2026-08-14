//! The host reads a staff position the way the core does — asserted against the
//! one file both sides share.
//!
//! The host measures the position a dragged note reaches and sends it; a client
//! resolves that position against the page it engraved, through
//! `clausters_core::notation::DisplayList::staff_position`. The two crates are
//! in separate workspaces and neither can call the other, so the guard against
//! them drifting is that both assert against
//! `crates/clausters-core/tests/staff_position_vector.json` — this test and
//! `the_vector_positions_are_what_the_core_reads` over there. A change to
//! either reading that is not a change to both fails here.

#![cfg(feature = "notation")]

use clausters_gui::host::graphics::score::ScoreData;

const VECTOR: &str =
    include_str!("../../../crates/clausters-core/tests/staff_position_vector.json");

#[test]
fn the_host_reads_the_same_staff_positions_as_the_core() {
    let vector: serde_json::Value = serde_json::from_str(VECTOR).expect("the vector parses");
    let props = vector["page"].as_object().expect("a page object");
    let data = ScoreData::parse(props);

    let expected = vector["positions"].as_object().expect("a position table");
    assert!(!expected.is_empty(), "the vector asserts something");
    for (id, want) in expected {
        let want = want.as_i64().map(|n| n as i32);
        assert_eq!(data.staff_position(id), want, "staff position of {id}");
    }
}
