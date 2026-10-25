//! **The context's JSON door**: one verb of an editing context, read from
//! JSON and answered as JSON -- what both clients bind through the C ABI and
//! wasm, and what the GUI host's application calls the context with.

use super::*;

/// A record as a client hands one over for an external member.
#[derive(Deserialize)]
struct Recorded {
    #[serde(default)]
    label: String,
    #[serde(default)]
    legs: Vec<RecordedLeg>,
}

#[derive(Deserialize)]
struct RecordedLeg {
    forward: Value,
    backward: Value,
    #[serde(default)]
    key: Option<String>,
    #[serde(default, rename = "holdsForward")]
    holds_forward: Vec<u64>,
    #[serde(default, rename = "holdsBackward")]
    holds_backward: Vec<u64>,
}

/// **One verb of a context, over JSON** -- the door both clients bind.
///
/// `request` names the `verb` and carries its arguments:
///
/// - `openMultitrack` -- `key`, and what a multitrack editor is built from
///   (`clausters_apps::multitrack::editor::new_json`, the version aside):
///   `{"member", "structure"}`, or `{"error"}`.
/// - `openAudio` -- `key`, and what an audio editor is built from
///   (`clausters_apps::audio::editor::new_json`): `{"member", "structure"}`,
///   or `{"error"}`. Its file has an undo order of its own; `sharedHistory`
///   `true` joins it to the context's shared order under `key` instead.
/// - `bytes` -- `bytes`, or `null` for none: the most bytes of takes only the
///   history may hold. Answers `{}`.
/// - `openNotes` -- `key`, `sequence` (the sequence as data, which the
///   editor then holds alone -- a caller that shares one opens it with
///   [`Editing::open_notes`]) and what a notes editor is built from
///   (`clausters_apps::notes::editor::new_json`): `{"member", "structure"}`.
/// - `openPoints` -- `key`, `points` (the curve as flat quads) and `name`,
///   and what a points editor is built from
///   (`clausters_apps::points::editor::new_json`): `{"member", "structure"}`.
///   A second points editor under a key one already has edits that one's
///   curve.
/// - `external` -- `key`, `domain`: `{"member", "structure"}`.
/// - `event` -- `member`, `addr`, `args`: a [`Turned`], or `null`.
/// - `act` -- `member`, `call`: a verb a client calls on a member, which
///   edits -- a [`Turned`], or `null` (the score editor's verbs, built with the
///   `notation` feature).
/// - `step` -- `direction` (`"undo"` or `"redo"`): a [`Stepped`].
/// - `record` -- `member`, `label`, `legs` (`forward`, `backward`, `key`),
///   `coalesce`: `{"recorded", "version"}`.
/// - `member` -- `member`, and a verb of that member's own door with its
///   arguments, the version filled in: what that door answers. `event` and
///   `apply` are the context's and answer `{}` here.
/// - `moved` -- an edit that leaves no entry: `{"version"}`, moved on.
/// - `actions` -- `member`: each action the member answers, by verb, as
///   [`clausters_editing::verbs::ActionState`] reads it now. A member's
///   `window` answer carries the same object as `actions`.
/// - `state` -- `{"version", "canUndo", "canRedo", "undoLabel", "redoLabel"}`.
///
/// An unknown verb answers `{}`.
pub fn call_json(editing: &mut Editing, request: &str) -> String {
    match serde_json::from_str::<Value>(request) {
        Ok(request) => call_value(editing, request).to_string(),
        Err(_) => "{}".into(),
    }
}

