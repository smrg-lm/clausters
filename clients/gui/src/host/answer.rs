//! **A gesture answered**: the `/gui_event` a gesture makes, delivered to a
//! client or -- when this host owns what it draws -- answered here by the
//! application that holds the window, its corrections drawn and the stamp
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
        self.owner_answered();
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
    /// A gesture on a window of an application this host holds is that
    /// application's turn, read out of the same message a client would get;
    /// anything else is a client's.
    pub fn deliver(&mut self, def_id: i32, message: &OscMessage) -> bool {
        diag::debug!(
            "deliver: window={def_id} owner={} args={:?}",
            self.owner.is_some(),
            message.args
        );
        #[cfg(any(not(target_arch = "wasm32"), feature = "apps"))]
        if self.owns_audio(def_id) {
            return self.answer_audio(def_id, message);
        }
        #[cfg(feature = "notation")]
        if self
            .owner
            .as_ref()
            .is_some_and(|o| o.scores.contains_key(&def_id))
        {
            return self.answer_score(def_id, message);
        }
        false
    }

    /// **Frees window `def_id`, as its editor asked** -- a close, by the rule
    /// every editor closes by (`clausters_apps::closing`): this host is the
    /// window's client, so it frees what it defined, and a standalone host
    /// whose last window goes ends.
    pub(super) fn free_own_window(&mut self, def_id: i32) {
        use std::net::{Ipv4Addr, SocketAddr};

        // **The windows that depend on it go first**: a roll opened from the
        // multitrack is the multitrack's, and does not outlive it
        for dependent in self.take_dependents(def_id) {
            self.free_own_window(dependent);
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

    /// **An answer the editor gave, carried out on this host**: the corrections
    /// drawn and the stamp settled -- what a client's `/gui_ack`, or its bundled
    /// push, does when it reaches a host, without the socket.
    pub(super) fn tell(&mut self, answer: clausters_editing::conversation::Answer) {
        use clausters_editing::conversation::Answer;

        let (seq, doc_version, reason, corrections) = match answer {
            Answer::Silent => return,
            Answer::Ack {
                seq,
                doc_version,
                reason,
            } => (seq, doc_version, reason, Vec::new()),
            Answer::Push {
                seq,
                doc_version,
                reason,
                corrections,
            } => (seq, doc_version, reason, corrections),
        };
        let mut fx = Vec::new();
        for correction in corrections {
            if let (Ok(id), Value::Object(props)) =
                (i32::try_from(correction.widget), correction.props)
            {
                self.set_props(id, props.into_iter().collect(), &mut fx);
            }
        }
        // **What the corrections changed is drawn now**: the redraw a set
        // asks for goes out with them. Dropped, a correction that came alone
        // (a channel folded from the menu) waited for the next frame
        // something else asked for, seconds on a still window.
        self.pending_effects.extend(fx);
        self.settle(ack::Acked {
            seq: seq as i32,
            doc_version,
            reason,
            ..Default::default()
        });
    }
}
