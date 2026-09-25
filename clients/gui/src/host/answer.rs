//! **A gesture answered**: the `/gui_event` a gesture makes, delivered to a
//! client or -- when this host owns what it draws -- answered here, with the
//! document applied, the samples written, the picture adopted and the stamp
//! settled. The same order a client's editor answers one in.

use super::*;

impl Host {
    /// Retires everything an acknowledgement covers and lets go of what it was
    /// drawing -- the two halves of *drop every pending at or below the stamp,
    /// and adopt what arrived*.
    ///
    /// It is a method rather than the body of `/gui_ack` because a host that
    /// **is** its own owner answers itself, and the two paths must retire an
    /// edit the same way or a standalone editor would keep drawing edits it had
    /// already applied.
    ///
    /// Returns whether anything was retired.
    pub fn settle(&mut self, acked: ack::Acked) -> bool {
        let reason = acked.reason.clone();
        let settled = self.outbox.borrow_mut().ack(acked);
        if settled.is_empty() {
            return false;
        }
        // **The one reader the reason has ever had.** The mechanism does not
        // need it -- applied, transformed and refused are one message -- but a
        // person does: an edit that springs back with nothing said teaches that
        // it sometimes does not work. It lands on the window of the newest edit
        // the answer covers, which is the one the hand just made.
        if let Some(reason) = reason.filter(|r| !r.is_empty())
            && let Some(last) = settled.last()
        {
            self.say(
                last.def_id,
                status::Line::of_reason(Some(last.widget_id), &reason),
            );
        }
        diag::debug!("retired {} pending edit(s)", settled.len());
        // What the owner pushed is already in the samples, so letting go is
        // what makes the picture the document's again rather than the hand's.
        for p in &settled {
            if let Some(w) = self
                .window_def_mut(p.def_id)
                .and_then(|t| t.find_mut(p.widget_id))
            {
                w.kind.set_pending_edit(None);
            }
        }
        true
    }

    /// Answers a gesture **itself**, when this host owns what it draws.
    ///
    /// Returns whether it did: `false` is every host driven by a script, and
    /// every payload that is not an edit, both of which go out on the wire as
    /// they always have. There is deliberately no third outcome -- a host that
    /// owned the document but could not read the payload emits it, so a script
    /// attached alongside still sees what it always saw.
    ///
    /// The window id is not for the binding -- a widget id is unique across the
    /// registry -- but for **adopting the answer**: what an edit leaves has to
    /// be written back onto the picture, and a widget is reached through the
    /// tree it is in.
    pub fn answer_own(&mut self, def_id: i32, widget_id: i32, seq: i32, args: &[OscType]) -> bool {
        let message = self.event_message(widget_id, seq, args.to_vec());
        self.deliver(def_id, &message)
    }

    /// **A `/gui_event` as a client receives it**: the widget, the stamp, the
    /// version this host is drawing -- what the edit was made against -- and
    /// the payload.
    ///
    /// Both fronts build every event here, whether it then crosses a socket or
    /// is delivered in memory ([`Host::deliver`]), so the two are one message.
    /// The version is read now rather than carried in the gesture because it
    /// is the conversation's state: what the edit was made against is what the
    /// host had been told when it went out.
    pub fn event_message(&self, widget_id: i32, seq: i32, args: Vec<OscType>) -> OscMessage {
        let mut msg_args = vec![
            OscType::Int(widget_id),
            OscType::Int(seq),
            OscType::Long(self.outbox.borrow().version()),
        ];
        msg_args.extend(args);
        OscMessage {
            addr: GUI_EVENT.into(),
            args: msg_args,
        }
    }

