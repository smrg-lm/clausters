//! Pointer-interaction primitives over the widget tree — the value/hit logic
//! shared by both fronts.
//!
//! Hit-testing a point, reading and writing a control's value, flipping a toggle,
//! cycling a menu: all of it is pure work on the [`Host`]'s typed tree plus the
//! [`layout`] and [`controls`] math, with no platform dependency. The native
//! windowed front ([`super::gui`]) and the browser front ([`super::web`]) both
//! call these, so a turned knob updates the tree and decides bound-vs-event the
//! same way on either platform — only the event *source* (winit vs browser
//! pointer events) and the event *sink* (a socket vs the binding surface) differ.

use clausters_core::osc::OscType;

#[cfg(not(target_arch = "wasm32"))]
use super::bpf;
use super::layout::{self, Rect};
#[cfg(not(target_arch = "wasm32"))]
use super::track;
use super::widget::{Widget, WidgetKind};
use super::{Host, controls};
#[cfg(not(target_arch = "wasm32"))]
use crate::viewport::View;

/// The deepest interactive widget under `(x, y)` in window `def_id`: its id, its
/// laid-out rect and a clone of its kind. Containers (`window`/`panel`) are not
/// hit targets. `fb_w`/`fb_h` is the window's framebuffer size in device pixels.
pub(crate) fn hit(
    host: &Host,
    def_id: i32,
    fb_w: u32,
    fb_h: u32,
    x: f64,
    y: f64,
) -> Option<(i32, Rect, WidgetKind)> {
    let tree = host.window_def(def_id)?;
    let area = Rect::new(0.0, 0.0, fb_w as f32, fb_h as f32);
    let mut found = None;
    for p in layout::layout(area, tree) {
        if p.rect.contains(x, y)
            && let Some(id) = p.widget.id
            && !matches!(
                p.widget.kind,
                WidgetKind::Window { .. } | WidgetKind::Panel { .. }
            )
        {
            found = Some((id, p.rect, p.widget.kind.clone()));
        }
    }
    found
}

/// The current 0..1 fraction of a continuous control (slider/knob/number) in the
/// host tree — the live value used to drive an incremental drag.
pub(crate) fn fraction_of(host: &Host, def_id: i32, widget_id: i32) -> Option<f32> {
    fn walk(w: &Widget, id: i32) -> Option<f32> {
        if w.id == Some(id) {
            return match &w.kind {
                WidgetKind::Slider { range: r, .. }
                | WidgetKind::Knob(r)
                | WidgetKind::Number(r) => Some(r.fraction()),
                _ => None,
            };
        }
        w.children.iter().find_map(|c| walk(c, id))
    }
    walk(host.window_def(def_id)?, widget_id)
}

/// Sets a continuous control's value from a 0..1 fraction, in the host tree.
pub(crate) fn set_fraction(host: &mut Host, def_id: i32, widget_id: i32, t: f32) {
    if let Some(tree) = host.window_def_mut(def_id)
        && let Some(w) = tree.find_mut(widget_id)
    {
        match &mut w.kind {
            WidgetKind::Slider { range: r, .. } | WidgetKind::Knob(r) | WidgetKind::Number(r) => {
                r.set_fraction(t)
            }
            _ => {}
        }
    }
}

/// Flips a `toggle`'s boolean state in the host tree.
pub(crate) fn flip_toggle(host: &mut Host, def_id: i32, id: i32) {
    if let Some(tree) = host.window_def_mut(def_id)
        && let Some(w) = tree.find_mut(id)
        && let WidgetKind::Toggle { value, .. } = &mut w.kind
    {
        *value = !*value;
    }
}

/// Advances a `menu`'s selected option (wrapping) in the host tree.
pub(crate) fn cycle_menu(host: &mut Host, def_id: i32, id: i32) {
    if let Some(tree) = host.window_def_mut(def_id)
        && let Some(w) = tree.find_mut(id)
        && let WidgetKind::Menu { index, options, .. } = &mut w.kind
        && !options.is_empty()
    {
        *index = (*index + 1) % options.len();
    }
}

