//! **The host's side of the file dialog**: a `file_dialog` in a tree
//! replaced by what its presenter stands it in with, its `open` and its
//! request set over the wire, what its own widgets say answered here, and
//! its report delivered as an event of the element.

use clausters_core::osc::{OscMessage, OscType};
use serde_json::{Map, Value};

use super::{Answer, Drawn, IDS, KIND, Presenter, Request, truthy};
use crate::host::files::Listing;
use crate::host::guidef::GuiNode;
use crate::host::{Host, HostEffect, diag};

/// **One file dialog the host presents**: what presents it, the props it
/// was asked with, and the block of ids its widgets take.
#[derive(Debug)]
pub(crate) struct Presented {
    presenter: Box<dyn Presenter + Send>,
    props: Map<String, Value>,
    pub(super) base: i32,
    /// **Where the keys go once the tree is in**: a dialog a def opened
    /// gives them to its first stop after the window settles its own.
    keys: Option<i32>,
}

/// The props that are the request's, set over the wire as they are defined.
const REQUEST: [&str; 7] = [
    "mode",
    "kinds",
    "all_files",
    "multiple",
    "path",
    "title",
    "accept",
];

impl Host {
    /// **Every `file_dialog` in `node` stood in for** by its presenter, as
    /// the tree is defined: the same element keeps its presenter, and with
    /// it where the hand was, across a window drawn again. An `open` the
    /// def states is taken; a def that states none leaves the dialog as it
    /// stands.
    pub(in crate::host) fn present_file_dialogs(&mut self, node: &mut GuiNode) {
        if node.kind == KIND {
            let Some(id) = node.id else {
                return diag::warn!("{KIND}: a file dialog needs an id");
            };
            let request = Request::read(&node.props);
            if !self.file_dialogs.contains_key(&id) {
                let Some(base) = self
                    .widget_ids
                    .alloc_block(IDS)
                    .and_then(|b| i32::try_from(b).ok())
                else {
                    return diag::warn!("{KIND} {id}: the host's widget ids are spent");
                };
                let presenter = Box::new(Drawn::new(id, base, request.clone()));
                self.file_dialogs.insert(
                    id,
                    Presented {
                        presenter,
                        props: Map::new(),
                        base,
                        keys: None,
                    },
                );
            }
            let Some(shown) = self.file_dialogs.get_mut(&id) else {
                return;
            };
            shown.props = node.props.clone();
            shown.presenter.update(request);
            match node.props.get("open").map(truthy) {
                Some(true) if !shown.presenter.is_up() => {
                    shown.keys = shown
                        .presenter
                        .open()
                        .set
                        .into_iter()
                        .find(|(_, props)| props.iter().any(|(k, _)| k == "focus"))
                        .map(|(id, _)| id);
                }
                Some(true) => {}
                Some(false) => {
                    let _ = shown.presenter.close();
                }
                None => {}
            }
            match serde_json::from_value::<GuiNode>(shown.presenter.stand_in()) {
                Ok(stand_in) => *node = stand_in,
                Err(e) => diag::warn!("{KIND} {id}: {e}"),
            }
            return;
        }
        for child in &mut node.children {
            self.present_file_dialogs(child);
        }
    }

    /// **The keys given to the dialogs a def just opened**, once the window
    /// is in and has settled where its own go.
    pub(in crate::host) fn focus_opened_file_dialogs(&mut self, effects: &mut Vec<HostEffect>) {
        let keys: Vec<i32> = self
            .file_dialogs
            .values_mut()
            .filter_map(|shown| shown.keys.take())
            .collect();
        for id in keys {
            self.set_props(id, vec![("focus".into(), Value::from(1))], effects);
        }
    }

    /// **A set of a file dialog**: its `open`, and what it asks; answers the
    /// props that are neither, which are its stand-in's.
    pub(in crate::host) fn set_file_dialog(
        &mut self,
        id: i32,
        props: Vec<(String, Value)>,
        effects: &mut Vec<HostEffect>,
    ) -> Vec<(String, Value)> {
        let mut rest = Vec::new();
        let mut opened = None;
        let mut asked = false;
        let Some(shown) = self.file_dialogs.get_mut(&id) else {
            return props;
        };
        for (key, value) in props {
            if key == "open" {
                opened = Some(truthy(&value));
            } else if REQUEST.contains(&key.as_str()) {
                shown.props.insert(key, value);
                asked = true;
            } else {
                rest.push((key, value));
            }
        }
        if asked {
            shown.presenter.update(Request::read(&shown.props));
        }
        let answer = match opened {
            Some(true) => shown.presenter.open(),
            Some(false) => shown.presenter.close(),
            None => Answer::default(),
        };
        self.answer_file_dialog(id, answer, effects);
        rest
    }