    /// **Delivers an event to what this host owns**, the one message a client
    /// would have received ([`Host::event_message`]); answers whether it was
    /// taken, which is what tells a front not to send it on.
    ///
    /// A gesture on the multitrack's own window is the **editor's turn** -- the same
    /// one a script and a page run, read out of the same message: stamped,
    /// versioned, applied with its inverse and answered. What is left is the
    /// tree's, for a document written before the turn, and the window's own
    /// verbs.
    pub fn deliver(&mut self, def_id: i32, message: &OscMessage) -> bool {
        let (Some(OscType::Int(widget_id)), Some(OscType::Int(seq))) =
            (message.args.first(), message.args.get(1))
        else {
            return false;
        };
        let (widget_id, seq) = (*widget_id, *seq);
        let args = message.args.get(3..).unwrap_or_default();
        diag::debug!(
            "deliver: widget={widget_id} seq={seq} owner={} args={args:?}",
            self.owner.is_some()
        );
        // **The editor reads every message addressed to its window**: its
        // gestures, its transport row and the space bar, and the window's undo
        // and redo, which are a step of the history it answers once the host has
        // walked it. What it does not take is the tree's.
        if self
            .owner
            .as_ref()
            .is_some_and(|o| o.draws_multitrack() && o.editor().is_some())
            && self.answer_multitrack(def_id, message)
        {
            return true;
        }
        self.answer_tree(def_id, widget_id, seq, args)
    }

    /// **The tree's answer**, and the window's own verbs: history, save, and
    /// the payloads a document written before the multitrack describes itself in.
    pub(super) fn answer_tree(
        &mut self,
        def_id: i32,
        widget_id: i32,
        seq: i32,
        args: &[OscType],
    ) -> bool {
        let Some(owner) = self.owner.as_mut() else {
            return false;
        };
        // The window's own verbs, which are not intents and address no node:
        // history is a walk through the log, and a save is a file. They arrive
        // addressed to the window (`keys::history`), so they are read before
        // anything looks for a node binding.
        match args.first() {
            Some(OscType::String(tag)) if tag == "undo" || tag == "redo" => {
                let applied = if tag == "undo" {
                    owner.undo()
                } else {
                    owner.redo()
                };
                self.adopt(def_id, &applied);
                self.replay_writes(def_id, &applied);
                if let Some(last) = applied.last() {
                    self.settle_at(seq, last.version as i64);
                }
                return true;
            }
            Some(OscType::String(tag)) if tag == "save" => {
                match owner.save_now() {
                    Ok(path) => diag::info!("session saved to {}", path.display()),
                    Err(e) => diag::warn!("save: {e}"),
                }
                return true;
            }
            // The **tree's** description of the same two payloads, for a
            // document written before the multitrack existed: one vocabulary or the
            // other, and either way the run is one entry in one history.
            Some(OscType::String(tag)) if tag == "clips" || tag == "lanes" => {
                let against = clausters_document::Against::default();
                let intents = owner.read_events(widget_id, args);
                let applied = owner.apply_all(&intents, &against);
                self.adopt(def_id, &applied);
                self.settle_at(seq, applied.last().map_or(0, |a| a.version as i64));
                return true;
            }
            _ => {}
        }
        let Some((intent, label)) = owner.read_event(widget_id, args) else {
            return false;
        };
        // A **destructive** edit is the one that reaches past the document: the
        // samples are not in it, so they are written to the buffer itself and
        // the inverse comes from the hand that drew over them.
        let write = match &intent {
            clausters_document::Intent::WriteSamples {
                channel,
                start,
                values,
                ..
            } => Some((*channel as usize, *start, values.clone())),
            _ => None,
        };
        let inverse = owner.read_inverse(widget_id, args);
        if let Some((channel, start, values)) = &write
            && let Err(why) = self.can_write(def_id, widget_id, *channel, *start, values.len())
        {
            // Refused, and said so: the pending drawing is dropped by the same
            // acknowledgement an applied edit sends, so the picture snaps back
            // to the samples rather than keeping a stroke nobody stored.
            diag::warn!("refusing to write {} sample(s): {why}", values.len());
            let version = self.owner.as_ref().map_or(0, |o| o.document.version as i64);
            self.settle_at(seq, version);
            return true;
        }
        let against = clausters_document::Against::default();
        let Some(owner) = self.owner.as_mut() else {
            return false;
        };
        let applied = match inverse {
            // The one edit whose inverse the document cannot read back.
            Some(inverse) => owner.apply_with_inverse(&intent, &inverse, &against, label),
            None => owner.apply(&intent, &against, label),
        };
        let version = applied.version;
        if applied.applied
            && let Some((channel, start, values)) = write
        {
            self.write_buffer_samples(def_id, widget_id, channel, start, &values);
        }
        self.adopt(def_id, &[applied]);
        self.settle_at(seq, version as i64);
        true
    }

