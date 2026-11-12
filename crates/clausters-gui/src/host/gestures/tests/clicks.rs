//! When two presses are one double click.

use super::*;

/// A front with no clock sees every press as a single one -- which is what a
/// front with no clock can honestly say, and it is what every test that never
/// sets `now_ms` is relying on.
#[test]
fn a_front_that_hands_over_no_clock_never_doubles() {
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 600, 400);
    assert_eq!(g.count_press(&ctx, 10.0, 10.0), 1);
    assert_eq!(g.count_press(&ctx, 10.0, 10.0), 1);
}