/// **[`call_json`], over a request already read**: what the context's own
/// callers in this crate speak, with no text in between.
pub(crate) fn call_value(editing: &mut Editing, mut request: Value) -> Value {
    let get = |request: &Value, key: &str| request.get(key).cloned().unwrap_or(Value::Null);
    let member = int(&get(&request, "member")) as MemberId;
    let key = get(&request, "key")
        .as_str()
        .unwrap_or_default()
        .to_string();
    let verb = request
        .get("verb")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if let Some(map) = request.as_object_mut() {
        map.insert("version".into(), json!(editing.version));
    }
    match verb.as_str() {
        "openMultitrack" => match multitrack::new_json(&request.to_string()) {
            Some(editor) => joined(editing, &key, Member::Multitrack(Box::new(editor))),
            None => json!({ "error": "the request names no multitrack" }),
        },
        "openAudio" => match audio::new_json(&request.to_string()) {
            Ok(editor) => {
                // An audio file's history is its own unless the caller
                // composes it into the context's shared order.
                let editor = Member::Audio(Box::new(editor));
                let member = if request["sharedHistory"] == true {
                    editing.join(&key, editor)
                } else {
                    editing.open_independent(editor)
                };
                if let Ok(server) =
                    serde_json::from_value::<ServerInstance>(get(&request, "server"))
                {
                    editing.set_server(member, server);
                }
                json!({"member": member, "structure": editing.structure(member)})
            }
            Err(error) => json!({ "error": error }),
        },
        "openNotes" => {
            let request = request.to_string();
            let editor = notes::new_json(notes::shared_of(&request), &request);
            joined(editing, &key, Member::Notes(Box::new(editor)))
        }
        "openPoints" => {
            let curve = editing
                .shared_curve(&key)
                .unwrap_or_else(|| crate::points::shared_of(&request));
            let editor = points::new_json(curve, &request.to_string());
            joined(editing, &key, Member::Points(Box::new(editor)))
        }
        "operation" => editing.audio_operation_json(&request),
        "cleanup" => match request["action"].as_str().unwrap_or_default() {
            "prepare" => serde_json::from_value::<ResourceId>(get(&request, "resource"))
                .ok()
                .and_then(|id| editing.services.resources.begin_release(id))
                .map(|release| serde_json::to_value(release).unwrap())
                .unwrap_or_else(|| json!({"accepted":false})),
            "complete" => {
                let accepted =
                    serde_json::from_value::<crate::application::resources::ReleaseTicket>(get(
                        &request, "ticket",
                    ))
                    .is_ok_and(|ticket| {
                        editing.services.resources.finish_release(
                            ticket,
                            if request["success"] == true {
                                Ok(())
                            } else {
                                Err(request["reason"]
                                    .as_str()
                                    .unwrap_or("cleanup was refused")
                                    .to_string())
                            },
                        )
                    });
                json!({"accepted":accepted})
            }
            "pending" => {
                json!({"tickets":editing.services.resources.pending_releases().into_iter().map(|ticket|json!({"ticket":ticket,"reason":editing.services.resources.release_error(ticket)})).collect::<Vec<_>>()})
            }
            _ => json!({"error":"unknown cleanup action"}),
        },
        "exchange" => editing.exchange_json(&request),
        "resources" => {
            editing.sync_resources();
            serde_json::to_value(editing.services.resources.sources()).unwrap_or_default()
        }
        "retainMember" => {
            editing.sync_resources();
            let retained = if let Some(seat) = editing.seats.get(member as usize)
                && let Member::Audio(editor) = &seat.member
                && let Ok(kind) = serde_json::from_value::<RootKind>(get(&request, "kind"))
                && !matches!(kind, RootKind::Contents | RootKind::History)
            {
                editing.services.resources.retain(
                    Root {
                        kind,
                        owner: u64::from(member),
                    },
                    editor.rooted().into_iter().map(|source| ResourceId {
                        server: seat.server,
                        buffer: source.0 as i64,
                    }),
                )
            } else {
                false
            };
            let freed = editing.release();
            json!({"retained": retained, "freed": freed})
        }
        "retain" => {
            editing.sync_resources();
            let root = serde_json::from_value::<Root>(get(&request, "root"));
            let sources = serde_json::from_value::<Vec<ResourceId>>(get(&request, "sources"));
            let retained = match (root, sources) {
                (Ok(root), Ok(sources)) => editing.services.resources.retain(root, sources),
                _ => false,
            };
            json!({"retained": retained})
        }
        "dropRoot" => {
            if let Ok(root) = serde_json::from_value::<Root>(get(&request, "root")) {
                editing.services.resources.drop_root(root);
            }
            let freed = editing.release();
            json!({"freed": freed})
        }
        "invalidate" => {
            let invalidated = serde_json::from_value::<ServerInstance>(get(&request, "server"))
                .map(|server| editing.invalidate_server(server))
                .unwrap_or_default();
            json!({"invalidated": invalidated})
        }
        "bytes" => {
            editing.set_bytes(get(&request, "bytes").as_u64());
            json!({})
        }
        "external" => {
            let domain = get(&request, "domain")
                .as_str()
                .unwrap_or_default()
                .to_string();
            joined(editing, &key, Member::External { domain })
        }
        "event" => {
            let event = serde_json::from_value::<Event>(request.clone()).unwrap_or_default();
            let prepared = editing.prepare_audio_event(member, &event);
            to_value(&prepared.or_else(|| editing.event(member, &event)))
        }
        "act" => {
            let call = get(&request, "call");
            let prepared = editing.prepare_audio_action(member, &call);
            to_value(&prepared.or_else(|| editing.act(member, &call)))
        }
        "step" => {
            let target = match request.get("member") {
                None | Some(Value::Null) => None,
                Some(value) => {
                    let target = value
                        .as_u64()
                        .and_then(|id| MemberId::try_from(id).ok())
                        .filter(|id| editing.member(*id).is_some());
                    let Some(target) = target else {
                        return to_value(&Stepped {
                            stepped: false,
                            operation: None,
                            reason: Some("the history step member is invalid".into()),
                            effects: Vec::new(),
                            corrections: Vec::new(),
                            freed: Vec::new(),
                            version: editing.version(),
                        });
                    };
                    Some(target)
                }
            };
            editing.select(target);
            let direction = get(&request, "direction")
                .as_str()
                .and_then(Direction::parse)
                .unwrap_or(Direction::Undo);
            let prepared = editing.prepare_audio_walk(direction);
            to_value(&prepared.unwrap_or_else(|| editing.step(direction)))
        }
        "record" => {
            let coalesce = get(&request, "coalesce").as_bool().unwrap_or(false);
            let recorded = serde_json::from_value::<Recorded>(request.clone()).ok();
            let taken = recorded.is_some_and(|r| {
                let record = Record {
                    label: r.label,
                    legs: r
                        .legs
                        .into_iter()
                        .map(|leg| crate::turn::Leg {
                            forward: leg.forward,
                            backward: leg.backward,
                            key: leg.key.unwrap_or_default(),
                            holds_forward: leg.holds_forward,
                            holds_backward: leg.holds_backward,
                        })
                        .collect(),
                };
                editing.record(member, &record, coalesce)
            });
            json!({ "recorded": taken, "version": editing.version() })
        }
        "member" => {
            editing.select_member(member);
            editing.sync_resources();
            let before = match editing.member(member) {
                Some(Member::Audio(editor)) => Some(editor.focused_file()),
                _ => None,
            };
            let inner = get(&request, "call")
                .get("verb")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if inner == "close_file"
                && let Some(seat) = editing.seats.get(member as usize)
                && let Member::Audio(editor) = &seat.member
            {
                let file = if let Some(index) = request["call"]["index"].as_u64() {
                    editor.file_at(index as usize)
                } else {
                    Some(
                        request["call"]["file"]
                            .as_i64()
                            .unwrap_or(editor.focused_file()),
                    )
                };
                if file.is_some_and(|object| {
                    editing
                        .audio_runtime
                        .operations
                        .busy(crate::application::operations::Target {
                            member,
                            object,
                            server: seat.server,
                        })
                }) {
                    return json!({"error":"the file has pending work; close after it completes"});
                }
            }
            if inner == "add_file" && matches!(editing.member(member), Some(Member::Audio(_))) {
                return editing.prepare_add_file(member, &get(&request, "call"));
            }
            if matches!(inner.as_str(), "event" | "apply") {
                return json!({});
            }
            let mut call = get(&request, "call");
            if let Some(map) = call.as_object_mut() {
                map.insert("version".into(), json!(editing.version));
            }
            let answer = match editing.member_mut(member) {
                Some(Member::Multitrack(editor)) => {
                    parsed(&multitrack::call_json(editor, &call.to_string()))
                }
                Some(Member::Audio(editor)) => audio::call_value(editor, call),
                Some(Member::Notes(editor)) => parsed(&notes::call_json(editor, &call.to_string())),
                Some(Member::Points(editor)) => {
                    parsed(&points::call_json(editor, &call.to_string()))
                }
                #[cfg(feature = "notation")]
                Some(Member::Score(editor)) => parsed(&score::call_json(editor, &call.to_string())),
                _ => json!({}),
            };
            editing.select_member(member);
            let after = match editing.member(member) {
                Some(Member::Audio(editor)) => Some(editor.focused_file()),
                _ => None,
            };
            if before != after {
                editing.version += 1;
            }
            let mut value = answer;
            if inner == "window" {
                value["actions"] = editing.action_states(member);
            }
            if value.get("binding").is_some_and(Value::is_object) {
                value["binding"]["structure"] = json!(editing.structure(member));
                value["binding"]["version"] = json!(editing.version);
            }
            // The switch moved the version, and the answer is how the host
            // learns it: its next gesture names this one.
            if let Some(answer) = value.get_mut("answer").filter(|a| a.is_object()) {
                answer["docVersion"] = json!(editing.version);
            }
            if inner == "close_file"
                && value.get("error").is_none()
                && let Some(file) = value["fileAction"]["file"].as_i64()
            {
                editing.close_audio_file(member, file);
                editing.sync_resources();
                if value["close"] != true {
                    editing.select_member(member);
                }
            }
            value
        }
        "moved" => json!({ "version": editing.moved() }),
        "actions" => match request
            .get("member")
            .and_then(Value::as_u64)
            .and_then(|id| MemberId::try_from(id).ok())
            .filter(|id| editing.member(*id).is_some())
        {
            Some(member) => editing.action_states(member),
            None => json!({"error": "the action member is invalid"}),
        },
        "state" => {
            editing.select(
                request
                    .get("member")
                    .and_then(Value::as_u64)
                    .map(|m| m as MemberId),
            );
            json!({
            "version": editing.version(),
            "canUndo": editing.can_undo(),
            "canRedo": editing.can_redo(),
            "undoLabel": editing.undo_label(),
            "redoLabel": editing.redo_label(),
            })
        }
        _ => json!({}),
    }
}

