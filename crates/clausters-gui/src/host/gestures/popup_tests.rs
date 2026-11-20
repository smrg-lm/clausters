//! The popup layer through the gesture machine: a chooser's list, a menu in
//! its three places, and what the wheel, the keys and pointer motion do while
//! one is open.

use clausters_core::osc::{OscMessage, OscPacket, OscType};

use super::super::popup::Owner;
use super::super::widget::element::Key;
use super::super::{ClientId, GUI_DEF, Host};
use super::*;
use clausters_gui_draw::rect::Rect;

fn from() -> ClientId {
    ClientId::Udp(std::net::SocketAddr::from((
        std::net::Ipv4Addr::LOCALHOST,
        9000,
    )))
}

/// A host with window 1 defined from `json` -- status bar and all, since where
/// a list goes relative to the bar is half of what is tested here.
fn host_from(json: &str) -> Host {
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

/// The `verbs` a score editor's window brings, the part of them these tests
/// press: its scopes are the application's, so a window that names them
/// declares them.
const SCORE_VERBS: &str = r#""verbs":{
    "score":{"entry":{"keys":"N","label":"Note entry on or off"},"deselect":"Escape"},
    "note_entry":{"entry":"N","entry_off":"Escape","pitch_e":"E"}}"#;

/// The window `json` defines, bringing [`SCORE_VERBS`].
fn scoped(json: &str) -> String {
    format!("{{{SCORE_VERBS},{}", &json[1..])
}

fn ctx() -> GestureCtx {
    GestureCtx::new(1, 600, 400)
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

/// A click: a press and the release after it, where the hand never moved.
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

fn value_of(host: &Host, id: i32) -> Option<OscType> {
    host.window_def(1)?.find(id)?.kind.event_value()
}

fn emitted(effects: &[GestureEffect], id: i32) -> Vec<Vec<OscType>> {
    effects
        .iter()
        .filter_map(|e| match e {
            GestureEffect::Emit {
                widget_id, args, ..
            } if *widget_id == id => Some(args.clone()),
            _ => None,
        })
        .collect()
}

fn redraws(effects: &[GestureEffect]) -> bool {
    effects
        .iter()
        .any(|e| matches!(e, GestureEffect::Redraw(1)))
}

const CHOOSER: &str = r#"{"type":"window","margin":0,"flow":"col","children":[
    {"id":6,"type":"label","text":"filler","weight":1},
    {"id":7,"type":"choice","label":"View","w":200,"h":48,
     "options":["ruler: shown","ruler: hidden","ruler: locked"]}]}"#;

#[test]
fn a_press_opens_the_choosers_list_and_changes_nothing_yet() {
    let mut host = host_from(CHOOSER);
    let mut g = Gestures::default();
    let ctx = ctx();
    let at = mid(rect_of(&host, &ctx, 7));
    let effects = click(&mut g, &mut host, &ctx, (at.0, at.1 + 10.0));
    let stack = host.popup(1).expect("the list is open");
    assert_eq!(stack.owner, Owner::Element(7));
    assert_eq!(stack.levels[0].entries.len(), 3);
    assert_eq!(
        value_of(&host, 7),
        Some(OscType::Int(0)),
        "opening picks nothing"
    );
    assert!(emitted(&effects, 7).is_empty(), "and emits nothing");
}

/// The list is placed inside the work area: clear of the status bar, which is
/// drawn after it and used to cover its last rows.
#[test]
fn a_list_at_the_bottom_of_the_window_opens_clear_of_the_status_bar() {
    let mut host = host_from(CHOOSER);
    let mut g = Gestures::default();
    let ctx = ctx();
    let field = rect_of(&host, &ctx, 7);
    let at = mid(field);
    click(&mut g, &mut host, &ctx, (at.0, at.1 + 10.0));
    let band = host
        .status_bar_rect(1, ctx.fb_w, ctx.fb_h)
        .expect("the window has its bar");
    let placed = host.popup_placed(1, ctx.fb_w, ctx.fb_h).unwrap();
    let list = placed[0].rect;
    assert!(
        list.y + list.h <= band.y,
        "the list {list:?} stops above the bar {band:?}"
    );
    assert!(
        list.y + list.h <= field.y + field.h,
        "there is no room under the field, so it opens over it"
    );
}

#[test]
fn a_press_on_a_row_picks_that_option_and_closes() {
    let mut host = host_from(CHOOSER);
    let mut g = Gestures::default();
    let ctx = ctx();
    let at = mid(rect_of(&host, &ctx, 7));
    click(&mut g, &mut host, &ctx, (at.0, at.1 + 10.0));
    let placed = host.popup_placed(1, ctx.fb_w, ctx.fb_h).unwrap();
    let effects = click(&mut g, &mut host, &ctx, mid(placed[0].rows[1]));
    assert!(host.popup(1).is_none(), "the list closes");
    assert_eq!(value_of(&host, 7), Some(OscType::Int(1)));
    assert_eq!(emitted(&effects, 7), vec![vec![OscType::Int(1)]]);
}

#[test]
fn a_press_off_the_list_closes_it_and_reaches_nothing_under_it() {
    let mut host = host_from(
        r#"{"type":"window","margin":0,"flow":"col","children":[
            {"id":7,"type":"choice","w":200,"h":32,"options":["a","b"]},
            {"id":6,"type":"label","text":"filler","h":200},
            {"id":8,"type":"toggle","label":"under","h":32}]}"#,
    );
    let mut g = Gestures::default();
    let ctx = ctx();
    let at = mid(rect_of(&host, &ctx, 7));
    click(&mut g, &mut host, &ctx, at);
    assert!(host.popup(1).is_some());
    let toggle = rect_of(&host, &ctx, 8);
    // Off the list, on the toggle's box: the press dismisses and stops there.
    let on_the_box = (toggle.x as f64 + 6.0, mid(toggle).1);
    let effects = click(&mut g, &mut host, &ctx, on_the_box);
    assert!(host.popup(1).is_none());
    assert!(
        emitted(&effects, 7).is_empty() && emitted(&effects, 8).is_empty(),
        "{effects:?}"
    );
    assert_eq!(
        value_of(&host, 8),
        Some(OscType::Int(0)),
        "the toggle stayed"
    );
}

/// The row under the pointer lights up on motion alone, and the window is
/// asked to repaint -- which is what a static window never did for it.
#[test]
fn pointer_motion_lights_the_row_and_asks_for_a_repaint() {
    let mut host = host_from(CHOOSER);
    let mut g = Gestures::default();
    let ctx = ctx();
    let at = mid(rect_of(&host, &ctx, 7));
    click(&mut g, &mut host, &ctx, (at.0, at.1 + 10.0));
    let placed = host.popup_placed(1, ctx.fb_w, ctx.fb_h).unwrap();
    let (x, y) = mid(placed[0].rows[2]);
    let effects = g.motion(&mut host, &ctx, x, y);
    assert!(redraws(&effects));
    assert_eq!(host.popup(1).unwrap().levels[0].hover, Some(2));
    assert!(
        !redraws(&g.motion(&mut host, &ctx, x + 1.0, y)),
        "the same row again is nothing new to draw"
    );
}

#[test]
fn the_keys_walk_the_list_enter_picks_and_escape_closes() {
    let mut host = host_from(CHOOSER);
    let mut g = Gestures::default();
    let ctx = ctx();
    let at = mid(rect_of(&host, &ctx, 7));
    click(&mut g, &mut host, &ctx, (at.0, at.1 + 10.0));
    assert!(g.key(&mut host, &ctx, Key::Down).is_some());
    g.key(&mut host, &ctx, Key::Down);
    assert_eq!(host.popup(1).unwrap().levels[0].hover, Some(1));
    let effects = g.key(&mut host, &ctx, Key::Enter).unwrap();
    assert!(host.popup(1).is_none());
    assert_eq!(emitted(&effects, 7), vec![vec![OscType::Int(1)]]);

    click(&mut g, &mut host, &ctx, (at.0, at.1 + 10.0));
    assert!(host.popup(1).is_some());
    assert!(g.key(&mut host, &ctx, Key::Escape).is_some());
    assert!(host.popup(1).is_none());
    assert_eq!(
        value_of(&host, 7),
        Some(OscType::Int(1)),
        "Escape picks nothing"
    );
    // With nothing open Escape is not the machine's: the front keeps it.
    assert!(g.key(&mut host, &ctx, Key::Escape).is_none());
}

/// A list longer than the window scrolls under the wheel, and the wheel stops
/// there -- it used to reach whatever lay under the list.
#[test]
fn the_wheel_scrolls_a_long_list_and_goes_no_further() {
    let options: Vec<String> = (0..60).map(|i| format!("\"option {i}\"")).collect();
    let mut host = host_from(&format!(
        r#"{{"type":"window","margin":0,"flow":"col","children":[
            {{"id":7,"type":"choice","w":200,"h":32,"options":[{}]}},
            {{"id":8,"type":"slider","h":40,"min":0,"max":1,"value":0.5}}]}}"#,
        options.join(",")
    ));
    let mut g = Gestures::default();
    let ctx = ctx();
    let at = mid(rect_of(&host, &ctx, 7));
    click(&mut g, &mut host, &ctx, at);
    let placed = host.popup_placed(1, ctx.fb_w, ctx.fb_h).unwrap();
    assert!(placed[0].max_scroll > 0.0, "it does not fit");
    let work = host.content_area(1, ctx.fb_w, ctx.fb_h);
    assert!(placed[0].rect.y + placed[0].rect.h <= work.y + work.h);
    let (x, y) = mid(placed[0].rect);
    let effects = g.wheel(&mut host, &ctx, x, y, -3.0);
    assert!(redraws(&effects));
    let after = host.popup_placed(1, ctx.fb_w, ctx.fb_h).unwrap();
    assert!(after[0].scroll > 0.0);
    assert_eq!(value_of(&host, 8), Some(OscType::Float(0.5)));
}

const BAR: &str = r#"{"type":"window","margin":0,"flow":"col",
    "menu":[
        {"label":"File","menu":[
            {"label":"Open","verb":"open"},
            "-",
            {"label":"Loop","verb":"loop","checked":false},
            {"label":"Recent","menu":["a.wav","b.wav"]},
            {"label":"Export","verb":"export","enabled":false}]},
        {"label":"View","menu":[
            {"label":"Waveform","verb":"wave","group":"view","checked":true},
            {"label":"Spectrogram","verb":"spec","group":"view"}]},
        {"label":"Help","verb":"help"}],
    "children":[
        {"id":5,"type":"toggle","label":"under the bar","h":32,
         "context":[{"label":"Reset","verb":"reset"}]},
        {"id":6,"type":"button","label":"Tools","h":32,
         "menu":["Split","Join"]},
        {"id":9,"type":"label","text":"no menu here"}]}"#;

fn bar_title(host: &Host, ctx: &GestureCtx, n: usize) -> (f64, f64) {
    let tree = host.window_def(1).unwrap();
    let band = host.menu_bar_rect(1, ctx.fb_w, ctx.fb_h).unwrap();
    let titles = crate::host::menubar::titles(
        crate::host::menubar::entries(tree).unwrap(),
        band,
        host.metrics_for(1),
    );
    mid(titles[n].1)
}

