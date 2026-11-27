//! A free-standing value ruler is its view's vertical strip, wherever it
//! stands: a wheel there zooms the view's vertical, a drag there pans it.

use super::*;

/// A take ruled on its right by a value ruler, its own strip off.
const RULED: &str = r#"{"type":"window","flow":"row","margin":0,"children":[
    {"id":61,"type":"signal","view":"trace","data":[0.0,0.5,-0.5,1.0],"base_bucket":2,
     "weight":1,"axes":{"y":{"strip":false}}},
    {"id":62,"type":"field","axes":{"y":{"rules":61,"dir":"left"}}}]}"#;

/// The ruler's rectangle and the view's, as laid out.
fn rects(host: &Host) -> (Rect, Rect) {
    let placed = host.layout_window(1, 800, 300).expect("laid out");
    let rect = |id: i32| {
        placed
            .iter()
            .find(|p| p.widget.id == Some(id))
            .map(|p| p.rect)
            .expect("placed")
    };
    (rect(62), rect(61))
}

fn y_view(host: &Host) -> (f64, f64) {
    host.window_def(1)
        .unwrap()
        .find(61)
        .unwrap()
        .kind
        .editor()
        .unwrap()
        .y_view()
}

/// **The wheel over the ruler zooms the view's vertical**, and the view's
/// left edge, where it keeps no strip now, is its time axis like the rest of
/// it.
#[test]
fn a_wheel_over_a_value_ruler_zooms_the_vertical_of_the_view_it_rules() {
    let mut host = host_from(RULED);
    host.set_timeline_total(61, 1000);
    let (ruler, view) = rects(&host);
    assert!(ruler.x >= view.x + view.w, "the ruler stands on the right");
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 800, 300);
    let effects = g.wheel(
        &mut host,
        &ctx,
        (ruler.x + ruler.w * 0.5) as f64,
        150.0,
        4.0,
    );
    assert!(has_emit_tag(&effects, 61, "view_y"), "{effects:?}");
    assert!(y_view(&host).1 < 1.0, "the vertical zoomed in");

    let mut host = host_from(RULED);
    host.set_timeline_total(61, 1000);
    let effects = g.wheel(&mut host, &ctx, (view.x + 4.0) as f64, 150.0, 4.0);
    assert!(
        !has_emit_tag(&effects, 61, "view_y"),
        "the view's left edge is no strip"
    );
}

/// **A drag on the ruler pans the view's vertical**, as a drag on its own
/// strip does.
#[test]
fn a_drag_on_a_value_ruler_pans_the_vertical_of_the_view_it_rules() {
    let mut host = host_from(RULED);
    host.set_timeline_total(61, 1000);
    let (ruler, _) = rects(&host);
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 800, 300);
    let x = (ruler.x + ruler.w * 0.5) as f64;
    g.wheel(&mut host, &ctx, x, 150.0, 4.0);
    let (before, _) = y_view(&host);
    g.press(&mut host, &ctx, x, 150.0);
    let effects = g.drag_to(&mut host, &ctx, x, 200.0);
    g.release(&mut host, &ctx, x, 200.0);
    assert!(has_emit_tag(&effects, 61, "view_y"), "{effects:?}");
    assert_ne!(y_view(&host).0, before, "the window moved");
}

/// **A value ruler is as wide as the labels of the view it rules**: the role
/// over an ordinary axis, and the room its numbers take once the view is
/// zoomed onto a narrow range -- measured over the view as it was placed,
/// the way a strip of the view's own is.
#[test]
fn a_value_ruler_widens_to_the_labels_of_a_zoomed_view() {
    let host = host_from(RULED);
    let role = host.metrics_for(1).ruler_w;
    assert_eq!(rects(&host).0.w, role, "the role holds an ordinary axis");
    let zoomed = RULED.replace(
        r#""axes":{"y":{"strip":false}}"#,
        r#""axes":{"y":{"strip":false,"start":0.4995,"len":0.001}}"#,
    );
    let host = host_from(&zoomed);
    assert!(
        rects(&host).0.w > role,
        "the labels of a zoomed axis take room"
    );
}
