//! **A score editor this host owns**: the applications crate's score editor
//! opened here, over a score the caller hands in, and every gesture on its
//! window answered here -- what a client's `edit(score)` does, done for a
//! score this host edits with nobody behind it.
//!
//! The host holds no engraver: the score arrives open, its engraver already a
//! port behind it ([`clausters_apps::score::Shared`]), which is what keeps
//! libverovio out of every host that does not open a score file itself.

use super::*;

/// The role a score editor's widgets play, in the names the host's own widget
/// ids are asked for by.
const SCORE: &str = "score";

impl Host {
    /// **Opens a score editor over `score`** in a window of its own, in the
    /// owner's editing context, and answers the window's id. Ctrl+S writes the
    /// score, as MEI, to `save_to` when one is named.
    ///
    /// `None` when this host owns nothing (no owner to hold the history) or
    /// has run out of its own widget ids.
    pub fn open_score(
        &mut self,
        score: clausters_apps::score::Shared,
        title: &str,
        size: (i64, i64),
        save_to: Option<std::path::PathBuf>,
    ) -> Option<i32> {
        use clausters_apps::editing::Member;
        use std::net::{Ipv4Addr, SocketAddr};

        let structure = self.owner.as_ref().map(|o| o.scores.len() as i64)?;
        let def_id = self.own_widget(structure, SCORE, "window")?;
        let ids = clausters_apps::score::Ids {
            page: self.own_widget(structure, SCORE, "page")?,
            scroll: self.own_widget(structure, SCORE, "scroll"),
            status: self.own_widget(structure, SCORE, "status"),
        };
        // the toolbar's tools, each under an id of this host's own
        let tools: clausters_apps::score::tools::Ids = clausters_apps::score::tools::TOOLS
            .iter()
            .filter_map(|name| {
                let id = self.own_widget(structure, SCORE, &format!("tool:{name}"))?;
                Some(((*name).to_string(), id))
            })
            .collect();
        // and the dialogs' widgets the same way
        let dialogs: clausters_apps::score::dialogs::Ids = clausters_apps::score::dialogs::names()
            .into_iter()
            .filter_map(|name| {
                let id = self.own_widget(structure, SCORE, &format!("dialog:{name}"))?;
                Some((name, id))
            })
            .collect();
        // and the palettes' entries
        let palettes: clausters_apps::score::palettes::Ids =
            clausters_apps::score::palettes::names()
                .into_iter()
                .filter_map(|name| {
                    let id = self.own_widget(structure, SCORE, &format!("palette:{name}"))?;
                    Some((name, id))
                })
                .collect();
        let chrome = clausters_apps::score::Chrome {
            tools,
            dialogs,
            palettes,
        };
        let owner = self.owner.as_mut()?;
        let request = serde_json::json!({"title": title, "w": size.0, "h": size.1}).to_string();
        let opened: serde_json::Value = serde_json::from_str(&owner.editing.open_score(
            &format!("score:{structure}"),
            score,
            &request,
        ))
        .unwrap_or_default();
        let member = opened["member"]
            .as_u64()
            .map(|m| m as clausters_apps::editing::MemberId)?;
        let def = match owner.editing.member_mut(member) {
            Some(Member::Score(editor)) => {
                let def = editor.window(ids, chrome);
                // the file it saves to is the one the caller named, if any
                let path = save_to.as_ref().map(|p| p.display().to_string());
                clausters_apps::score::editor::call_json(
                    editor,
                    &serde_json::json!({"verb": "sync", "window": def_id, "path": path})
                        .to_string(),
                );
                def
            }
            _ => return None,
        };
        owner.scores.insert(def_id, (member, save_to));
        let origin = ClientId::Udp(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)));
        let effects = self.handle_packet(
            OscPacket::Message(OscMessage {
                addr: GUI_DEF.into(),
                args: vec![OscType::Int(def_id), OscType::String(def.to_string())],
            }),
            origin,
        );
        self.pending_effects.extend(effects);
        // **The page's cursor is the position of the transport its score
        // plays on**, as a client's score editor binds it -- where this host
        // has a player to play it.
        if let Some(transport) = self.score_transport(def_id) {
            let effects = self.handle_packet(
                OscPacket::Message(OscMessage {
                    addr: GUI_CLOCK.into(),
                    args: vec![
                        OscType::Int(def_id),
                        OscType::String("transport".into()),
                        OscType::Int(transport),
                    ],
                }),
                origin,
            );
            self.pending_effects.extend(effects);
        }
        Some(def_id)
    }

    /// The score of `member` as the sequence it plays as, at the engraver's
    /// tempo -- what a pass plays and an export writes -- or `None`, said.
    fn score_render(
        &mut self,
        member: clausters_apps::editing::MemberId,
    ) -> Option<clausters_document::events::EventSequence> {
        use clausters_apps::editing::Member;

        let Some(Member::Score(editor)) = self
            .owner
            .as_mut()
            .and_then(|o| o.editing.member_mut(member))
        else {
            return None;
        };
        let rendered = editor
            .rendered()
            .and_then(|value| serde_json::from_value(value).map_err(|e| e.to_string()));
        match rendered {
            Ok(sequence) => Some(sequence),
            Err(why) => {
                diag::warn!("the score cannot be rendered: {why}");
                None
            }
        }
    }

    /// **A gesture on a score editor's window**, answered by the editor: the
    /// turn is the editing context's, and the window is corrected with what it
    /// moved. A file is the host's: where the editor's turn names one to
    /// save to or to open, this writes and reads it.
    pub(super) fn answer_score(&mut self, def_id: i32, message: &OscMessage) -> bool {
        use clausters_apps::editing::Outcome;
        use clausters_apps::turn::{Event, Kind};

        let Some(owner) = self.owner.as_mut() else {
            return false;
        };
        let Some((member, _)) = owner.scores.get(&def_id).cloned() else {
            return false;
        };
        let Some(owner) = self.owner.as_mut() else {
            return false;
        };
        let Some(turned) = owner.editing.event(
            member,
            &Event {
                addr: message.addr.clone(),
                args: message
                    .args
                    .iter()
                    .map(document::multitrack::atom)
                    .collect(),
            },
        ) else {
            return false;
        };
        let Outcome::Score(outcome) = turned.outcome else {
            return false;
        };
        if outcome.turn == Kind::Closed {
            owner.scores.remove(&def_id);
            self.close_score_playback(def_id);
            return true;
        }
        // an edit, or a step of the history, is heard on from where it plays
        let stepped_now = turned.stepped.as_ref().is_some_and(|s| s.stepped);
        if let Some(stepped) = turned.stepped
            && stepped.stepped
        {
            for corrected in stepped.corrections {
                self.tell(corrected.answer);
            }
        }
        for corrected in turned.corrections {
            self.tell(corrected.answer);
        }
        if let Some(answer) = outcome.answer {
            self.tell(answer);
        }
        // A file is this host's to write and to read: the editor said which.
        // A close waits for the file it saves to, and stays when it was not
        // written.
        let written = outcome
            .save
            .is_none_or(|path| self.save_score(member, Path::new(&path)));
        if outcome.close && written {
            self.close_score_playback(def_id);
            self.close_score(def_id);
            return true;
        }
        if let Some(path) = outcome.open {
            self.open_into_score(def_id, member, Path::new(&path));
        }
        // An export is the score rendered and written as the sequence writes
        // a MIDI file, with the writer the clients use.
        if let Some(export) = outcome.export {
            self.export_score(member, &export);
        }
        // What a turn asked of the playback: the cursor placed, a play or a
        // stop, the loop switch -- and the lane taking an edit.
        if (outcome.changed || stepped_now)
            && let Some(sequence) = self.score_render(member)
        {
            self.update_score(def_id, sequence);
        }
        if let Some(beat) = outcome.locate {
            self.cue_score(def_id, beat);
        }
        if let Some(pass) = outcome.play
            && let Some(sequence) = self.score_render(member)
        {
            self.roll_score(def_id, sequence, &pass);
        }
        if let Some(pass) = outcome.relooped {
            self.reloop_score(def_id, &pass);
        }
        true
    }

    /// Writes the score of `member` rendered to the file `export` names --
    /// `{"path", "format"}`, a Standard MIDI File (`smf`) or a MIDI 2.0 Clip
    /// File (`clip`) -- as a client's `export` writes it, or says why not.
    fn export_score(
        &mut self,
        member: clausters_apps::editing::MemberId,
        export: &serde_json::Value,
    ) {
        let path = export["path"].as_str().unwrap_or_default().to_string();
        let Some(sequence) = self.score_render(member) else {
            return;
        };
        let Some(data) = midi_file(&sequence, export["format"] == "clip") else {
            return diag::warn!("export: {path}: this host was built without a MIDI file writer");
        };
        match std::fs::write(&path, data) {
            Ok(()) => diag::info!("score exported to {path}"),
            Err(e) => diag::warn!("export: {path}: {e}"),
        }
    }

    /// Reads the document at `path` into the score of `member`, as the
    /// editor's `open` verb -- one entry of the history -- or says why not.
    fn open_into_score(
        &mut self,
        def_id: i32,
        member: clausters_apps::editing::MemberId,
        path: &Path,
    ) {
        use clausters_apps::editing::Outcome;

        let data = match std::fs::read_to_string(path) {
            Ok(data) => data,
            Err(e) => {
                diag::warn!("open: {}: {e}", path.display());
                return;
            }
        };
        let Some(owner) = self.owner.as_mut() else {
            return;
        };
        let request = serde_json::json!({"action": "open", "data": data});
        let Some(turned) = owner.editing.act(member, &request) else {
            return;
        };
        for corrected in turned.corrections {
            self.tell(corrected.answer);
        }
        if let Outcome::Score(outcome) = turned.outcome
            && let Some(answer) = outcome.answer
        {
            self.tell(answer);
        }
        // what plays is the score now open
        if let Some(sequence) = self.score_render(member) {
            self.update_score(def_id, sequence);
        }
        diag::info!("score {def_id}: opened {}", path.display());
    }

    /// Writes the score of `member` to `path` as MEI, and tells the editor
    /// its file now holds it -- or says why it did not. Whether it wrote.
    fn save_score(&mut self, member: clausters_apps::editing::MemberId, path: &Path) -> bool {
        use clausters_apps::editing::Member;

        let Some(Member::Score(editor)) = self
            .owner
            .as_mut()
            .and_then(|o| o.editing.member_mut(member))
        else {
            return false;
        };
        let mei = editor
            .score()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .mei();
        match std::fs::write(path, mei) {
            Ok(()) => {
                clausters_apps::score::editor::call_json(editor, r#"{"verb": "saved"}"#);
                diag::info!("score saved to {}", path.display());
                true
            }
            Err(e) => {
                diag::warn!("save: {}: {e}", path.display());
                false
            }
        }
    }

    /// Closes the score editor's window `def_id`, as the File menu's Close
    /// asked once nothing was left to lose: the window is freed, and the
    /// editor is this host's no longer.
    fn close_score(&mut self, def_id: i32) {
        use std::net::{Ipv4Addr, SocketAddr};

        if let Some(owner) = self.owner.as_mut() {
            owner.scores.remove(&def_id);
        }
        let origin = ClientId::Udp(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)));
        let effects = self.handle_packet(
            OscPacket::Message(OscMessage {
                addr: GUI_FREE.into(),
                args: vec![OscType::Int(def_id)],
            }),
            origin,
        );
        self.pending_effects.extend(effects);
    }
}