/// The bar is chrome along the top: the tree starts under it, so a widget is
/// hit on the pixels it was drawn on and a press on the band reaches no widget.
#[test]
fn the_menu_bar_takes_its_band_out_of_the_layout() {
    let host = host_from(BAR);
    let ctx = ctx();
    let band = host.menu_bar_rect(1, ctx.fb_w, ctx.fb_h).unwrap();
    assert!(band.h > 0.0 && band.y == 0.0);
    assert_eq!(rect_of(&host, &ctx, 5).y, band.h);
    let work = host.content_area(1, ctx.fb_w, ctx.fb_h);
    assert_eq!(work.y, band.h);
}

#[test]
fn a_press_on_a_title_opens_its_menu_and_a_pick_reports_the_verb() {
    let mut host = host_from(BAR);
    let mut g = Gestures::default();
    let ctx = ctx();
    let at = bar_title(&host, &ctx, 0);
    click(&mut g, &mut host, &ctx, at);
    assert!(matches!(host.popup(1).unwrap().owner, Owner::Bar(_)));
    let placed = host.popup_placed(1, ctx.fb_w, ctx.fb_h).unwrap();
    let band = host.menu_bar_rect(1, ctx.fb_w, ctx.fb_h).unwrap();
    assert_eq!(placed[0].rect.y, band.y + band.h, "it hangs under the bar");
    let effects = click(&mut g, &mut host, &ctx, mid(placed[0].rows[0]));
    assert!(host.popup(1).is_none());
    assert_eq!(
        emitted(&effects, 1),
        vec![vec![
            OscType::String("menu".into()),
            OscType::String("open".into())
        ]],
        "the window reports its menu's pick"
    );
}

/// **A key bound to a verb the bar names is that entry's pick**: it reports
/// what a click on the entry reports, and a disabled entry's key does nothing
/// -- but is still consumed, as the click on it is.
#[test]
fn a_key_bound_to_a_bar_entry_reports_its_pick() {
    let mut host = host_from(BAR);
    let mut g = Gestures::default();
    let mut ctx = ctx();
    assert!(host.keys.bind("open", &["Ctrl+O"]).is_empty());
    assert!(host.keys.bind("export", &["F2"]).is_empty());
    ctx.ctrl = true;
    let effects = g
        .press_key(&mut host, &ctx, Key::Char('o'), None)
        .expect("consumed");
    assert_eq!(
        emitted(&effects, 1),
        vec![vec![
            OscType::String("menu".into()),
            OscType::String("open".into())
        ]],
        "the same message a pick sends"
    );
    ctx.ctrl = false;
    let effects = g
        .press_key(&mut host, &ctx, Key::F(2), None)
        .expect("consumed");
    assert!(
        emitted(&effects, 1).is_empty(),
        "a disabled entry: {effects:?}"
    );
}

/// **A command chord goes through an open menu**: the list closes and the
/// command runs, as a key equivalent does during a platform's menu tracking --
/// while a bare letter stays the open list's.
#[test]
fn a_command_chord_closes_an_open_menu_and_runs() {
    let mut host = host_from(BAR);
    let mut g = Gestures::default();
    let mut ctx = ctx();
    host.keys.bind("open", &["Ctrl+O"]);
    host.keys.bind("help", &["H"]);
    let at = bar_title(&host, &ctx, 1);
    click(&mut g, &mut host, &ctx, at);
    assert!(host.popup(1).is_some(), "View is open");
    // A bare letter bound to a verb is still the list's, and does nothing.
    let effects = g
        .press_key(&mut host, &ctx, Key::Char('h'), None)
        .expect("the open list takes it");
    assert!(emitted(&effects, 1).is_empty());
    assert!(host.popup(1).is_some());
    ctx.ctrl = true;
    let effects = g
        .press_key(&mut host, &ctx, Key::Char('o'), None)
        .expect("consumed");
    assert!(host.popup(1).is_none(), "the list closed");
    assert_eq!(
        emitted(&effects, 1),
        vec![vec![
            OscType::String("menu".into()),
            OscType::String("open".into())
        ]],
        "and File > Open ran, from another menu"
    );
}

/// **A field keeps its editing chords and lets the program's through**, as
/// every platform's text field does: Ctrl+V pastes into it, Ctrl+O is the
/// window's.
#[test]
fn a_focused_field_lets_a_command_chord_through() {
    let mut host = host_from(
        r#"{"type":"window","margin":0,"flow":"col",
        "menu":[{"label":"File","menu":[{"label":"Open","verb":"open"}]}],
        "children":[{"id":5,"type":"text","h":32}]}"#,
    );
    let mut g = Gestures::default();
    let mut ctx = ctx();
    host.keys.bind("open", &["Ctrl+O"]);
    g.press_key(&mut host, &ctx, Key::Tab, None);
    assert_eq!(host.focused(), Some((1, 5)));
    ctx.ctrl = true;
    let effects = g
        .press_key(&mut host, &ctx, Key::Char('o'), None)
        .expect("consumed");
    assert_eq!(
        emitted(&effects, 1),
        vec![vec![
            OscType::String("menu".into()),
            OscType::String("open".into())
        ]]
    );
    // Ctrl+Z is the field's while it is typed in, even with nothing to undo.
    let effects = g
        .press_key(&mut host, &ctx, Key::Char('z'), None)
        .expect("the field took it");
    assert!(
        emitted(&effects, 1).is_empty(),
        "no document undo: {effects:?}"
    );
}

/// **A window with no chrome still has commands**: a verb the host does not
/// perform and no menu names is reported bare, from the window.
#[test]
fn a_verb_no_menu_names_reaches_the_window_owner_bare() {
    let mut host = host_from(PANEL);
    let mut g = Gestures::default();
    let ctx = ctx();
    assert!(host.keys.bind("about", &["F1"]).is_empty());
    let effects = g
        .press_key(&mut host, &ctx, Key::F(1), None)
        .expect("consumed");
    assert_eq!(
        emitted(&effects, 1),
        vec![vec![OscType::String("about".into())]]
    );
    // A key bound to nothing is nobody's.
    assert!(g.press_key(&mut host, &ctx, Key::F(3), None).is_none());
}

/// **An open list shows the chord bound to each entry's verb**, at every
/// depth, and is as wide as that needs.
#[test]
fn an_open_menu_shows_the_chord_beside_an_entry_bound_to_one() {
    let mut host = host_from(BAR);
    let mut g = Gestures::default();
    let ctx = ctx();
    let at = bar_title(&host, &ctx, 0);
    click(&mut g, &mut host, &ctx, at);
    let bare = host.popup_placed(1, ctx.fb_w, ctx.fb_h).unwrap()[0].rect.w;
    host.close_popup(1);
    host.keys.bind("open", &["Ctrl+Shift+O"]);
    click(&mut g, &mut host, &ctx, at);
    let stack = host.popup(1).unwrap();
    let entries = &stack.levels[0].entries;
    assert_eq!(entries[0].key.as_deref(), Some("Ctrl+Shift+O"));
    assert_eq!(entries[2].key.as_deref(), Some("L"), "loop's default");
    assert_eq!(entries[4].key, None, "export is bound to nothing");
    let wide = host.popup_placed(1, ctx.fb_w, ctx.fb_h).unwrap()[0].rect.w;
    assert!(
        wide > bare,
        "the list widened for the chords: {bare} -> {wide}"
    );
}

#[test]
fn a_check_flips_and_reports_its_state_and_reads_back_as_a_prop() {
    let mut host = host_from(BAR);
    let mut g = Gestures::default();
    let ctx = ctx();
    let at = bar_title(&host, &ctx, 0);
    click(&mut g, &mut host, &ctx, at);
    let placed = host.popup_placed(1, ctx.fb_w, ctx.fb_h).unwrap();
    let effects = click(&mut g, &mut host, &ctx, mid(placed[0].rows[2]));
    assert_eq!(
        emitted(&effects, 1),
        vec![vec![
            OscType::String("menu".into()),
            OscType::String("loop".into()),
            OscType::Int(1)
        ]]
    );
    // The state is the prop's: the tree holds it, and a query reads it back.
    let info = host.window_def(1).unwrap().info();
    let menu = info.iter().find(|(k, _)| k == "menu").unwrap().1.clone();
    let menu: serde_json::Value = serde_json::from_str(menu.as_str().unwrap()).unwrap();
    assert_eq!(menu[0]["menu"][2]["checked"], serde_json::json!(true));
}

#[test]
fn one_of_several_turns_its_group_over_and_a_disabled_entry_does_nothing() {
    let mut host = host_from(BAR);
    let mut g = Gestures::default();
    let ctx = ctx();
    let at = bar_title(&host, &ctx, 1);
    click(&mut g, &mut host, &ctx, at);
    let placed = host.popup_placed(1, ctx.fb_w, ctx.fb_h).unwrap();
    let effects = click(&mut g, &mut host, &ctx, mid(placed[0].rows[1]));
    assert_eq!(emitted(&effects, 1)[0][1], OscType::String("spec".into()));
    let view = host.window_def(1).unwrap().menu.as_ref().unwrap()[1]
        .submenu()
        .unwrap()
        .to_vec();
    assert!(!view[0].marked() && view[1].marked());

    let at = bar_title(&host, &ctx, 0);

    click(&mut g, &mut host, &ctx, at);
    let placed = host.popup_placed(1, ctx.fb_w, ctx.fb_h).unwrap();
    let effects = click(&mut g, &mut host, &ctx, mid(placed[0].rows[4]));
    assert!(
        emitted(&effects, 1).is_empty(),
        "a disabled entry is not picked"
    );
    assert!(host.popup(1).is_some(), "and the menu stays up");
}

#[test]
fn a_submenu_opens_beside_its_row_and_picks_by_its_own_path() {
    let mut host = host_from(BAR);
    let mut g = Gestures::default();
    let ctx = ctx();
    let at = bar_title(&host, &ctx, 0);
    click(&mut g, &mut host, &ctx, at);
    let placed = host.popup_placed(1, ctx.fb_w, ctx.fb_h).unwrap();
    let (x, y) = mid(placed[0].rows[3]);
    assert!(redraws(&g.motion(&mut host, &ctx, x, y)));
    assert_eq!(host.popup(1).unwrap().levels.len(), 2, "pointing opens it");
    let placed = host.popup_placed(1, ctx.fb_w, ctx.fb_h).unwrap();
    assert!(placed[1].rect.x >= placed[0].rect.x + placed[0].rect.w);
    let effects = click(&mut g, &mut host, &ctx, mid(placed[1].rows[1]));
    assert_eq!(emitted(&effects, 1)[0][1], OscType::String("b.wav".into()));
}

