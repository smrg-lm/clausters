//! **An audio member called by its coordinator**: the context's typed calls
//! around one of the editor's verbs -- the order focused on its file, the
//! sources it holds brought up to date, the version moved when the focus
//! did, a file with work pending kept open -- which the JSON door's
//! `member` verb makes too.

use super::*;
use clausters_editing::conversation::Answer;

impl Editing {
    /// Audio member `member`'s editor, to read.
    pub(crate) fn audio(&self, member: MemberId) -> Option<&AudioEditor> {
        match self.member(member)? {
            Member::Audio(editor) => Some(editor),
            _ => None,
        }
    }

    /// Audio member `member`'s editor, to read or set.
    pub(crate) fn audio_mut(&mut self, member: MemberId) -> Option<&mut AudioEditor> {
        match self.member_mut(member)? {
            Member::Audio(editor) => Some(editor),
            _ => None,
        }
    }

    /// **A verb of audio member `member` that comes to an outcome**
    /// (`call`, a script's or the window's list's), with what the context
    /// keeps around it: the binding it answers numbered with the structure
    /// and the version, the answer with the version, a closed file's order
    /// let go. A verb that reads or sets without a turn is carried out and
    /// comes to nothing. Refused with why: no file open, a file with work
    /// pending closed, a request the editor cannot take.
    pub(crate) fn audio_verb(
        &mut self,
        member: MemberId,
        call: &Value,
    ) -> Result<audio::Outcome, String> {
        let verb = call["verb"].as_str().unwrap_or_default().to_string();
        let closes = (verb == "close_file").then(|| audio::FileAt::of(call));
        let mut call = call.clone();
        self.audio_turned(member, &verb, closes, |editor, version| {
            call["version"] = json!(version);
            match editor.turn_verb(&call, version) {
                Some(turned) => turned,
                None => {
                    let answer = audio::call_value(editor, call);
                    match answer["error"].as_str() {
                        Some(error) => Err(error.into()),
                        None => Ok(audio::Outcome::default()),
                    }
                }
            }
        })
    }

    /// **A file of audio member `member`'s list focused or closed**, `at`
    /// where it is named: [`Self::audio_verb`]'s `focus_file` and
    /// `close_file`.
    pub(crate) fn audio_file_verb(
        &mut self,
        member: MemberId,
        verb: audio::FileVerb,
        at: audio::FileAt,
    ) -> Result<audio::Outcome, String> {
        let (name, closes) = match verb {
            audio::FileVerb::FocusFile => ("focus_file", None),
            audio::FileVerb::CloseFile => ("close_file", Some(at)),
        };
        self.audio_turned(member, name, closes, |editor, _| editor.file_verb(verb, at))
    }

    /// **One turn of audio member `member`**, `turn` given its editor and the
    /// version, with what the context keeps around it ([`Self::audio_verb`]).
    /// `closes` names the file a close closes, which work pending keeps open.
    fn audio_turned(
        &mut self,
        member: MemberId,
        verb: &str,
        closes: Option<audio::FileAt>,
        turn: impl FnOnce(&mut AudioEditor, i64) -> Result<audio::Outcome, String>,
    ) -> Result<audio::Outcome, String> {
        self.select_member(member);
        self.sync_resources();
        let server = self
            .seats
            .get(member as usize)
            .map(|seat| seat.server)
            .unwrap_or_default();
        let editor = self
            .audio(member)
            .ok_or("the member is not an audio application")?;
        if let Some(error) = audio::refusal(editor, verb) {
            return Err(error.into());
        }
        let before = editor.focused_file();
        if let Some(at) = closes {
            let file = match at {
                audio::FileAt::Index(index) => editor.file_at(index),
                audio::FileAt::File(file) => Some(file),
                audio::FileAt::Focused => Some(before),
            };
            if file.is_some_and(|object| {
                self.audio_runtime
                    .operations
                    .busy(crate::application::operations::Target {
                        member,
                        object,
                        server,
                    })
            }) {
                return Err("the file has pending work; close after it completes".into());
            }
        }
        let version = self.version;
        let editor = self.audio_mut(member).expect("checked above");
        let mut outcome = turn(editor, version)?;
        self.select_member(member);
        if self.audio(member).map(AudioEditor::focused_file) != Some(before) {
            self.version += 1;
        }
        self.number_binding(member, &mut outcome);
        if let Some(answer) = outcome.answer.as_mut() {
            set_doc_version(answer, self.version);
        }
        if closes.is_some()
            && let Some(action) = outcome.file_action
        {
            self.close_audio_file(member, action.file);
            self.sync_resources();
            if !outcome.close {
                self.select_member(member);
            }
        }
        Ok(outcome)
    }

    /// **A cut's second half committed**, once its copy is on the clipboard.
    pub(crate) fn prepare_finish_cut(
        &mut self,
        member: MemberId,
        cut: &audio::FinishCut,
    ) -> Turned {
        self.prepare_audio_cut(member, cut)
    }

    /// **Audio member `member`'s playback holds what it reads**: the takes
    /// its files are read from, rooted while it plays them. Answers what
    /// nothing reaches any more.
    pub(crate) fn retain_audio_playback(&mut self, member: MemberId) -> Vec<Freed> {
        self.sync_resources();
        if let Some(seat) = self.seats.get(member as usize)
            && let Member::Audio(editor) = &seat.member
        {
            self.services.resources.retain(
                Root {
                    kind: RootKind::Playback,
                    owner: u64::from(member),
                },
                editor.rooted().into_iter().map(|source| ResourceId {
                    server: seat.server,
                    buffer: source.0 as i64,
                }),
            );
        }
        self.release()
    }

    /// **Audio member `member`'s playback is gone**, and what it held with
    /// it. Answers what nothing reaches any more.
    pub(crate) fn drop_audio_playback(&mut self, member: MemberId) -> Vec<Freed> {
        self.services.resources.drop_root(Root {
            kind: RootKind::Playback,
            owner: u64::from(member),
        });
        self.release()
    }

    /// An outcome's binding, numbered with member `member`'s structure and
    /// the context's version: what the host names its next gesture with.
    pub(crate) fn number_binding(&self, member: MemberId, outcome: &mut audio::Outcome) {
        if let Some(binding) = outcome.binding.as_mut().filter(|b| b.is_object()) {
            binding["structure"] = json!(self.structure(member));
            binding["version"] = json!(self.version);
        }
    }
}

/// An answer, told the version the host names back.
fn set_doc_version(answer: &mut Answer, version: i64) {
    match answer {
        Answer::Ack { doc_version, .. } | Answer::Push { doc_version, .. } => {
            *doc_version = version
        }
        Answer::Silent => {}
    }
}
