//! **Quitting is the application's act, asked once, by the host**
//! (`clausters_apps::quitting`).
//!
//! The close of the dock's window is one request, never a close per pane,
//! and it goes one way whoever closes it -- the desktop's close, a page left,
//! a script's `{"verb": "close"}`, an application's File > Quit:
//!
//! - **The host always knows what would be lost.** The applications it holds
//!   say their unsaved documents whenever that changes, and a client that
//!   said it quits the application (`{"verb": "own_quit"}` on [`GUI_DOCK`])
//!   declares its own (`{"verb": "own_unsaved", "unsaved": [titles]}`). That
//!   client is told the applications' list as it changes
//!   ([`GUI_DOCK_UNSAVED`]), so a page asked to unload knows at once.
//! - **With nothing to lose, the application ends.** With something, the
//!   host asks one question over all of it (Don't save, Cancel, Save all) in
//!   a window of its own along the dock's edge, which stands over whatever
//!   page is shown.
//! - **Each holder does what its own end needs.** Save all saves the
//!   applications' documents through their own Save and tells the client
//!   `{"do": "save"}` ([`GUI_DOCK_QUIT`]), which saves its own and answers
//!   `{"verb": "saved", "saved": bool}`; the end is the client's
//!   `{"do": "quit"}` when there is one -- it remembers what it keeps and
//!   ends the program -- or every window closed when there is none.
//!
//! As a window's `ask_close` does, a second close while the first is
//! unanswered ends it anyway: an owner that stopped answering cannot keep
//! the application open.

use serde_json::{Value, json};

use super::*;

/// **What the close of the dock's window does**, as [`Host::dock_close`]
/// decides it: the front carries it out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DockClose {
    /// Each pane closes as its own close does: no application asked.
    Panes,
    /// Every window closes, nothing asked: the application quits.
    All,
    /// The application was asked; its answer decides.
    Asked,
}

/// **A quit under way.**
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Quitting {
    /// The question is up, unanswered.
    Asked,
    /// Save all is under way: whether the applications' documents and the
    /// client's own are still being saved.
    Saving { apps: bool, owner: bool },
    /// The client that quits the application was told to end, and has not
    /// yet.
    Ending,
}

/// The structure the host's own question is numbered under.
#[cfg(any(not(target_arch = "wasm32"), feature = "apps"))]
const QUESTION: i64 = -7;

impl Host {
    /// **The dock's window was asked to close** -- by the desktop, or a
    /// script's `{"verb": "close"}`: what to do, by what would be lost.
    pub fn dock_close(&mut self, effects: &mut Vec<HostEffect>) -> DockClose {
        if self.quit.is_some() {
            // asked once and not answered: it goes anyway
            self.quit = None;
            self.drop_question(effects);
            return DockClose::All;
        }
        if self.quitter.is_none() && !self.holds_applications() {
            return DockClose::Panes;
        }
        let titles = self.would_lose();
        if titles.is_empty() {
            return self.end_quit(effects);
        }
        self.quit = Some(Quitting::Asked);
        let titles: Vec<&str> = titles.iter().map(String::as_str).collect();
        self.ask_question(&titles, effects);
        DockClose::Asked
    }

    /// **Every document a quit would lose work in, by title**: the
    /// client's own, then the applications'.
    fn would_lose(&self) -> Vec<String> {
        let own = self.quitter.map(|_| self.declared.iter().cloned());
        own.into_iter()
            .flatten()
            .chain(self.at_risk.iter().map(|(_, title)| title.clone()))
            .collect()
    }

    /// **The quit ends**: the client that quits the application is told to,
    /// and ends it as its own end needs; with none, every window closes.
    fn end_quit(&mut self, effects: &mut Vec<HostEffect>) -> DockClose {
        match self.quitter {
            Some(to) => {
                self.quit = Some(Quitting::Ending);
                effects.push(HostEffect::Send {
                    to,
                    msg: OscMessage {
                        addr: GUI_DOCK_QUIT.into(),
                        args: vec![OscType::String(json!({"do": "quit"}).to_string())],
                    },
                });
                DockClose::Asked
            }
            None => {
                self.quit = None;
                DockClose::All
            }
        }
    }

    /// The quit ended where nothing closes the dock in answer: its close
    /// carried out as an effect.
    fn end_quit_now(&mut self, effects: &mut Vec<HostEffect>) {
        if self.end_quit(effects) == DockClose::All {
            effects.push(HostEffect::DockClose(DockClose::All));
        }
    }