    /// **An event of a file dialog's own widgets**, which no client hears:
    /// `true` when it was one.
    pub(in crate::host) fn file_dialog_event(&mut self, message: &OscMessage) -> bool {
        let Some(OscType::Int(widget)) = message.args.first() else {
            return false;
        };
        let Some((&id, _)) = self
            .file_dialogs
            .iter()
            .find(|(_, shown)| shown.presenter.owns(*widget))
        else {
            return false;
        };
        let (tag, args) = match message.args.get(3) {
            Some(OscType::String(tag)) => (tag.as_str(), message.args.get(4..).unwrap_or(&[])),
            _ => ("", message.args.get(3..).unwrap_or(&[])),
        };
        let answer = match self.file_dialogs.get_mut(&id) {
            Some(shown) => shown.presenter.event(*widget, tag, args),
            None => return false,
        };
        let mut effects = Vec::new();
        self.answer_file_dialog(id, answer, &mut effects);
        self.pending_effects.extend(effects);
        true
    }

    /// Whether widget `widget` is one of a file dialog's, which hears what
    /// its folder holds.
    pub(in crate::host) fn file_dialog_hears(&self, widget: i32) -> bool {
        self.file_dialogs
            .values()
            .any(|shown| shown.presenter.owns(widget))
    }

    /// **A file dialog's chooser listed its folder**: the dialog told, and
    /// what it answers carried out with the host's next effects.
    pub(in crate::host) fn file_dialog_listed(
        &mut self,
        widget: i32,
        listing: Result<&Listing, &str>,
    ) {
        let Some((&id, shown)) = self
            .file_dialogs
            .iter_mut()
            .find(|(_, shown)| shown.presenter.owns(widget))
        else {
            return;
        };
        let answer = shown.presenter.listed(widget, listing);
        let mut effects = Vec::new();
        self.answer_file_dialog(id, answer, &mut effects);
        self.pending_effects.extend(effects);
    }

    /// **What a presenter answered, carried out**: its widgets set, and its
    /// report delivered as an event of element `id` -- to what this host
    /// owns, or to the client whose window it is.
    fn answer_file_dialog(&mut self, id: i32, answer: Answer, effects: &mut Vec<HostEffect>) {
        for (widget, props) in answer.set {
            self.set_props(widget, props, effects);
        }
        let Some(report) = answer.report else {
            return;
        };
        let Some(def_id) = self.registry.root_of(id) else {
            return;
        };
        let seq = self.outbox.borrow_mut().stamp(def_id, id);
        let message = self.event_message(id, seq, report);
        if !self.deliver(def_id, &message) {
            effects.push(HostEffect::Emit { def_id, message });
        }
    }

    /// The file dialogs whose element is gone, forgotten, and their ids
    /// given back.
    pub(in crate::host) fn file_dialogs_forget_gone(&mut self) {
        let gone: Vec<i32> = self
            .file_dialogs
            .keys()
            .copied()
            .filter(|id| !self.registry.contains(*id))
            .collect();
        for id in gone {
            if let Some(shown) = self.file_dialogs.remove(&id) {
                for k in 0..IDS as i32 {
                    self.widget_ids.free(i64::from(shown.base + k));
                }
            }
        }
    }
}

#[cfg(test)]
impl Host {
    /// Whether file dialog `id` is up.
    pub(crate) fn file_dialog_up(&self, id: i32) -> bool {
        self.file_dialogs
            .get(&id)
            .is_some_and(|shown| shown.presenter.is_up())
    }

    /// **File dialog `id` of window `def_id` answered by a hand**: `name`
    /// typed in its name field, and its accepting button pressed.
    pub(crate) fn file_dialog_answer(&mut self, def_id: i32, id: i32, name: &str) {
        let Some(base) = self.file_dialogs.get(&id).map(|shown| shown.base) else {
            return;
        };
        let typed = self.event_message(base + super::NAME, 0, vec![OscType::String(name.into())]);
        self.deliver(def_id, &typed);
        let click = self.event_message(
            base + super::ACCEPT,
            0,
            vec![OscType::String("click".into())],
        );
        self.deliver(def_id, &click);
    }
}
