//! The dock's tests: the layout, the hand and the door with no device,
//! and the composed picture against each pane's own.

use serde_json::json;

use super::call::Call;
use super::*;

/// The size roles the tests lay out with: the shipped ones, a divider of
/// `divider` pixels.
fn metrics(divider: f32) -> Metrics {
    Metrics {
        divider_w: divider,
        ..Metrics::default()
    }
}

/// A dock of `size` holding `panes` as columns.
fn dock(size: (u32, u32), divider: f32, panes: &[i32]) -> Dock {
    let mut d = Dock::new(size, metrics(divider));
    for &p in panes {
        d.add(p);
    }
    d.take_dirty();
    d
}

/// The height of a tab strip in these tests.
fn strip(d: &Dock) -> u32 {
    d.groups()[0].body.y
}

/// **Panes are equal columns with a divider between them**, each under its
/// tab strip, the last taking what the division left; a pane taken out gives
/// its room to the others.
#[test]
fn panes_are_equal_columns_and_share_a_removed_panes_room() {
    let mut ws = dock((403, 100), 3.0, &[1]);
    let s = strip(&ws);
    assert!(s > 0 && s < 50, "a strip of a line of control: {s}");
    assert_eq!(
        ws.rect(1),
        Some(PaneRect {
            x: 0,
            y: s,
            w: 403,
            h: 100 - s
        })
    );
    ws.add(2);
    ws.add(3);
    let rects: Vec<_> = ws.placed().map(|(_, r)| (r.x, r.w)).collect();
    assert_eq!(rects, [(0, 132), (135, 133), (271, 132)]);
    assert!(ws.remove(2));
    let rects: Vec<_> = ws.placed().map(|(id, r)| (id, r.x, r.w)).collect();
    assert_eq!(rects, [(1, 0, 200), (3, 203, 200)]);
    assert!(!ws.remove(2), "a pane is taken out once");
    assert!(!ws.add(1), "a pane is added once");
}

/// **A point is in the pane under it, in the pane's own pixels**; a divider
/// and a tab strip are in none.
#[test]
fn a_point_is_read_in_the_pane_under_it() {
    let ws = dock((204, 80), 4.0, &[7, 8]);
    let s = f64::from(strip(&ws));
    assert_eq!(ws.at(10.0, s + 5.0), Some((7, (10.0, 5.0))));
    assert_eq!(ws.at(110.0, s + 20.0), Some((8, (6.0, 20.0))));
    assert_eq!(ws.at(101.0, s + 20.0), None, "the divider");
    assert_eq!(ws.at(10.0, s - 1.0), None, "the tab strip");
    assert_eq!(ws.at(10.0, 90.0), None, "below the surface");
}

/// **A press holds its pane until the release**: motion keeps going to it in
/// its own pixels wherever the pointer is, no pane hears the pointer leave
/// meanwhile, and the keyboard follows the press.
#[test]
fn a_press_holds_its_pane_until_the_release() {
    let mut ws = dock((204, 80), 4.0, &[7, 8]);
    let s = f64::from(strip(&ws));
    assert_eq!(ws.focus(), Some(8), "the pane added last has the keyboard");
    let m = ws.moved(10.0, s + 10.0);
    assert_eq!(m.to, Some((7, (10.0, 10.0))));
    assert_eq!(m.left, None);
    assert_eq!(ws.pressed(), Some(7));
    assert_eq!(ws.focus(), Some(7), "a press gives its pane the keyboard");
    let m = ws.moved(150.0, s + 10.0);
    assert_eq!(m.to, Some((7, (150.0, 10.0))), "the drag stays in pane 7");
    assert_eq!(m.left, None);
    assert_eq!(
        ws.left(),
        None,
        "a held pane keeps the pointer off the surface"
    );
    assert_eq!(ws.released(), Some(7));
    let m = ws.moved(150.0, s + 10.0);
    assert_eq!(m.to, Some((8, (46.0, 10.0))));
    assert_eq!(m.left, Some(7), "crossing into pane 8 leaves pane 7");
    assert_eq!(ws.left(), Some(8));
    assert_eq!(ws.pointer(), None);
}

/// **A pane taken out lets go of the hand** that was on it, and the keyboard
/// goes to a pane that is shown.
#[test]
fn a_removed_pane_lets_go_of_the_hand() {
    let mut ws = dock((204, 80), 4.0, &[7, 8]);
    let s = f64::from(strip(&ws));
    ws.moved(150.0, s + 10.0);
    ws.pressed();
    ws.remove(8);
    assert_eq!(ws.held(), None);
    assert_eq!(ws.pointer(), None);
    assert_eq!(ws.focus(), Some(7));
}