/// The current event value of widget `id` in `tree` (what a `/gui_event` or a
/// bound forward carries).
pub(crate) fn value_of(tree: &Widget, id: i32) -> Option<OscType> {
    fn walk(w: &Widget, id: i32) -> Option<OscType> {
        if w.id == Some(id) {
            return w.kind.event_value();
        }
        w.children.iter().find_map(|c| walk(c, id))
    }
    walk(tree, id)
}

/// Runs `f` over a `bpf` widget's model in the host tree — the one door every
/// envelope edit goes through, so the fronts never unpack the variant
/// themselves. `f` gets the breakpoints and the display mapping (the time
/// domain, the value range and the `exp` scale); its return value is passed
/// through (`None` when the widget is not a `bpf`). Editing gestures are
/// native-only today (the browser keeps display + `/gui_set` parity, the
/// editor-view posture), so the helpers are compiled out of the wasm build.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn bpf_edit<R>(
    host: &mut Host,
    def_id: i32,
    widget_id: i32,
    f: impl FnOnce(&mut Vec<bpf::BpfPoint>, f64, f32, f32, bool) -> R,
) -> Option<R> {
    let w = host.window_def_mut(def_id)?.find_mut(widget_id)?;
    match &mut w.kind {
        WidgetKind::Bpf {
            points,
            min,
            max,
            duration,
            exp,
            ..
        } => Some(f(points, *duration, *min, *max, *exp)),
        _ => None,
    }
}

/// A break-point curve's edit-back payload: the `"points"` tag plus the flat
/// breakpoint list (`t v shape curve` per point) — what a `/gui_event` carries
/// to the script, and what a bound editor forwards to the audio server. Shared
/// by the `bpf` view and the **automation clip**, whose curve is the same model
/// placed on a lane: one payload, so a script (or an `Automation`) consumes an
/// edit without caring which view drew it.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn bpf_event_args(tree: &Widget, id: i32) -> Option<Vec<OscType>> {
    let points = match &tree.find(id)?.kind {
        WidgetKind::Bpf { points, .. } => points,
        WidgetKind::Clip { points, .. } if !points.is_empty() => points,
        _ => return None,
    };
    let mut args = vec![OscType::String("points".into())];
    args.extend(bpf::points_args(points));
    Some(args)
}

/// Completes a rewiring drag on a `graph` patch: the dragged port (`member` and
/// its control) is pointed at the bus under the cursor — or at **none**, when the
/// release lands on empty space, which unwires it. Writes the patch in the host
/// tree and returns the edit as `(member, control, bus)` (an empty bus = unwired)
/// — the payload of the flat `"wire"` event a script re-realizes from. `None`
/// when there was nothing to rewire.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn wire_set(
    host: &mut Host,
    def_id: i32,
    widget_id: i32,
    port: (usize, usize),
    area: Rect,
    cx: f64,
    cy: f64,
) -> Option<(usize, String, String)> {
    let w = host.window_def_mut(def_id)?.find_mut(widget_id)?;
    let WidgetKind::Graph { graph, .. } = &mut w.kind else {
        return None;
    };
    let (member, index) = port;
    let control = graph.members.get(member)?.ports.get(index)?.clone();
    let bus = super::graph::bus_hit(area, graph, cx, cy)
        .and_then(|b| graph.buses.get(b).cloned())
        .unwrap_or_default();

    // One wire per (member, control): a control touches one bus, or none.
    graph
        .wires
        .retain(|wire| !(wire.member == member && wire.control == control));
    if !bus.is_empty() {
        graph.wires.push(super::graph::Wire {
            member,
            control: control.clone(),
            bus: bus.clone(),
        });
    }
    Some((member, control, bus))
}