#[test]
fn with_a_menu_open_the_pointer_crossing_another_title_moves_it_there() {
    let mut host = host_from(BAR);
    let mut g = Gestures::default();
    let ctx = ctx();
    let at = bar_title(&host, &ctx, 0);
    click(&mut g, &mut host, &ctx, at);
    let (x, y) = bar_title(&host, &ctx, 1);
    g.motion(&mut host, &ctx, x, y);
    let stack = host.popup(1).unwrap();
    assert_eq!(stack.levels[0].entries[0].label, "Waveform");
    // And the arrows walk the titles from the keyboard.
    g.key(&mut host, &ctx, Key::Left);
    assert_eq!(host.popup(1).unwrap().levels[0].entries[0].label, "Open");
}

/// **Only the title under the pointer is lit**: a title pressed and then
/// left for another, the list following, does not stay lit beside it.
#[test]
fn with_a_menu_open_the_title_left_behind_is_not_lit() {
    let mut host = host_from(BAR);
    let mut g = Gestures::default();
    let ctx = ctx();
    let (x, y) = bar_title(&host, &ctx, 0);
    g.motion(&mut host, &ctx, x, y);
    click(&mut g, &mut host, &ctx, (x, y));
    let (x, y) = bar_title(&host, &ctx, 1);
    g.motion(&mut host, &ctx, x, y);
    let lit = host.popups(1).and_then(|p| p.bar_hover);
    let open = match host.popup(1).unwrap().owner {
        Owner::Bar(title) => title,
        _ => panic!("a bar's list"),
    };
    assert_eq!(lit, Some(open), "the lit title is the open one");
}

#[test]
fn with_a_menu_open_crossing_a_plain_title_closes_it_and_picks_nothing() {
    let mut host = host_from(BAR);
    let mut g = Gestures::default();
    let ctx = ctx();
    let at = bar_title(&host, &ctx, 0);
    click(&mut g, &mut host, &ctx, at);
    let (x, y) = bar_title(&host, &ctx, 2);
    let effects = g.motion(&mut host, &ctx, x, y);
    assert!(host.popup(1).is_none(), "the list closes");
    assert!(emitted(&effects, 1).is_empty(), "and nothing is picked");
}

#[test]
fn a_title_that_is_a_plain_entry_is_picked_by_the_press() {
    let mut host = host_from(BAR);
    let mut g = Gestures::default();
    let ctx = ctx();
    let at = bar_title(&host, &ctx, 2);
    let effects = click(&mut g, &mut host, &ctx, at);
    assert!(host.popup(1).is_none());
    assert_eq!(emitted(&effects, 1)[0][1], OscType::String("help".into()));
}

#[test]
fn the_secondary_button_opens_the_context_menu_at_the_pointer() {
    let mut host = host_from(BAR);
    let mut g = Gestures::default();
    let ctx = ctx();
    let at = mid(rect_of(&host, &ctx, 5));
    let effects = g.context(&mut host, &ctx, at.0, at.1).expect("it has one");
    assert!(redraws(&effects));
    assert_eq!(host.popup(1).unwrap().owner, Owner::Context(5));
    let placed = host.popup_placed(1, ctx.fb_w, ctx.fb_h).unwrap();
    assert_eq!(
        (placed[0].rect.x as f64, placed[0].rect.y as f64),
        (at.0 as f32 as f64, at.1 as f32 as f64)
    );
    let effects = click(&mut g, &mut host, &ctx, mid(placed[0].rows[0]));
    assert_eq!(
        emitted(&effects, 5),
        vec![vec![
            OscType::String("menu".into()),
            OscType::String("reset".into())
        ]]
    );
    // Where nothing under the pointer carries one, there is no request.
    let nowhere = mid(rect_of(&host, &ctx, 9));
    assert!(g.context(&mut host, &ctx, nowhere.0, nowhere.1).is_none());
}

#[test]
fn a_button_that_carries_a_menu_opens_it_instead_of_firing() {
    let mut host = host_from(BAR);
    let mut g = Gestures::default();
    let ctx = ctx();
    let button = rect_of(&host, &ctx, 6);
    let effects = click(&mut g, &mut host, &ctx, mid(button));
    assert!(emitted(&effects, 6).is_empty(), "the press is the menu's");
    assert_eq!(host.popup(1).unwrap().owner, Owner::Button(6));
    let placed = host.popup_placed(1, ctx.fb_w, ctx.fb_h).unwrap();
    assert_eq!(placed[0].rect.y, button.y + button.h);
    let effects = click(&mut g, &mut host, &ctx, mid(placed[0].rows[1]));
    assert_eq!(emitted(&effects, 6)[0][1], OscType::String("Join".into()));
}

/// A finger held still is the context request a finger has no second button
/// for; the press it grew out of is let go of without a click.
#[test]
fn a_press_held_still_by_a_finger_asks_for_the_context_menu() {
    let mut host = host_from(BAR);
    let mut g = Gestures::default();
    let mut ctx = ctx();
    ctx.touch = true;
    ctx.now_ms = 1000.0;
    let at = mid(rect_of(&host, &ctx, 5));
    // On the toggle's box, at its left.
    let at = (rect_of(&host, &ctx, 5).x as f64 + 6.0, at.1);
    g.press(&mut host, &ctx, at.0, at.1);
    assert!(g.pending(), "the hold is armed");
    ctx.now_ms = 1200.0;
    assert!(g.elapsed(&mut host, &ctx).is_empty(), "not long enough yet");
    ctx.now_ms = 1600.0;
    let effects = g.elapsed(&mut host, &ctx);
    assert!(redraws(&effects));
    assert_eq!(host.popup(1).unwrap().owner, Owner::Context(5));
    assert!(!g.pending() && !g.dragging());
}

#[test]
fn a_hold_that_moved_is_a_drag_and_asks_for_nothing() {
    let mut host = host_from(BAR);
    let mut g = Gestures::default();
    let mut ctx = ctx();
    ctx.touch = true;
    ctx.now_ms = 1000.0;
    let r = rect_of(&host, &ctx, 5);
    g.press(&mut host, &ctx, r.x as f64 + 6.0, mid(r).1);
    g.drag_to(&mut host, &ctx, r.x as f64 + 80.0, mid(r).1);
    ctx.now_ms = 2000.0;
    g.elapsed(&mut host, &ctx);
    assert!(host.popup(1).is_none());
}

const TIPS: &str = r#"{"type":"window","margin":0,"flow":"row","children":[
    {"id":5,"type":"button","label":"A","w":60,"tip":"the first tool"},
    {"id":6,"type":"button","label":"B","w":60,"tip":"the second tool"},
    {"id":7,"type":"button","label":"C","w":60}]}"#;

fn tip_of(host: &Host) -> Option<String> {
    host.popups(1)?.tip.as_ref().map(|t| t.text.clone())
}

#[test]
fn a_tip_shows_once_the_pointer_has_rested_and_goes_when_it_leaves() {
    let mut host = host_from(TIPS);
    let mut g = Gestures::default();
    let mut ctx = ctx();
    ctx.now_ms = 1000.0;
    let a = mid(rect_of(&host, &ctx, 5));
    g.motion(&mut host, &ctx, a.0, a.1);
    assert!(g.pending(), "the wait started");
    assert_eq!(tip_of(&host), None);
    ctx.now_ms = 1300.0;
    g.elapsed(&mut host, &ctx);
    assert_eq!(tip_of(&host), None, "it has not rested long enough");
    ctx.now_ms = 1700.0;
    assert!(redraws(&g.elapsed(&mut host, &ctx)));
    assert_eq!(tip_of(&host).as_deref(), Some("the first tool"));
    assert!(!g.pending());
    // Onto the next tool while one is up: its tip shows at once.
    let b = mid(rect_of(&host, &ctx, 6));
    g.motion(&mut host, &ctx, b.0, b.1);
    assert_eq!(tip_of(&host).as_deref(), Some("the second tool"));
    // Onto a widget with none: it goes.
    let c = mid(rect_of(&host, &ctx, 7));
    assert!(redraws(&g.motion(&mut host, &ctx, c.0, c.1)));
    assert_eq!(tip_of(&host), None);
}

/// **A tool's tip shows the key of its verb**, read from the key table as the
/// tip comes up: the chord the table binds, the one a rebinding left, and
/// nothing for a verb no key performs or a button that is no tool.
#[test]
fn a_tool_s_tip_shows_the_key_of_its_verb() {
    let mut host = host_from(
        r#"{"type":"window","margin":0,"flow":"row","children":[
            {"id":5,"type":"button","label":"L","w":60,"verb":"loop","tip":"Loop"},
            {"id":6,"type":"button","label":"X","w":60,"verb":"export","tip":"Export"},
            {"id":7,"type":"button","label":"B","w":60,"tip":"a plain button"},
            {"id":8,"type":"toggle","label":"T","w":60,"tip":"Loop the score","shortcut":"loop"}]}"#,
    );
    let ctx = ctx();
    let tip = |host: &Host, id: i32| {
        let at = mid(rect_of(host, &ctx, id));
        host.tip_at(1, ctx.fb_w, ctx.fb_h, at.0, at.1)
            .map(|t| (t.text, t.key))
    };
    let pair = |text: &str, key: Option<&str>| Some((text.to_string(), key.map(str::to_string)));
    assert_eq!(tip(&host, 5), pair("Loop", Some("L")));
    assert_eq!(tip(&host, 6), pair("Export", None), "no key performs it");
    assert_eq!(tip(&host, 7), pair("a plain button", None));
    // a latch is no tool: it names the verb whose key presses it
    assert_eq!(tip(&host, 8), pair("Loop the score", Some("L")));
    let table = serde_json::json!({"loop": "Ctrl+L", "export": "F9"});
    host.keys.overlay_json(table.as_object().unwrap());
    assert_eq!(tip(&host, 5), pair("Loop", Some("Ctrl+L")));
    assert_eq!(tip(&host, 6), pair("Export", Some("F9")));
}

#[test]
fn a_press_and_a_key_take_a_tip_down() {
    let mut host = host_from(TIPS);
    let mut g = Gestures::default();
    let mut ctx = ctx();
    ctx.now_ms = 1000.0;
    let a = mid(rect_of(&host, &ctx, 5));
    g.motion(&mut host, &ctx, a.0, a.1);
    ctx.now_ms = 2000.0;
    g.elapsed(&mut host, &ctx);
    assert!(tip_of(&host).is_some());
    assert!(redraws(&g.key_began(&mut host, &ctx)));
    assert_eq!(tip_of(&host), None);

    g.motion(&mut host, &ctx, a.0 + 1.0, a.1);
    ctx.now_ms = 3000.0;
    g.elapsed(&mut host, &ctx);
    assert!(tip_of(&host).is_some());
    g.press(&mut host, &ctx, a.0, a.1);
    assert_eq!(tip_of(&host), None);
}

/// The control under the pointer is the hovered one, and the window repaints
/// when that **changes** -- not at every pixel of the way across it.
#[test]
fn the_hovered_control_changes_on_entering_and_leaving_only() {
    let mut host = host_from(TIPS);
    let mut g = Gestures::default();
    let ctx = ctx();
    let c = mid(rect_of(&host, &ctx, 7));
    assert!(redraws(&g.motion(&mut host, &ctx, c.0, c.1)));
    assert_eq!(host.popups(1).unwrap().hover, Some(7));
    assert!(!redraws(&g.motion(&mut host, &ctx, c.0 + 2.0, c.1)));
    assert!(redraws(&g.hover(&mut host, &ctx, None)));
    assert_eq!(host.popups(1).unwrap().hover, None);
}

