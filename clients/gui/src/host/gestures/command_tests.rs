//! **Commands**: a tool's verb, a menu entry's, and an element's own context
//! menu -- each performed as the key bound to it would be, on the view the
//! window's focus or its `main` names.

use clausters_core::osc::{OscMessage, OscPacket, OscType};

use super::super::interact;
use super::super::layout::Rect;
use super::super::popup::Owner;
use super::super::widget::WidgetKind;
use super::super::{ClientId, GUI_DEF, Host};
use super::*;

fn host_from(json: &str) -> Host {
    let mut host = Host::new();
    host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: GUI_DEF.into(),
            args: vec![OscType::Int(1), OscType::String(json.into())],
        }),
        ClientId::Udp(std::net::SocketAddr::from((
            std::net::Ipv4Addr::LOCALHOST,
            9000,
        ))),
    );
    host
}

fn ctx() -> GestureCtx {
    GestureCtx::new(1, 800, 400)
}

fn rect_of(host: &Host, ctx: &GestureCtx, id: i32) -> Rect {
    host.layout_window(ctx.def_id, ctx.fb_w, ctx.fb_h)
        .unwrap()
        .iter()
        .find(|p| p.widget.id == Some(id))
        .expect("the widget is placed")
        .rect
}

fn mid(r: Rect) -> (f64, f64) {
    ((r.x + r.w * 0.5) as f64, (r.y + r.h * 0.5) as f64)
}

fn click(
    g: &mut Gestures,
    host: &mut Host,
    ctx: &GestureCtx,
    at: (f64, f64),
) -> Vec<GestureEffect> {
    let mut out = g.press(host, ctx, at.0, at.1);
    out.extend(g.release(host, ctx, at.0, at.1));
    out
}

fn emitted(effects: &[GestureEffect]) -> Vec<(i32, Vec<OscType>)> {
    effects
        .iter()
        .filter_map(|e| match e {
            GestureEffect::Emit {
                widget_id, args, ..
            } => Some((*widget_id, args.clone())),
            _ => None,
        })
        .collect()
}

fn selected_notes(host: &Host, id: i32) -> Vec<usize> {
    let WidgetKind::Custom(el) = &host.window_def(1).unwrap().find(id).unwrap().kind else {
        panic!("not an element")
    };
    el.as_any()
        .and_then(|a| a.downcast_ref::<crate::host::elements::notes::Notes>())
        .expect("a roll")
        .selected()
        .to_vec()
}

/// A roll the window names as its `main` view, a tool beside it, and a bar.
const ROLL: &str = r#"{"type":"window","margin":0,"flow":"col","main":90,
    "menu":[{"label":"Edit","menu":[{"label":"Select all","verb":"select_all"},
                                    {"label":"Save","verb":"save"}]}],
    "children":[
      {"id":5,"type":"button","flat":true,"verb":"select_all","label":"All"},
      {"id":90,"type":"notes","min":48.0,"max":72.0,"weight":1,
       "notes":[0.0,400.0,60.0,100,0, 6000.0,400.0,61.0,100,0]}]}"#;

/// **A tool performs its verb on the window's `main` view**, before a hand
/// has been in it, and says nothing of its own: no press, no click, and the
/// focus is not taken from what the tool acts on.
#[test]
fn a_tool_performs_its_verb_on_the_window_s_main_view() {
    let mut host = host_from(ROLL);
    let mut g = Gestures::default();
    let ctx = ctx();
    assert!(selected_notes(&host, 90).is_empty());
    let at = mid(rect_of(&host, &ctx, 5));
    let effects = click(&mut g, &mut host, &ctx, at);
    assert_eq!(selected_notes(&host, 90), vec![0, 1], "every note, held");
    assert!(
        emitted(&effects).iter().all(|(id, _)| *id != 5),
        "a tool reports nothing of its own: {effects:?}"
    );
    assert_ne!(host.focused(), Some((1, 5)), "a tool takes no focus");
}

/// ...and a press on it leaves the focus where it was: what it acts on is
/// what the focus is on.
#[test]
fn a_press_on_a_tool_leaves_the_focus_where_it_was() {
    let mut host = host_from(
        r#"{"type":"window","margin":0,"flow":"col","children":[
            {"id":5,"type":"button","flat":true,"verb":"select_all","label":"All"},
            {"id":6,"type":"text","h":32}]}"#,
    );
    let mut g = Gestures::default();
    let ctx = ctx();
    let at = mid(rect_of(&host, &ctx, 6));
    click(&mut g, &mut host, &ctx, at);
    let focused = host.focused();
    assert_eq!(focused, Some((1, 6)));
    let at = mid(rect_of(&host, &ctx, 5));
    click(&mut g, &mut host, &ctx, at);
    assert_eq!(host.focused(), focused);
}