    /// Whether this host holds an application's window.
    fn holds_applications(&self) -> bool {
        #[cfg(any(not(target_arch = "wasm32"), feature = "apps"))]
        {
            self.audio.as_ref().is_some_and(|audio| audio.has_windows())
        }
        #[cfg(all(target_arch = "wasm32", not(feature = "apps")))]
        false
    }

    /// **What the applications would lose, as they said it changed**: kept,
    /// and told to the client that quits the application.
    #[cfg(any(not(target_arch = "wasm32"), feature = "apps"))]
    pub(super) fn unsaved_said(&mut self, work: Vec<(i32, String)>, effects: &mut Vec<HostEffect>) {
        self.at_risk = work;
        self.tell_unsaved(effects);
    }

    /// The applications' unsaved documents told to the client that quits
    /// the application ([`GUI_DOCK_UNSAVED`]).
    fn tell_unsaved(&self, effects: &mut Vec<HostEffect>) {
        if let Some(to) = self.quitter {
            effects.push(HostEffect::Send {
                to,
                msg: OscMessage {
                    addr: GUI_DOCK_UNSAVED.into(),
                    args: vec![OscType::String(
                        json!({"unsaved": self.listed()}).to_string(),
                    )],
                },
            });
        }
    }

    /// The applications' unsaved documents, as the wire lists them.
    fn listed(&self) -> Vec<Value> {
        self.at_risk
            .iter()
            .map(|(window, title)| json!({"window": window, "title": title}))
            .collect()
    }

    /// **A dock call about quitting**, answered here rather than by the
    /// front's dock: `None` for any other verb.
    pub(super) fn quit_call(
        &mut self,
        call: &Value,
        from: ClientId,
        effects: &mut Vec<HostEffect>,
    ) -> Option<Result<Value, (String, String)>> {
        let refused = |why: &str| Err(("value".to_string(), why.to_string()));
        Some(match call["verb"].as_str()? {
            "own_quit" => {
                self.quitter = Some(from);
                self.declared.clear();
                self.tell_unsaved(effects);
                Ok(json!({"own_quit": true}))
            }
            "own_unsaved" if self.quitter != Some(from) => {
                refused("only the client that quits the application declares its documents")
            }
            "own_unsaved" => match call["unsaved"].as_array() {
                Some(titles) => {
                    self.declared = titles
                        .iter()
                        .filter_map(|t| t.as_str().map(str::to_string))
                        .collect();
                    Ok(json!({"own_unsaved": self.declared.len()}))
                }
                None => refused("own_unsaved needs \"unsaved\", a list of titles"),
            },
            "saved" if self.quitter != Some(from) => {
                refused("only the client that quits the application saves for it")
            }
            "saved" => {
                let saved = call["saved"].as_bool().unwrap_or(false);
                self.owner_saved(saved, effects);
                Ok(json!({"saved": saved}))
            }
            "close" => {
                let close = self.dock_close(effects);
                effects.push(HostEffect::DockClose(close));
                Ok(json!({"close": format!("{close:?}").to_lowercase()}))
            }
            "unsaved" => Ok(json!({"unsaved": self.listed()})),
            _ => return None,
        })
    }

    /// **Save all, from the question**: the applications' documents saved
    /// through their own Save and the client told to save its own; the
    /// quit ends once both say so.
    #[cfg(any(not(target_arch = "wasm32"), feature = "apps"))]
    fn save_all(&mut self, effects: &mut Vec<HostEffect>) {
        let apps = !self.at_risk.is_empty()
            && self
                .audio_host()
                .ask(super::audio_app::ToApp::SaveAll)
                .is_ok();
        let owner = match self.quitter {
            Some(to) if !self.declared.is_empty() => {
                effects.push(HostEffect::Send {
                    to,
                    msg: OscMessage {
                        addr: GUI_DOCK_QUIT.into(),
                        args: vec![OscType::String(json!({"do": "save"}).to_string())],
                    },
                });
                true
            }
            _ => false,
        };
        self.quit = Some(Quitting::Saving { apps, owner });
        self.saving_went_on(effects);
    }

    /// The quit ends when nothing is left being saved.
    fn saving_went_on(&mut self, effects: &mut Vec<HostEffect>) {
        if self.quit
            == Some(Quitting::Saving {
                apps: false,
                owner: false,
            })
        {
            self.end_quit_now(effects);
        }
    }

    /// **The applications' Save all ended**: `true` with nothing left to
    /// lose; `false` calls the quit off.
    #[cfg(any(not(target_arch = "wasm32"), feature = "apps"))]
    pub(super) fn saved_all(&mut self, saved: bool, effects: &mut Vec<HostEffect>) {
        if let Some(Quitting::Saving { owner, .. }) = self.quit {
            self.quit = saved.then_some(Quitting::Saving { apps: false, owner });
            self.saving_went_on(effects);
        }
    }