/// Runs `f` over an automation clip's break-points in the host tree — the one
/// door a curve edit on a lane goes through (the clip sibling of `bpf_edit`).
#[cfg(not(target_arch = "wasm32"))]
fn clip_curve<R>(
    host: &mut Host,
    def_id: i32,
    clip_id: i32,
    f: impl FnOnce(&mut Vec<bpf::BpfPoint>, f32, f32, bool) -> R,
) -> Option<R> {
    let w = host.window_def_mut(def_id)?.find_mut(clip_id)?;
    match &mut w.kind {
        WidgetKind::Clip {
            points,
            points_min,
            points_max,
            exp,
            ..
        } => Some(f(points, *points_min, *points_max, *exp)),
        _ => None,
    }
}

/// Moves break-point `index` of an automation clip to the cursor: the time maps
/// back through the **shared axis** (a clip's curve lives on the timeline, not on
/// a widget-local domain) and the value through the clip's range, then the point
/// is placed with the `bpf` model's own monotonic clamp. Returns whether it moved.
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::too_many_arguments)] // one display mapping, all scalars
pub(crate) fn clip_point_move(
    host: &mut Host,
    def_id: i32,
    clip_id: i32,
    index: usize,
    rect: Rect,
    body: Rect,
    nav: &View,
    offset: f64,
    cx: f64,
    cy: f64,
) -> bool {
    let Some(widget) = host.window_def(def_id).and_then(|t| t.find(clip_id)) else {
        return false;
    };
    let WidgetKind::Clip { dur, .. } = widget.kind else {
        return false;
    };
    let t = track::curve_time_at(body, nav, offset, cx).min(dur);
    clip_curve(host, def_id, clip_id, |points, min, max, exp| {
        let value = track::curve_value_at(rect, min, max, exp, cy);
        bpf::place_point(points, index, t, value, dur.max(t));
    })
    .is_some()
}

/// Adds a break-point at the cursor on an automation clip (Ctrl+click), or
/// removes the one under it — the `bpf` view's gesture, on a lane.
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::too_many_arguments)] // one display mapping, all scalars
pub(crate) fn clip_point_edit(
    host: &mut Host,
    def_id: i32,
    clip_id: i32,
    hit: Option<usize>,
    rect: Rect,
    body: Rect,
    nav: &View,
    offset: f64,
    cx: f64,
    cy: f64,
) -> bool {
    let t = track::curve_time_at(body, nav, offset, cx);
    clip_curve(host, def_id, clip_id, |points, min, max, exp| match hit {
        Some(i) => bpf::remove_point(points, i),
        None => {
            let value = track::curve_value_at(rect, min, max, exp, cy);
            bpf::insert_point(points, t, value);
            true
        }
    })
    .unwrap_or(false)
}

/// Which part of a clip a press landed on: its body (move) or one of its edges
/// (resize). The edge zone is a few pixels at each end; a clip too narrow for
/// two edge zones is all body.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ClipPart {
    Body,
    Start,
    End,
}

/// A clip press: the clip id, its current placement (`offset`/`dur`), the lane
/// body and the shared navigation window the drag maps through (so the front
/// turns cursor pixels into timeline samples), and which part was hit.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct ClipHit {
    pub id: i32,
    pub offset: f64,
    pub dur: f64,
    pub body: Rect,
    /// The clip's own rectangle (the body the curve of an automation clip is
    /// drawn in, so its point edits map onto the pixels drawn).
    pub rect: Rect,
    pub nav: View,
    pub part: ClipPart,
    /// The break-point under the cursor on an automation clip: a point wins over
    /// the clip's body (as it wins over a segment in the `bpf` view), so the
    /// curve is edited in place while the clip still moves by its empty space.
    pub point: Option<usize>,
}

/// The clip edge hit zone, device pixels.
#[cfg(not(target_arch = "wasm32"))]
const CLIP_EDGE_PX: f32 = 6.0;