/// A disabled widget is out of the hand's reach, and so is everything inside a
/// disabled container.
#[test]
fn a_disabled_widget_takes_no_press_and_no_focus() {
    let mut host = host_from(
        r#"{"type":"window","margin":0,"flow":"col","children":[
            {"id":5,"type":"toggle","label":"off","h":32,"enabled":false},
            {"id":6,"type":"layout","flow":"col","h":40,"enabled":false,"children":[
                {"id":7,"type":"toggle","label":"inside","h":32}]},
            {"id":8,"type":"toggle","label":"on","h":32}]}"#,
    );
    let mut g = Gestures::default();
    let ctx = ctx();
    for id in [5, 7] {
        let r = rect_of(&host, &ctx, id);
        click(&mut g, &mut host, &ctx, (r.x as f64 + 6.0, mid(r).1));
        assert_eq!(value_of(&host, id), Some(OscType::Int(0)), "widget {id}");
    }
    let r = rect_of(&host, &ctx, 8);
    click(&mut g, &mut host, &ctx, (r.x as f64 + 6.0, mid(r).1));
    assert_eq!(value_of(&host, 8), Some(OscType::Int(1)));
}

const PANEL: &str = r#"{"type":"window","margin":0,"flow":"col","children":[
    {"id":5,"type":"button","label":"fire","h":32},
    {"id":6,"type":"toggle","label":"on","h":32},
    {"id":7,"type":"slider","h":40,"min":0,"max":10,"step":1,"value":5},
    {"id":8,"type":"choice","h":32,"options":["a","b","c"]}]}"#;

fn press_key(g: &mut Gestures, host: &mut Host, ctx: &GestureCtx, k: Key) -> Vec<GestureEffect> {
    g.key(host, ctx, k).unwrap_or_default()
}

/// Every control is a stop on the ring and is worked from the keyboard, by the
/// standard keys -- and none of them reports the focus it took, since its
/// event stream is its value.
#[test]
fn every_control_takes_the_focus_and_answers_the_standard_keys() {
    let mut host = host_from(PANEL);
    let mut g = Gestures::default();
    let ctx = ctx();
    // Tab enters the ring at the button; nothing is said about the focus.
    let e = press_key(&mut g, &mut host, &ctx, Key::Tab);
    assert_eq!(host.focused(), Some((1, 5)));
    assert!(
        emitted(&e, 5).is_empty(),
        "a control's focus is silent: {e:?}"
    );
    // Space is a click: the gate opens and closes, and the command fires.
    let e = emitted(&press_key(&mut g, &mut host, &ctx, Key::Char(' ')), 5);
    assert!(e.contains(&vec![OscType::String("press".into())]));
    assert!(e.contains(&vec![OscType::String("click".into())]));
    assert!(e.contains(&vec![OscType::Int(1)]) && e.contains(&vec![OscType::Int(0)]));
    // Enter flips the toggle.
    press_key(&mut g, &mut host, &ctx, Key::Tab);
    press_key(&mut g, &mut host, &ctx, Key::Enter);
    assert_eq!(value_of(&host, 6), Some(OscType::Int(1)));
    // The arrows step the slider on its own grid.
    press_key(&mut g, &mut host, &ctx, Key::Tab);
    press_key(&mut g, &mut host, &ctx, Key::Right);
    assert_eq!(value_of(&host, 7), Some(OscType::Float(6.0)));
    // ...and move the choice; Enter opens its list.
    press_key(&mut g, &mut host, &ctx, Key::Tab);
    let e = press_key(&mut g, &mut host, &ctx, Key::Down);
    assert_eq!(emitted(&e, 8), vec![vec![OscType::Int(1)]]);
    press_key(&mut g, &mut host, &ctx, Key::Enter);
    assert_eq!(
        host.popup(1).map(|s| s.owner.clone()),
        Some(Owner::Element(8))
    );
}

#[test]
fn a_press_on_a_control_gives_it_the_focus_without_saying_so() {
    let mut host = host_from(PANEL);
    let mut g = Gestures::default();
    let ctx = ctx();
    let r = rect_of(&host, &ctx, 6);
    let effects = click(&mut g, &mut host, &ctx, (r.x as f64 + 6.0, mid(r).1));
    assert_eq!(host.focused(), Some((1, 6)));
    assert_eq!(emitted(&effects, 6), vec![vec![OscType::Int(1)]]);
}

// ---- what a container shows of itself ----

const SECTION: &str = r#"{"type":"window","margin":0,"flow":"col","status":false,"children":[
    {"id":2,"type":"layout","title":"Filter","collapsed":false,"flow":"col","children":[
        {"id":3,"type":"toggle","label":"inside","h":32}]},
    {"id":4,"type":"label","text":"after","weight":1}]}"#;

#[test]
fn a_press_on_a_sections_title_folds_it_and_reports_the_change() {
    let mut host = host_from(SECTION);
    let mut g = Gestures::default();
    let ctx = ctx();
    let strip = rect_of(&host, &ctx, 2);
    let at = (strip.x as f64 + 20.0, strip.y as f64 + 4.0);
    let effects = click(&mut g, &mut host, &ctx, at);
    assert_eq!(
        emitted(&effects, 2),
        vec![vec![OscType::String("collapsed".into()), OscType::Int(1)]]
    );
    let placed = host.layout_window(1, ctx.fb_w, ctx.fb_h).unwrap();
    assert!(
        placed.iter().all(|p| p.widget.id != Some(3)),
        "its content is no longer placed"
    );
    drop(placed);
    // The state is the prop: a query reads it back.
    let info = host.window_def(1).unwrap().find(2).unwrap().info();
    assert!(info.contains(&("collapsed".into(), serde_json::json!(true))));
    // The same press again unfolds it.
    let effects = click(&mut g, &mut host, &ctx, at);
    assert_eq!(emitted(&effects, 2)[0][1], OscType::Int(0));
    assert_eq!(value_of(&host, 3), Some(OscType::Int(0)), "it is back");
}

const SPLIT_ROW: &str = r#"{"type":"window","margin":0,"gap":6,"flow":"row","split":true,
    "status":false,"children":[
    {"id":2,"type":"layout","children":[]},
    {"id":3,"type":"layout","children":[]},
    {"id":4,"type":"layout","children":[]}]}"#;

#[test]
fn dragging_a_divider_trades_room_between_its_two_neighbours() {
    let mut host = host_from(SPLIT_ROW);
    let mut g = Gestures::default();
    let ctx = ctx();
    let widths =
        |host: &Host| -> Vec<f32> { [2, 3, 4].map(|id| rect_of(host, &ctx, id).w).to_vec() };
    let before = widths(&host);
    assert!(before.iter().all(|w| (*w - before[0]).abs() < 0.01), "even");
    let gap = rect_of(&host, &ctx, 2);
    let at = ((gap.x + gap.w + 3.0) as f64, 100.0);
    g.press(&mut host, &ctx, at.0, at.1);
    assert!(g.dragging(), "the gap is a divider");
    g.drag_to(&mut host, &ctx, at.0 + 50.0, at.1);
    let during = widths(&host);
    assert!((during[0] - before[0] - 50.0).abs() < 0.5, "{during:?}");
    assert!((during[1] - before[1] + 50.0).abs() < 0.5, "{during:?}");
    assert!(
        (during[2] - before[2]).abs() < 0.5,
        "the third is not touched"
    );
    let effects = g.release(&mut host, &ctx, at.0 + 50.0, at.1);
    let report = &emitted(&effects, 1)[0];
    assert_eq!(report[0], OscType::String("split".into()));
    assert_eq!(report.len(), 4, "a size per child");
    // What it wrote reads back as the props they are.
    let info = host.window_def(1).unwrap().find(2).unwrap().info();
    assert!(info.iter().any(|(k, _)| k == "weight"), "{info:?}");
    // And it stops at the least a neighbour is left with.
    g.press(&mut host, &ctx, at.0 + 50.0, at.1);
    g.drag_to(&mut host, &ctx, at.0 + 5000.0, at.1);
    let squeezed = widths(&host);
    assert!(
        squeezed[1] >= host.metrics_for(1).control_h - 0.5,
        "{squeezed:?}"
    );
}

const SCROLLED: &str = r#"{"type":"window","margin":0,"status":false,"children":[
    {"id":2,"type":"plane","bars":true,"zoom":false,"axis":"y","margin":0,
     "content_w":600,"content_h":2000,"children":[
        {"id":3,"type":"label","text":"far down","x":0,"y":1500,"w":100,"h":30}]}]}"#;

fn view_y(host: &Host) -> f64 {
    match &host.window_def(1).unwrap().find(2).unwrap().kind {
        crate::host::widget::WidgetKind::Scroll { view, .. } => view.view_y,
        other => panic!("not a plane: {other:?}"),
    }
}

#[test]
fn dragging_a_scroll_bars_thumb_pans_the_plane() {
    let mut host = host_from(SCROLLED);
    let mut g = Gestures::default();
    let ctx = ctx();
    // The thumb stands at the top of the bar along the right edge.
    g.press(&mut host, &ctx, 597.0, 10.0);
    assert!(g.dragging());
    let effects = g.drag_to(&mut host, &ctx, 597.0, 110.0);
    assert!(view_y(&host) > 300.0, "it moved: {}", view_y(&host));
    assert!(
        emitted(&effects, 2)
            .iter()
            .any(|a| a[0] == OscType::String("view".into()))
    );
    g.release(&mut host, &ctx, 597.0, 110.0);
    // A press on the groove, off the thumb, brings the thumb there.
    g.press(&mut host, &ctx, 597.0, 395.0);
    g.release(&mut host, &ctx, 597.0, 395.0);
    assert!(
        (view_y(&host) - 1600.0).abs() < 1.0,
        "the end: {}",
        view_y(&host)
    );
}

const DIALOG: &str = r#"{"type":"window","margin":0,"flow":"col","status":false,"children":[
    {"id":2,"type":"toggle","label":"behind","h":32},
    {"id":3,"type":"layout","modal":true,"w":240,"h":120,"flow":"col","children":[
        {"id":4,"type":"label","text":"Discard the take?"},
        {"id":5,"type":"toggle","label":"in the dialog","h":32}]}]}"#;

