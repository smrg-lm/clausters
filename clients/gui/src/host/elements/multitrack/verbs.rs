//! **The verbs**: what the multitrack can be asked to do, and what it reports back.
//!
//! A verb acts on what the hand is holding and answers with the multitrack as it now
//! stands -- or with a refusal, in the words the reader needs ("these boxes are
//! already on the grid"). The box arithmetic under them is
//! `structures::boxes`, which a roll's verbs call too; what is here is what a
//! *clip* is: the name a new one is minted with, the lane a join is confined
//! to, the source window a trim moves.

use super::*;

impl Multitrack {
    /// **The edit-back: the clips as they now are.** One payload for every
    /// gesture there is -- a move, a trim, a lane crossed, a block -- because
    /// what is reported is the multitrack and not what the hand did to it.
    pub(super) fn clips_event(&self) -> Events {
        let mut args = vec![OscType::String("clips".into())];
        if let Value::Array(flat) = model::clips_json(&self.clips) {
            args.extend(flat.into_iter().map(json_arg));
        }
        Events::message(args)
    }

    /// **The edit-back for the mixer: the lanes as they now are.** The second
    /// of the two payloads, and separate from the clips for the reason they are
    /// two structures -- a fader moved must not resend every clip.
    pub(super) fn lanes_event(&self) -> Events {
        let mut args = vec![OscType::String("lanes".into())];
        if let Value::Array(flat) = model::lanes_json(&self.lanes) {
            args.extend(flat.into_iter().map(json_arg));
        }
        Events::message(args)
    }

    /// **Adds a track at `at`**, reported as the rows now stand.
    ///
    /// The name is minted here, the way a split's is: it is a name the client
    /// never said, and what makes it a *new* track to whoever reads the report
    /// is precisely that it names no track they know -- the same rule a new box
    /// travels under. The client mints the id; the host mints the word.
    pub(super) fn add_lane(&mut self, at: usize) -> Claim {
        let at = at.min(self.lanes.len());
        let height = self.lanes.first().map_or(LANE_H, |l| l.height);
        let mut made = Lane::new(self.fresh_lane_name(), height);
        // **A track a hand makes asks for no automation** *(found 2026-09-12 by
        // the user: "todas las pistas agregadas aparecen con automatizacion de
        // gain visible")*. A lane is shown-by-default because a row a *multitrack*
        // drew is a row it meant to be seen -- and a row nobody has drawn yet
        // has nothing to show, so the default said "show me this track's
        // automation" and the owner, reading that as the verb it is, made one.
        // Adding a track and adding a curve are two gestures, and the second
        // one is the `A` beside it.
        made.curves = false;
        self.lanes.insert(at, made);
        // The hand keeps hold of what it asked for, and the boxes it was
        // holding are on rows that may have moved under them.
        self.track = Some(at);
        self.selected.clear();
        Claim::Take(Take {
            events: self.lanes_event(),
            ..Take::default()
        })
    }

    /// **Removes the selected track**, and everything on it, reported as the
    /// rows now stand.
    ///
    /// The boxes go with it and nothing says so: the rows report is the multitrack's
    /// tracks, and a track that is not in it is gone with its contents. So this
    /// sends one payload where a removal per box would send two and undo in
    /// two steps.
    pub(super) fn remove_lane(&mut self) -> Option<Events> {
        let at = self.track?;
        if at >= self.lanes.len() {
            return None;
        }
        let name = self.lanes.remove(at).name;
        self.clips.retain(|c| c.lane != name);
        self.selected.clear();
        self.track = (!self.lanes.is_empty()).then(|| at.min(self.lanes.len() - 1));
        Some(self.lanes_event())
    }

    /// **What the samples behind box `n` allow an edge to do**: how many frames
    /// there are, and whether the window may run off them.
    ///
    /// A box is a **window** onto a source, so pulling an edge past what the
    /// source holds has to answer for what is there. Three cases and one rule:
    /// a box that **loops** may be pulled anywhere (past the end is the
    /// beginning again); one that does not **stops at the last frame**, because
    /// past it there is nothing to show and nothing to play; and a box over
    /// samples nobody loaded stops at nothing, since there is no length to stop
    /// at -- which is the silence the edge used to leave in every case.
    pub(super) fn contents_of(&self, n: usize) -> Contents {
        let Some(clip) = self.clips.get(n) else {
            return Contents::default();
        };
        Contents {
            total: self
                .takes
                .get(&clip.source)
                .and_then(SignalElement::sample_shape)
                .map(|(_, frames)| frames as f64),
            looping: self.wraps(&clip.name),
        }
    }

    /// **Whether box `name`'s window wraps** -- what the `loops` prop names.
    pub(super) fn wraps(&self, name: &str) -> bool {
        self.loops.iter().any(|n| n == name)
    }

    /// A name no lane here has yet -- a word, since the client's own names are
    /// ids and a word can never be mistaken for one.
    pub(super) fn fresh_lane_name(&self) -> String {
        let mut n = 1;
        loop {
            let name = format!("track {n}");
            if !self.lanes.iter().any(|l| l.name == name) {
                return name;
            }
            n += 1;
        }
    }