impl Editing {
    /// **Opens a notes editor over `sequence`**, shared with the caller -- the
    /// door a binding's sequence handle opens through, so the editor edits the
    /// very sequence the script holds. `request` is what
    /// [`notes::new_json`] reads; the answer is the `openNotes` one.
    pub fn open_notes(&mut self, key: &str, sequence: Shared, request: &str) -> String {
        let mut request = serde_json::from_str::<Value>(request).unwrap_or_else(|_| json!({}));
        if let Some(map) = request.as_object_mut() {
            map.insert("version".into(), json!(self.version));
        }
        let editor = notes::new_json(sequence, &request.to_string());
        joined(self, key, Member::Notes(Box::new(editor))).to_string()
    }
}

#[cfg(feature = "notation")]
impl Editing {
    /// **Opens a score editor over `score`**, shared with the caller -- the
    /// door a binding's score handle opens through, so the editor edits the
    /// very score the script holds. `request` is what [`score::new_json`]
    /// reads; the answer is the `openNotes` one's shape.
    pub fn open_score(&mut self, key: &str, shared: crate::score::Shared, request: &str) -> String {
        let mut request = serde_json::from_str::<Value>(request).unwrap_or_else(|_| json!({}));
        if let Some(map) = request.as_object_mut() {
            map.insert("version".into(), json!(self.version));
        }
        let editor = score::new_json(shared, &request.to_string());
        joined(self, key, Member::Score(Box::new(editor))).to_string()
    }