/// **A divider dragged moves the room between its two neighbours**, no pane
/// hears the drag, and neither neighbour is taken under its least.
#[test]
fn a_dragged_divider_trades_room_between_its_neighbours() {
    let mut ws = dock((404, 200), 4.0, &[1, 2]);
    let s = f64::from(strip(&ws));
    let gap = ws.dividers()[0].rect;
    assert_eq!((gap.x, gap.w), (200.0, 4.0));
    let m = ws.moved(202.0, s + 50.0);
    assert_eq!(m.to, None, "the divider is no pane's");
    assert!(
        ws.take_dirty().chrome,
        "the divider under the pointer lights"
    );
    assert_eq!(ws.pressed(), None, "a press on a divider is the dock's");
    let m = ws.moved(252.0, s + 60.0);
    assert_eq!(m.to, None);
    assert!(ws.take_dirty().layout);
    let widths: Vec<_> = ws.placed().map(|(_, r)| r.w).collect();
    assert_eq!(widths, [250, 150]);
    ws.moved(2000.0, s + 60.0);
    let least = 2 * ws.groups()[0].strip.h as u32;
    assert_eq!(
        ws.rect(2).map(|r| r.w),
        Some(least),
        "pane 2 keeps its least"
    );
    assert_eq!(ws.released(), None);
    let shares = ws.tree().shares(&[]).expect("a split");
    assert!((shares.iter().sum::<f32>() - 1.0).abs() < 1e-5);
}

/// **A tab pressed shows its pane**; the one it hid is no longer placed, and
/// the keyboard goes to the shown one.
#[test]
fn a_tab_pressed_shows_its_pane() {
    let mut ws = dock((400, 200), 4.0, &[1, 2]);
    ws.land(2, Landing::Join { of: 1, at: None })
        .expect("joins");
    assert_eq!(ws.tree().to_json(), json!({"tabs": [1, 2], "active": 2}));
    assert!(ws.shown(2) && !ws.shown(1));
    assert!(ws.take_dirty().layout);
    let tabs = ws.tab_parts(&ws.groups()[0].clone());
    let (_, first) = tabs[0];
    ws.moved(f64::from(first.x + 2.0), f64::from(first.y + 2.0));
    assert_eq!(ws.pressed(), None, "a press on a tab is the dock's");
    assert_eq!(ws.released(), None);
    assert!(ws.shown(1) && !ws.shown(2));
    assert_eq!(ws.focus(), Some(1));
    assert_eq!(ws.placed().map(|(id, _)| id).collect::<Vec<_>>(), [1]);
    assert_eq!(
        ws.rect(2),
        ws.rect(1),
        "a hidden tab keeps its group's body"
    );
}

/// **A tab dragged lands where it is let go**: on another group's body a
/// group of its own on the side whose edge is near, or among its tabs from
/// the middle; the place is shown while it is dragged.
#[test]
fn a_dragged_tab_lands_where_it_is_let_go() {
    let mut ws = dock((400, 300), 4.0, &[1, 2, 3]);
    // pane 3's tab, dragged into the middle of pane 1's body: joins it
    let tab = ws.tab_parts(&ws.groups()[2].clone())[0].1;
    let (tx, ty) = (f64::from(tab.x + tab.w * 0.5), f64::from(tab.y + 3.0));
    ws.moved(tx, ty);
    ws.pressed();
    ws.moved(tx + 2.0, ty + 2.0);
    assert_eq!(
        ws.landing(),
        None,
        "a few pixels choose the tab, not drag it"
    );
    let one = ws.rect(1).expect("placed");
    let (cx, cy) = (f64::from(one.x + one.w / 2), f64::from(one.y + one.h / 2));
    ws.moved(cx, cy);
    let (landing, place) = ws.landing().expect("over a group");
    assert_eq!(landing, Landing::Join { of: 1, at: None });
    assert_eq!(place, one.rect());
    ws.released();
    assert_eq!(ws.groups().len(), 2);
    assert_eq!(ws.groups()[0].panes, [1, 3]);
    assert!(ws.shown(3));
    // pane 2's tab, dragged near pane 3's bottom edge: a group under it
    let tab = ws.tab_parts(&ws.groups()[1].clone())[0].1;
    ws.moved(f64::from(tab.x + tab.w * 0.5), f64::from(tab.y + 3.0));
    ws.pressed();
    let body = ws.rect(3).expect("placed");
    ws.moved(
        f64::from(body.x + body.w / 2),
        f64::from(body.y + body.h - 4),
    );
    let (landing, place) = ws.landing().expect("over a group");
    assert_eq!(
        landing,
        Landing::Split {
            of: 3,
            side: Side::Bottom
        }
    );
    assert_eq!(place.h, body.h as f32 * 0.5);
    ws.released();
    assert_eq!(
        ws.tree().to_json()["split"],
        json!("col"),
        "the tabs of 1 and 3 over pane 2"
    );
    assert_eq!(ws.tree().panes(), [1, 3, 2]);
}

