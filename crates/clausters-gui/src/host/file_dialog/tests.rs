use super::*;
use crate::host::{ClientId, GUI_SET, Host, HostEffect};
use clausters_core::osc::{OscMessage, OscPacket};
use serde_json::json;

fn request(props: Value) -> Request {
    Request::read(props.as_object().unwrap())
}

fn s(text: &str) -> OscType {
    OscType::String(text.into())
}

fn drawn(props: Value) -> Drawn {
    let mut d = Drawn::new(20, 1000, request(props));
    d.open();
    d
}

/// A listing of folder `path` holding files `names` and a folder `sub`.
fn listing(path: &str, names: &[&str]) -> Listing {
    let entry = |name: &str, dir: bool| crate::host::files::DirEntry {
        name: name.into(),
        dir,
        size: 0,
    };
    Listing::ordered(
        path.into(),
        names
            .iter()
            .map(|n| entry(n, false))
            .chain([entry("sub", true)])
            .collect(),
    )
}

/// The props an answer sets on widget `id`, merged.
fn set_on(answer: &Answer, id: i32) -> Map<String, Value> {
    answer
        .set
        .iter()
        .filter(|(w, _)| *w == id)
        .flat_map(|(_, props)| props.iter().cloned())
        .collect()
}

/// The report an answer carries, as text.
fn report(answer: &Answer) -> Option<Vec<String>> {
    answer.report.as_ref().map(|r| {
        r.iter()
            .map(|a| match a {
                OscType::String(s) => s.clone(),
                other => format!("{other:?}"),
            })
            .collect()
    })
}

/// **What each mode offers in its file-type field.**
#[test]
fn a_request_offers_its_kinds_and_every_file_where_it_says() {
    let kinds = json!([["WAV", ["wav", "wave"]], ["FLAC", ["flac"]]]);
    let labels = |r: Request| -> Vec<String> { r.offers().into_iter().map(|o| o.label).collect() };
    assert_eq!(
        labels(request(json!({"kinds": kinds}))),
        [
            "All supported (*.wav, *.wave, *.flac)",
            "WAV (*.wav, *.wave)",
            "FLAC (*.flac)",
            "All files (*.*)"
        ]
    );
    assert_eq!(
        labels(request(json!({"mode": "save", "kinds": kinds}))),
        ["WAV (*.wav, *.wave)", "FLAC (*.flac)"]
    );
    assert_eq!(
        labels(request(
            json!({"mode": "save", "kinds": [["Python", ["py"]]], "all_files": 1})
        )),
        ["Python (*.py)", "All files (*.*)"]
    );
    assert!(request(json!({"mode": "folder"})).offers().is_empty());
    // the kinds also ride as the JSON a set carries a list as
    let carried = request(json!({"kinds": "[[\"MEI\", [\"mei\"]]]"}));
    assert_eq!(carried.kinds[0].extensions, ["mei"]);
}

/// **A save names the file as the kind chosen**, and with every file chosen
/// writes the name as typed; it opens in the folder of the file it writes.
#[test]
fn a_save_writes_the_kind_chosen() {
    let mut d = drawn(
        json!({"mode": "save", "kinds": [["MEI", ["mei"]]], "all_files": true,
        "path": "/scores/a.mei"}),
    );
    assert_eq!((d.dir.as_str(), d.typed.as_str()), ("/scores", "a.mei"));
    d.listed(1001, Ok(&listing("/scores", &["a.mei"])));
    d.event(1002, "b", &[]);
    assert_eq!(
        report(&d.event(1006, "click", &[])),
        Some(vec!["chosen".into(), "MEI".into(), "/scores/b.mei".into()])
    );
    assert!(!d.up, "a choice takes the dialog down");
    d.open();
    d.listed(1001, Ok(&listing("/scores", &["a.mei"])));
    d.event(1003, "", &[OscType::Int(1)]);
    d.event(1002, "notes.txt", &[]);
    assert_eq!(
        report(&d.event(1006, "click", &[])),
        Some(vec![
            "chosen".into(),
            ALL_FILES.into(),
            "/scores/notes.txt".into()
        ])
    );
}

/// A save with no name says so and stays up.
#[test]
fn a_save_with_no_name_says_so() {
    let mut d = drawn(json!({"mode": "save", "kinds": [["WAV", ["wav"]]]}));
    let answer = d.event(1006, "click", &[]);
    assert_eq!(answer.report, None);
    assert!(d.up);
    assert!(d.storage.says.contains("name"));
}