    /// **Opens a score editor over `sequence`**, on the page it is read
    /// into: `shared` is the score the caller's handle holds, loaded here
    /// with the reading, and `sequence` the one a roll over it shares. `key`
    /// is the sequence's, so the editor's entries are the sequence's and one
    /// order with every roll over it. `request` is what [`score::over_json`]
    /// reads; the answer is the `openNotes` one's shape, or `{"error"}` when
    /// the sequence cannot be read.
    pub fn open_score_over(
        &mut self,
        key: &str,
        shared: crate::score::Shared,
        sequence: Shared,
        request: &str,
    ) -> String {
        let mut request = serde_json::from_str::<Value>(request).unwrap_or_else(|_| json!({}));
        if let Some(map) = request.as_object_mut() {
            map.insert("version".into(), json!(self.version));
        }
        match score::over_json(shared, sequence, &request.to_string()) {
            Ok(editor) => joined(self, key, Member::Score(Box::new(editor))).to_string(),
            Err(error) => json!({ "error": error }).to_string(),
        }
    }
}

impl Editing {
    /// **Opens a multitrack editor over `multitrack`**, shared with the caller:
    /// the door a binding's multitrack handle opens through, so the editor
    /// edits the very multitrack the script holds. `request` is what
    /// [`multitrack::over_json`] reads; the answer is the `openMultitrack` one.
    pub fn open_multitrack(
        &mut self,
        key: &str,
        multitrack: crate::multitrack::Shared,
        request: &str,
    ) -> String {
        let mut request = serde_json::from_str::<Value>(request).unwrap_or_else(|_| json!({}));
        if let Some(map) = request.as_object_mut() {
            map.insert("version".into(), json!(self.version));
        }
        match multitrack::over_json(multitrack, &request.to_string()) {
            Some(editor) => joined(self, key, Member::Multitrack(Box::new(editor))).to_string(),
            None => json!({ "error": "the request does not read" }).to_string(),
        }
    }
}