/// The topmost `clip` under `(x, y)`, if the point is over a track's lane body
/// (not its header) and inside a clip. Reconstructs the shared time axis
/// ([`track::window_nav`]) so it hit-tests against the same geometry the
/// renderer drew. Native-only, like the other edit-back gestures.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn clip_hit(
    host: &Host,
    def_id: i32,
    fb_w: u32,
    fb_h: u32,
    x: f64,
    y: f64,
) -> Option<ClipHit> {
    let tree = host.window_def(def_id)?;
    let full = track::window_nav(tree);
    let area = Rect::new(0.0, 0.0, fb_w as f32, fb_h as f32);
    for p in layout::layout(area, tree) {
        let WidgetKind::Track { editor, .. } = &p.widget.kind else {
            continue;
        };
        if !p.rect.contains(x, y) {
            continue;
        }
        // The lane's *group* window — the same one the renderer drew through, so
        // a zoomed or panned axis hit-tests where it looks.
        let nav = p
            .widget
            .id
            .and_then(|id| host.timeline_nav(id))
            .map_or(full, |(nav, _total)| nav);
        // The same body the renderer drew (its ruler strip reserved), so the
        // pixels a clip occupies are the pixels it is grabbed by.
        let body = track::lane_body(p.rect, editor.ruler != super::widget::Ruler::Off);
        if !body.contains(x, y) {
            return None; // over the header or the ruler strip, not a clip
        }
        // Topmost clip wins: later children draw over earlier ones.
        for c in p.widget.children.iter().rev() {
            if let WidgetKind::Clip { offset, dur, .. } = c.kind
                && let Some((x0, x1)) = track::clip_x_range(body, &nav, offset, dur)
                && (x as f32) >= x0
                && (x as f32) <= x1
                && let Some(id) = c.id
            {
                let rect = track::clip_rect(body, x0, x1);
                let point = track::clip_draw(c).and_then(|clip| {
                    (!clip.points.is_empty())
                        .then(|| track::curve_hit(&clip, rect, body, &nav, x, y))
                        .flatten()
                });
                return Some(ClipHit {
                    id,
                    offset,
                    dur,
                    body,
                    rect,
                    nav,
                    part: clip_part(x0, x1, x as f32),
                    point,
                });
            }
        }
    }
    None
}

/// Which part of a clip spanning pixels `[x0, x1]` the pointer x fell on.
#[cfg(not(target_arch = "wasm32"))]
fn clip_part(x0: f32, x1: f32, x: f32) -> ClipPart {
    if x1 - x0 < 2.0 * CLIP_EDGE_PX {
        return ClipPart::Body; // too narrow to grab an edge
    }
    if x - x0 <= CLIP_EDGE_PX {
        ClipPart::Start
    } else if x1 - x <= CLIP_EDGE_PX {
        ClipPart::End
    } else {
        ClipPart::Body
    }
}

/// Writes a clip's placement (`offset`/`dur`, each clamped `>= 0`) in the host
/// tree — the drag's mutation, the sibling of [`set_fraction`].
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn clip_set(
    host: &mut Host,
    def_id: i32,
    clip_id: i32,
    new_offset: Option<f64>,
    new_dur: Option<f64>,
) {
    if let Some(tree) = host.window_def_mut(def_id)
        && let Some(w) = tree.find_mut(clip_id)
        && let WidgetKind::Clip { offset, dur, .. } = &mut w.kind
    {
        if let Some(o) = new_offset {
            *offset = o.max(0.0);
        }
        if let Some(d) = new_dur {
            *dur = d.max(0.0);
        }
    }
}

/// A clip's edit-back payload: the `"clip"` tag plus the new `offset`/`dur` —
/// what a `/gui_event` carries to the script (and what a bound clip would
/// forward). Flat OSC primitives, the same pattern as the `bpf` `"points"`
/// payload.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn clip_event_args(tree: &Widget, id: i32) -> Option<Vec<OscType>> {
    match &tree.find(id)?.kind {
        WidgetKind::Clip { offset, dur, .. } => Some(vec![
            OscType::String("clip".into()),
            OscType::Float(*offset as f32),
            OscType::Float(*dur as f32),
        ]),
        _ => None,
    }
}

/// Snaps a timeline sample value to a drag grid: to the nearest multiple of
/// `grid` when it is positive, else to a whole sample.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn snap(v: f64, grid: f64) -> f64 {
    if grid > 0.0 {
        (v / grid).round() * grid
    } else {
        v.round()
    }
}