    /// [`Self::settle`] for an answer this host gave itself: everything up to
    /// `seq`, at document version `version`, with no reason to say.
    pub(super) fn settle_at(&mut self, seq: i32, version: i64) {
        self.settle(ack::Acked {
            seq,
            doc_version: version,
            ..Default::default()
        });
    }

    /// **A gesture on the multitrack's window, answered by the multitrack editor.**
    ///
    /// The turn is the applications crate's; what is carried out here is what a
    /// host holds: the entry recorded in the owner's history, the multitrack written
    /// back, a minted source made before the picture is redrawn (a box naming a
    /// source with no buffer draws empty and sounds through nothing), the picture
    /// and the readers brought in step, a placed cursor cued, and the stamp
    /// settled with the reason the turn gave -- so a refusal is said in the
    /// window that asked.
    pub(super) fn answer_multitrack(&mut self, def_id: i32, message: &OscMessage) -> bool {
        use clausters_apps::multitrack::editor::{Event, Kind, TransportVerb};

        let Some(owner) = self.owner.as_mut() else {
            return false;
        };
        let Some(member) = owner.editor_member() else {
            return false;
        };
        owner.sync_editor();
        // **The message a client would have received**, whole: its stamp, and
        // the version the host was drawing when the hand made the edit -- which
        // is what lets the conversation refuse one that a route the hand never
        // saw has overtaken, here as in a script. The turn is the editing
        // context's: it records the entry and moves the version.
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
        let clausters_apps::editing::Turned {
            outcome, stepped, ..
        } = turned;
        let clausters_apps::editing::Outcome::Multitrack(outcome) = outcome else {
            return false;
        };
        if outcome.turn == Kind::Nothing {
            return false;
        }
        if outcome.turn == Kind::Step {
            return self.step_multitrack(def_id, stepped, outcome.answer);
        }
        if outcome.changed
            && let Some(edited) = owner.editor().map(|editor| editor.multitrack().clone())
        {
            owner.multitrack = edited;
        }
        let minted: Vec<_> = outcome
            .minted
            .iter()
            .filter_map(|m| serde_json::from_value(m.clone()).ok())
            .collect();
        self.mint_sources(&minted);
        let applied = document::Applied {
            effective: None,
            version: self.owner.as_ref().map_or(0, |o| o.multitrack.version),
            applied: outcome.changed,
        };
        self.adopt(def_id, &[applied]);
        if let Some(secs) = outcome.locate {
            self.cue_multitrack(secs);
        }
        match outcome.transport {
            Some(TransportVerb::Toggle) => {
                #[cfg(test)]
                self.exchange
                    .asked
                    .push(serde_json::json!([if self.multitrack_rolling() {
                        "pause"
                    } else {
                        "play"
                    }]));
                self.roll_multitrack();
            }
            Some(TransportVerb::Stop { mark }) => self.stop_multitrack(mark),
            Some(TransportVerb::Cue { secs }) => self.cue_multitrack(secs),
            None => {}
        }
        if let Some(answer) = outcome.answer {
            self.tell(answer);
        }
        // **A name the host minted is answered with the one the multitrack kept**:
        // once a changed turn is carried out and a minted source has its
        // buffer, the editor compares what the window was told with what the
        // multitrack holds -- its `settle`, which a client's asks after every change.
        if outcome.changed
            && let Some(owner) = self.owner.as_mut()
        {
            // **The table as it is now**, a minted source included: the turn
            // handed the editor the table it had before `mint_sources` put the
            // join's buffer in it, and a settle projected from that one drew
            // the joined box over no buffer (found 2026-09-13, by measuring the
            // clip's props: `source=-1` until the box was moved to another
            // track, whose turn starts by handing the table over again). A
            // client's editor is handed it before every call (`_sync_core`).
            owner.sync_editor();
            let version = owner.editing.version();
            let settled = owner.editor_mut().map(|editor| editor.settle(version));
            if let Some(settled) = settled {
                self.tell(settled);
            }
        }
        true
    }