/// **`sequence` as a MIDI file**, a MIDI 2.0 Clip File when `clip`, else a
/// Standard MIDI File: what a client's `to_clip` and `to_smf` write, at the
/// ticks per beat they write it at, with the writer they bind.
#[cfg(all(feature = "midi", not(target_arch = "wasm32")))]
fn midi_file(sequence: &clausters_document::events::EventSequence, clip: bool) -> Option<Vec<u8>> {
    const PPQ: u16 = 480;
    if clip {
        return Some(clausters_midi::write_clip_ump(&sequence.to_ump(PPQ), PPQ));
    }
    let (events, tempo) = sequence.to_midi(PPQ);
    let events: Vec<clausters_midi::TimedMessage> = events
        .into_iter()
        .map(|(tick, message)| {
            let mut bytes = [0u8; 3];
            let n = message.len().min(3);
            bytes[..n].copy_from_slice(&message[..n]);
            clausters_midi::TimedMessage { tick, bytes }
        })
        .collect();
    let tempo: Vec<clausters_midi::TempoMark> = tempo
        .into_iter()
        .map(|(tick, micros)| clausters_midi::TempoMark { tick, micros })
        .collect();
    Some(clausters_midi::write_smf_with_tempo(&events, PPQ, &tempo))
}

/// A host without the MIDI crate writes no MIDI file.
#[cfg(not(all(feature = "midi", not(target_arch = "wasm32"))))]
fn midi_file(_: &clausters_document::events::EventSequence, _: bool) -> Option<Vec<u8>> {
    None
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use clausters_core::notation::{
        AnyEngraver, Engraver, Item, Marks, Pitch, Score, Sheet, Staff, Step, Voice, sheet_to_mei,
    };
    use clausters_core::ratio::Ratio;

    use super::*;
    use crate::host::document::Owner;

    /// An engraver that keeps the document it was handed and draws nothing.
    struct Kept(Mutex<String>);

    impl Engraver for Kept {
        type Guard = ();
        fn lock(&self) -> Self::Guard {}
        fn load_data(&self, data: &str) -> bool {
            *self.0.lock().unwrap() = data.to_string();
            !data.is_empty()
        }
        fn render_svg(&self, _page: i32) -> String {
            String::new()
        }
        fn mei(&self) -> String {
            self.0.lock().unwrap().clone()
        }
        fn edit(&self, _action: &str) -> bool {
            false
        }
        fn timemap(&self, _options: &str) -> String {
            "[]".into()
        }
        fn midi_values(&self, _xml_id: &str) -> Option<String> {
            None
        }
    }

    /// `count` quarters, ids from 1.
    fn items_of(count: u64) -> Vec<Item> {
        (1..=count)
            .map(|id| Item::Note {
                id,
                pitches: vec![Pitch {
                    step: Step::G,
                    alter: 0,
                    octave: 4,
                    forced: false,
                }],
                dur: Ratio::new(1, 4),
                tie: false,
                marks: Marks::default(),
            })
            .collect()
    }

    /// A bar of four quarters, ids 1 to 4.
    fn score() -> clausters_apps::score::Shared {
        let sheet = Sheet {
            next_id: 5,
            staves: vec![Staff {
                clef: "G2".into(),
                voices: vec![Voice { items: items_of(4) }],
                ..Staff::default()
            }],
            ..Sheet::default()
        };
        let engraver = AnyEngraver::new(Kept(Mutex::new(String::new())));
        let opened = Score::open(engraver, &sheet_to_mei(&sheet).unwrap()).unwrap();
        Arc::new(Mutex::new(opened))
    }

    /// The type names in `widget`'s tree that this host builds as nothing.
    fn unknown(widget: &Widget) -> Vec<String> {
        let own = match &widget.kind {
            WidgetKind::Unknown(name) => Some(name.clone()),
            _ => None,
        };
        own.into_iter()
            .chain(widget.children.iter().flat_map(unknown))
            .collect()
    }

    fn items(held: &clausters_apps::score::Shared) -> usize {
        held.lock().unwrap().sheet().unwrap().staves[0].voices[0]
            .items
            .len()
    }

    #[test]
    fn a_score_opens_in_its_editor_and_answers_its_gestures_here() {
        let mut host = Host::new();
        host.owner = Some(Owner::new(clausters_document::Document::empty()));
        let held = score();
        let def_id = host
            .open_score(held.clone(), "score", (960, 640), None)
            .expect("a window");
        assert!(
            host.window_defs.contains_key(&def_id),
            "the window is defined"
        );
        let page = host.own_widget(0, SCORE, "page").expect("the page");
        // **Every node the crate wrote is a widget this host builds.** A type
        // name the host does not know builds as nothing and takes its
        // children with it -- which is how the page once stood in a window
        // that never drew it, while every message to its id was still
        // answered.
        let window = &host.window_defs[&def_id];
        assert_eq!(unknown(window), Vec::<String>::new());
        assert!(window.find(page).is_some(), "the page is in the window");

        // the window opens on the score's own keys, and N puts it in note
        // entry: its keys are in force there
        assert_eq!(host.window_keys(def_id), vec!["score".to_string()]);
        let key = |host: &mut Host, seq: i32, verb: &str| {
            let message = host.event_message(def_id, seq, vec![OscType::String(verb.into())]);
            assert!(host.deliver(def_id, &message), "{verb}");
        };
        key(&mut host, 1, "entry");
        assert_eq!(
            host.window_keys(def_id),
            vec!["score".to_string(), "note_entry".to_string()]
        );
        // past the bar, a C writes a quarter, on the holder's own score
        for _ in 0..4 {
            key(&mut host, 1, "cursor_right");
        }
        key(&mut host, 1, "pitch_c");
        assert_eq!(items(&held), 5);

        // and the window's undo takes it back, through the one history
        let undo = host.event_message(def_id, 2, vec![OscType::String("undo".into())]);
        assert!(host.deliver(def_id, &undo));
        assert_eq!(items(&held), 4);
    }

    #[test]
    fn the_toolbar_and_the_menu_bar_are_answered_by_the_editor() {
        let mut host = Host::new();
        host.owner = Some(Owner::new(clausters_document::Document::empty()));
        let held = score();
        let def_id = host
            .open_score(held.clone(), "score", (960, 640), None)
            .expect("a window");
        let page = host.own_widget(0, SCORE, "page").expect("the page");
        // every tool is a widget of this host's own, under its name
        let rest = host.own_widget(0, SCORE, "tool:rest").expect("the tool");
        assert!(
            host.window_defs[&def_id].find(rest).is_some(),
            "it is in the window"
        );

        // the tool holds state and reports its value: a press in note entry,
        // in the second voice, now writes a rest
        let on = host.event_message(rest, 1, vec![OscType::Int(1)]);
        assert!(host.deliver(def_id, &on));
        for verb in ["entry", "voice_2"] {
            let key = host.event_message(def_id, 1, vec![OscType::String(verb.into())]);
            assert!(host.deliver(def_id, &key));
        }
        let enter = host.event_message(
            page,
            2,
            vec![
                OscType::String("enter".into()),
                OscType::String("n4".into()),
                OscType::Int(-2),
                OscType::Int(0),
            ],
        );
        assert!(host.deliver(def_id, &enter));
        let second = |held: &clausters_apps::score::Shared| -> Vec<bool> {
            held.lock().unwrap().sheet().unwrap().staves[0]
                .voices
                .get(1)
                .map(|v| v.items.iter().map(Item::sounds).collect())
                .unwrap_or_default()
        };
        assert_eq!(second(&held), vec![false, false], "a pad, then the rest");
        assert_eq!(items(&held), 4, "the first voice did not move");

        // what was written is selected, and a pick of the bar is a verb over it
        let delete = host.event_message(
            def_id,
            3,
            vec![
                OscType::String("menu".into()),
                OscType::String(r#"{"action":"delete"}"#.into()),
            ],
        );
        assert!(host.deliver(def_id, &delete));
        assert_eq!(second(&held), vec![false]);
    }

    /// The index of the stack the dialogs are pages of, as the window stands.
    fn dialog_page(host: &Host, def_id: i32) -> i32 {
        let stack = host
            .own_widget_id(0, SCORE, "dialog:stack")
            .expect("the stack");
        match host.window_defs[&def_id].find(stack).map(|w| &w.kind) {
            Some(crate::host::widget::WidgetKind::Stack { index, .. }) => *index,
            other => panic!("a stack: {other:?}"),
        }
    }

    #[test]
    fn a_form_is_opened_by_the_bar_and_written_on_ok_in_this_window() {
        let mut host = Host::new();
        host.owner = Some(Owner::new(clausters_document::Document::empty()));
        let held = score();
        let def_id = host
            .open_score(held.clone(), "score", (960, 640), None)
            .expect("a window");
        assert_eq!(dialog_page(&host, def_id), 0, "no dialog is up");

        // the bar's entry turns the stack to the form's page: the dialog is up
        let open = host.event_message(
            def_id,
            1,
            vec![
                OscType::String("menu".into()),
                OscType::String("dialog:text".into()),
            ],
        );
        assert!(host.deliver(def_id, &open));
        assert_eq!(dialog_page(&host, def_id), 1);

        // a field typed and OK pressed: the score has it, and the dialog is down
        let title = host.own_widget(0, SCORE, "dialog:text:title").unwrap();
        let ok = host.own_widget(0, SCORE, "dialog:text:ok").unwrap();
        let typed = host.event_message(title, 2, vec![OscType::String("A title".into())]);
        assert!(host.deliver(def_id, &typed));
        let click = host.event_message(ok, 3, vec![OscType::String("click".into())]);
        assert!(host.deliver(def_id, &click));
        assert_eq!(
            held.lock().unwrap().sheet().unwrap().header.title,
            "A title"
        );
        assert_eq!(dialog_page(&host, def_id), 0);
    }

    #[test]
    fn the_scores_file_is_written_and_read_by_this_host() {
        let dir = std::env::temp_dir().join(format!("clausters-score-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let saved = dir.join("saved.mei");
        let mut host = Host::new();
        host.owner = Some(Owner::new(clausters_document::Document::empty()));
        let held = score();
        let def_id = host
            .open_score(held.clone(), "score", (960, 640), Some(saved.clone()))
            .expect("a window");

        // Ctrl+S is the window's save: the editor names the file it was
        // opened with, and the host writes the score there
        let save = host.event_message(def_id, 1, vec![OscType::String("save".into())]);
        assert!(host.deliver(def_id, &save));
        let written = std::fs::read_to_string(&saved).expect("the file");
        assert_eq!(written, held.lock().unwrap().mei());

        // a file named in the open form is read into the score, as one entry
        let other = dir.join("other.mei");
        let two = Sheet {
            next_id: 3,
            staves: vec![Staff {
                clef: "F4".into(),
                voices: vec![Voice { items: items_of(2) }],
                ..Staff::default()
            }],
            ..Sheet::default()
        };
        std::fs::write(&other, sheet_to_mei(&two).unwrap()).unwrap();
        let pick = host.event_message(
            def_id,
            2,
            vec![
                OscType::String("menu".into()),
                OscType::String("dialog:open".into()),
            ],
        );
        assert!(host.deliver(def_id, &pick));
        let path = host.own_widget(0, SCORE, "dialog:file:path").unwrap();
        let ok = host.own_widget(0, SCORE, "dialog:file:ok").unwrap();
        let typed = host.event_message(path, 3, vec![OscType::String(other.display().to_string())]);
        assert!(host.deliver(def_id, &typed));
        let click = host.event_message(ok, 4, vec![OscType::String("click".into())]);
        assert!(host.deliver(def_id, &click));
        assert_eq!(items(&held), 2);
        assert_eq!(held.lock().unwrap().sheet().unwrap().staves[0].clef, "F4");
        // and the window's undo brings the first score back
        let undo = host.event_message(def_id, 5, vec![OscType::String("undo".into())]);
        assert!(host.deliver(def_id, &undo));
        assert_eq!(items(&held), 4);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn close_saves_first_when_asked_and_frees_the_window() {
        let dir = std::env::temp_dir().join(format!("clausters-close-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let saved = dir.join("closed.mei");
        let mut host = Host::new();
        host.owner = Some(Owner::new(clausters_document::Document::empty()));
        let held = score();
        let def_id = host
            .open_score(held.clone(), "score", (960, 640), Some(saved.clone()))
            .expect("a window");
        let menu = |host: &mut Host, seq: i32, verb: &str| {
            let pick = host.event_message(
                def_id,
                seq,
                vec![OscType::String("menu".into()), OscType::String(verb.into())],
            );
            assert!(host.deliver(def_id, &pick), "{verb}");
        };
        // New is an edit, so the score has changes its file does not hold:
        // Close asks, and the window stays while it does
        menu(&mut host, 1, "new");
        assert_eq!(items(&held), 4, "four empty bars");
        menu(&mut host, 2, r#"{"action":"page","landscape":true}"#);
        menu(&mut host, 3, "close");
        assert_eq!(dialog_page(&host, def_id), 5);
        assert!(host.window_defs.contains_key(&def_id));
        // Save writes the file a save names -- a new score has none, so the
        // form asks -- and the window goes once it is written
        let ok = host.own_widget(0, SCORE, "dialog:close:ok").unwrap();
        let click = host.event_message(ok, 4, vec![OscType::String("click".into())]);
        assert!(host.deliver(def_id, &click));
        assert_eq!(dialog_page(&host, def_id), 4, "the file form");
        let path = host.own_widget(0, SCORE, "dialog:file:path").unwrap();
        let typed = host.event_message(path, 5, vec![OscType::String(saved.display().to_string())]);
        assert!(host.deliver(def_id, &typed));
        let ok = host.own_widget(0, SCORE, "dialog:file:ok").unwrap();
        let click = host.event_message(ok, 6, vec![OscType::String("click".into())]);
        assert!(host.deliver(def_id, &click));
        assert_eq!(
            std::fs::read_to_string(&saved).expect("the file"),
            held.lock().unwrap().mei()
        );
        assert!(
            !host.window_defs.contains_key(&def_id),
            "the window is freed"
        );
        assert!(host.owner.as_ref().unwrap().scores.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **What a player is sent, answered as one would**: every message the
    /// socket `player` receives, its address kept, a `/server_sync` answered
    /// with its reply and anything else with its `/done`, until the host has
    /// nothing more to send.
    fn played(host: &mut Host, player: &std::net::UdpSocket) -> Vec<String> {
        fn flat(packet: OscPacket, out: &mut Vec<OscMessage>) {
            match packet {
                OscPacket::Message(m) => out.push(m),
                OscPacket::Bundle(b) => b.content.into_iter().for_each(|p| flat(p, out)),
            }
        }
        player.set_nonblocking(true).unwrap();
        let mut buf = vec![0u8; 65536];
        let mut addrs = Vec::new();
        for _ in 0..32 {
            let mut got = Vec::new();
            while let Ok(len) = player.recv(&mut buf) {
                if let Ok(packet) = clausters_core::osc::decode_packet(&buf[..len]) {
                    flat(packet, &mut got);
                }
            }
            if got.is_empty() {
                break;
            }
            for message in got {
                let reply = if message.addr == "/server_sync" {
                    OscMessage {
                        addr: "/server_sync.reply".into(),
                        args: message.args.clone(),
                    }
                } else {
                    let mut args = vec![OscType::String(message.addr.clone())];
                    args.extend(message.args.first().cloned());
                    OscMessage {
                        addr: "/done".into(),
                        args,
                    }
                };
                addrs.push(message.addr);
                host.multitrack_reply(crate::host::instance::Leg::Player, &reply);
            }
        }
        addrs
    }

    #[test]
    fn the_score_plays_through_the_player() {
        let player = std::net::UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        let mut host = Host::new();
        host.set_player_link(crate::host::ServerLink::Udp(
            crate::host::ServerLeg::connect(player.local_addr().unwrap()).unwrap(),
        ));
        host.owner = Some(Owner::new(clausters_document::Document::empty()));
        let held = score();
        let def_id = host
            .open_score(held.clone(), "score", (960, 720), None)
            .expect("a window");
        // the space bar is the window's play: the score's render goes onto an
        // event lane of a transport of its own, and the transport rolls
        let play = host.event_message(def_id, 1, vec![OscType::String("play".into())]);
        assert!(host.deliver(def_id, &play));
        let addrs = played(&mut host, &player);
        assert!(addrs.iter().any(|a| a == "/transport_play"), "{addrs:?}");
        assert!(addrs.iter().any(|a| a.starts_with("/lane")), "{addrs:?}");
    }

    /// The loop is one switch: the toolbar's turns the one `L` turns, so a
    /// press of `L` after it never asks for the state the switch has.
    #[test]
    fn the_toolbars_loop_and_the_key_are_one_switch() {
        let mut host = Host::new();
        host.owner = Some(Owner::new(clausters_document::Document::empty()));
        let def_id = host
            .open_score(score(), "score", (960, 720), None)
            .expect("a window");
        assert!(!host.monitor_loops());
        let switch = host.own_widget(0, SCORE, "tool:loop").expect("the tool");
        let on = host.event_message(switch, 1, vec![OscType::Int(1)]);
        assert!(host.deliver(def_id, &on));
        assert!(host.monitor_loops(), "the toolbar turned the host's switch");
        // `L`: the host turns its switch and tells the window where it stands
        host.toggle_monitor_loop();
        let key = host.event_message(def_id, 2, host.loop_verb());
        assert!(host.deliver(def_id, &key));
        assert!(!host.monitor_loops(), "and nothing turned it back");
        // the menu's is the same one
        let pick = host.event_message(
            def_id,
            3,
            vec![
                OscType::String("menu".into()),
                OscType::String("loop".into()),
                OscType::Int(1),
            ],
        );
        assert!(host.deliver(def_id, &pick));
        assert!(host.monitor_loops());
    }

    /// An export writes the render as a Standard MIDI File, with the writer
    /// the `midi` feature links.
    #[cfg(feature = "midi")]
    #[test]
    fn the_score_exports_as_a_midi_file() {
        let mut host = Host::new();
        host.owner = Some(Owner::new(clausters_document::Document::empty()));
        let def_id = host
            .open_score(score(), "score", (960, 720), None)
            .expect("a window");
        let dir = std::env::temp_dir().join(format!("clausters-export-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("score.mid");
        let pick = host.event_message(
            def_id,
            2,
            vec![
                OscType::String("menu".into()),
                OscType::String("dialog:export_midi".into()),
            ],
        );
        assert!(host.deliver(def_id, &pick));
        let path = host.own_widget(0, SCORE, "dialog:file:path").unwrap();
        let typed = host.event_message(path, 3, vec![OscType::String(out.display().to_string())]);
        assert!(host.deliver(def_id, &typed));
        let ok = host.own_widget(0, SCORE, "dialog:file:ok").unwrap();
        let click = host.event_message(ok, 4, vec![OscType::String("click".into())]);
        assert!(host.deliver(def_id, &click));
        let written = std::fs::read(&out).expect("the file");
        assert_eq!(&written[..4], b"MThd");
        let read = clausters_midi::read_smf(&written).unwrap();
        let ons = read
            .events
            .iter()
            .filter(|(_, bytes)| bytes[0] & 0xF0 == 0x90 && bytes[2] > 0)
            .count();
        assert_eq!(ons, 4, "the four quarters");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