/// A dialog takes every press while it exists: what is behind it is out of
/// reach of the pointer and of the keyboard alike.
#[test]
fn nothing_behind_a_dialog_can_be_reached() {
    let mut host = host_from(DIALOG);
    let mut g = Gestures::default();
    let ctx = ctx();
    let behind = rect_of(&host, &ctx, 2);
    click(
        &mut g,
        &mut host,
        &ctx,
        (behind.x as f64 + 6.0, mid(behind).1),
    );
    assert_eq!(
        value_of(&host, 2),
        Some(OscType::Int(0)),
        "the press went nowhere"
    );
    let inside = rect_of(&host, &ctx, 5);
    click(
        &mut g,
        &mut host,
        &ctx,
        (inside.x as f64 + 6.0, mid(inside).1),
    );
    assert_eq!(value_of(&host, 5), Some(OscType::Int(1)));
    // The ring is the dialog's: Tab cannot walk behind it.
    host.clear_focus();
    press_key(&mut g, &mut host, &ctx, Key::Tab);
    assert_eq!(host.focused(), Some((1, 5)));
    // Freed, the window is in reach again.
    host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: crate::host::GUI_FREE.into(),
            args: vec![OscType::Int(3)],
        }),
        from(),
    );
    click(
        &mut g,
        &mut host,
        &ctx,
        (behind.x as f64 + 6.0, mid(behind).1),
    );
    assert_eq!(value_of(&host, 2), Some(OscType::Int(1)));
}

/// A dialog defined into a holder, the way an application opens one, is freed
/// by its own id -- and the holder takes another the next time. Both went
/// nowhere when a redefined widget was cut off from its window.
#[test]
fn a_dialog_defined_into_a_holder_closes_when_freed_and_opens_again() {
    let mut host = host_from(
        r#"{"type":"window","margin":0,"flow":"col","status":false,"children":[
            {"id":2,"type":"toggle","label":"behind","h":32},
            {"id":3,"type":"layout","h":0}]}"#,
    );
    let ctx = ctx();
    let open = |host: &mut Host, n: i32| {
        host.handle_packet(
            OscPacket::Message(OscMessage {
                addr: GUI_DEF.into(),
                args: vec![
                    OscType::Int(3),
                    OscType::String(format!(
                        r#"{{"type":"layout","h":0,"children":[
                            {{"id":{n},"type":"layout","modal":1,"w":200,"h":100,"children":[
                                {{"id":{},"type":"button","label":"OK"}}]}}]}}"#,
                        n + 1
                    )),
                ],
            }),
            from(),
        );
    };
    let up = |host: &Host| {
        let placed = host.layout_window(1, ctx.fb_w, ctx.fb_h).unwrap();
        crate::host::chrome::modal_start(&placed).and_then(|i| placed[i].widget.id)
    };
    open(&mut host, 10);
    assert_eq!(up(&host), Some(10));
    host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: crate::host::GUI_FREE.into(),
            args: vec![OscType::Int(10)],
        }),
        from(),
    );
    assert_eq!(up(&host), None, "freed, it is gone from the window");
    open(&mut host, 20);
    assert_eq!(up(&host), Some(20), "and the holder takes the next one");
    // Escape asks it to go, and closes nothing else.
    let mut g = Gestures::default();
    let e = press_key(&mut g, &mut host, &ctx, Key::Escape);
    assert_eq!(
        emitted(&e, 20),
        vec![vec![OscType::String("cancel".into())]]
    );
}

/// A titled dialog ends its strip in a close mark, which asks it to go the
/// way Escape does.
#[test]
fn a_dialogs_close_mark_asks_it_to_go() {
    let mut host = host_from(
        r#"{"type":"window","margin":0,"flow":"col","status":false,"children":[
            {"id":5,"type":"layout","modal":1,"title":"About","w":200,"h":100,"children":[
                {"id":6,"type":"label","text":"x"}]}]}"#,
    );
    let ctx = ctx();
    let mark = {
        let placed = host.layout_window(1, ctx.fb_w, ctx.fb_h).unwrap();
        let p = placed.iter().find(|p| p.widget.id == Some(5)).unwrap();
        crate::host::chrome::close_mark(p).expect("a titled dialog has one")
    };
    let mut g = Gestures::default();
    let e = click(&mut g, &mut host, &ctx, mid(mark));
    assert_eq!(emitted(&e, 5), vec![vec![OscType::String("cancel".into())]]);
}

/// A widget defined into an open window inherits the window's theme group --
/// the dialog an application opens draws in the window's colors.
#[test]
fn a_subtree_defined_into_a_window_takes_the_windows_theme() {
    let mut host = host_from(
        r##"{"type":"window","theme":{"popup":"#ff0000"},"status":false,"children":[
            {"id":3,"type":"layout","h":0}]}"##,
    );
    host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: GUI_DEF.into(),
            args: vec![
                OscType::Int(3),
                OscType::String(
                    r#"{"type":"layout","children":[{"id":4,"type":"layout","modal":1}]}"#.into(),
                ),
            ],
        }),
        from(),
    );
    let dialog = host.window_def(1).unwrap().find(4).unwrap();
    let theme = dialog.theme.as_deref().expect("resolved");
    assert_eq!(theme.popup, [1.0, 0.0, 0.0, 1.0]);
}

const FIELD: &str = r#"{"type":"window","margin":0,"flow":"col","children":[
    {"id":5,"type":"text","h":32,"value":"hello world"},
    {"id":6,"type":"text","h":32,"context":[{"label":"Mine","verb":"mine"}]}]}"#;

/// **A field's secondary button opens the standard edit menu** -- the host's,
/// where the field carries no `context` of its own -- and a pick is the edit
/// itself: Cut puts the selection on the clipboard and takes it out.
#[test]
fn a_field_has_the_standard_edit_menu_and_a_pick_edits() {
    let mut host = host_from(FIELD);
    let mut g = Gestures::default();
    let mut ctx = ctx();
    let at = mid(rect_of(&host, &ctx, 5));
    // A triple click selects the line, which turns Cut, Copy and Delete on.
    for t in [1_000.0, 1_100.0, 1_200.0] {
        ctx.now_ms = t;
        click(&mut g, &mut host, &ctx, at);
    }
    g.context(&mut host, &ctx, at.0, at.1)
        .expect("a menu opened");
    let stack = host.popup(1).expect("open");
    assert_eq!(stack.owner, Owner::Edit(5));
    let labels: Vec<(&str, bool)> = stack.levels[0]
        .entries
        .iter()
        .filter(|e| !e.is_separator())
        .map(|e| (e.label.as_str(), e.enabled))
        .collect();
    assert_eq!(
        labels,
        [
            ("Cut", true),
            ("Copy", true),
            ("Paste", true),
            ("Delete", true),
            ("Select All", true)
        ]
    );
    let placed = host.popup_placed(1, ctx.fb_w, ctx.fb_h).unwrap();
    ctx.now_ms = 5_000.0;
    let effects = click(&mut g, &mut host, &ctx, mid(placed[0].rows[0]));
    assert!(host.popup(1).is_none());
    assert_eq!(
        host.clipboard.text(),
        "hello world",
        "cut onto the clipboard"
    );
    assert_eq!(
        emitted(&effects, 5),
        vec![vec![OscType::String(String::new())]],
        "and out of the field"
    );
    // A field with a `context` of its own opens that one instead.
    let other = mid(rect_of(&host, &ctx, 6));
    g.context(&mut host, &ctx, other.0, other.1);
    assert_eq!(host.popup(1).unwrap().owner, Owner::Context(6));
}

/// **The middle button pastes the primary selection at the pointer**: a word
/// selected in one field goes into the other where the button was pressed.
#[test]
fn the_middle_button_pastes_the_selection_where_it_is_pressed() {
    let mut host = host_from(FIELD);
    let mut g = Gestures::default();
    let mut ctx = ctx();
    let from = rect_of(&host, &ctx, 5);
    let word = (from.x as f64 + 8.0, mid(from).1);
    for t in [1_000.0, 1_100.0] {
        ctx.now_ms = t;
        click(&mut g, &mut host, &ctx, word);
    }
    assert_eq!(host.clipboard.primary().as_deref(), Some("hello"));
    let into = rect_of(&host, &ctx, 6);
    let effects = g.middle(&mut host, &ctx, mid(into).0, mid(into).1);
    assert_eq!(host.focused(), Some((1, 6)), "the field takes the focus");
    assert!(
        emitted(&effects, 6).contains(&vec![OscType::String("hello".into())]),
        "{effects:?}"
    );
}

/// **A dialog is as tall as the sizes its rows declare**, heavy views
/// included. A spectrogram or a canvas wants no height of its own, so a dialog
/// that measured only what its rows wanted took half the window instead, and
/// the button under them was drawn outside its frame -- where a press is a
/// press behind the dialog, and goes nowhere.
#[test]
fn a_dialog_holding_heavy_views_reaches_its_last_row() {
    let mut host = host_from(
        r#"{"type":"window","margin":0,"flow":"col","status":false,"children":[
            {"id":2,"type":"spectrogram","data":[0.0,0.5,0.0,-0.5],"weight":1},
            {"id":5,"type":"layout","modal":1,"title":"T","flow":"col","w":520,"hug":1,"frame":1,"children":[
                {"id":6,"type":"label","text":"x"},
                {"id":7,"type":"spectrogram","data":[0.0,0.5,0.0,-0.5],"h":160},
                {"id":8,"type":"canvas","h":120},
                {"id":9,"type":"button","label":"Close"}]}]}"#,
    );
    let ctx = ctx();
    let dialog = rect_of(&host, &ctx, 5);
    let close = rect_of(&host, &ctx, 9);
    assert!(
        close.y >= dialog.y && close.y + close.h <= dialog.y + dialog.h,
        "the last row {close:?} is inside the dialog {dialog:?}"
    );
    let mut g = Gestures::default();
    let e = click(&mut g, &mut host, &ctx, mid(close));
    assert!(
        emitted(&e, 9).contains(&vec![OscType::String("click".into())]),
        "the press reached the button: {:?}",
        emitted(&e, 9)
    );
}

/// **A window's scopes are read before the table**: in a window whose `keys`
/// name note entry, `E` is that scope's pitch where everywhere else it is the
/// host's `split`, and Escape -- with no dialog up and no element that takes
/// it -- is the scope's to bind rather than the front's close.
#[test]
fn a_windows_key_scopes_are_read_before_the_table() {
    let mut host = host_from(&scoped(
        r#"{"type":"window","keys":["score","note_entry"],"children":[
            {"id":5,"type":"label","text":"x"}]}"#,
    ));
    let mut g = Gestures::default();
    let ctx = ctx();
    let effects = g
        .press_key(&mut host, &ctx, Key::Char('e'), None)
        .expect("consumed");
    assert_eq!(
        emitted(&effects, 1),
        vec![vec![OscType::String("pitch_e".into())]]
    );
    let effects = g
        .press_key(&mut host, &ctx, Key::Escape, None)
        .expect("the scope's, not the front's");
    assert_eq!(
        emitted(&effects, 1),
        vec![vec![OscType::String("entry_off".into())]]
    );
    // a list rides a scalar wire as its JSON string
    host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: crate::host::GUI_SET.into(),
            args: vec![
                OscType::Int(1),
                OscType::String("keys".into()),
                OscType::String(r#"["note_entry"]"#.into()),
            ],
        }),
        from(),
    );
    assert_eq!(host.window_keys(1), vec!["note_entry".to_string()]);
    // the scopes go with the prop, and Escape is the front's again
    host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: crate::host::GUI_SET.into(),
            args: vec![
                OscType::Int(1),
                OscType::String("keys".into()),
                OscType::String("score".into()),
            ],
        }),
        from(),
    );
    assert_eq!(host.window_keys(1), vec!["score".to_string()]);
    // one key, another verb: outside note entry Escape lets the selection go
    let effects = g
        .press_key(&mut host, &ctx, Key::Escape, None)
        .expect("the score's own");
    assert_eq!(
        emitted(&effects, 1),
        vec![vec![OscType::String("deselect".into())]]
    );
    // and with no scope named it is the front's again
    host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: crate::host::GUI_SET.into(),
            args: vec![
                OscType::Int(1),
                OscType::String("keys".into()),
                OscType::String("[]".into()),
            ],
        }),
        from(),
    );
    assert!(host.window_keys(1).is_empty());
    assert!(g.press_key(&mut host, &ctx, Key::Escape, None).is_none());
}

