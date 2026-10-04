//! The popup layer through the gesture machine: a chooser's list, a menu in
//! its three places, and what the wheel, the keys and pointer motion do while
//! one is open.

use clausters_core::osc::{OscMessage, OscPacket, OscType};

use super::super::layout::Rect;
use super::super::popup::Owner;
use super::super::widget::element::Key;
use super::super::{ClientId, GUI_DEF, Host};
use super::*;

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
            ("Select all", true)
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
