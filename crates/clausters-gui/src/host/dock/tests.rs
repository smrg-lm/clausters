//! The dock's tests: the layout, the hand and the door with no device,
//! and the composed picture against each pane's own.

use serde_json::json;

use clausters_gui_draw::paint::Mesh;

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

/// Where part `want` of group `group`'s bar is.
fn part(d: &Dock, group: usize, want: BarPart) -> Rect {
    d.bar_parts(&d.groups()[group].clone())
        .into_iter()
        .find(|(p, _)| *p == want)
        .map(|(_, r)| r)
        .expect("the bar has it")
}

/// What a hand drags group `group`'s first window by: its tab, or the title
/// of a panel showing no tabs.
fn handle(d: &Dock, group: usize) -> Rect {
    if d.shows_tabs(&d.groups()[group].panes) {
        part(d, group, BarPart::Tab(0))
    } else {
        part(d, group, BarPart::Title)
    }
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
    let first = part(&ws, 0, BarPart::Tab(0));
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
    let tab = handle(&ws, 2);
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
    let tab = handle(&ws, 1);
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
    let tab = handle(&ws, 1);
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
    assert_eq!(ws.take_dirty().out, [2]);
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

/// **A page is a layout of its own**: a page of panels is made and shown;
/// switching pages hides what the shown page does not name and frees
/// nothing; every page keeps its arrangement.
#[test]
fn a_page_keeps_its_own_arrangement() {
    let mut ws = dock((600, 400), 4.0, &[1, 2]);
    assert_eq!(ws.page_names(), [FIRST_PAGE]);
    ws.take_dirty();
    ws.show_page("Mix");
    ws.add(3);
    assert_eq!(ws.page_name(), "Mix");
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
    .expect("under the other");
    let here = ws.tree().to_json();
    assert!(ws.show_page(FIRST_PAGE));
    assert_eq!(ws.tree().panes(), [1, 2], "the first page as it was left");
    assert!(ws.show_page("Mix"));
    assert_eq!(ws.tree().to_json(), here, "and this one as it was left");
    // a pane freed leaves every page
    ws.remove(1);
    assert_eq!(ws.tree().panes(), [3]);
    ws.show_page(FIRST_PAGE);
    assert_eq!(ws.tree().panes(), [2]);
    // the page tabs are a panel's: their strip is a panel's bar
    assert_eq!(ws.page_strip_h(), ws.strip_h());
    // a page closed: its arrangement goes, its panes stay
    ws.close_page("Mix").expect("closes");
    assert!(ws.holds(3) && !ws.shown(3));
    // one page left: the strip goes and the page takes its room
    let whole = ws.groups()[0].whole;
    assert_eq!(whole.y + whole.h, ws.size.1, "{whole:?}");
    assert!(
        ws.close_page(FIRST_PAGE).is_err(),
        "the dock keeps one page"
    );
}

/// **The page strip switches pages**, a page's cross closes it (the
/// first page has none), its tabs open no menu, and a tab dropped on another
/// page's tab moves its pane there. It shows only while there are two pages
/// to choose from.
#[test]
fn the_page_strip_switches_and_takes_a_dragged_tab() {
    let mut ws = dock((600, 400), 4.0, &[1, 2]);
    let press = |ws: &mut Dock, r: Rect| {
        ws.moved(f64::from(r.x + r.w * 0.5), f64::from(r.y + r.h * 0.5));
        ws.pressed();
        ws.released();
    };
    assert_eq!(ws.page_strip().h, 0.0, "no strip for one page");
    assert_eq!(ws.groups()[0].body.y + ws.groups()[0].body.h, 400);
    ws.new_page(None, true).expect("a copy");
    assert!(ws.page_strip().h > 0.0, "a strip for two");
    assert_eq!(ws.page_names(), [FIRST_PAGE, "Page 2"]);
    assert_eq!(ws.page_name(), "Page 2");
    assert_eq!(ws.tree().panes(), [1, 2], "a copy of the page shown");
    ws.new_page(None, false).expect("a page");
    let parts = ws.page_parts();
    assert!(
        !parts.iter().any(|(p, _)| *p == PagePart::Close(0)),
        "the first page has no cross"
    );
    assert_eq!(parts.len(), 5, "three tabs, two crosses, no new page's tab");
    let first = ws.page_tab(0).expect("a tab");
    ws.moved(f64::from(first.x + 4.0), f64::from(first.y + first.h * 0.5));
    assert!(ws.context_pressed(), "the dock takes the press");
    assert!(ws.menu().is_none(), "and opens no menu");
    press(&mut ws, first);
    assert_eq!(ws.page_name(), FIRST_PAGE);
    // Page 3's cross closes it
    let cross = parts
        .iter()
        .find(|(p, _)| *p == PagePart::Close(2))
        .expect("a cross")
        .1;
    press(&mut ws, cross);
    assert_eq!(ws.page_names(), [FIRST_PAGE, "Page 2"]);
    // pane 2's tab dragged onto Page 2's tab: it leaves this page
    let tab = handle(&ws, 1);
    ws.moved(f64::from(tab.x + tab.w * 0.5), f64::from(tab.y + 3.0));
    ws.pressed();
    let to = ws.page_tab(1).expect("a tab");
    ws.moved(f64::from(to.x + 2.0), f64::from(to.y + to.h * 0.5));
    ws.moved(f64::from(to.x + 4.0), f64::from(to.y + to.h * 0.5));
    assert_eq!(ws.landing().map(|l| l.0), Some(Landing::Page(1)));
    ws.released();
    assert_eq!(ws.tree().panes(), [1]);
    assert!(ws.holds(2));
    ws.show_page("Page 2");
    assert_eq!(ws.tree().panes(), [1, 2]);
}

/// **An application is a page**: a window naming a page opens on a new
/// one of its own (named after it, numbered when taken) with no panel's bar
/// over it; a panel opened meanwhile goes to a page that is not an
/// application's; no pane is dragged, placed or moved onto an application's
/// page, nor the application off it; its cross closes the application, and
/// the application's going takes its page.
#[test]
fn an_application_is_alone_on_a_page_of_its_own() {
    let mut ws = dock((600, 400), 4.0, &[1]);
    assert!(ws.add_on(7, Some("Score")));
    assert_eq!(ws.page_names(), [FIRST_PAGE, "Score"]);
    assert_eq!(ws.page_name(), "Score");
    assert!(ws.is_app(7) && ws.page_app(1) == Some(7));
    let g = &ws.groups()[0];
    assert_eq!(g.strip.h, 0.0, "no panel's bar over an application");
    assert_eq!(g.body, g.whole);
    assert!(ws.add_on(8, Some("Score")));
    assert_eq!(ws.page_names(), [FIRST_PAGE, "Score", "Score 2"]);
    // a panel opened while an application is shown goes to the first page
    assert!(ws.add_on(9, None));
    assert_eq!(ws.page_name(), FIRST_PAGE);
    assert_eq!(ws.tree().panes(), [1, 9]);
    assert!(ws.groups()[0].strip.h > 0.0, "a panel keeps its bar");
    // nothing joins an application, nor leaves it
    assert!(ws.land(9, Landing::Page(1)).is_err());
    assert!(ws.land(1, Landing::Join { of: 7, at: None }).is_err());
    assert!(ws.place(7).is_err(), "an application stays on its page");
    ws.show_page("Score");
    assert!(ws.place(1).is_err(), "a panel stays off it");
    assert_eq!(ws.tree().panes(), [7]);
    // its page is no new page's copy
    let copy = ws.new_page(None, true).expect("a page");
    assert!(ws.tree().panes().is_empty());
    ws.close_page(&copy).expect("closed");
    // its cross asks the application to close; its going takes the page
    ws.close_page("Score").expect("closed");
    assert_eq!(ws.take_dirty().close, [7]);
    assert!(ws.remove(7));
    assert_eq!(ws.page_names(), [FIRST_PAGE, "Score 2"]);
}

/// **An application opened on an empty dock takes its one page**, named
/// after it, and leaves no empty page beside it.
#[test]
fn an_application_on_an_empty_dock_takes_its_page() {
    let mut ws = Dock::new((600, 400), metrics(4.0));
    assert!(ws.add_on(7, Some("Multitrack")));
    assert_eq!(ws.page_names(), ["Multitrack"]);
    assert_eq!(ws.page_strip().h, 0.0);
}

/// **The state says the scale its device pixels are at**, so an owner reads
/// the window's size in logical pixels -- what it opens at next time.
#[test]
fn the_state_says_its_scale() {
    let mut m = metrics(4.0);
    m.ui_scale = 2.0;
    let d = Dock::new((1200, 800), m);
    let state = d.state();
    assert_eq!(state["size"], json!([1200, 800]));
    assert_eq!(state["scale"], json!(2.0));
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
    assert_eq!(state["page_tabs"].as_array().map(Vec::len), Some(2));
}

/// **The divider lit while it is dragged is where it is now**, not where
/// the press found it -- the line used to stay behind, drawn at the press.
#[test]
fn the_dragged_divider_is_lit_where_it_is() {
    let mut ws = dock((404, 200), 4.0, &[1, 2]);
    let s = f64::from(strip(&ws));
    ws.moved(202.0, s + 50.0);
    ws.pressed();
    ws.moved(252.0, s + 50.0);
    let lit = ws.lit_divider().expect("lit").rect;
    assert_eq!(lit, ws.dividers()[0].rect);
    assert_eq!(lit.x, 250.0, "it followed the hand");
    ws.released();
}

/// **A divider asks for a resize cursor**: under the pointer and while it
/// is dragged, sideways for one standing upright and up and down for one
/// lying across; off it the pointer is the arrow again.
#[test]
fn a_divider_asks_for_a_resize_cursor() {
    let mut ws = dock((400, 300), 4.0, &[1, 2]);
    let gap = ws.dividers()[0].rect;
    assert_eq!(ws.resize_cursor(), None);
    ws.moved(f64::from(gap.x + 1.0), 150.0);
    assert_eq!(ws.resize_cursor(), Some(true), "an upright divider");
    ws.pressed();
    ws.moved(f64::from(gap.x - 40.0), 150.0);
    assert_eq!(
        ws.resize_cursor(),
        Some(true),
        "while dragged, off its old place"
    );
    ws.released();
    ws.moved(50.0, 150.0);
    assert_eq!(ws.resize_cursor(), None);
    ws.land(
        2,
        Landing::Split {
            of: 1,
            side: Side::Bottom,
        },
    )
    .expect("splits");
    let across = ws
        .dividers()
        .iter()
        .find(|d| !d.row)
        .expect("one across")
        .rect;
    ws.moved(f64::from(across.x + 5.0), f64::from(across.y + 1.0));
    assert_eq!(ws.resize_cursor(), Some(false), "a divider lying across");
}

/// **An empty dock has no panel**: nothing measured, nothing to draw or to
/// press -- a page's dock keeps its canvas after its last pane goes.
#[test]
fn an_empty_dock_has_no_panel() {
    let mut ws = dock((400, 300), 4.0, &[1]);
    ws.remove(1);
    assert!(ws.groups().is_empty());
    let mut mesh = Mesh::new();
    let m = *ws.metrics();
    ws.draw(&mut clausters_gui_draw::draw::Draw::new(
        &mut mesh,
        &m,
        &Theme::default(),
    ));
    ws.moved(10.0, 5.0);
    assert_eq!(ws.pressed(), None);
    assert_eq!(ws.state()["panes"], json!([]));
}

/// **A panel of one window shows its title, not a tab**, with a grip, a
/// menu button and a close mark -- unless that window is a document, which
/// is its tab as an editor's single file is; a panel of several shows a tab
/// each, with a close mark each, and no close mark of its own.
#[test]
fn a_panel_shows_tabs_for_several_windows_or_a_document() {
    let mut ws = dock((600, 300), 4.0, &[1, 2]);
    let kinds = |ws: &Dock, g: usize| -> Vec<BarPart> {
        ws.bar_parts(&ws.groups()[g].clone())
            .into_iter()
            .map(|(p, _)| p)
            .collect()
    };
    assert_eq!(
        kinds(&ws, 0),
        [BarPart::Grip, BarPart::Title, BarPart::Menu, BarPart::Close]
    );
    ws.set_tab(1, true);
    assert_eq!(
        kinds(&ws, 0),
        [
            BarPart::Grip,
            BarPart::TabClose(0),
            BarPart::Tab(0),
            BarPart::Menu
        ],
        "a document is its tab"
    );
    ws.set_tab(1, false);
    ws.land(2, Landing::Join { of: 1, at: None })
        .expect("joins");
    assert_eq!(
        kinds(&ws, 0),
        [
            BarPart::Grip,
            BarPart::TabClose(0),
            BarPart::Tab(0),
            BarPart::TabClose(1),
            BarPart::Tab(1),
            BarPart::Menu
        ]
    );
    // a tab's close mark and the panel's close the window, through the front
    let close = part(&ws, 0, BarPart::TabClose(0));
    ws.moved(f64::from(close.x + 2.0), f64::from(close.y + 2.0));
    ws.take_dirty();
    assert_eq!(ws.pressed(), None);
    ws.released();
    assert_eq!(ws.take_dirty().close, [1]);
    assert!(ws.holds(1), "the front closes it, not the dock");
}

/// **A panel's grip drags the whole panel**: every window of it lands where
/// it is let go, the one shown still shown.
#[test]
fn a_panels_grip_drags_all_its_windows() {
    let mut ws = dock((600, 300), 4.0, &[1, 2, 3]);
    ws.land(2, Landing::Join { of: 1, at: None })
        .expect("joins");
    let grip = part(&ws, 0, BarPart::Grip);
    ws.moved(f64::from(grip.x + 2.0), f64::from(grip.y + 3.0));
    ws.pressed();
    let body = ws.rect(3).expect("placed");
    let (cx, cy) = (
        f64::from(body.x + body.w / 2),
        f64::from(body.y + body.h / 2),
    );
    ws.moved(cx - 10.0, cy);
    ws.moved(cx, cy);
    assert_eq!(
        ws.landing().map(|l| l.0),
        Some(Landing::Join { of: 3, at: None })
    );
    ws.released();
    assert_eq!(ws.tree().to_json(), json!({"tabs": [3, 1, 2], "active": 2}));
}

/// **A panel's menu** opens under its button and acts on its window: split
/// right, move to a new page, close -- and a press off it takes it down.
#[test]
fn a_panels_menu_moves_and_closes_its_window() {
    let mut ws = dock((800, 400), 4.0, &[1, 2]);
    ws.land(2, Landing::Join { of: 1, at: None })
        .expect("joins");
    let pick = |ws: &mut Dock, label: &str| {
        let button = part(ws, 0, BarPart::Menu);
        ws.moved(f64::from(button.x + 2.0), f64::from(button.y + 2.0));
        ws.pressed();
        ws.released();
        let placed = ws.menu_placed();
        let menu = ws.menu().expect("open").clone();
        let row = menu.stack.levels[0]
            .entries
            .iter()
            .position(|e| e.label == label)
            .expect("listed");
        let r = placed[0].rows[row];
        ws.moved(f64::from(r.x + 4.0), f64::from(r.y + r.h * 0.5));
        ws.pressed();
        ws.released();
        assert!(ws.menu().is_none(), "a pick takes the menu down");
    };
    pick(&mut ws, "Split Right");
    assert_eq!(ws.tree().panes(), [1, 2], "2 beside 1, its own panel");
    assert_eq!(ws.groups().len(), 2);
    // open, then press off it: down, and nothing else happens
    let button = part(&ws, 0, BarPart::Menu);
    ws.moved(f64::from(button.x + 2.0), f64::from(button.y + 2.0));
    ws.pressed();
    ws.released();
    assert!(ws.menu().is_some());
    ws.moved(5.0, 390.0 - f64::from(strip(&ws)));
    assert_eq!(ws.pressed(), None);
    assert!(ws.menu().is_none());
    // Escape takes it down too
    ws.open_pane_menu(1, super::bar::MenuAt::Point(10.0, 10.0));
    assert!(ws.escape());
    assert!(ws.menu().is_none());
    ws.take_dirty();
    ws.open_pane_menu(1, super::bar::MenuAt::Point(10.0, 10.0));
    ws.picked(super::bar::Target::Pane(1), "close");
    assert_eq!(ws.take_dirty().close, [1]);
    ws.picked(super::bar::Target::Pane(2), "page:new");
    assert_eq!(ws.page_names().len(), 2);
    assert_eq!(ws.tree().panes(), [2], "2 on the new page, shown");
}

/// **A floating panel keeps its bar and asks its way back**: the window of
/// a pane taken out of the dock is its bar and its body, with no page strip
/// and no grip to drag, and its menu's first pick hands the pane back.
#[test]
fn a_floating_panel_has_a_bar_and_a_way_back() {
    let mut ws = Dock::floating((300, 200), metrics(4.0));
    ws.add(7);
    ws.take_dirty();
    let bar = ws.bar_height();
    let g = ws.groups()[0].clone();
    assert_eq!(g.body.y, bar, "the body under the bar");
    assert_eq!(g.body.h, 200 - bar, "no page strip under the body");
    let parts: Vec<BarPart> = ws.bar_parts(&g).into_iter().map(|(p, _)| p).collect();
    assert_eq!(parts, [BarPart::Title, BarPart::Menu, BarPart::Close]);
    // the title takes no panel in hand: a drag moves nothing
    let title = part(&ws, 0, BarPart::Title);
    ws.moved(f64::from(title.x + 4.0), f64::from(title.y + 4.0));
    ws.pressed();
    ws.moved(150.0, 150.0);
    assert!(ws.landing().is_none());
    ws.released();
    ws.take_dirty();
    let button = part(&ws, 0, BarPart::Menu);
    ws.moved(f64::from(button.x + 2.0), f64::from(button.y + 2.0));
    ws.pressed();
    ws.released();
    let menu = ws.menu().expect("open").clone();
    assert_eq!(menu.stack.levels[0].entries[0].label, "Back into the Dock");
    ws.close_menu();
    ws.picked(super::bar::Target::Pane(7), "dock");
    assert_eq!(ws.take_dirty().back, [7]);
}

/// **A tab alone in its group dragged onto its own group lands nowhere**:
/// the move would change nothing.
#[test]
fn a_lone_tab_over_its_own_group_lands_nowhere() {
    let mut ws = dock((400, 300), 4.0, &[1, 2]);
    let tab = handle(&ws, 0);
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
            y: 2.5,
            panel: None,
        })
    );
    assert_eq!(
        Call::read(r#"{"verb": "capture", "path": "p.png", "panel": 4}"#),
        Ok(Call::Capture {
            path: "p.png".into(),
            panel: Some(4),
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
        // the panel's bar over it, and no page strip for one page
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
        .capture(
            &dock,
            clear,
            &views,
            &super::Overlay {
                chrome: Mesh::new(),
                floats: Vec::new(),
            },
        )
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
    for y in [strip, h / 2, h - strip - 1] {
        let at = ((y * cw + x) * 4) as usize;
        let px: Vec<i32> = composed[at..at + 3].iter().map(|&c| i32::from(c)).collect();
        assert!(
            px.iter().zip(bg).all(|(a, b)| (a - b).abs() <= 2),
            "the divider is the clear color at y={y}: {px:?} against {bg:?}"
        );
    }
}

/// **A window along an edge is no panel**: the bar spans the top, the footer
/// the bottom under the page strip, the panels take what is between, and
/// neither edge pane has a tab, a bar or a place on a page.
#[test]
fn edge_panes_span_the_dock_outside_the_panels() {
    let mut d = dock((400, 300), 1.0, &[1, 2]);
    assert!(d.add_edge(10, Edge::Top, 40));
    assert!(d.add_edge(11, Edge::Bottom, 20));
    let bar = d.rect(10).expect("placed");
    let foot = d.rect(11).expect("placed");
    assert_eq!((bar.x, bar.y, bar.w, bar.h), (0, 0, 400, 40));
    assert_eq!((foot.y, foot.h), (280, 20));
    let strip = d.page_strip();
    assert_eq!(
        strip.y + strip.h,
        279.0,
        "the page strip sits over the footer"
    );
    for g in d.groups() {
        assert!(
            g.strip.y >= 41.0,
            "a panel starts under the bar: {:?}",
            g.strip
        );
        assert!(g.body.y + g.body.h <= strip.y as u32);
        assert!(!g.panes.contains(&10) && !g.panes.contains(&11));
    }
    assert!(d.shown(10) && d.holds(11));
    assert_eq!(d.at(5.0, 5.0).map(|(id, _)| id), Some(10));
    // a press on the bar leaves the keyboard on the panel it was in
    let focus = d.focus();
    d.moved(5.0, 5.0);
    assert_eq!(d.pressed(), Some(10));
    d.released();
    assert_eq!(d.focus(), focus);
    assert!(d.remove(10));
    assert_eq!(d.groups()[0].strip.y, 0.0, "the panels take the bar's room");
}

/// **A pane's list floats over the dock**: it may go anywhere over it, the
/// pointer over it reaches the pane that opened it in that pane's pixels, it
/// takes the keys while it is open, and a press elsewhere takes it down.
#[test]
fn a_list_a_pane_opens_floats_over_the_dock() {
    let mut d = dock((400, 300), 1.0, &[1, 2]);
    d.add_edge(10, Edge::Top, 40);
    d.take_dirty();
    let area = d.popup_area(10).expect("placed");
    assert_eq!((area.x, area.y, area.w, area.h), (0.0, 0.0, 400.0, 300.0));
    let right = d.rect(2).expect("placed");
    let area = d.popup_area(2).expect("placed");
    assert_eq!((area.x, area.y), (-(right.x as f32), -(right.y as f32)));
    // the bar's list hangs below it, over pane 1
    let mut list = Mesh::new();
    list.rect(Rect::new(0.0, 40.0, 120.0, 100.0), [1.0, 1.0, 1.0, 1.0]);
    let layers = [list];
    let floats = [super::PaneFloat {
        pane: 10,
        layers: &layers,
        keys: true,
        dialog: false,
    }];
    let over = d.picture(&Theme::default(), &floats, None);
    assert!(!over.floats[0].is_empty());
    assert_eq!(d.at(50.0, 100.0), Some((10, (50.0, 100.0))));
    assert_eq!(d.focus(), Some(10), "the open list takes the keys");
    // a row of the list over pane 1's bar is the list's, not the bar's:
    // neither lit, nor pressed, nor a tab's tip
    let bar = d.groups()[0].strip;
    assert!(bar.y < 140.0, "pane 1's bar is under the list");
    let (x, y) = (50.0, f64::from(bar.y + bar.h * 0.5));
    assert!(d.bar_at(x, y).is_some());
    d.moved(x, y);
    assert_eq!(d.hot, None);
    assert_eq!(d.pressed(), Some(10));
    d.released();
    assert!(!d.context_pressed() && !d.middle_pressed());
    // a press on pane 2 goes to it, and takes the bar's list down
    d.moved(300.0, 200.0);
    assert_eq!(d.pressed(), Some(2));
    assert_eq!(d.take_dirty().dismiss, vec![10]);
    assert_eq!(d.focus(), Some(2));
}

/// **A pane that fills its panel's header is drawn under the bar**: its
/// rectangle is the whole panel, the bar keeps the grip at its start and
/// its buttons at its end and shows no title, and the middle between them
/// is the pane's -- told to it as its band, reached by the pointer in the
/// pane's own pixels -- while the grip stays the dock's.
#[test]
fn a_pane_with_a_header_fills_its_panels_bar() {
    let mut ws = dock((600, 300), 1.0, &[1]);
    let s = strip(&ws);
    ws.set_header(1, true);
    assert_eq!(
        ws.rect(1),
        Some(PaneRect {
            x: 0,
            y: 0,
            w: 600,
            h: 300
        }),
        "the whole panel"
    );
    let g = ws.groups()[0].clone();
    let kinds: Vec<BarPart> = ws.bar_parts(&g).into_iter().map(|(p, _)| p).collect();
    assert_eq!(kinds, [BarPart::Grip, BarPart::Menu, BarPart::Close]);
    let grip = part(&ws, 0, BarPart::Grip);
    let menu = part(&ws, 0, BarPart::Menu);
    let band = ws.band(1).expect("a band");
    assert_eq!(band.h, s as f32);
    assert_eq!(band.left, grip.x + grip.w);
    assert_eq!(band.right, 600.0 - menu.x);
    // the middle is the pane's, in its own pixels; the grip is the dock's
    let (x, y) = (f64::from(band.left + 10.0), 5.0);
    assert_eq!(ws.at(x, y), Some((1, (x, y))));
    assert_eq!(ws.at(f64::from(grip.x + 2.0), 5.0), None);
    ws.set_header(1, false);
    assert_eq!(ws.rect(1).map(|r| r.y), Some(s), "under the bar again");
    assert!(ws.band(1).is_none());
}

/// **A pane its owner says may not be closed has no close mark**, on its
/// bar or its tab, and no Close in its panel's menu; Close the other tabs
/// leaves it open.
#[test]
fn a_pane_that_may_not_be_closed_has_no_close_mark() {
    let mut ws = dock((600, 300), 1.0, &[1, 2]);
    ws.set_closable(1, false);
    let kinds = |ws: &Dock, g: usize| -> Vec<BarPart> {
        ws.bar_parts(&ws.groups()[g].clone())
            .into_iter()
            .map(|(p, _)| p)
            .collect()
    };
    assert_eq!(
        kinds(&ws, 0),
        [BarPart::Grip, BarPart::Title, BarPart::Menu]
    );
    ws.land(2, Landing::Join { of: 1, at: None })
        .expect("joins");
    assert!(
        !kinds(&ws, 0).contains(&BarPart::TabClose(0)),
        "no mark on its tab"
    );
    assert!(kinds(&ws, 0).contains(&BarPart::TabClose(1)));
    ws.open_pane_menu(1, bar::MenuAt::Point(10.0, 10.0));
    let labels: Vec<String> = ws.menu().unwrap().stack.levels[0]
        .entries
        .iter()
        .map(|e| e.label.clone())
        .collect();
    assert!(!labels.iter().any(|l| l == "Close"), "{labels:?}");
    ws.close_menu();
    ws.take_dirty();
    ws.picked(bar::Target::Pane(2), "close_others");
    assert!(ws.take_dirty().close.is_empty(), "the post window stays");
}

/// **A document's tab says what its window says**: a dot for changes not
/// kept in place of its close mark, the close mark again under the hand,
/// and its tip once the pointer has rested on it -- gone as it leaves.
#[test]
fn a_documents_tab_shows_its_changes_and_its_tip() {
    let mut ws = dock((600, 300), 1.0, &[1]);
    ws.describe(
        1,
        PaneLook {
            title: Some("a.py".into()),
            tab: true,
            closable: true,
            modified: true,
            tip: Some("/home/a.py".into()),
            ..Default::default()
        },
    );
    assert!(ws.is_modified(1));
    let tab = part(&ws, 0, BarPart::Tab(0));
    ws.moved(f64::from(tab.x + 4.0), f64::from(tab.y + 4.0));
    assert!(ws.waiting(), "a tip waits");
    assert!(!ws.elapsed(1000.0), "the clock starts");
    assert!(!ws.elapsed(1300.0));
    assert!(ws.elapsed(1700.0), "rested long enough");
    assert_eq!(ws.shown_tip(), Some((1, "/home/a.py")));
    assert!(!ws.waiting());
    ws.moved(300.0, 200.0);
    assert_eq!(ws.shown_tip(), None, "gone as the pointer leaves");
}

/// **The middle button on a tab closes its window**, and anywhere else is
/// a pane's.
#[test]
fn the_middle_button_closes_a_tab() {
    let mut ws = dock((600, 300), 1.0, &[1, 2]);
    ws.land(2, Landing::Join { of: 1, at: None })
        .expect("joins");
    ws.take_dirty();
    let tab = part(&ws, 0, BarPart::Tab(1));
    ws.moved(f64::from(tab.x + 4.0), f64::from(tab.y + 4.0));
    assert!(ws.middle_pressed());
    assert_eq!(ws.take_dirty().close, [2]);
    ws.moved(300.0, 200.0);
    assert!(!ws.middle_pressed(), "over a pane");
}