/// **F1 shows the window's keys**, in the middle of the window: a section
/// per scope in force and the table's own, read and never picked. A press
/// anywhere but its close mark is swallowed; the close mark and Escape take
/// it down.
#[test]
fn f1_shows_the_windows_keys_until_its_close_mark_or_escape() {
    let mut host = host_from(&scoped(
        r#"{"type":"window","margin":0,"flow":"col","keys":["score"],"children":[
            {"id":6,"type":"button","label":"under","weight":1}]}"#,
    ));
    let mut g = Gestures::default();
    let ctx = ctx();
    let effects = g
        .press_key(&mut host, &ctx, Key::F(1), None)
        .expect("consumed");
    assert!(
        emitted(&effects, 1).is_empty(),
        "the host's, not the owner's"
    );
    let stack = host.popup(1).expect("the sheet is up");
    assert_eq!(stack.owner, Owner::Keys);
    let labels: Vec<&str> = stack.levels[0]
        .entries
        .iter()
        .map(|e| e.label.as_str())
        .collect();
    assert!(labels.contains(&"Score") && labels.contains(&"Window"));
    assert!(labels.contains(&"Note entry on or off"));
    assert!(stack.levels[0].entries.iter().all(|e| !e.pickable()));
    // centred, with a strip that ends in the close mark
    let placed = host.popup_placed(1, ctx.fb_w, ctx.fb_h).unwrap();
    let sheet = &placed[0];
    let mark = sheet.close().expect("the sheet has a close mark");
    let work = host.content_area(1, ctx.fb_w, ctx.fb_h);
    let mid_of = |r: Rect| (r.x + r.w * 0.5, r.y + r.h * 0.5);
    let (sx, sy) = mid_of(sheet.rect);
    let (wx, wy) = mid_of(work);
    assert!(
        (sx - wx).abs() < 1.0 && (sy - wy).abs() < 1.0,
        "{:?} stands in the middle of {work:?}",
        sheet.rect
    );
    // a press on a row, and one off the sheet, leave it up and reach nothing
    let row = sheet.rows[1];
    let effects = click(&mut g, &mut host, &ctx, mid(row));
    assert!(emitted(&effects, 6).is_empty());
    assert!(host.popup(1).is_some(), "a row does not take it down");
    let effects = click(&mut g, &mut host, &ctx, (2.0, 2.0));
    assert!(
        emitted(&effects, 6).is_empty(),
        "the button under it is not reached"
    );
    assert!(host.popup(1).is_some(), "a press off it does not either");
    click(&mut g, &mut host, &ctx, mid(mark));
    assert!(host.popup(1).is_none(), "its close mark does");
    g.press_key(&mut host, &ctx, Key::F(1), None);
    assert!(host.popup(1).is_some());
    g.press_key(&mut host, &ctx, Key::Escape, None);
    assert!(host.popup(1).is_none(), "and so does Escape");
}

/// **A sheet taller than the window scrolls**: the arrows a row at a time,
/// Home and End to its ends, and never past them.
#[test]
fn the_key_sheet_scrolls_by_the_arrows_and_the_ends() {
    let mut host = host_from(&scoped(
        r#"{"type":"window","margin":0,"flow":"col","keys":["score","note_entry"],
            "children":[{"id":6,"type":"label","text":"x","weight":1}]}"#,
    ));
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 600, 240);
    g.press_key(&mut host, &ctx, Key::F(1), None);
    let placed = |host: &Host| host.popup_placed(1, ctx.fb_w, ctx.fb_h).unwrap()[0].clone();
    let first = placed(&host);
    assert!(
        first.max_scroll > 0.0,
        "the sheet is longer than the window"
    );
    let work = host.content_area(1, ctx.fb_w, ctx.fb_h);
    assert!(
        first.rect.h <= work.h * crate::host::popup::CENTRE_FILL + 0.5,
        "and stands over it, not as tall as it: {:?} in {work:?}",
        first.rect
    );
    assert!(
        first.rows[0].y >= first.body().y,
        "the rows start under the strip"
    );
    g.press_key(&mut host, &ctx, Key::Down, None);
    assert!(placed(&host).scroll > 0.0);
    g.press_key(&mut host, &ctx, Key::End, None);
    assert_eq!(placed(&host).scroll, first.max_scroll);
    g.press_key(&mut host, &ctx, Key::Down, None);
    assert_eq!(placed(&host).scroll, first.max_scroll, "no further");
    g.press_key(&mut host, &ctx, Key::Up, None);
    assert!(placed(&host).scroll < first.max_scroll);
    g.press_key(&mut host, &ctx, Key::Home, None);
    assert_eq!(placed(&host).scroll, 0.0);
    assert!(host.popup(1).is_some(), "walking it never takes it down");
}

/// **A window whose owner asks first is not closed by its close mark**: the
/// window reports `close` -- its bar's entry, where it has one -- and stays
/// until the owner frees it. A second request the owner has not answered
/// closes it; a window that does not ask closes at once.
#[test]
fn a_window_that_asks_first_reports_its_close_to_the_owner() {
    let mut g = Gestures::default();
    let ctx = ctx();
    let mut host = host_from(r#"{"type":"window","ask_close":true,"children":[]}"#);
    let effects = g.close(&mut host, &ctx).expect("the owner is asked");
    assert_eq!(
        emitted(&effects, 1),
        vec![vec![OscType::String("close".into())]]
    );
    assert!(
        g.close(&mut host, &ctx).is_none(),
        "asked again with no answer, it closes"
    );
    // an owner that answered is asked again
    g.close(&mut host, &ctx).expect("asked");
    host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: super::super::GUI_ACK.into(),
            args: vec![OscType::Int(1), OscType::Int(0)],
        }),
        from(),
    );
    assert!(g.close(&mut host, &ctx).is_some(), "asked, not closed");
    // a bar with a Close entry: the mark is that entry's pick
    let mut host = host_from(
        r#"{"type":"window","ask_close":true,
            "menu":[{"label":"File","menu":[{"label":"Close","verb":"close"}]}],"children":[]}"#,
    );
    let effects = g.close(&mut host, &ctx).expect("asked");
    assert_eq!(
        emitted(&effects, 1),
        vec![vec![
            OscType::String("menu".into()),
            OscType::String("close".into())
        ]]
    );
    let mut host = host_from(r#"{"type":"window","children":[]}"#);
    assert!(g.close(&mut host, &ctx).is_none(), "it does not ask");
}

/// **A window that asks is told when it is activated and put away**, as its
/// `activated` verb, and one that does not ask hears nothing; neither says
/// anything on the status bar.
#[test]
fn a_window_that_asks_is_told_when_it_is_activated() {
    let mut g = Gestures::default();
    let ctx = ctx();
    let mut host = host_from(r#"{"type":"window","activation":true,"children":[]}"#);
    let came = g.activated(&mut host, &ctx, true).expect("told");
    assert_eq!(
        emitted(&came, 1),
        vec![vec![OscType::String("activated".into()), OscType::Int(1)]]
    );
    let went = g.activated(&mut host, &ctx, false).expect("told");
    assert_eq!(
        emitted(&went, 1),
        vec![vec![OscType::String("activated".into()), OscType::Int(0)]]
    );
    assert!(
        host.statuses().get(&1).is_none_or(|s| s.is_empty()),
        "no line said"
    );
    let mut quiet = host_from(r#"{"type":"window","children":[]}"#);
    assert!(g.activated(&mut quiet, &ctx, true).is_none());
}

/// **A row of tools is read across its gaps**: just after a tip went for
/// the pointer leaving its tool, the next tool's shows at once; later, it
/// waits again.
#[test]
fn tips_show_at_once_just_after_one_went() {
    let mut host = host_from(TIPS);
    let mut g = Gestures::default();
    let mut ctx = ctx();
    let (a, b, c) = (
        mid(rect_of(&host, &ctx, 5)),
        mid(rect_of(&host, &ctx, 6)),
        mid(rect_of(&host, &ctx, 7)),
    );
    ctx.now_ms = 1000.0;
    g.motion(&mut host, &ctx, a.0, a.1);
    ctx.now_ms = 1700.0;
    g.elapsed(&mut host, &ctx);
    assert_eq!(tip_of(&host).as_deref(), Some("the first tool"));
    // off it, onto a widget with none: it goes
    g.motion(&mut host, &ctx, c.0, c.1);
    assert_eq!(tip_of(&host), None);
    // back within the moment: at once
    ctx.now_ms = 1900.0;
    g.motion(&mut host, &ctx, b.0, b.1);
    assert_eq!(tip_of(&host).as_deref(), Some("the second tool"));
    g.motion(&mut host, &ctx, c.0, c.1);
    // long after: it waits
    ctx.now_ms = 2500.0;
    g.motion(&mut host, &ctx, a.0, a.1);
    assert_eq!(tip_of(&host), None);
    assert!(g.pending());
}

/// **No tip while a list is open**: the list is what is read.
#[test]
fn no_tip_shows_while_a_list_is_open() {
    let mut host = host_from(
        r#"{"type":"window","margin":0,"flow":"col","children":[
        {"id":5,"type":"button","label":"A","w":60,"tip":"the first tool"},
        {"id":6,"type":"label","text":"filler","weight":1},
        {"id":7,"type":"choice","label":"View","w":200,"h":48,
         "options":["one","two","three"]}]}"#,
    );
    let mut g = Gestures::default();
    let mut ctx = ctx();
    let chooser = mid(rect_of(&host, &ctx, 7));
    click(&mut g, &mut host, &ctx, chooser);
    assert!(host.popup(1).is_some(), "the list is open");
    let a = mid(rect_of(&host, &ctx, 5));
    ctx.now_ms = 1000.0;
    g.motion(&mut host, &ctx, a.0, a.1);
    ctx.now_ms = 3000.0;
    g.elapsed(&mut host, &ctx);
    assert_eq!(tip_of(&host), None);
}

const CODE: &str = r#"{"type":"window","margin":0,"flow":"col","children":[
    {"id":9,"type":"code","weight":1,"text":"import math\nmath.sqrt(2)\n"}]}"#;