/// The 0..1 fraction a slider press/drag at `(cx, cy)` maps to, by orientation:
/// the cursor x along a horizontal track, or y (bottom = 0, top = 1) on a
/// `vertical` one.
pub(crate) fn slider_t(body: Rect, cx: f64, cy: f64, vertical: bool) -> f32 {
    if vertical {
        controls::slider_fraction_v(body, cy)
    } else {
        controls::slider_fraction(body, cx)
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use clausters_core::osc::{OscMessage, OscPacket, OscType};

    use super::super::{ClientId, GUI_DEF};
    use super::*;

    fn from() -> ClientId {
        ClientId::Udp(std::net::SocketAddr::from((
            std::net::Ipv4Addr::LOCALHOST,
            9000,
        )))
    }

    /// A window (id 1) with one track (id 5) holding two abutting clips: A
    /// (id 10) over [0, 400), B (id 11) over [400, 400), grid 100.
    fn track_host() -> Host {
        let json = r#"{"type":"window","children":[
            {"id":5,"type":"track","snap":100.0,"children":[
                {"id":10,"type":"clip","offset":0.0,"dur":400.0},
                {"id":11,"type":"clip","offset":400.0,"dur":400.0}
            ]}
        ]}"#;
        let mut host = Host::new();
        host.handle_packet(
            OscPacket::Message(OscMessage {
                addr: GUI_DEF.into(),
                args: vec![OscType::Int(1), OscType::String(json.into())],
            }),
            from(),
        );
        host
    }

    /// The lane body and shared nav of the one track, computed the same way the
    /// renderer and hit-test do — so the test hits real pixels.
    fn geometry(host: &Host, fb_w: u32, fb_h: u32) -> (Rect, View) {
        let tree = host.window_def(1).unwrap();
        let nav = track::window_nav(tree);
        let area = Rect::new(0.0, 0.0, fb_w as f32, fb_h as f32);
        let track_rect = layout::layout(area, tree)
            .into_iter()
            .find(|p| matches!(p.widget.kind, WidgetKind::Track { .. }))
            .unwrap()
            .rect;
        (track::lane_body(track_rect, false), nav)
    }

    #[test]
    fn snap_rounds_to_the_grid_or_to_whole_samples() {
        assert_eq!(snap(437.0, 100.0), 400.0);
        assert_eq!(snap(451.0, 100.0), 500.0);
        assert_eq!(snap(12.4, 0.0), 12.0); // no grid: whole samples
    }

    #[test]
    fn clip_part_splits_body_from_edges() {
        // A wide clip: edges at each end, body in the middle.
        assert_eq!(clip_part(100.0, 300.0, 102.0), ClipPart::Start);
        assert_eq!(clip_part(100.0, 300.0, 297.0), ClipPart::End);
        assert_eq!(clip_part(100.0, 300.0, 200.0), ClipPart::Body);
        // Too narrow to grab an edge: all body.
        assert_eq!(clip_part(100.0, 108.0, 101.0), ClipPart::Body);
    }

    #[test]
    fn clip_hit_finds_the_clip_and_the_part_under_the_cursor() {
        let host = track_host();
        let (fb_w, fb_h) = (1000, 200);
        let (body, nav) = geometry(&host, fb_w, fb_h);
        let (ax0, ax1) = track::clip_x_range(body, &nav, 0.0, 400.0).unwrap();
        let midy = (body.y + body.h / 2.0) as f64;

        // The body of clip A → a move on id 10.
        let h = clip_hit(&host, 1, fb_w, fb_h, ((ax0 + ax1) / 2.0) as f64, midy).unwrap();
        assert_eq!((h.id, h.part), (10, ClipPart::Body));
        // Its left/right edges → resize.
        let h = clip_hit(&host, 1, fb_w, fb_h, (ax0 + 2.0) as f64, midy).unwrap();
        assert_eq!((h.id, h.part), (10, ClipPart::Start));
        let h = clip_hit(&host, 1, fb_w, fb_h, (ax1 - 2.0) as f64, midy).unwrap();
        assert_eq!((h.id, h.part), (10, ClipPart::End));
        // Deeper into the lane → clip B.
        let (bx0, bx1) = track::clip_x_range(body, &nav, 400.0, 400.0).unwrap();
        let h = clip_hit(&host, 1, fb_w, fb_h, ((bx0 + bx1) / 2.0) as f64, midy).unwrap();
        assert_eq!(h.id, 11);
        // Over the header strip → no clip.
        assert!(clip_hit(&host, 1, fb_w, fb_h, (body.x - 10.0) as f64, midy).is_none());
    }

    #[test]
    fn clip_set_and_event_args_move_and_report() {
        let mut host = track_host();
        clip_set(&mut host, 1, 10, Some(150.0), Some(250.0));
        // A negative offset clamps to 0.
        clip_set(&mut host, 1, 11, Some(-5.0), None);
        let tree = host.window_def(1).unwrap();
        let args = clip_event_args(tree, 10).unwrap();
        assert_eq!(args[0], OscType::String("clip".into()));
        assert_eq!(args[1], OscType::Float(150.0));
        assert_eq!(args[2], OscType::Float(250.0));
        assert_eq!(clip_event_args(tree, 11).unwrap()[1], OscType::Float(0.0));
    }

    /// A window (id 1) with a `graph` patch (id 7): a source and a sink wired
    /// through the internal bus `mix`, the sink's output on `OUT`.
    fn graph_host() -> Host {
        let json = r#"{"type":"window","children":[
            {"id":7,"type":"graph","label":"chain",
             "members":[{"name":"gsrc","ports":["out"]},
                        {"name":"gsink","ports":["in","out"]}],
             "buses":["mix","OUT"],
             "wires":[0,"out","mix", 1,"in","mix", 1,"out","OUT"]}
        ]}"#;
        let mut host = Host::new();
        host.handle_packet(
            OscPacket::Message(OscMessage {
                addr: GUI_DEF.into(),
                args: vec![OscType::Int(1), OscType::String(json.into())],
            }),
            from(),
        );
        host
    }

    fn patch(host: &Host) -> super::super::graph::GraphDraw {
        match &host.window_def(1).unwrap().find(7).unwrap().kind {
            WidgetKind::Graph { graph, .. } => graph.clone(),
            other => panic!("expected a graph, got {other:?}"),
        }
    }

    #[test]
    fn a_wire_dropped_on_a_bus_rewires_the_control_and_reports_it() {
        let mut host = graph_host();
        let area = Rect::new(0.0, 0.0, 600.0, 400.0);
        let g = patch(&host);
        // Drag the source's `out` (member 0, port 0) onto the `OUT` bus (index 1).
        let bus = super::super::graph::bus_rect(area, &g, 1);
        let edit = wire_set(
            &mut host,
            1,
            7,
            (0, 0),
            area,
            bus.x as f64 + 4.0,
            bus.y as f64 + 4.0,
        )
        .unwrap();
        assert_eq!(edit, (0, "out".to_string(), "OUT".to_string()));

        let g = patch(&host);
        let src: Vec<_> = g.wires.iter().filter(|w| w.member == 0).collect();
        assert_eq!(src.len(), 1, "a control touches one bus at a time");
        assert_eq!(src[0].bus, "OUT", "and it is the one it was dropped on");
    }

    #[test]
    fn a_wire_dropped_on_empty_space_unwires_it() {
        let mut host = graph_host();
        let area = Rect::new(0.0, 0.0, 600.0, 400.0);
        // Released over the middle of the patch: no bus there.
        let edit = wire_set(&mut host, 1, 7, (1, 0), area, 300.0, 380.0).unwrap();
        assert_eq!(
            edit,
            (1, "in".to_string(), String::new()),
            "an empty bus = unwired"
        );
        assert!(
            !patch(&host)
                .wires
                .iter()
                .any(|w| w.member == 1 && w.control == "in"),
            "the wire is gone from the patch"
        );
    }
}