/// **A tab dragged past the dock's edge lands out of it**: the dock moves
/// nothing and tells its front which pane to give a window of its own.
#[test]
fn a_tab_dragged_out_of_the_dock_is_told_to_the_front() {
    let mut ws = dock((400, 300), 4.0, &[1, 2]);
    let tab = ws.tab_parts(&ws.groups()[1].clone())[0].1;
    ws.moved(f64::from(tab.x + tab.w * 0.5), f64::from(tab.y + 3.0));
    ws.pressed();
    ws.moved(450.0, 100.0);
    let (landing, place) = ws.landing().expect("out");
    assert_eq!(landing, Landing::Out);
    assert_eq!(
        place,
        ws.rect(2).expect("placed").rect(),
        "the pane it leaves"
    );
    ws.take_dirty();
    assert_eq!(ws.released(), None);
    assert_eq!(ws.take_dirty().out, Some(2));
    assert_eq!(ws.tree().panes(), [1, 2], "the front takes it out");
    assert!(ws.land(2, Landing::Out).is_err());
    assert_eq!(
        Call::read(r#"{"verb": "undock", "pane": 2}"#),
        Ok(Call::Undock(2))
    );
    assert_eq!(
        Call::read(r#"{"verb": "dock", "pane": 2}"#),
        Ok(Call::Dock(2))
    );
}

/// **A page is a layout of its own**: a window naming a page opens on it,
/// made and shown; switching pages hides what the shown page does not name
/// and frees nothing; every page keeps its arrangement.
#[test]
fn a_page_keeps_its_own_arrangement() {
    let mut ws = dock((600, 400), 4.0, &[1, 2]);
    assert_eq!(ws.page_names(), [FIRST_PAGE]);
    ws.take_dirty();
    ws.add_on(3, Some("Multitrack"));
    assert_eq!(ws.page_name(), "Multitrack");
    assert!(ws.take_dirty().layout);
    assert_eq!(ws.placed().map(|(id, _)| id).collect::<Vec<_>>(), [3]);
    assert!(
        ws.holds(1) && !ws.shown(1),
        "a pane on another page is kept, hidden"
    );
    assert_eq!(ws.rect(1), None);
    ws.place(1).expect("on this page too");
    ws.land(
        1,
        Landing::Split {
            of: 3,
            side: Side::Bottom,
        },
    )
    .expect("under the multitrack");
    let here = ws.tree().to_json();
    assert!(ws.show_page(FIRST_PAGE));
    assert_eq!(ws.tree().panes(), [1, 2], "the first page as it was left");
    assert!(ws.show_page("Multitrack"));
    assert_eq!(ws.tree().to_json(), here, "and this one as it was left");
    // a pane freed leaves every page
    ws.remove(1);
    assert_eq!(ws.tree().panes(), [3]);
    ws.show_page(FIRST_PAGE);
    assert_eq!(ws.tree().panes(), [2]);
    // a page closed: its arrangement goes, its panes stay
    ws.close_page("Multitrack").expect("closes");
    assert!(ws.holds(3) && !ws.shown(3));
    assert!(
        ws.close_page(FIRST_PAGE).is_err(),
        "the dock keeps one page"
    );
}

/// **The page strip switches pages**, its last tab makes a copy of the page
/// shown, and a tab dropped on another page's tab moves its pane there.
#[test]
fn the_page_strip_switches_and_takes_a_dragged_tab() {
    let mut ws = dock((600, 400), 4.0, &[1, 2]);
    let press = |ws: &mut Dock, r: Rect| {
        ws.moved(f64::from(r.x + r.w * 0.5), f64::from(r.y + r.h * 0.5));
        ws.pressed();
        ws.released();
    };
    let plus = ws.page_parts()[1].1;
    press(&mut ws, plus);
    assert_eq!(ws.page_names(), [FIRST_PAGE, "Page 2"]);
    assert_eq!(ws.page_name(), "Page 2");
    assert_eq!(ws.tree().panes(), [1, 2], "a copy of the page shown");
    let first = ws.page_parts()[0].1;
    press(&mut ws, first);
    assert_eq!(ws.page_name(), FIRST_PAGE);
    // pane 2's tab dragged onto Page 2's tab: it leaves this page
    let tab = ws.tab_parts(&ws.groups()[1].clone())[0].1;
    ws.moved(f64::from(tab.x + tab.w * 0.5), f64::from(tab.y + 3.0));
    ws.pressed();
    let to = ws.page_parts()[1].1;
    ws.moved(f64::from(to.x + 2.0), f64::from(to.y + to.h * 0.5));
    ws.moved(f64::from(to.x + to.w * 0.5), f64::from(to.y + to.h * 0.5));
    assert_eq!(ws.landing().map(|l| l.0), Some(Landing::Page(1)));
    ws.released();
    assert_eq!(ws.tree().panes(), [1]);
    assert!(ws.holds(2));
    ws.show_page("Page 2");
    assert_eq!(ws.tree().panes(), [1, 2]);
}

/// **The pages round-trip through their JSON**, and a pane no page names
/// after a restore goes on the page shown.
#[test]
fn the_pages_round_trip() {
    let mut ws = dock((600, 400), 4.0, &[1, 2, 3]);
    ws.show_page("Mix");
    ws.place(3).expect("placed");
    let state = ws.state();
    let saved = json!({"pages": state["pages"], "page": state["page"]});
    let mut other = dock((600, 400), 4.0, &[1, 2, 3]);
    other.restore(&saved).expect("restores");
    let again = other.state();
    assert_eq!(again["pages"], state["pages"]);
    assert_eq!(again["page"], json!("Mix"));
    other
        .restore(&json!({"pages": [{"name": "A", "layout": {"tabs": [1]}}]}))
        .expect("restores");
    assert_eq!(other.page_names(), ["A"]);
    assert_eq!(other.tree().panes(), [1, 2, 3], "the panes no page named");
    let call = Call::read(r#"{"verb": "page", "name": "B"}"#).expect("reads");
    let state = other.apply(&call).expect("shows");
    assert_eq!(state["page"], json!("B"));
    assert_eq!(state["page_tabs"].as_array().map(Vec::len), Some(3));
}

/// **A tab alone in its group dragged onto its own group lands nowhere**:
/// the move would change nothing.
#[test]
fn a_lone_tab_over_its_own_group_lands_nowhere() {
    let mut ws = dock((400, 300), 4.0, &[1, 2]);
    let tab = ws.tab_parts(&ws.groups()[0].clone())[0].1;
    ws.moved(f64::from(tab.x + tab.w * 0.5), f64::from(tab.y + 3.0));
    ws.pressed();
    let body = ws.rect(1).expect("placed");
    ws.moved(f64::from(body.x + 4), f64::from(body.y + body.h / 2));
    assert_eq!(ws.landing(), None);
    ws.released();
    assert_eq!(ws.tree().panes(), [1, 2]);
}

/// **The tree stays normal through the moves**: a split of one part is that
/// part, a split inside one of its axis joins it, the shares sum to one, a
/// pane alone cannot be split off its own group.
#[test]
fn the_tree_stays_normal_through_the_moves() {
    let mut t = Node::default();
    t.add(1);
    t.add(2);
    t.add(3);
    t.split(3, Side::Right, 1).expect("splits");
    assert_eq!(t.panes(), [1, 3, 2], "beside 1 in the same row");
    let shares = t.shares(&[]).expect("a row");
    assert_eq!(shares.len(), 3);
    assert!((shares[0] - 0.25).abs() < 1e-5 && (shares[1] - 0.25).abs() < 1e-5);
    t.split(2, Side::Top, 1).expect("splits");
    assert_eq!(
        t.to_json(),
        json!({"split": "row", "shares": [0.5, 0.5], "parts": [
            {"split": "col", "shares": [0.5, 0.5], "parts": [
                {"tabs": [2], "active": 2}, {"tabs": [1], "active": 1}]},
            {"tabs": [3], "active": 3}]})
    );
    assert!(t.split(3, Side::Left, 3).is_err(), "alone in its group");
    t.join(2, 1, None).expect("joins");
    t.join(3, 1, Some(0)).expect("joins");
    assert_eq!(t.to_json(), json!({"tabs": [3, 1, 2], "active": 3}));
    t.remove(3);
    t.remove(1);
    t.remove(2);
    assert_eq!(t, Node::default());
}

/// **A layout round-trips through its JSON**, and one read back over the
/// panes the dock holds drops what is not a pane and adds what it did not
/// name.
#[test]
fn a_layout_round_trips_and_is_read_over_the_panes_held() {
    let mut ws = dock((600, 400), 4.0, &[1, 2, 3]);
    ws.land(
        3,
        Landing::Split {
            of: 1,
            side: Side::Bottom,
        },
    )
    .expect("splits");
    ws.land(2, Landing::Join { of: 3, at: Some(0) })
        .expect("joins");
    let saved = ws.tree().to_json();
    let mut other = dock((600, 400), 4.0, &[1, 2, 3]);
    other.restore(&saved).expect("restores");
    assert_eq!(other.tree().to_json(), saved);
    assert_eq!(other.groups(), ws.groups(), "measured the same");
    let read = Node::from_json(
        &json!({"split": "row", "shares": [3, 1], "parts": [
            {"tabs": [1, 9, 1], "active": 9}, {"tabs": [3]}]}),
        &[1, 2, 3],
    )
    .expect("reads");
    // the shares read are kept; the pane added takes a column's share
    assert_eq!(read.panes(), [1, 3, 2]);
    let shares = read.shares(&[]).expect("a row");
    for (got, want) in shares.iter().zip([0.5, 1.0 / 6.0, 1.0 / 3.0]) {
        assert!((got - want).abs() < 1e-5, "{shares:?}");
    }
    assert_eq!(read.group(&[0]).map(|g| g.panes.clone()), Some(vec![1]));
    assert!(Node::from_json(&json!({"split": "diagonal"}), &[1]).is_err());
}

/// **The door reads its calls and answers the dock as it stands**: a move
/// it cannot make is refused as a command, a verb it does not know as a
/// value.
#[test]
fn the_door_reads_calls_and_answers_the_state() {
    let mut ws = dock((400, 300), 4.0, &[1, 2]);
    let call = Call::read(r#"{"verb": "join", "pane": 2, "of": 1}"#).expect("reads");
    let state = ws.apply(&call).expect("joins");
    assert_eq!(state["layout"], json!({"tabs": [1, 2], "active": 2}));
    assert_eq!(state["panes"][0]["shown"], json!(false));
    assert_eq!(state["panes"][1]["shown"], json!(true));
    assert!(
        state["panes"][0]["tab"].is_array(),
        "where to point at a tab"
    );
    let call =
        Call::read(r#"{"verb": "split", "pane": 2, "side": "left", "of": 1}"#).expect("reads");
    let state = ws.apply(&call).expect("splits");
    assert_eq!(state["dividers"].as_array().map(Vec::len), Some(1));
    let call = Call::read(r#"{"verb": "select", "pane": 9}"#).expect("reads");
    assert_eq!(ws.apply(&call).map_err(|e| e.0), Err("command".into()));
    assert_eq!(
        Call::read(r#"{"verb": "fold"}"#).map_err(|e| e.0),
        Err("value".into())
    );
    assert_eq!(
        Call::read(r#"{"verb": "pointer", "kind": "press", "x": 1, "y": 2.5}"#),
        Ok(Call::Pointer {
            kind: crate::host::PointerKind::Press,
            x: 1.0,
            y: 2.5
        })
    );
}

/// **The composed picture is each pane's own picture, in its place**: two
/// windows drawn as panes of one dock read back, region by region, as
/// each window's own capture at the same size does -- within the two levels
/// a channel two draws of one picture may differ by -- and the divider
/// between them is the clear color.
#[test]
#[cfg(not(target_arch = "wasm32"))]
fn the_composed_picture_is_each_panes_own_picture() {
    use std::collections::HashMap;

    use clausters_core::osc::{OscMessage, OscPacket, OscType};

    use crate::gpu::Gpu;
    use crate::host::frame::{self, Batches, FrameInputs, clear_color};
    use crate::host::world::World;
    use crate::view::Renderers;

    let (w, h, gap) = (200u32, 150u32, 4u32);
    let Ok(dock) = pollster::block_on(Gpu::offscreen(2 * w + gap, h, 4)) else {
        eprintln!("no adapter on this machine: nothing to draw offscreen with");
        return;
    };
    let mut host = crate::host::Host::new();
    let from = crate::host::ClientId::Udp(std::net::SocketAddr::from((
        std::net::Ipv4Addr::LOCALHOST,
        9000,
    )));
    let defs = [
        (
            1,
            r#"{"type":"window","w":200,"h":150,"children":[
                {"id":2,"type":"slider","value":0.7}]}"#,
        ),
        (
            3,
            r#"{"type":"window","w":200,"h":150,"children":[
                {"id":4,"type":"button","text":"play"},
                {"id":5,"type":"knob","value":0.25}]}"#,
        ),
    ];
    for (id, json) in defs {
        host.handle_packet(
            OscPacket::Message(OscMessage {
                addr: crate::host::GUI_DEF.into(),
                args: vec![OscType::Int(id), OscType::String(json.into())],
            }),
            from,
        );
    }
    let mut ws = Dock::new((2 * w + gap, h), metrics(gap as f32));
    let mut panes = Vec::new();
    let mut own = Vec::new();
    for (id, _) in defs {
        ws.add(id);
    }
    let strip = ws.groups()[0].body.y;
    for (id, _) in defs {
        let rect = ws.rect(id).expect("placed");
        assert_eq!((rect.w, rect.h), (w, h - strip));
        let mut gpu = Gpu::pane(&dock, rect.w, rect.h);
        let mut renderers = Renderers::new(&gpu.device, gpu.target());
        let mut batches = Batches::new(&gpu.device, gpu.target());
        let inputs = FrameInputs {
            metrics: host.metrics_for(id),
            world: World {
                timelines: host.timelines(),
                ..World::default()
            },
            ..FrameInputs::default()
        };
        let tree = host.window_def(id).expect("defined");
        frame::render(
            &mut gpu,
            &mut renderers,
            &mut batches,
            &mut HashMap::new(),
            &mut HashMap::new(),
            &mut HashMap::new(),
            tree,
            &inputs,
            &host.theme,
            true,
        );
        // the window's own picture, at the pane's size, by the capture path
        let mut alone = Gpu::pane(&dock, rect.w, rect.h);
        let mut alone_renderers = Renderers::new(&alone.device, alone.target());
        let mut alone_batches = Batches::new(&alone.device, alone.target());
        let readback = frame::capture(
            &mut alone,
            &mut alone_renderers,
            &mut alone_batches,
            &mut HashMap::new(),
            &mut HashMap::new(),
            &mut HashMap::new(),
            tree,
            &inputs,
            &host.theme,
        )
        .expect("reads back");
        readback.wait(&alone.device).expect("mapped");
        own.push((rect, readback.pixels().2));
        panes.push((rect, gpu));
    }
    let clear = clear_color(&host.theme);
    let views: Vec<_> = panes
        .iter()
        .map(|(rect, gpu)| (*rect, gpu.pane_view().expect("a pane")))
        .collect();
    let mut composer = Composer::new(&dock.device, dock.config.format);
    let readback = composer
        .capture(&dock, clear, &views, &Mesh::new())
        .expect("reads back");
    readback.wait(&dock.device).expect("mapped");
    let (cw, ch, composed) = readback.pixels();
    assert_eq!((cw, ch), (2 * w + gap, h));
    for (rect, pixels) in &own {
        let mut differ = 0;
        let mut drawn = 0;
        for y in 0..rect.h {
            for x in 0..rect.w {
                let a = ((y * rect.w + x) * 4) as usize;
                let b = (((rect.y + y) * cw + rect.x + x) * 4) as usize;
                if (0..3).any(|c| (i32::from(pixels[a + c]) - i32::from(composed[b + c])).abs() > 2)
                {
                    differ += 1;
                }
                if (0..3).any(|c| pixels[a + c] != pixels[c]) {
                    drawn += 1;
                }
            }
        }
        assert!(drawn > 300, "the window draws something: {drawn} pixels");
        assert_eq!(differ, 0, "pane at x={} is its own picture", rect.x);
    }
    let bg = [clear.r, clear.g, clear.b].map(|c| (c * 255.0).round() as i32);
    let x = w + gap / 2;
    for y in [strip, h / 2, h - 1] {
        let at = ((y * cw + x) * 4) as usize;
        let px: Vec<i32> = composed[at..at + 3].iter().map(|&c| i32::from(c)).collect();
        assert!(
            px.iter().zip(bg).all(|(a, b)| (a - b).abs() <= 2),
            "the divider is the clear color at y={y}: {px:?} against {bg:?}"
        );
    }
}