    /// **A step of the history, asked of the multitrack's window**: taken by the
    /// editing context inside the turn, carried out here on what the owner
    /// holds, then every window the crate corrected told and the stamp answered
    /// -- with the crate's reason when nothing could apply the step. The order a
    /// client's editor answers one in.
    pub(super) fn step_multitrack(
        &mut self,
        def_id: i32,
        stepped: Option<clausters_apps::editing::Stepped>,
        answer: Option<clausters_editing::conversation::Answer>,
    ) -> bool {
        let Some(owner) = self.owner.as_mut() else {
            return false;
        };
        let applied = stepped.as_ref().map_or_else(Vec::new, |s| owner.carry(s));
        self.adopt(def_id, &applied);
        self.replay_writes(def_id, &applied);
        if let Some(stepped) = stepped
            && stepped.stepped
        {
            for corrected in stepped.corrections {
                self.tell(corrected.answer);
            }
        }
        if let Some(answer) = answer {
            self.tell(answer);
        }
        true
    }

    /// **An answer the editor gave, carried out on this host**: the corrections
    /// drawn and the stamp settled -- what a client's `/gui_ack`, or its bundled
    /// push, does when it reaches a host, without the socket.
    pub(super) fn tell(&mut self, answer: clausters_editing::conversation::Answer) {
        use clausters_editing::conversation::Answer;

        let (kind, seq, doc_version, reason, corrections) = match answer {
            Answer::Silent => return,
            Answer::Ack {
                seq,
                doc_version,
                reason,
            } => ("ack", seq, doc_version, reason, Vec::new()),
            Answer::Push {
                seq,
                doc_version,
                reason,
                corrections,
            } => ("push", seq, doc_version, reason, corrections),
        };
        #[cfg(test)]
        self.exchange.told.push(serde_json::json!([
            kind,
            seq,
            doc_version,
            reason,
            corrections
                .iter()
                .map(|c| serde_json::json!([c.widget, c.props]))
                .collect::<Vec<_>>(),
        ]));
        #[cfg(not(test))]
        let _ = kind;
        let mut fx = Vec::new();
        for correction in corrections {
            if let (Ok(id), Value::Object(props)) =
                (i32::try_from(correction.widget), correction.props)
            {
                self.set_props(id, props.into_iter().collect(), &mut fx);
            }
        }
        self.settle(ack::Acked {
            seq: seq as i32,
            doc_version,
            reason,
            ..Default::default()
        });
    }

    /// Whether a destructive write can land on this widget's samples, or why
    /// it cannot.
    ///
    /// Checked **before** the document is touched, because a write that the
    /// samples refuses must not bump the source's generation: that number is
    /// what tells every reader its copy is stale, and moving it for an edit
    /// nothing performed would send them all back to the server for nothing.
    pub(super) fn can_write(
        &self,
        def_id: i32,
        widget_id: i32,
        channel: usize,
        start: u64,
        len: usize,
    ) -> Result<(), String> {
        let Some((channels, frames)) = self
            .samples_of(def_id, widget_id)
            .and_then(widget::element::Samples::sample_shape)
        else {
            return Err("this widget is not drawing any samples".into());
        };
        // The span is **frames of one channel** on both sides of the seam -- the
        // picture is drawn per channel and the server is written per channel
        // (`/buffer_setRangeChannel`) -- so this is the same check whatever the
        // buffer's shape, which is what it took to stop refusing stereo.
        if channel >= channels {
            return Err(format!(
                "the take has {channels} channel(s); there is no channel {channel}"
            ));
        }
        if start + len as u64 > frames {
            return Err(format!(
                "the span ends at frame {} and the buffer is {frames} frame(s) long",
                start + len as u64,
            ));
        }
        // Either somebody holds the samples for us, or we hold it ourselves.
        // The second case is the editor's: the take is mapped, so a stroke
        // lands whether or not anything is currently playing it.
        #[cfg(unix)]
        let held = self.server.is_some() || self.shared_buffers.is_some();
        #[cfg(not(unix))]
        let held = self.server.is_some();
        if !held {
            return Err("no audio server holds this buffer".into());
        }
        if self.buffer_of(def_id, widget_id).is_none() {
            return Err("this widget draws no server buffer".into());
        }
        Ok(())
    }