impl Editing {
    /// The curve a points editor under `key` already edits, so a second
    /// window over it edits the same one.
    fn shared_curve(&self, key: &str) -> Option<crate::points::Shared> {
        let structure = self.services.histories.named(key)?;
        self.seats.iter().find_map(|seat| match &seat.member {
            Member::Points(editor) if seat.structure == structure => Some(editor.curve().clone()),
            _ => None,
        })
    }
}

impl Editing {
    /// **Binds a multitrack member's source to a sequence** shared with the
    /// caller: a region over `source` draws the sequence's notes, and a notes
    /// editor opened over the same handle edits what that region draws. The
    /// answer is the member's picture corrected with it (the `Answer` a
    /// correction is), or `{}` when `member` is not a multitrack.
    pub fn bind_sequence(
        &mut self,
        member: MemberId,
        source: SourceId,
        sequence: Shared,
    ) -> String {
        let version = self.version;
        match self.member_mut(member) {
            Some(Member::Multitrack(editor)) => {
                editor.bind_sequence(source, sequence);
                to_json(&editor.resync_all(version))
            }
            _ => "{}".into(),
        }
    }

    /// **Hands a multitrack member the engraver its boxes' pages are
    /// engraved with**: a score the caller made for that and reads no more,
    /// which the editor loads with each sequence in turn. The answer is the
    /// member's picture corrected with it, or `{}` when `member` is not a
    /// multitrack.
    #[cfg(feature = "notation")]
    pub fn bind_engraver(&mut self, member: MemberId, score: crate::score::Shared) -> String {
        let version = self.version;
        match self.member_mut(member) {
            Some(Member::Multitrack(editor)) => {
                editor.bind_engraver(score);
                to_json(&editor.resync_all(version))
            }
            _ => "{}".into(),
        }
    }

    /// [`Self::bind_sequence`] off a `{"member", "source"}` request: the one
    /// door every binding's sequence handle binds through.
    pub fn bind_sequence_json(&mut self, sequence: Shared, request: &str) -> String {
        let request = serde_json::from_str::<Value>(request).unwrap_or_default();
        let number = |key: &str| request.get(key).and_then(Value::as_u64);
        match (number("member"), number("source")) {
            (Some(member), Some(source)) => {
                self.bind_sequence(member as MemberId, SourceId(source), sequence)
            }
            _ => "{}".into(),
        }
    }
}

/// A member taken in, as the door answers it: its number and the structure it
/// is in the order.
fn joined(editing: &mut Editing, key: &str, member: Member) -> Value {
    let member = editing.join(key, member);
    let structure = editing.structure(member).map(|s| s.0);
    json!({ "member": member, "structure": structure })
}

/// A turn's answer as JSON.
fn to_json<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "{}".into())
}

/// A turn's answer as a value.
fn to_value<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or_else(|_| json!({}))
}

/// What a member's text door answered, read.
fn parsed(answer: &str) -> Value {
    serde_json::from_str(answer).unwrap_or_default()
}
