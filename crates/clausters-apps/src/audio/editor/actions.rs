//! **The window's actions as they read now**: what each verb addresses,
//! whether it is available and why not, as a menu, a key and a script read
//! it.

use super::*;

impl AudioEditor {
    /// **The window's actions as they read now**, by verb: what each
    /// addresses, whether it is available and why not, a switch's state and a
    /// choice's alternative in force. `undo`/`redo` and `pending` are the
    /// focused file's, which the editing context supplies.
    pub(crate) fn actions(
        &self,
        member: u32,
        server: ServerInstance,
        undo: bool,
        redo: bool,
        pending: bool,
        playing: bool,
    ) -> BTreeMap<&'static str, ActionState> {
        let view = &self.file.presentation;
        let views = match view.views {
            Views::Waveform => "view_wave",
            Views::Spectrogram => "view_spectrogram",
            Views::Stacked => "view_both",
        };
        let layer = if view.edit_layer.starts_with("points") {
            "show_envelope"
        } else {
            "layer_take"
        };
        let selected = view.selection.is_some_and(|(a, b)| b > a);
        let document = ActionTarget::Document {
            member,
            object: self.focused_file(),
        };
        // The window's verbs a take has no meaning for: a take has no grid,
        // and its spans are not boxes to split, join or quantize. With no
        // reading the host neither performs them nor lists their keys.
        const NOT_A_TAKE_S: &[&str] = &["split", "join", "quantize", "snap"];
        verbs::WINDOW
            .iter()
            .chain(verbs::AUDIO)
            .filter(|verb| !NOT_A_TAKE_S.contains(&verb.name))
            .map(|verb| {
                let name = verb.name;
                let window = WINDOW_LEVEL.contains(&name);
                let target = if window {
                    ActionTarget::Window
                } else {
                    document.clone()
                };
                let mut state = ActionState::new(verb, target);
                state.value = match verb.group {
                    Some("views") => Some(views.into()),
                    Some("layer") => Some(layer.into()),
                    _ => None,
                };
                state.checked = match name {
                    "loop" => Some(view.looping),
                    "toggle_files" => Some(self.window_state.files_visible),
                    _ => state.value.as_deref().map(|on| on == name),
                };
                if !window && !self.open_files.open {
                    state.refuse("no file is open");
                }
                match name {
                    "undo" if !undo => state.refuse("the file in focus has nothing to undo"),
                    "redo" if !redo => state.refuse("the file in focus has nothing to redo"),
                    "show_envelope" if self.window_state.envelope_widget.is_none() => {
                        state.refuse("this window has no envelope body");
                    }
                    _ => {}
                }
                // Work on the file in focus holds back what would edit, save or
                // close it; what only looks at it stays available.
                if matches!(
                    name,
                    "undo"
                        | "redo"
                        | "cut"
                        | "paste"
                        | "mix"
                        | "delete"
                        | "save"
                        | "save_as"
                        | "close_file"
                        | "apply"
                        | "apply_envelope"
                        | "reverse"
                ) {
                    state.pending = pending;
                    if pending {
                        state.refuse("the file has pending work");
                    }
                }
                // A file is not edited while it plays.
                if playing && edits_contents(name) {
                    state.refuse(PLAYING);
                }
                // Last: an empty selection is the one objection a host reads
                // for itself, so it must not hide another.
                if matches!(name, "cut" | "copy" | "delete" | "reverse") && !selected {
                    state.refuse_unselected();
                }
                if matches!(name, "paste" | "mix") {
                    state.paste = Some(PasteTarget::Samples {
                        channels: self.file.channels,
                        rate: self.file.rate,
                        connection: server.connection,
                        generation: server.generation,
                    });
                }
                (name, state)
            })
            .collect()
    }
}