    /// **Carries a destructive edit through to the samples**: the server's
    /// buffer, and every picture of it this host is holding.
    ///
    /// The server's copy first, because it is the one that sounds and the one a
    /// save writes. Then the host's -- and *every* view of that buffer in the
    /// window, not the one the hand was over: a session draws a take twice (the
    /// clip in its lane, the editor under the tracks), they are one buffer,
    /// and a stroke that reached only the view under the pointer leaves the
    /// other showing samples that no longer exist anywhere. Refetching to find
    /// that out would be a round trip per gesture; the buffer number is what
    /// says which pictures are of this buffer.
    ///
    /// The write itself belongs to each element ([`widget::element::Samples::write_samples`]),
    /// because a take drawn as a clip holds samples and the same take drawn as
    /// a navigable view holds a pyramid.
    pub(super) fn write_buffer_samples(
        &mut self,
        def_id: i32,
        widget_id: i32,
        channel: usize,
        start: u64,
        values: &[f32],
    ) {
        let Some(bufnum) = self.buffer_of(def_id, widget_id) else {
            return;
        };
        // **Channel-addressed, in frames** -- the unit the picture, the gesture
        // and the document all speak. The flat `/buffer_setRange` cannot say
        // this: a channel of interleaved storage is a strided span, which is
        // why a stereo take used to be refused here.
        // **The mapped path: a store, and nothing sent.** The cells this
        // writes are the cells the engine reads on its next block -- this
        // process's engine or the RT server attached to the same segment, it
        // makes no difference to the write. What used to happen instead was a
        // blob out, a job on the server, a reply, and this host reconciling its
        // own picture with what it had just sent.
        #[cfg(unix)]
        if let Some(take) = self
            .shared_buffers
            .as_ref()
            .and_then(|m| m.map(bufnum as usize))
        {
            take.write_channel(channel, start, values);
            self.announce_write(bufnum, channel, start, values.len());
            if let Some(tree) = self.window_def_mut(def_id) {
                buffer_views(tree, bufnum, &mut |el| {
                    el.write_samples(channel, start, values)
                });
            }
            return;
        }
        let mut blob = Vec::with_capacity(values.len() * 4);
        for v in values {
            blob.extend_from_slice(&v.to_le_bytes());
        }
        if let Some(server) = self.server.as_ref()
            && let Err(e) = server.send(OscMessage {
                addr: "/buffer_setRangeChannel".into(),
                args: vec![
                    OscType::Int(bufnum),
                    OscType::Int(channel as i32),
                    OscType::Int(start as i32),
                    OscType::Blob(blob),
                ],
            })
        {
            diag::warn!("failed to write buffer {bufnum}: {e}");
            return;
        }
        let Some(tree) = self.window_def_mut(def_id) else {
            return;
        };
        if buffer_views(tree, bufnum, &mut |el| {
            el.write_samples(channel, start, values)
        }) == 0
        {
            diag::warn!("the picture refused a write the samples accepted -- they will disagree");
        }
    }