/// **An open opens what is selected, or the path typed**, several at once
/// where it chooses several, and a pick accepts.
#[test]
fn an_open_opens_the_selection_or_the_path_typed() {
    let mut d = drawn(json!({"kinds": [["WAV", ["wav"]]], "multiple": true, "path": "/takes"}));
    assert_eq!(d.dir, "/takes");
    d.event(1001, "select", &[s("/takes/a.wav"), s("/takes/b.wav")]);
    assert_eq!(
        report(&d.event(1006, "click", &[])),
        Some(vec![
            "chosen".into(),
            "WAV".into(),
            "/takes/a.wav".into(),
            "/takes/b.wav".into()
        ])
    );
    d.open();
    d.event(1002, "/elsewhere/c.wav", &[]);
    assert_eq!(
        report(&d.event(1006, "click", &[])).unwrap()[2],
        "/elsewhere/c.wav"
    );
    d.open();
    assert_eq!(
        report(&d.event(1001, "pick", &[s("/takes/d.wav")])).unwrap()[2],
        "/takes/d.wav"
    );
}

/// **It opens again where the hand left it**, and a file chosen in a folder
/// left behind is forgotten.
#[test]
fn it_opens_again_where_the_hand_went() {
    let mut d = drawn(json!({"path": "/takes"}));
    d.event(1001, "/takes/a.wav", &[]);
    assert_eq!(d.typed, "/takes/a.wav");
    let left = d.event(1001, "path", &[s("/takes/old")]);
    assert_eq!(set_on(&left, 1002)["value"], json!(""));
    assert_eq!(set_on(&left, 1000 + BAR)["path"], json!("/takes/old"));
    d.event(1005, "click", &[]);
    d.open();
    assert_eq!(d.dir, "/takes/old");
}

/// A kind chosen filters the chooser to its extensions, every file to none.
#[test]
fn a_kind_chosen_filters_the_chooser() {
    let mut d = drawn(json!({"kinds": [["WAV", ["wav"]]]}));
    assert_eq!(
        d.event(1003, "", &[OscType::Float(1.0)]).set,
        vec![(1001, vec![("filter".into(), json!([]))])]
    );
    assert_eq!(
        d.event(1003, "0", &[]).set,
        vec![(1001, vec![("filter".into(), json!(["wav"]))])]
    );
}

/// **A folder is the one shown**, or the one typed; it lists folders alone
/// and has no file type.
#[test]
fn a_folder_is_the_one_shown_or_typed() {
    let mut d = drawn(json!({"mode": "folder", "path": "/docs"}));
    assert_eq!(d.filter(), [FOLDERS_ONLY]);
    assert_eq!(
        report(&d.event(1006, "click", &[])),
        Some(vec!["chosen".into(), String::new(), "/docs".into()])
    );
    let tree = d.stand_in().to_string();
    assert!(!tree.contains("File type"));
}

/// **The tools are the dialog's**: a delete asks once, then asks the
/// chooser; the chooser's answer is said.
#[test]
fn the_tools_ask_the_chooser_and_say_its_answer() {
    let mut d = drawn(json!({"path": "/a"}));
    let delete = 1000 + TOOLS + 4;
    d.event(1001, "/a/x.wav", &[]);
    let asked = d.event(delete, "click", &[]);
    assert!(asked.set.iter().all(|(id, _)| *id != 1001), "not yet");
    let gone = d.event(delete, "click", &[]);
    assert!(gone.set.contains(&(
        1001,
        vec![("change".into(), json!({"op": "remove", "path": "/a/x.wav"}))]
    )));
    let said = d.event(1001, "changed", &[s("/a/x.wav: busy")]);
    let (id, props) = &said.set[0];
    assert_eq!(*id, 1004);
    assert!(props.contains(&("text".into(), json!("/a/x.wav: busy"))));
}

/// Escape, the close mark and Cancel report a cancel.
#[test]
fn a_cancel_is_reported() {
    let mut d = drawn(json!({}));
    assert_eq!(
        report(&d.event(1000, "cancel", &[])),
        Some(vec!["cancel".into()])
    );
    assert_eq!(
        d.event(1005, "click", &[]),
        Answer::default(),
        "down already"
    );
}