    /// **The client saved its own documents**, or could not: `false` calls
    /// the quit off.
    fn owner_saved(&mut self, saved: bool, effects: &mut Vec<HostEffect>) {
        if let Some(Quitting::Saving { apps, .. }) = self.quit {
            self.quit = saved.then_some(Quitting::Saving { apps, owner: false });
            self.saving_went_on(effects);
        }
    }

    /// **The host's question goes up**: one alert over the dock naming every
    /// document, in a window of the host's own along the dock's top edge.
    #[cfg(any(not(target_arch = "wasm32"), feature = "apps"))]
    fn ask_question(&mut self, titles: &[&str], effects: &mut Vec<HostEffect>) {
        use clausters_apps::{closing, quitting};

        let ids = [
            self.own_widget(QUESTION, "quit", "window"),
            self.own_widget(QUESTION, "quit", "dialog"),
            self.own_widget(QUESTION, "quit", closing::SAVE),
            self.own_widget(QUESTION, "quit", closing::DISCARD),
            self.own_widget(QUESTION, "quit", closing::CANCEL),
        ];
        let [
            Some(window),
            Some(dialog),
            Some(save),
            Some(discard),
            Some(cancel),
        ] = ids
        else {
            return;
        };
        let ids = closing::Ids {
            stack: window,
            dialog,
            save,
            discard,
            cancel,
        };
        let form = quitting::dialog(Some(dialog), &quitting::says(titles), titles.len(), |n| {
            ids.button(n)
        });
        self.question = Some(ids);
        let mut def = json!({
            "type": "window", "id": window, "title": quitting::TITLE,
            "dock_edge": "top", "h": 1,
            "children": [{"type": "layout", "w": 0, "h": 0, "margin": 0, "children": [form]}],
        });
        self.number_own(&mut def, QUESTION, "quit");
        let origin = own_origin();
        effects.extend(self.handle_packet(
            OscPacket::Message(OscMessage {
                addr: GUI_DEF.into(),
                args: vec![OscType::Int(window), OscType::String(def.to_string())],
            }),
            origin,
        ));
    }

    #[cfg(all(target_arch = "wasm32", not(feature = "apps")))]
    fn ask_question(&mut self, _titles: &[&str], _effects: &mut Vec<HostEffect>) {}

    /// **An event of the host's own question**, which no client hears:
    /// `true` when it was one.
    pub(super) fn question_event(&mut self, def_id: i32, message: &OscMessage) -> bool {
        #[cfg(any(not(target_arch = "wasm32"), feature = "apps"))]
        {
            let Some(ids) = self.question else {
                return false;
            };
            if def_id != ids.stack {
                return false;
            }
            let widget = match message.args.first() {
                Some(OscType::Int(w)) => i64::from(*w),
                _ => return true,
            };
            let tag = match message.args.get(3) {
                Some(OscType::String(t)) => t.as_str(),
                _ => "",
            };
            let mut effects = Vec::new();
            match ids.read(widget, tag) {
                Some(clausters_apps::closing::Choice::Discard) => {
                    self.drop_question(&mut effects);
                    self.end_quit_now(&mut effects);
                }
                Some(clausters_apps::closing::Choice::Cancel) => {
                    self.quit = None;
                    self.drop_question(&mut effects);
                }
                Some(clausters_apps::closing::Choice::Save) => {
                    self.drop_question(&mut effects);
                    self.save_all(&mut effects);
                }
                None => {}
            }
            self.pending_effects.extend(effects);
            true
        }
        #[cfg(all(target_arch = "wasm32", not(feature = "apps")))]
        {
            let _ = (def_id, message);
            false
        }
    }

    /// The host's question taken down, if it is up.
    fn drop_question(&mut self, effects: &mut Vec<HostEffect>) {
        #[cfg(any(not(target_arch = "wasm32"), feature = "apps"))]
        if let Some(ids) = self.question.take() {
            effects.extend(self.handle_packet(
                OscPacket::Message(OscMessage {
                    addr: GUI_FREE.into(),
                    args: vec![OscType::Int(ids.stack)],
                }),
                own_origin(),
            ));
        }
        #[cfg(all(target_arch = "wasm32", not(feature = "apps")))]
        let _ = effects;
    }
}

/// The origin of what the host sends itself.
#[cfg(any(not(target_arch = "wasm32"), feature = "apps"))]
fn own_origin() -> ClientId {
    use std::net::{Ipv4Addr, SocketAddr};
    ClientId::Udp(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
}