/// **Rests the pointer on a name of the code** and has the owner answer
/// what it is with `doc`: the point it rested at.
fn rest_on_a_name(
    g: &mut Gestures,
    host: &mut Host,
    ctx: &mut GestureCtx,
    doc: &str,
) -> (f64, f64) {
    let r = rect_of(host, ctx, 9);
    let mut found = None;
    'scan: for dy in (2..60).step_by(4) {
        for dx in (2..300).step_by(4) {
            let at = (f64::from(r.x) + dx as f64, f64::from(r.y) + dy as f64);
            ctx.now_ms += 1000.0;
            g.motion(host, ctx, at.0, at.1);
            ctx.now_ms += 400.0;
            let out = g.elapsed(host, ctx);
            if emitted(&out, 9)
                .iter()
                .any(|a| a.first() == Some(&OscType::String("inspect".into())))
            {
                found = Some(at);
                break 'scan;
            }
        }
    }
    let at = found.expect("a name to rest on");
    let answer = serde_json::json!({
        "name": "sqrt", "kind": "function", "signature": "sqrt(x, /)", "doc": doc,
    });
    host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: super::super::GUI_SET.into(),
            args: vec![
                OscType::Int(9),
                OscType::String("inspected".into()),
                OscType::String(answer.to_string()),
            ],
        }),
        from(),
    );
    at
}

fn card_rect(host: &Host, ctx: &GestureCtx) -> Option<Rect> {
    host.card_place(ctx.def_id, ctx.fb_w, ctx.fb_h)
        .map(|(.., place)| place.rect)
}

/// **The code's help on a name is a hover card**: the pointer can cross into
/// it and it stays; it goes a moment after the pointer leaves both, and
/// Escape takes it down at once.
#[test]
fn a_hover_card_stays_while_the_pointer_is_on_it_and_goes_after_it_leaves() {
    let mut host = host_from(CODE);
    let mut g = Gestures::default();
    let mut ctx = ctx();
    let at = rest_on_a_name(&mut g, &mut host, &mut ctx, "Return the square root of x.");
    let card = card_rect(&host, &ctx).expect("the card is up");
    // into the card: it stays, however long
    let inside = mid(card);
    g.motion(&mut host, &ctx, inside.0, inside.1);
    ctx.now_ms += 5000.0;
    g.elapsed(&mut host, &ctx);
    assert!(card_rect(&host, &ctx).is_some(), "on the card it stays");
    // back on the name: it stays
    g.motion(&mut host, &ctx, at.0, at.1);
    ctx.now_ms += 5000.0;
    g.elapsed(&mut host, &ctx);
    assert!(card_rect(&host, &ctx).is_some(), "on the name it stays");
    // off both: a moment, then it goes
    let away = (f64::from(card.x + card.w) + 40.0, at.1 + 120.0);
    g.motion(&mut host, &ctx, away.0, away.1);
    assert!(card_rect(&host, &ctx).is_some(), "not at once");
    assert!(g.pending(), "the leave is timed");
    ctx.now_ms += 100.0;
    g.elapsed(&mut host, &ctx);
    assert!(card_rect(&host, &ctx).is_some());
    ctx.now_ms += 300.0;
    assert!(redraws(&g.elapsed(&mut host, &ctx)));
    assert!(card_rect(&host, &ctx).is_none(), "gone after it left both");
    // Escape takes it down at once, and is spent on it
    rest_on_a_name(&mut g, &mut host, &mut ctx, "Return the square root of x.");
    assert!(card_rect(&host, &ctx).is_some());
    g.key_began(&mut host, &ctx);
    assert!(
        card_rect(&host, &ctx).is_some(),
        "a key is the card's to answer"
    );
    assert!(g.press_key(&mut host, &ctx, Key::Escape, None).is_some());
    assert!(card_rect(&host, &ctx).is_none());
}

/// **A hover card scrolls and its text is selected and copied**: the wheel
/// over it scrolls its documentation and nothing under it; a drag selects;
/// the copy key copies what is selected and leaves the card up.
#[test]
fn a_hover_card_scrolls_and_its_text_is_selected_and_copied() {
    let mut host = host_from(CODE);
    let mut g = Gestures::default();
    let mut ctx = ctx();
    let doc: String = (0..80)
        .map(|i| format!("Line {i} of the help.\n"))
        .collect();
    rest_on_a_name(&mut g, &mut host, &mut ctx, &doc);
    let (_, note, place) = host.card_place(1, ctx.fb_w, ctx.fb_h).expect("a card");
    assert!(place.max_scroll() > 0.0, "a long help scrolls");
    let inside = mid(place.doc);
    g.motion(&mut host, &ctx, inside.0, inside.1);
    assert!(redraws(&g.wheel(&mut host, &ctx, inside.0, inside.1, -1.0)));
    let scroll = host
        .popups(1)
        .and_then(|p| p.card.as_ref())
        .map(|c| c.scroll);
    assert!(scroll.is_some_and(|s| s > 0.0), "scrolled: {scroll:?}");
    // a drag over the head selects it
    let (y, x0, x1) = (
        f64::from(place.head.y + place.head.h * 0.5),
        f64::from(place.head.x) + 0.1,
        f64::from(place.head.x + place.head.w),
    );
    g.press(&mut host, &ctx, x0, y);
    assert!(g.dragging(), "the press on the card selects");
    g.drag_to(&mut host, &ctx, x1, y);
    g.release(&mut host, &ctx, x1, y);
    assert!(card_rect(&host, &ctx).is_some(), "the press kept it up");
    let sel = host
        .popups(1)
        .and_then(|p| p.card.as_ref())
        .and_then(|c| c.select)
        .expect("a selection");
    assert_eq!(note.text_between(&place, sel.0, sel.1), "sqrt(x, /)");
    // the copy key copies it, and the card stays
    ctx.ctrl = true;
    g.key_began(&mut host, &ctx);
    g.press_key(&mut host, &ctx, Key::Char('c'), None);
    assert_eq!(host.clipboard.text(), "sqrt(x, /)");
    assert!(card_rect(&host, &ctx).is_some());
}

/// **A hover card's scroll bar is dragged**: a press on its thumb holds
/// it, a drag scrolls the documentation as far as the thumb moves and
/// selects nothing, and a release lets it go.
#[test]
fn a_hover_card_s_scroll_bar_is_dragged() {
    let mut host = host_from(CODE);
    let mut g = Gestures::default();
    let mut ctx = ctx();
    let doc: String = (0..80)
        .map(|i| format!("Line {i} of the help.\n"))
        .collect();
    rest_on_a_name(&mut g, &mut host, &mut ctx, &doc);
    let (_, _, place) = host.card_place(1, ctx.fb_w, ctx.fb_h).expect("a card");
    let m = *host.metrics_for(1);
    let bar = place.bar(0.0, &m).expect("a long help has a bar");
    let thumb = mid(bar.thumb);
    g.motion(&mut host, &ctx, thumb.0, thumb.1);
    g.press(&mut host, &ctx, thumb.0, thumb.1);
    assert!(g.dragging(), "the thumb is held");
    let to = thumb.1 + f64::from(bar.track.h) * 0.25;
    g.drag_to(&mut host, &ctx, thumb.0, to);
    g.release(&mut host, &ctx, thumb.0, to);
    let card = host.popups(1).and_then(|p| p.card.clone()).expect("up");
    assert!(card.scroll > 0.0, "scrolled: {}", card.scroll);
    assert!(card.select.is_none(), "and nothing selected");
}

/// The placement of dialog `id` in window 1.
fn dialog_place(
    host: &Host,
    ctx: &GestureCtx,
    id: i32,
) -> (Rect, Option<Rect>, crate::host::chrome::Head) {
    let placed = host.layout_window(ctx.def_id, ctx.fb_w, ctx.fb_h).unwrap();
    let p = placed.iter().find(|p| p.widget.id == Some(id)).expect("up");
    (
        p.rect,
        crate::host::chrome::close_mark(p),
        crate::host::chrome::head(p).expect("a dialog has a header"),
    )
}

/// **A dialog is the specification's card**: centred, no wider than a
/// dialog grows, its title and description inside its inset with the body
/// the dialog's gap under them, and its close button at the corner. An
/// alert dialog grows wider, sets its title larger and has no close button.
#[test]
fn a_dialog_is_a_card_with_a_header_and_an_alert_has_no_close_button() {
    let m = clausters_gui_draw::metrics::Metrics::default();
    let ctx = ctx();
    let host = host_from(
        r#"{"type":"window","margin":0,"status":false,"children":[
            {"id":5,"type":"layout","modal":1,"title":"Rename","hug":1,
             "description":"The new name of the take, as the list of takes shows it to everyone who reads the session.",
             "children":[{"id":6,"type":"text","value":"take 1"}]}]}"#,
    );
    let (card, close, head) = dialog_place(&host, &ctx, 5);
    assert_eq!(
        card.w, m.dialog_w,
        "the card takes the width a dialog takes"
    );
    assert_eq!(card.x, ((600.0 - card.w) * 0.5).round());
    let title = head.title.expect("titled");
    assert_eq!(
        (title.x, title.y),
        (card.x + m.dialog_inset, card.y + m.dialog_inset)
    );
    assert_eq!(title.h, m.type_sm, "the title is set solid");
    assert!(head.lines.len() >= 2, "the description wraps to the card");
    assert_eq!(head.lines[0].1.y, title.y + title.h + m.dialog_title_gap);
    let field = rect_of(&host, &ctx, 6);
    assert_eq!(
        field.y,
        title.y + head.h,
        "the body stands the gap under it"
    );
    assert_eq!(field.x, card.x + m.dialog_inset);
    let close = close.expect("a dialog has a close button");
    assert_eq!(
        (close.x + close.w, close.y),
        (card.x + card.w - m.dialog_close, card.y + m.dialog_close)
    );

    let host = host_from(
        r#"{"type":"window","margin":0,"status":false,"children":[
            {"id":5,"type":"layout","modal":1,"alert":1,"title":"Save the session?","hug":1,
             "children":[{"id":6,"type":"label","text":"x"}]}]}"#,
    );
    let (card, close, head) = dialog_place(&host, &ctx, 5);
    assert_eq!(card.w, m.alert_w);
    assert_eq!(close, None, "an alert dialog is answered by its buttons");
    assert_eq!(head.title.expect("titled").h, m.line_lg);
}

/// **A press on the scrim asks a dialog to go**, as Escape does; an alert
/// dialog stays, answered by its buttons alone. Either way nothing behind it
/// hears the press.
#[test]
fn a_press_on_the_scrim_asks_a_dialog_to_go_and_an_alert_stays() {
    let ctx = ctx();
    for (alert, said) in [(0, true), (1, false)] {
        let mut host = host_from(&format!(
            r#"{{"type":"window","margin":0,"flow":"col","status":false,"children":[
                {{"id":2,"type":"button","label":"behind","h":32}},
                {{"id":5,"type":"layout","modal":1,"alert":{alert},"w":200,"h":100,"children":[
                    {{"id":6,"type":"label","text":"x"}}]}}]}}"#
        ));
        let mut g = Gestures::default();
        let e = click(&mut g, &mut host, &ctx, (5.0, 5.0));
        let cancel = vec![vec![OscType::String("cancel".into())]];
        assert_eq!(emitted(&e, 5) == cancel, said, "alert {alert}");
        assert!(emitted(&e, 2).is_empty(), "nothing behind it is reached");
    }
}