/// **A save over a file the folder holds asks first**: the accepting button
/// says Replace, a second accept replaces it, and a name changed takes the
/// question back.
#[test]
fn a_save_over_a_file_asks_before_replacing_it() {
    let mut d = drawn(json!({"mode": "save", "kinds": [["WAV", ["wav"]]], "path": "/takes/a.wav"}));
    d.listed(1001, Ok(&listing("/takes", &["a.wav"])));
    let asked = d.event(1006, "click", &[]);
    assert_eq!(asked.report, None);
    assert!(d.storage.says.contains("a.wav already exists"));
    assert_eq!(set_on(&asked, 1006)["label"], json!("Replace"));
    // another name: the question goes, and a new file is written at once
    let changed = d.event(1002, "b", &[]);
    assert_eq!(set_on(&changed, 1006)["label"], json!("Save"));
    assert_eq!(
        report(&d.event(1006, "click", &[])).unwrap()[2],
        "/takes/b.wav"
    );
    d.open();
    d.listed(1001, Ok(&listing("/takes", &["a.wav"])));
    d.event(1006, "click", &[]);
    assert_eq!(
        report(&d.event(1006, "click", &[])).unwrap()[2],
        "/takes/a.wav",
        "asked once, replaced on the second"
    );
}

/// **A save before its folder is listed, or to another folder**, is decided
/// once the folder is in: the dialog goes there first, the name kept.
#[test]
fn a_save_elsewhere_goes_there_first() {
    let mut d = drawn(json!({"mode": "save", "kinds": [["WAV", ["wav"]]], "path": "/takes/a.wav"}));
    d.event(1002, "/elsewhere/c", &[]);
    let went = d.event(1006, "click", &[]);
    assert_eq!(went.report, None);
    assert_eq!(set_on(&went, 1001)["path"], json!("/elsewhere"));
    assert_eq!(set_on(&went, 1002)["value"], json!("c.wav"));
    let done = d.listed(1001, Ok(&listing("/elsewhere", &[])));
    assert_eq!(report(&done).unwrap()[2], "/elsewhere/c.wav");
    // a folder that cannot be listed says why, and nothing is written
    d.open();
    d.event(1002, "/nowhere/c", &[]);
    d.event(1006, "click", &[]);
    let failed = d.listed(1001, Err("/nowhere: not found"));
    assert_eq!(failed.report, None);
    assert!(d.storage.says.contains("not found"));
}

/// **A save opens with the name's stem selected**, so what is typed
/// replaces it and the extension stays.
#[test]
fn a_save_opens_with_the_stem_selected() {
    let mut d = Drawn::new(
        20,
        1000,
        request(json!({"mode": "save", "path": "/t/take.2.wav"})),
    );
    let opened = d.open();
    assert_eq!(set_on(&opened, 1002)["select"], json!([0, 6]));
    assert!(d.stand_in().to_string().contains(r#""select":[0,6]"#));
    assert_eq!(stem_len(".bashrc"), 7);
    assert_eq!(stem_len("notes"), 5);
}

/// **The bar says where the chooser is, as its listing resolved it**, and
/// back, forward and up walk the folders the hand went through.
#[test]
fn back_forward_and_up_walk_where_the_hand_went() {
    let mut d = drawn(json!({}));
    let resolved = d.listed(1001, Ok(&listing("/home/ana", &[])));
    assert_eq!(set_on(&resolved, 1000 + BAR)["path"], json!("/home/ana"));
    assert_eq!(set_on(&resolved, 1000 + BACK)["enabled"], json!(false));
    d.event(1001, "path", &[s("/home/ana/sub")]);
    let up = d.event(1000 + UP, "click", &[]);
    assert_eq!(set_on(&up, 1001)["path"], json!("/home/ana"));
    let bar = d.event(1000 + BAR, GO, &[s("/home")]);
    assert_eq!(set_on(&bar, 1001)["path"], json!("/home"));
    assert_eq!(d.back, ["/home/ana", "/home/ana/sub", "/home/ana"]);
    let back = d.event(1000 + BACK, "click", &[]);
    assert_eq!(set_on(&back, 1001)["path"], json!("/home/ana"));
    assert_eq!(set_on(&back, 1000 + FORWARD)["enabled"], json!(true));
    let forward = d.event(1000 + FORWARD, "click", &[]);
    assert_eq!(set_on(&forward, 1001)["path"], json!("/home"));
    assert!(d.forward.is_empty());
}

/// **The path typed**: the field takes the bar's place with the path
/// selected; Enter goes where it says, Escape gives the bar back and the
/// dialog stays up.
#[test]
fn a_path_typed_goes_with_enter_and_escape_gives_the_bar_back() {
    let mut d = drawn(json!({"path": "/takes"}));
    let typing = d.event(1000 + TYPE_PATH, "click", &[]);
    assert_eq!(set_on(&typing, 1000 + PLACE)["index"], json!(1));
    assert_eq!(set_on(&typing, 1000 + PATH)["select"], json!([0, 6]));
    d.event(1000 + PATH, "/scores/old", &[]);
    let went = d.event(1006, "click", &[]);
    assert_eq!(went.report, None, "Enter goes, it chooses nothing");
    assert_eq!(set_on(&went, 1001)["path"], json!("/scores/old"));
    assert_eq!(set_on(&went, 1000 + PLACE)["index"], json!(0));
    d.event(1000 + BAR, EDIT, &[]);
    let back = d.event(1000, "cancel", &[]);
    assert_eq!(back.report, None);
    assert!(d.up && !d.editing);
    assert_eq!(
        set_on(&back, 1001)["focus"],
        json!(1),
        "the keys back to the files"
    );
}

fn from() -> ClientId {
    ClientId::Udp("127.0.0.1:9".parse().unwrap())
}

fn def(host: &mut Host, json: &str) {
    host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: crate::host::GUI_DEF.into(),
            args: vec![OscType::Int(1), OscType::String(json.into())],
        }),
        from(),
    );
}