    /// A name no clip here has yet, derived from `base` -- what a **split** needs
    /// and what a **paste** needs.
    ///
    /// The identity is the client's word, and a split makes one the client never
    /// said. So the host mints it the way it mints a marker's number, from the
    /// name that was there, and it comes back in the report as any other name
    /// does: the script learns it by being told, not by guessing a rule.
    pub(super) fn fresh_name(&self, base: &str) -> String {
        let mut n = 2;
        loop {
            let name = format!("{base} {n}");
            if self.clip(&name).is_none() {
                return name;
            }
            n += 1;
        }
    }

    /// The clip of this name, if it is here.
    pub(super) fn clip(&self, name: &str) -> Option<&Clip> {
        self.clips.iter().find(|c| c.name == name)
    }

    /// **Cut every held clip at `at`**, keeping the halves in the hand.
    ///
    /// The window over the contents moves with the cut -- `boxes::split_at`
    /// is the arithmetic, the same one a note's split uses -- so the second half
    /// reads on from where the first stopped rather than from the source's
    /// start.
    pub(super) fn split_held(&mut self, at: f64) -> bool {
        let held = self.selected.clone();
        let cut = boxes::split(self, &held, at);
        if cut.is_empty() {
            return false;
        }
        // The heads were already in hand; what the cut adds is the tails.
        self.selected
            .extend(cut.into_iter().filter(|i| !held.contains(i)));
        true
    }

    /// **Ask for the held boxes to be joined.**
    ///
    /// A join is the one verb here that is *asked for* rather than performed.
    /// Every other one edits the picture and reports it, and what it meant is
    /// read back out of the difference -- but a join and a "delete one, lengthen
    /// the other" leave a lane holding exactly the same thing, and a box in a
    /// `clips` report names **one** source and one start, so fragments joined
    /// into one box have no report that describes them.
    ///
    /// What is missing when they do not read on from each other is the
    /// **source** they would be a window onto, and making one is the document's
    /// (`clausters_document::multitrack::picture::read_join`): it knows what
    /// each box reads, this only knows what each box is called. So the widget
    /// says which boxes, and the answer comes back as the picture that now
    /// holds -- including the refusals, which are about the material rather than
    /// about the picture and which this could not have made.
    pub(super) fn join_event(&self) -> Events {
        let mut args = vec![OscType::String("join".into())];
        args.extend(
            self.selected
                .iter()
                .filter_map(|i| self.clips.get(*i))
                .map(|clip| OscType::String(clip.name.clone())),
        );
        Events::message(args)
    }

    /// **The edit-back for the curves: every break-point of every one of
    /// them.** The third of the payloads, and the whole list for the same
    /// reason the other two are whole: applying what came back is the identity,
    /// and its own inverse is the list that was there.
    pub(super) fn points_event(&self) -> Events {
        Events::message(self.points_args())
    }

    /// The `"points"` payload's arguments, so a press that both moves the layer
    /// and edits it reports two messages rather than choosing one.
    pub(super) fn points_args(&self) -> Vec<OscType> {
        let mut args = vec![OscType::String("points".into())];
        for curve in self.curves.iter().chain(&self.layers) {
            let Some(body) = self.bodies.get(&curve.name) else {
                continue;
            };
            for p in body.points() {
                args.push(OscType::String(curve.name.clone()));
                args.push(OscType::Double(p.time));
                args.push(OscType::Float(p.value));
                args.push(OscType::Int(p.shape));
                args.push(OscType::Float(p.curve));
            }
        }
        args
    }

    /// The `points` prop as a `/gui_set` would take it: every break-point of
    /// every curve, each naming the curve it is on.
    pub(super) fn points_json(&self) -> Value {
        let mut out = Vec::new();
        for curve in self.curves.iter().chain(&self.layers) {
            let Some(body) = self.bodies.get(&curve.name) else {
                continue;
            };
            for p in body.points() {
                out.push(Value::from(curve.name.clone()));
                out.push(Value::from(p.time));
                out.push(Value::from(p.value));
                out.push(Value::from(p.shape));
                out.push(Value::from(p.curve));
            }
        }
        Value::Array(out)
    }

    /// **The layer the hand is on**, reported when a press moved it.
    pub(super) fn layer_args(&self) -> Vec<OscType> {
        vec![
            OscType::String("layer".into()),
            OscType::String(
                self.layer
                    .clone()
                    .unwrap_or_else(|| "placement".to_string()),
            ),
        ]
    }

    /// The break-points of a curve as one comparable value -- what says on
    /// release whether the gesture changed anything, since a drag that came
    /// back to where it began is not an edit.
    pub(super) fn points_of(&self, name: &str) -> Value {
        self.bodies
            .get(name)
            .map(|b| crate::host::structures::points::points_json(b.points()))
            .unwrap_or(Value::Null)
    }

    /// The bodies a new list of curves gets: **the elements that survive keep
    /// their points**, so renaming a lane or adding a curve does not flatten
    /// the ones that were already drawn.
    pub(super) fn rebuilt(
        &self,
        rows: &[model::Curve],
        layers: &[model::Curve],
    ) -> HashMap<String, crate::host::elements::curve::Curve> {
        let kept = rows
            .iter()
            .chain(layers)
            .filter_map(|c| {
                let flat = self
                    .bodies
                    .get(&c.name)?
                    .points()
                    .iter()
                    .flat_map(|p| {
                        [
                            p.time,
                            f64::from(p.value),
                            f64::from(p.shape),
                            f64::from(p.curve),
                        ]
                    })
                    .collect();
                Some((c.name.clone(), flat))
            })
            .collect();
        curve_bodies(rows, layers, &kept)
    }
}