/// **A dialog takes the keys as it comes up and gives them back as it
/// goes**: to its first field, else to its default button; back to what had
/// them, which a page of a stack turning shows as well as a definition.
#[test]
fn a_dialog_takes_the_keys_as_it_comes_up_and_gives_them_back() {
    let dialog = |field: bool| {
        let first = if field {
            r#"{"id":11,"type":"text","value":""},"#
        } else {
            ""
        };
        format!(
            r#"{{"type":"window","margin":0,"flow":"col","status":false,"children":[
                {{"id":2,"type":"text","value":"behind"}},
                {{"id":3,"type":"layout","flow":"stack","index":0,"h":0,"margin":0,"children":[
                    {{"type":"layout"}},
                    {{"type":"layout","children":[
                        {{"id":10,"type":"layout","modal":1,"w":300,"h":200,"children":[
                            {first}
                            {{"id":12,"type":"button","label":"Cancel"}},
                            {{"id":13,"type":"button","label":"OK","default":1}}]}}]}}]}}]}}"#
        )
    };
    let set = |host: &mut Host, id: i32, key: &str, v: i32| {
        let mut effects = Vec::new();
        host.set_props(id, vec![(key.into(), serde_json::json!(v))], &mut effects);
    };
    for (field, stop) in [(true, 11), (false, 13)] {
        let mut host = host_from(&dialog(field));
        set(&mut host, 2, "focus", 1);
        assert_eq!(host.focused(), Some((1, 2)));
        set(&mut host, 3, "index", 1);
        assert_eq!(host.focused(), Some((1, stop)), "field {field}");
        // Tab comes round inside it, never behind it
        let mut g = Gestures::default();
        for _ in 0..4 {
            press_key(&mut g, &mut host, &ctx(), Key::Tab);
            let (_, at) = host.focused().expect("the dialog keeps the keys");
            assert!([11, 12, 13].contains(&at), "on {at}");
        }
        set(&mut host, 3, "index", 0);
        assert_eq!(host.focused(), Some((1, 2)), "given back");
    }
}

/// **A pane's dialog stands over the whole dock**, centred in the area its
/// front gave it, as its lists do.
#[test]
fn a_panes_dialog_is_centred_over_the_dock() {
    let ctx = ctx();
    let mut host = host_from(
        r#"{"type":"window","margin":0,"status":false,"children":[
            {"id":5,"type":"layout","modal":1,"w":200,"h":100,"children":[
                {"id":6,"type":"label","text":"x"}]}]}"#,
    );
    let dock = Rect::new(-300.0, -50.0, 1200.0, 800.0);
    host.set_popup_area(1, Some(dock));
    let (card, ..) = dialog_place(&host, &ctx, 5);
    assert_eq!((card.x, card.y), (200.0, 300.0));
}

/// **A wrapped label in a dialog stands as tall as its lines**: its height
/// follows the card's width, so a help text under a field is not cut to one
/// line, and the button under it stays inside the card.
#[test]
fn a_wrapped_label_in_a_dialog_is_as_tall_as_its_lines() {
    let ctx = ctx();
    let host = host_from(
        r#"{"type":"window","margin":0,"status":false,"children":[
            {"id":5,"type":"layout","modal":1,"hug":1,"title":"Help","children":[
                {"id":6,"type":"label","wrap":1,"text":"Runs every time the session starts: when it is first needed, when it is restarted or when its environment changes, and is shown in the post window."},
                {"id":7,"type":"button","label":"Close"}]}]}"#,
    );
    let card = rect_of(&host, &ctx, 5);
    let help = rect_of(&host, &ctx, 6);
    let close = rect_of(&host, &ctx, 7);
    assert!(help.h >= 2.0 * 16.0, "{} high", help.h);
    assert!(close.y >= help.y + help.h);
    assert!(close.y + close.h <= card.y + card.h);
}

/// **A divided list** stands each row at least a list row tall, with a
/// line between each two of them across the whole list.
#[test]
fn a_divided_list_has_rows_a_row_tall_and_a_line_between_them() {
    let m = clausters_gui_draw::metrics::Metrics::default();
    let ctx = ctx();
    let host = host_from(
        r#"{"type":"window","margin":0,"flow":"col","status":false,"children":[
            {"id":2,"type":"layout","flow":"col","divide":1,"hug":1,"frame":1,"margin":0,"children":[
                {"id":3,"type":"label","text":"one"},
                {"id":4,"type":"label","text":"two"},
                {"id":5,"type":"label","text":"three"}]}]}"#,
    );
    let rows: Vec<Rect> = [3, 4, 5]
        .iter()
        .map(|id| rect_of(&host, &ctx, *id))
        .collect();
    assert!(rows.iter().all(|r| r.h >= m.list_row_h), "{rows:?}");
    assert_eq!(rows[1].y, rows[0].y + rows[0].h, "no gap of their own");
    let placed = host.layout_window(1, ctx.fb_w, ctx.fb_h).unwrap();
    let lines = crate::host::chrome::rules(&placed, &m);
    assert_eq!(lines.len(), 2);
    let list = rect_of(&host, &ctx, 2);
    assert!(lines.iter().all(|(_, l)| l.x == list.x && l.w == list.w));
}

/// **A scroll bar is the specification's**: as thick as `scrollbar_w`,
/// with no track drawn under its thumb.
#[test]
fn a_planes_scroll_bar_is_as_thick_as_the_role() {
    let m = clausters_gui_draw::metrics::Metrics::default();
    let ctx = ctx();
    let host = host_from(
        r#"{"type":"window","margin":0,"status":false,"children":[
            {"id":2,"type":"plane","bars":true,"zoom":false,"axis":"y","margin":0,"content_h":4000,"children":[
                {"id":3,"type":"label","text":"x"}]}]}"#,
    );
    let placed = host.layout_window(1, ctx.fb_w, ctx.fb_h).unwrap();
    let plane = placed.iter().find(|p| p.widget.id == Some(2)).unwrap();
    let bars = crate::host::chrome::bars(plane, &m);
    assert_eq!(bars.len(), 1);
    assert_eq!(bars[0].track.w, m.scrollbar_w);
    assert_eq!(bars[0].thumb.w, m.scrollbar_w);
}

/// **The dock's bar shows the menu of the application on the page shown**:
/// the application's window draws no bar of its own, the edge bar shows its
/// titles, a pick that flips a state is the application's (its menu and the
/// bar's both read it), and one that does not is performed on its window.
#[test]
fn the_docks_bar_shows_the_menu_of_the_application_shown() {
    use crate::host::menubar::{self, MenuShown};
    let mut host = host_from(BAR);
    host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: GUI_DEF.into(),
            args: vec![
                OscType::Int(2),
                OscType::String(
                    r#"{"type":"window","dock_edge":"top","h":40,
                        "menu":[{"label":"Code","menu":[{"label":"New","verb":"new_file"}]}],
                        "children":[{"id":20,"type":"label","text":"tools"}]}"#
                        .into(),
                ),
            ],
        }),
        from(),
    );
    // the bar shows the application's menu, as the dock decides
    // (`dock::menus`, tested there)
    host.set_popup_area(2, Some(Rect::new(0.0, 0.0, 600.0, 440.0)));
    let entries = menubar::own_entries(host.window_def(1).unwrap())
        .unwrap()
        .to_vec();
    host.set_menu_shown(2, MenuShown::From { window: 1, entries });
    host.set_menu_shown(1, MenuShown::Away);
    let own =
        |host: &Host, id| menubar::own_entries(host.window_def(id).unwrap()).map(<[_]>::to_vec);
    let shown = |host: &Host, id| menubar::entries(host.window_def(id).unwrap()).map(<[_]>::to_vec);
    assert_eq!(
        shown(&host, 2),
        own(&host, 1),
        "the bar shows the application's"
    );
    assert_eq!(shown(&host, 1), None);
    assert!(
        host.menu_bar_rect(1, 600, 400).is_none(),
        "and it has no bar of its own"
    );

    // a pick that flips a state: the application's
    let mut g = Gestures::default();
    let bar = GestureCtx::new(2, 600, 40);
    let title = |host: &Host, n: usize| {
        let band = host.menu_bar_rect(2, 600, 40).unwrap();
        let titles = menubar::titles(
            menubar::entries(host.window_def(2).unwrap()).unwrap(),
            band,
            host.metrics_for(2),
        );
        mid(titles[n].1)
    };
    let at = title(&host, 0);
    click(&mut g, &mut host, &bar, at);
    let placed = host.popup_placed(2, 600, 40).unwrap();
    let effects = click(&mut g, &mut host, &bar, mid(placed[0].rows[2]));
    assert_eq!(
        emitted(&effects, 1),
        vec![vec![
            OscType::String("menu".into()),
            OscType::String("loop".into()),
            OscType::Int(1)
        ]],
        "the application's window reports it"
    );
    let file = |host: &Host, id| shown(host, id).unwrap()[0].submenu().unwrap()[2].marked();
    assert!(
        menubar::own_entries(host.window_def(1).unwrap()).unwrap()[0]
            .submenu()
            .unwrap()[2]
            .marked()
    );
    assert!(file(&host, 2), "the bar shows it checked");

    // one that does not: performed on the application's window
    click(&mut g, &mut host, &bar, at);
    let placed = host.popup_placed(2, 600, 40).unwrap();
    let effects = click(&mut g, &mut host, &bar, mid(placed[0].rows[0]));
    assert!(effects.contains(&GestureEffect::Perform {
        window: 1,
        verb: "open".into()
    }));
}

/// **The caret blinks while the keys are on text**, half a cycle shown and
/// half hidden, and a press or a key shows it again at once; the front is
/// told when the next change comes, and nothing with no caret.
#[test]
fn the_caret_blinks_and_holds_still_under_the_hand() {
    let mut host = host_from(CODE);
    assert!(
        host.caret_due(0.0).is_none(),
        "no caret, nothing to wake for"
    );
    host.focus(1, 9);
    let half = crate::host::blink::Blink::DEFAULT_MS * 0.5;
    assert!(host.caret_shown(1, 10.0));
    assert!(
        !host.caret_shown(1, half + 10.0),
        "hidden in the second half"
    );
    assert_eq!(host.caret_due(10.0), Some((1, half)));
    assert_eq!(host.caret_due(half + 10.0), Some((1, 2.0 * half)));
    // a key shows it again, from there
    let mut g = Gestures::default();
    let mut ctx = ctx();
    ctx.now_ms = half + 100.0;
    let _ = g.press_key(&mut host, &ctx, Key::Char('x'), None);
    assert!(host.caret_shown(1, half + 120.0), "shown at once");
    host.set_caret_blink(0.0);
    assert!(host.caret_shown(1, 3.0 * half + 1.0) && host.caret_due(0.0).is_none());
}