fn set(host: &mut Host, props: Vec<OscType>) {
    let mut args = vec![OscType::Int(20)];
    args.extend(props);
    host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: GUI_SET.into(),
            args,
        }),
        from(),
    );
}

const WINDOW: &str = r#"{"type":"window","title":"w","children":[
    {"id":20,"type":"file_dialog","mode":"save","kinds":[["WAV",["wav"]]],"path":"/takes/a.wav"}]}"#;

/// **The element in a window**: it stands in as the dialog the host draws,
/// opens on its `open`, answers its own widgets and reports what was chosen
/// as an event of the element, which goes to the window's client.
#[test]
fn a_file_dialog_in_a_window_reports_what_was_chosen() {
    let mut host = Host::new();
    def(&mut host, WINDOW);
    let base = host.file_dialogs[&20].base;
    assert!(
        host.registry.contains(base + FILES),
        "the chooser stands in"
    );
    assert_eq!(host.registry.get(20).unwrap().kind.as_str(), "layout");
    set(&mut host, vec![s("open"), OscType::Int(1)]);
    let info = host.registry.get(base + FILES).unwrap();
    assert_eq!(info.props.get("path"), Some(&json!("/takes")));
    // the chooser's folder listed, which the dialog hears
    for (def, widget, ask) in host.pending_listings() {
        assert!(host.deliver_listing(def, widget, ask.ticket, Ok(listing("/takes", &["a.wav"]))));
    }
    assert_eq!(
        host.registry.get(base + BAR).unwrap().props.get("path"),
        Some(&json!("/takes"))
    );
    let typed = host.event_message(base + NAME, 0, vec![s("b")]);
    assert!(host.deliver(1, &typed), "the dialog's own, heard by nobody");
    let click = host.event_message(base + ACCEPT, 0, vec![s("click")]);
    assert!(host.deliver(1, &click));
    let reported: Vec<OscMessage> = host
        .take_effects()
        .into_iter()
        .filter_map(|e| match e {
            HostEffect::Emit { def_id: 1, message } => Some(message),
            _ => None,
        })
        .collect();
    assert_eq!(reported.len(), 1);
    assert_eq!(reported[0].args[0], OscType::Int(20));
    assert_eq!(
        reported[0].args[3..],
        [s("chosen"), s("WAV"), s("/takes/b.wav")]
    );
}

/// **A window drawn again keeps the dialog as it stands**, and a window
/// without it forgets it and gives its ids back.
#[test]
fn a_window_drawn_again_keeps_the_dialog_and_one_without_it_forgets_it() {
    let mut host = Host::new();
    def(&mut host, WINDOW);
    set(&mut host, vec![s("open"), OscType::Int(1)]);
    let base = host.file_dialogs[&20].base;
    def(&mut host, WINDOW);
    assert_eq!(host.file_dialogs[&20].base, base);
    assert_eq!(
        host.registry.get(20).unwrap().props.get("index"),
        Some(&json!(1)),
        "still up"
    );
    def(&mut host, r#"{"type":"window","title":"w","children":[]}"#);
    assert!(host.file_dialogs.is_empty());
}