/// **A bar entry is its verb performed**, the way its key performs it: Select
/// all holds every note and reports no pick. A verb the host does not perform
/// is still the owner's, reported as the pick it always was.
#[test]
fn a_bar_entry_is_its_verb_performed() {
    let mut host = host_from(ROLL);
    let mut g = Gestures::default();
    let ctx = ctx();
    let band = host.menu_bar_rect(1, ctx.fb_w, ctx.fb_h).unwrap();
    let title = {
        let tree = host.window_def(1).unwrap();
        crate::host::menubar::titles(
            crate::host::menubar::entries(tree).unwrap(),
            band,
            host.metrics_for(1),
        )[0]
        .1
    };
    click(&mut g, &mut host, &ctx, mid(title));
    let rows = host.popup_placed(1, ctx.fb_w, ctx.fb_h).unwrap()[0]
        .rows
        .clone();
    let effects = click(&mut g, &mut host, &ctx, mid(rows[0]));
    assert_eq!(selected_notes(&host, 90), vec![0, 1]);
    assert!(
        emitted(&effects)
            .iter()
            .all(|(_, args)| args.first() != Some(&OscType::String("menu".into()))),
        "performed, not reported: {effects:?}"
    );
    click(&mut g, &mut host, &ctx, mid(title));
    let rows = host.popup_placed(1, ctx.fb_w, ctx.fb_h).unwrap()[0]
        .rows
        .clone();
    let effects = click(&mut g, &mut host, &ctx, mid(rows[1]));
    assert!(
        emitted(&effects).contains(&(
            1,
            vec![
                OscType::String("menu".into()),
                OscType::String("save".into())
            ]
        )),
        "the owner's verb, reported: {effects:?}"
    );
}

/// A multitrack whose one track has two curves, one of them shown.
const STACK: &str = r#"{"type":"window","margin":0,"flow":"col","children":[
    {"id":70,"type":"multitrack","link":9,"snap":0,"h":300,"sample_rate":48000,
     "tracks":["10", "bass", 120, 0, 0, 1.0, 1],
     "curves":["31","10","gain",0,1,40, "32","10","pan",0,1,40],
     "hidden":"32",
     "clips":["a","10",0,1000,0,"",-1, "b","10",4000,1000,0,"",-1]}]}"#;

/// **The context menu over a track lists its curves**, each a check that
/// shows or hides that one alone: the picture moves at once and the owner is
/// told in a `shown` report.
#[test]
fn a_track_s_context_menu_shows_its_curves_one_at_a_time() {
    let mut host = host_from(STACK);
    host.sync_track_totals();
    let mut g = Gestures::default();
    let ctx = ctx();
    let rect = rect_of(&host, &ctx, 70);
    let at = (f64::from(rect.x + rect.w * 0.5), f64::from(rect.y) + 30.0);
    g.context(&mut host, &ctx, at.0, at.1).expect("a menu");
    let stack = host.popup(1).unwrap();
    assert_eq!(stack.owner, Owner::Own(70));
    let labels: Vec<&str> = stack.levels[0]
        .entries
        .iter()
        .map(|e| e.label.as_str())
        .collect();
    assert_eq!(
        labels,
        [
            "gain",
            "pan",
            "",
            "Add track",
            "",
            "Reset track heights",
            "Compact tracks"
        ]
    );
    assert!(stack.levels[0].entries[0].marked(), "gain is shown");
    assert!(!stack.levels[0].entries[1].marked(), "pan is not");
    let rows = host.popup_placed(1, ctx.fb_w, ctx.fb_h).unwrap()[0]
        .rows
        .clone();
    let effects = click(&mut g, &mut host, &ctx, mid(rows[1]));
    assert!(host.popup(1).is_none());
    assert!(
        emitted(&effects).contains(&(
            70,
            vec![
                OscType::String("shown".into()),
                OscType::String("32".into()),
                OscType::Int(1)
            ]
        )),
        "{effects:?}"
    );
    // ...and the menu opened again says so
    g.context(&mut host, &ctx, at.0, at.1).expect("a menu");
    assert!(host.popup(1).unwrap().levels[0].entries[1].marked());
}

/// **Over a clip, the clip's own curves** -- and a clip with none says so,
/// in a line nothing picks.
#[test]
fn a_clip_s_context_menu_names_its_own_curves() {
    let mut host = host_from(STACK);
    host.sync_track_totals();
    let mut g = Gestures::default();
    let ctx = ctx();
    let rect = rect_of(&host, &ctx, 70);
    let indent = interact::hit(
        &host,
        1,
        800,
        400,
        f64::from(rect.x) + 5.0,
        f64::from(rect.y) + 5.0,
        &|_, _| 1,
    )
    .map(|h| h.indent)
    .unwrap();
    // the clip stands at the start of the body
    let at = (f64::from(rect.x + indent) + 3.0, f64::from(rect.y) + 30.0);
    g.context(&mut host, &ctx, at.0, at.1).expect("a menu");
    let first = &host.popup(1).unwrap().levels[0].entries[0];
    assert_eq!(first.label, "No clip automation");
    assert!(!first.pickable());
}
