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
                let def = editor.window(ids, tools);
                clausters_apps::score::editor::call_json(
                    editor,
                    &serde_json::json!({"verb": "sync", "window": def_id}).to_string(),
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
        Some(def_id)
    }

    /// **A gesture on a score editor's window**, answered by the editor: the
    /// turn is the editing context's, and the window is corrected with what it
    /// moved. Ctrl+S is the host's, since a file is.
    pub(super) fn answer_score(&mut self, def_id: i32, message: &OscMessage) -> bool {
        use clausters_apps::editing::Outcome;
        use clausters_apps::turn::{Event, Kind};

        let Some(owner) = self.owner.as_mut() else {
            return false;
        };
        let Some((member, save_to)) = owner.scores.get(&def_id).cloned() else {
            return false;
        };
        if matches!(message.args.get(3), Some(OscType::String(tag)) if tag == "save") {
            self.save_score(member, save_to.as_deref());
            return true;
        }
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
            return true;
        }
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
        true
    }

    /// Writes the score of `member` to `path` as MEI, or says why it did not.
    fn save_score(&mut self, member: clausters_apps::editing::MemberId, path: Option<&Path>) {
        use clausters_apps::editing::Member;

        let Some(path) = path else {
            diag::info!("score: read-only -- open it with --save-to <file> for Ctrl+S to write");
            return;
        };
        let Some(Member::Score(editor)) = self
            .owner
            .as_mut()
            .and_then(|o| o.editing.member_mut(member))
        else {
            return;
        };
        let mei = editor
            .score()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .mei();
        match std::fs::write(path, mei) {
            Ok(()) => diag::info!("score saved to {}", path.display()),
            Err(e) => diag::warn!("save: {}: {e}", path.display()),
        }
    }
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

    /// A bar of four quarters, ids 1 to 4.
    fn score() -> clausters_apps::score::Shared {
        let note = |id| Item::Note {
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
        };
        let sheet = Sheet {
            next_id: 5,
            staves: vec![Staff {
                clef: "G2".into(),
                voices: vec![Voice {
                    items: (1..=4).map(note).collect(),
                }],
            }],
            ..Sheet::default()
        };
        let engraver = AnyEngraver::new(Kept(Mutex::new(String::new())));
        let opened = Score::open(engraver, &sheet_to_mei(&sheet).unwrap()).unwrap();
        Arc::new(Mutex::new(opened))
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

        // a press on empty staff writes a quarter, on the holder's own score
        let insert = host.event_message(
            page,
            1,
            vec![
                OscType::String("insert".into()),
                OscType::String("n4".into()),
                OscType::Int(-2),
                OscType::Int(0),
            ],
        );
        assert!(host.deliver(def_id, &insert));
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

        // the tool holds state and reports its value: a press now writes a rest
        let on = host.event_message(rest, 1, vec![OscType::Int(1)]);
        assert!(host.deliver(def_id, &on));
        let insert = host.event_message(
            page,
            2,
            vec![
                OscType::String("insert".into()),
                OscType::String("n4".into()),
                OscType::Int(-2),
                OscType::Int(0),
            ],
        );
        assert!(host.deliver(def_id, &insert));
        let last_sounds = held.lock().unwrap().sheet().unwrap().staves[0].voices[0]
            .items
            .last()
            .is_some_and(Item::sounds);
        assert_eq!((items(&held), last_sounds), (5, false));

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
        assert_eq!(items(&held), 4);
    }
}