    /// Carries the sample half of an **undone or redone** write through to the
    /// samples, exactly as [`Self::write_buffer_samples`] does for a fresh one.
    ///
    /// A replayed write is complete on its own: the log holds the samples,
    /// because the hand that drew over them supplied the inverse, and the
    /// channel travels in the intent -- which is what makes an undo over one
    /// channel of a stereo take put back that channel and no other.
    pub(super) fn replay_writes(&mut self, def_id: i32, applied: &[document::Applied]) {
        let Some(owner) = self.owner.as_ref() else {
            return;
        };
        let writes: Vec<(i32, usize, u64, Vec<f32>)> = applied
            .iter()
            .filter(|a| a.applied)
            .filter_map(|a| match a.effective.as_ref()? {
                clausters_document::Intent::WriteSamples {
                    node,
                    channel,
                    start,
                    values,
                } if !values.is_empty() => owner
                    .widget_of(*node)
                    .map(|w| (w, *channel as usize, *start, values.clone())),
                _ => None,
            })
            .collect();
        for (widget_id, channel, start, values) in writes {
            if let Err(why) = self.can_write(def_id, widget_id, channel, start, values.len()) {
                diag::warn!("cannot restore {} sample(s): {why}", values.len());
                continue;
            }
            self.write_buffer_samples(def_id, widget_id, channel, start, &values);
        }
    }

    /// Writes what an edit left back onto the picture -- the *adopt* half of
    /// "drop every pending at or below the stamp, **and adopt what arrived**".
    ///
    /// A drag needs nothing from this: the gesture already moved the clip on
    /// screen, and what came back agrees with it. An **undo** is what makes it
    /// load-bearing -- the document goes back and the widget does not, so
    /// without this the picture keeps the position the hand left and the edit
    /// looks like it did nothing at all. That is exactly the shape of "the keys
    /// do nothing".
    pub(super) fn adopt(&mut self, _def_id: i32, applied: &[document::Applied]) {
        if !applied.iter().any(|a| a.applied) {
            return;
        }
        // **The multitrack is redrawn from the owner, never patched.** The
        // multitrack is one widget holding two lists, so what an applied edit
        // leaves is simply what the owner now says -- derived by the walk that
        // drew it, so the picture and the multitrack cannot disagree. It is also the
        // only thing that can adopt a *structural* edit: an undo of a lane
        // change puts a clip somewhere else entirely, and no per-widget patch
        // says that.
        //
        // There used to be a branch per intent here -- a `Place` writing an
        // offset into a `Clip` widget, a `Configure` writing a `Track`'s header
        // -- and it went with the widgets it addressed. One widget draws the
        // multitrack; one call redraws it.
        let Some(owner) = self.owner.as_ref() else {
            return;
        };
        let Some(widget) = owner.multitrack_widget() else {
            return;
        };
        let shown = owner.shown();
        // **The whole picture, and it used to be two props of it.** The
        // projection produces every key the widget draws from -- the rows, the
        // boxes, the curves, the layers, the break-points, which curves are
        // hidden and which boxes loop -- and this named `lanes` and `clips`.
        // So a multitrack that minted a curve (the header's `A`, whose entire job is
        // to make one) applied the edit, kept it in the document, and pushed
        // back nothing that draws it: the row never appeared, and the toggle
        // read as a dead key in a host with no client to answer for it.
        //
        // It is a map from the projection rather than a list written here for
        // the reason the vocabulary is asked and not restated: a key the
        // picture grows arrives without anyone remembering to pass it along.
        //
        // The effects are `Redraw`, and the front already repaints after an
        // answered gesture -- there is nothing here for a caller to carry.
        let mut fx = Vec::new();
        self.set_props(widget, shown.props.into_iter().collect(), &mut fx);
        // **And what sounds follows what is drawn**, by the same call and for
        // the same reason: the multitrack is a statement, so putting the readers
        // where it says is one verb whether it is the first time or the
        // hundredth. It reaches nodes that are already running, so a region
        // moved while the multitrack plays is heard where it was dropped with
        // nothing that is sounding cut.
        self.sound_multitrack();
    }
}
