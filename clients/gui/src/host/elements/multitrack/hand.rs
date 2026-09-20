//! **The hand**: what a press took, what a drag is doing, and what a key means.
//!
//! The whole interaction of the multitrack, in one place because a gesture is
//! read a phase at a time: what the press claimed (`Grab`, `Sizing`, `Fading`,
//! the held block), what the motion writes, and what the release reports. The
//! verbs a key spells are `verbs`; this is what decides that a key meant one.
//!
//! **Provisional, and it says so**: which letters these are and which modifier
//! does what is not settled -- see `clients/gui/PLAN.md`, "The whole
//! interaction vocabulary is provisional".

use super::*;

/// **What the hand took at the press, kept until it lets go.**
///
/// The snapshotted form of drag ([`crate::host::widget::element::Take`] says
/// why there are three): a press-time origin plus the axis and the grid, so a
/// clamped edge never drifts and a drag that comes back to where it began is
/// exactly where it began.
#[derive(Debug, Clone, Copy)]
pub(super) struct Grab {
    /// The clip the hand has, by index.
    pub(super) clip: usize,
    /// Which part of it -- the body, or one of the two edges.
    pub(super) part: Part,
    /// Where it sat when the press landed.
    pub(super) orig: Placement,
    /// The lane it was on when the press landed, by index.
    pub(super) lane: usize,
    /// The pointer's time at the press, so a body drag moves by the travel
    /// rather than by where inside the box the hand grabbed it.
    pub(super) grabbed_at: f64,
    /// **The axis the press found**, for a widget on no navigation group.
    ///
    /// Such a widget's axis is its own extent, and a drag *changes* the extent
    /// -- so re-deriving it per frame stretches the pixel-to-time map under the
    /// hand, the next step reads further, and the box runs away from the
    /// pointer. On a group the axis is read live instead, because there it is
    /// the group's and pans under the drag on purpose ([`Take::edge_scroll`]).
    pub(super) axis: View,
}

/// The row a hand is resizing, and what it was when the press landed.
#[derive(Debug, Clone, Copy)]
pub(super) struct Sizing {
    pub(super) lane: usize,
    pub(super) from: f32,
    pub(super) at: f64,
}

/// A **level knob** the hand is on, kept for the same reason a clip's grab is:
/// the value is read from the pointer against what the press found.
#[derive(Debug, Clone, Copy)]
pub(super) struct Fading {
    pub(super) lane: usize,
    /// The knob's own cell -- how far a full turn is, in pixels.
    pub(super) cell: Rect,
    /// The level the press found: a turn is measured from it.
    pub(super) from: f32,
    /// The y the press landed at, which the drag is measured against.
    pub(super) at: f64,
}

/// The block a hand took, as `(index, offset, row)` per clip -- the snapshot
/// `boxes::move_block` clamps against, so a block stopped at an edge does
/// not fold against it.
pub(super) type Block = Vec<(usize, f64, f32)>;
impl Multitrack {
    /// The clip under `(x, y)`, and which part of it -- **the topmost first**,
    /// since a later clip is drawn over an earlier one and the eye takes the
    /// one it can see.
    pub(super) fn clip_at(&self, input: &Input, at: (f64, f64)) -> Option<(usize, Part)> {
        let i = self.lane_at(input.rect, at.1)?;
        let rect = self.lane_rects(input.rect)[i];
        // A band carries its gap, and nothing of a lane is drawn there: a press
        // in it is a press on bare stack, which the container sweeps.
        if (at.1 as f32) >= rect.y + rect.h {
            return None;
        }
        let body = track::lane_body(rect, false, input.indent, input.metrics);
        let nav = self.view(input.time);
        self.clips
            .iter()
            .enumerate()
            .rev()
            .filter(|(_, c)| c.lane == self.lanes[i].name)
            .find_map(|(n, c)| {
                let (x0, x1) = stack::clip_x(c, body, &nav, MIN_CLIP_W)?;
                let cr = track::clip_rect(body, x0, x1);
                // **A grip is hit on the pixels it was drawn on.** The same
                // call the drawing made, so the handle and its hit area cannot
                // disagree -- the case nobody tests.
                let local = track::clip_local_view(body, &nav, c.place.offset, c.place.dur, cr);
                let ends = track::clip_ends_on_screen(&local, c.place.dur);
                if let Some((_, side)) = track::clip_grip_at(cr, ends, input.metrics, at.0 as f32) {
                    return Some((
                        n,
                        match side {
                            track::ClipSide::Start => Part::Start,
                            track::ClipSide::End => Part::End,
                        },
                    ));
                }
                let inside = at.0 as f32 >= x0 && at.0 as f32 <= x1;
                inside.then_some((n, Part::Body))
            })
    }

    /// The clips the hand is holding: the selection when the grabbed clip is in
    /// it, else the grabbed one alone.
    ///
    /// **Grabbing an unselected clip lets go of the block**, which is the rule a
    /// lane already had: a hand that reaches past its selection meant the box it
    /// reached for.
    pub(super) fn held(&self, clip: usize) -> Vec<usize> {
        if self.selected.contains(&clip) {
            self.selected.clone()
        } else {
            vec![clip]
        }
    }

    /// A press on a lane's header: the two toggles land on the press, the fader
    /// takes the drag.
    ///
    /// **The mixer state is the document's**, so all three report -- and they
    /// report the `"lanes"` list, not the one lane, because what a report says
    /// here is the multitrack as it now stands.
    pub(super) fn press_header(
        &mut self,
        lane: usize,
        part: track::HeaderPart,
        at: (f64, f64),
        input: &Input,
    ) -> Claim {
        let rect = self.lane_rects(input.rect)[lane];
        let band = crate::host::timeline::gutter_band(rect, input.indent);
        let parts = track::header_parts(
            band,
            &self.header(&self.lanes[lane], input.indent),
            input.metrics,
        );
        match part {
            track::HeaderPart::Mute => {
                self.lanes[lane].mute = !self.lanes[lane].mute;
                Claim::Take(Take {
                    events: self.lanes_event(),
                    ..Take::default()
                })
            }
            track::HeaderPart::Solo => {
                self.lanes[lane].solo = !self.lanes[lane].solo;
                Claim::Take(Take {
                    events: self.lanes_event(),
                    ..Take::default()
                })
            }
            // **Show or hide this track's automation rows.** A statement about
            // the track, so it rides the `lanes` report the mute and the solo
            // beside it ride, and the owner answers it by saying which curves
            // are visible -- which is where that fact lives.
            track::HeaderPart::Curves => {
                let showing = !self.lanes[lane].curves;
                self.lanes[lane].curves = showing;
                // **And the rows go with it, here** *(found 2026-09-12 by the
                // user: "la A sigue sin ocultar ni mostrar")*. What `stack`
                // draws from is `hidden`, which is the owner's answer, and the
                // owner answers with the picture only when a **name** changes.
                // Hiding changes none -- the curve is still there, it is not
                // shown -- so a press that only flipped this flag changed
                // nothing anybody draws from, and the row stayed exactly where
                // it was. The first press on a bare track looked like it worked
                // because it *mints* a curve, and a new name is answered.
                //
                // So the flag and `hidden` are one fact, and the press states
                // it in both: the picture moves under the hand the way a
                // dragged clip does, and the owner's `hidden` confirms it on
                // the next correction.
                let named: Vec<String> = self
                    .curves
                    .iter()
                    .filter(|c| c.owner == self.lanes[lane].name)
                    .map(|c| c.name.clone())
                    .collect();
                if showing {
                    self.hidden.retain(|h| !named.contains(h));
                } else {
                    for name in named {
                        if !self.hidden.contains(&name) {
                            self.hidden.push(name);
                        }
                    }
                }
                Claim::Take(Take {
                    events: self.lanes_event(),
                    ..Take::default()
                })
            }
            track::HeaderPart::Level => {
                let Some(cell) = parts.level else {
                    return Claim::Decline;
                };
                // **Relative**: a knob turns by the distance a drag travels,
                // and that is not a detail of the drawing -- a dial has no left
                // and right end to put the pointer between, so an absolute
                // reading would jump the value to wherever the press landed.
                // The press itself changes nothing; what it takes is the level
                // it found and the pixel it found it at.
                self.fading = Some(Fading {
                    lane,
                    cell,
                    from: self.lanes[lane].gain,
                    at: at.1,
                });
                Claim::take()
            }
            // **The bottom edge is the row's own height** -- the vertical zoom
            // of one track, which is what a hand reaches for when one take
            // needs to be read closely and the rest do not. Screen state, like
            // the scroll: nothing on the wire sets or reports it.
            track::HeaderPart::Edge => {
                self.sizing = Some(Sizing {
                    lane,
                    from: self.lanes[lane].height,
                    at: at.1,
                });
                Claim::take()
            }
            // **The space beside the controls is the track itself.** A click
            // selects it -- the second coordinate a paste needs -- and a double
            // click makes one, the gesture a desktop already spends on "open
            // this" spent here on "make one", which is what a stack has no
            // other way to ask for. The new track goes **after** the one that
            // was pointed at, which is where a hand asking for one from this
            // row means it.
            track::HeaderPart::Body => {
                if input.clicks >= 2 {
                    return self.add_lane(lane + 1);
                }
                self.track = Some(lane);
                Claim::take()
            }
        }
    }

    /// The gesture a curve reads its own geometry from: the rectangle it was
    /// drawn in, and the axis it was drawn against. A body has no gutter of its
    /// own -- the header band is the multitrack's -- so the indent is zero.
    pub(super) fn on_curve<'a>(input: &Input<'a>, rect: Rect, space: TimeSpace) -> Input<'a> {
        Input {
            rect,
            indent: 0.0,
            time: Some(space),
            ..*input
        }
    }

    /// Where a curve by name was drawn, and the space it was drawn against --
    /// the geometry a gesture on it is read with, taken from the one answer the
    /// drawing used.
    pub(super) fn curve_place(&self, name: &str, input: &Input) -> Option<(Rect, TimeSpace)> {
        self.curves_on_screen(input.rect, input.indent, input.metrics, input.time)
            .into_iter()
            .find(|(n, ..)| *n == name)
            .map(|(_, rect, space)| (rect, space))
    }

    /// **A press on a curve's own contents** -- a break-point, or the line
    /// between two of them -- never on the rectangle it shares with what is
    /// under it.
    ///
    /// That is what leaves the background to the container: a press on a box's
    /// empty pixels moves the box and takes the hand off the envelope drawn
    /// across it. **The active layer is asked first**, so what is already in
    /// hand keeps the pixels it draws on.
    pub(super) fn curve_at(&self, at: (f64, f64), input: &Input) -> Option<String> {
        let drawn = self.curves_on_screen(input.rect, input.indent, input.metrics, input.time);
        let ask = |name: &str, rect: Rect, space: TimeSpace| {
            let body = self.bodies.get(name)?;
            body.layer_hit(at, &Self::on_curve(input, rect, space))
                .then(|| name.to_string())
        };
        drawn
            .iter()
            .find(|(name, ..)| self.layer.as_deref() == Some(*name))
            .and_then(|&(name, rect, space)| ask(name, rect, space))
            .or_else(|| {
                drawn
                    .iter()
                    .rev()
                    .find_map(|&(name, rect, space)| ask(name, rect, space))
            })
    }

    /// A press the curves answered, or `None` for one none of them wanted.
    ///
    /// The layer moves to whatever was pressed and is reported once; the edit
    /// itself leaves on release, as every gesture here does. What the curve
    /// reports for itself is dropped: its payload is its own points, and the
    /// payload here is **every** curve's, so forwarding one would hand an owner
    /// a list that is not the multitrack.
    pub(super) fn press_curve(&mut self, at: (f64, f64), input: &Input) -> Option<Claim> {
        let name = self.curve_at(at, input)?;
        let (rect, mut space) = self.curve_place(&name, input)?;
        let moved = self.layer.as_deref() != Some(name.as_str());
        // The press is read as the active layer's, since that is what it just
        // became -- the curve offers a segment's bend only when it is in hand.
        space.active = true;
        self.layer = Some(name.clone());
        let before = self.points_of(&name);
        let sub = Self::on_curve(input, rect, space);
        let claim = self.bodies.get_mut(&name)?.press(at, &sub);
        let Claim::Take(take) = claim else {
            // The curve wanted none of it after all: the press goes on to the
            // box, and the layer it moved to stays where it moved.
            return moved.then(|| Claim::events(Events::message(self.layer_args())));
        };
        self.holding = Some((name.clone(), before.clone()));
        let mut events = Events::none();
        if moved {
            events = events.and(self.layer_args());
        }
        if self.points_of(&name) != before {
            events = events.and(self.points_args());
        }
        Some(Claim::Take(Take {
            events,
            edge_scroll: true,
            ..take
        }))
    }
}

impl Multitrack {
    /// **A press takes a clip or a header control, and declines everywhere
    /// else.** The slack between clips and beside them is the container's --
    /// that is where a click places the transport's cursor and a sweep starts a
    /// marquee -- so a press that found neither goes back to the chain rather
    /// than being swallowed.
    pub(super) fn press_at(&mut self, at: (f64, f64), input: &Input) -> Claim {
        self.grab = None;
        self.fading = None;
        self.sizing = None;
        self.holding = None;
        self.block.clear();
        // The header band first: it is drawn over the gutter, and nothing of
        // the axis is there.
        if let Some((lane, part)) = self.header_at(input, at) {
            return self.press_header(lane, part, at, input);
        }
        if at.0 < f64::from(input.rect.x + input.indent) {
            // **An automation row's header is the track's picture, not the
            // track.** A curve is drawn in a row of its own under the lane it
            // belongs to, and the band beside it is that row's label -- so a
            // press there addresses no track: it selects none, lets go of none
            // and asks for none. The press is consumed rather than declined,
            // because the header band is this widget's whatever is drawn in it.
            if matches!(self.row_kind(input, at.1), Some(stack::Row::Curve(_))) {
                return Claim::take();
            }
            // **The band under the last header**, where there is no track to
            // point at -- so nothing else could be meant by a double click
            // there than *make one*, and it goes at the end. A single click
            // lets go of the track the hand had, the way a click on bare stack
            // lets go of the boxes.
            if input.clicks >= 2 {
                return self.add_lane(self.lanes.len());
            }
            self.track = None;
            return Claim::take();
        }
        // **A press selects the layer it lands on**, and what lands on a curve
        // is its own points and the line between them -- never the rectangle it
        // shares with the box under it. So an envelope drawn across a box
        // leaves that box draggable by every pixel the line is not on.
        if let Some(claim) = self.press_curve(at, input) {
            return claim;
        }
        let Some((clip, part)) = self.clip_at(input, at) else {
            return Claim::Decline;
        };
        // **A box is entered to edit it**, and entering is a double click --
        // the gesture a desktop already spends on "open this". What leaves is
        // the box's name and nothing else: which editor that box asks for is a
        // question about its *contents*, and this widget owns where things are
        // rather than what is inside them.
        if input.clicks >= 2 {
            return Claim::events(Events::message(vec![
                OscType::String("enter".into()),
                OscType::String(self.clips[clip].name.clone()),
            ]));
        }
        // **Alt adds or removes that one**, the same key that adds a note to a
        // roll's selection. A plain click selects it alone, and that is decided
        // on release (see [`Element::release`]): a press is not yet a gesture.
        if input.mods.alt {
            boxes::toggle_selected(&mut self.selected, clip);
            return Claim::take();
        }
        let Some(lane) = self.lane_of(&self.clips[clip]) else {
            return Claim::Decline;
        };
        // **An edge is always one clip's**: two clips of different lengths have
        // no one edge to pull, so a trim lets go of the block.
        self.block = match part {
            Part::Body => self
                .held(clip)
                .into_iter()
                .map(|i| (i, self.clips[i].place.offset, self.row(i)))
                .collect(),
            _ => vec![(clip, self.clips[clip].place.offset, lane as f32)],
        };
        if part != Part::Body && !self.selected.contains(&clip) {
            self.selected.clear();
        }
        self.grab = Some(Grab {
            clip,
            part,
            orig: self.clips[clip].place,
            lane,
            grabbed_at: self.time_at(input, at.0),
            axis: self.view(input.time),
        });
        Claim::Take(Take {
            // Held past the edge of the axis, the machine keeps ticking and
            // pans the group under the hand -- a clip dragged off the right of
            // the window has to keep moving, and a held cursor sends nothing.
            edge_scroll: true,
            ..Take::default()
        })
    }
}

impl Multitrack {
    /// **What a rectangle swept over the stack caught.** The marquee's one
    /// question, answered with the clips the rectangle covered -- of every lane
    /// it crossed, since a selection the stack's sweep made is not one lane's.
    pub(super) fn swept(&mut self, from: (f64, f64), to: (f64, f64), input: &Input) -> Swept {
        let before = self.selected.len();
        let (t0, t1) = (self.time_at(input, from.0), self.time_at(input, to.0));
        // The same continuous answer a drag takes: a corner in a gap or past
        // an end still means the sweep passed through those lanes.
        let r0 = self.lane_toward(input.rect, from.1) as f32;
        let r1 = self.lane_toward(input.rect, to.1) as f32;
        self.selected = boxes::in_rect(self, t0, t1, r0, r1);
        Swept {
            changed: before != self.selected.len() || !self.selected.is_empty(),
            // **No band.** A multitrack's second axis is the stack of lanes,
            // not a value, so a rectangle over it restricts no value range --
            // the vertical half said *which clips*, and nothing else.
            band: None,
        }
    }
}

impl Multitrack {
    /// The clips follow the hand; **the edit leaves on release.**
    ///
    /// One gesture is one edit -- a placement per frame would be an undo step
    /// per frame, and a round trip whose acknowledgement the next frame
    /// outruns. What moves here is the picture. The **fader** is the exception
    /// and is not one: it is a control, its value *is* what the hand is doing,
    /// and it reports as it goes exactly as every other control does.
    pub(super) fn dragged(&mut self, at: (f64, f64), input: &Input) -> Events {
        // A curve in hand follows it, and reports nothing on the way: what it
        // says for itself is one curve's points, and one gesture is one edit.
        if let Some((name, _)) = self.holding.clone()
            && let Some((rect, space)) = self.curve_place(&name, input)
        {
            let sub = Self::on_curve(input, rect, space);
            if let Some(body) = self.bodies.get_mut(&name) {
                body.drag(at, &sub);
            }
            return Events::none();
        }
        if let Some(s) = self.sizing {
            let height = (s.from + (at.1 - s.at) as f32).clamp(MIN_LANE_H, MAX_LANE_H);
            self.lanes[s.lane].height = height;
            self.zoom.insert(self.lanes[s.lane].name.clone(), height);
            return Events::none();
        }
        if let Some(f) = self.fading {
            self.lanes[f.lane].gain = track::level_after(f.from, at.1 - f.at, f.cell);
            return self.lanes_event();
        }
        let Some(grab) = self.grab else {
            return Events::none();
        };
        let now = self.time_at(input, at.0);
        match grab.part {
            // **A block travels in time and across the stack, rigidly.** The
            // deltas are clamped as one, so a block stopped at an edge does not
            // fold against it, and no clip is resized.
            Part::Body => {
                let dt = boxes::snap(now - grab.grabbed_at, self.snap);
                let dr = self.lane_toward(input.rect, at.1) as f32 - grab.lane as f32;
                // **The grabbed box's own two edges look for a neighbour.** The
                // box under the hand is what the hand is aiming with, so it is
                // the one that snaps; the rest of a block travels with it, as
                // it does for everything else a block drag does.
                let orig = grab.orig;
                let dt = dt
                    + self.pull_to_edge(
                        input,
                        self.row(grab.clip) + dr,
                        &self.block.iter().map(|&(i, ..)| i).collect::<Vec<_>>(),
                        &[orig.offset + dt, orig.offset + orig.dur + dt],
                    );
                let rows = (0.0, self.lanes.len().saturating_sub(1) as f32);
                let block = std::mem::take(&mut self.block);
                boxes::move_block(self, &block, dt, dr, rows, None);
                self.block = block;
            }
            // **An edge is one clip's**, and it trims: the placement and the
            // window over the contents move together.
            part => {
                // An edge looks for a neighbour too: that is how a gap is
                // closed by trimming rather than by moving.
                let now = now + self.pull_to_edge(input, self.row(grab.clip), &[grab.clip], &[now]);
                let contents = self.contents_of(grab.clip);
                self.clips[grab.clip].place =
                    boxes::drag(part, now, grab.orig, contents, self.bounds());
            }
        }
        Events::none()
    }
}

impl Multitrack {
    /// **A gesture that changed nothing is not an edit.** A press and a release
    /// with nothing in between is a click, and a drag that came back to where
    /// it began is the same thing by another road: reporting it would hand the
    /// owner an intent to apply and a document an entry to undo, so looking at
    /// four clips would cost four undos.
    pub(super) fn released(&mut self, at: (f64, f64), inside: bool, input: &Input) -> Events {
        if let Some((name, before)) = self.holding.take() {
            if let Some((rect, space)) = self.curve_place(&name, input) {
                let sub = Self::on_curve(input, rect, space);
                if let Some(body) = self.bodies.get_mut(&name) {
                    body.release(at, inside, &sub);
                }
            }
            return if self.points_of(&name) == before {
                Events::none()
            } else {
                self.points_event()
            };
        }
        if self.sizing.take().is_some() {
            // Nothing leaves: how tall a row is drawn is this window's, and the
            // multitrack is not asked about it.
            return Events::none();
        }
        if self.fading.take().is_some() {
            // Already reported on the way, like any other control.
            return Events::none();
        }
        let Some(grab) = self.grab.take() else {
            return Events::none();
        };
        let block = std::mem::take(&mut self.block);
        let moved = block
            .iter()
            .any(|&(i, offset, row)| self.clips[i].place.offset != offset || self.row(i) != row)
            || self.clips[grab.clip].place != grab.orig;
        if !moved {
            // **A press that moved nothing is a click, and a click selects the
            // box it landed on** -- alone, whatever was held before, which is
            // what makes a hand able to point at one clip and then act on it
            // (place the cursor, split it, delete it). Alt is still the
            // additive one, and it answered at the press.
            //
            // It is decided here rather than at the press because a press is
            // not yet a gesture: the same movement is a click or a drag
            // depending on what happens next, and collapsing the selection at
            // the press would let go of a block the hand was about to move.
            //
            // Nothing leaves: a selection is the hand's, not the document's.
            self.selected = vec![grab.clip];
            return Events::none();
        }
        self.clips_event()
    }
}

impl Multitrack {
    /// **The stack's own vertical gestures**: scroll it, and zoom one row.
    ///
    /// The plain wheel stays the **time axis'**, which is what it is over every
    /// timeline view here and what a hand reaching for a wheel over an
    /// arrangement means most of the time. The two this adds are the ones the
    /// stack has and the axis does not:
    ///
    /// - `Shift` **scrolls the stack**, so a track that fell off the bottom is
    ///   reachable. The scroll is clamped to what there is to see -- a stack
    ///   that fits does not move at all, and one that does not cannot be pushed
    ///   past its last row, which is the difference between a scroll and a
    ///   surface that can be lost.
    /// - `Ctrl` **zooms the row under the cursor**, a lane or an automation row
    ///   alike and each on its own. The bottom-edge drag already zooms a lane
    ///   and is the better gesture for one; this is the one that reaches a
    ///   curve row, which has no edge to pull, and it is how one row is read
    ///   closely while the rest stay where they are.
    ///
    /// **A facility, and it says so.** These are here because the example needs
    /// to reach a stack taller than its window, and which keys they are is not
    /// settled -- see `clients/gui/PLAN.md`, "The whole interaction vocabulary is
    /// provisional" and "A shortcut is the application's, not the widget's".
    pub(super) fn wheeled(
        &mut self,
        at: (f64, f64),
        delta: (f64, f64),
        input: &Input,
    ) -> Option<Events> {
        let steps = if delta.1 != 0.0 { delta.1 } else { delta.0 };
        if steps == 0.0 {
            return None;
        }
        if input.mods.ctrl {
            let row = self.row_kind(input, at.1)?;
            // Up zooms in, which is the direction every other zoom here takes.
            let factor = 1.1f32.powf(steps as f32);
            match row {
                stack::Row::Lane(i) => {
                    let lane = self.lanes.get_mut(i)?;
                    lane.height = (lane.height * factor).clamp(MIN_LANE_H, MAX_LANE_H);
                    self.zoom.insert(lane.name.clone(), lane.height);
                }
                stack::Row::Curve(n) => {
                    let curve = self.curves.get_mut(n)?;
                    curve.height = (curve.height * factor).clamp(MIN_CURVE_H, MAX_LANE_H);
                    self.curve_zoom.insert(curve.name.clone(), curve.height);
                }
            }
            return Some(Events::none());
        }
        if input.mods.shift {
            // **Taken whether it moves or not** *(found 2026-09-12 by the user:
            // "cuando llega al limite pasa a hacer zoom temporal")*. Passing an
            // unusable wheel on is the right rule for a *surface* under the
            // pointer, which is why the scroll plane behind this one keeps it --
            // and it is the wrong one for a **modifier**, which is an address
            // rather than a place. Shift said *the stack*, so a stack already
            // at its end answers by doing nothing: reaching the last track and
            // having the multitrack zoom under the hand is the gesture turning into
            // a different gesture at the moment the hand leans on it.
            self.scroll = self.clamped_scroll(input.rect, self.scroll - steps as f32 * WHEEL_ROWS);
            return Some(Events::none());
        }
        None
    }
}

impl Multitrack {
    /// The verbs a hand has over what it is holding.
    ///
    /// `q` quantizes onto the lane's own `snap` grid -- the grid a drag already
    /// lands on -- `e` splits at the window's cursor and `j` joins a touching
    /// run, Delete removes (the **selected track**, with everything on it, when
    /// no box is held), and `Ctrl`+`C`/`X`/`V` move a block through the
    /// host-wide clipboard. All of them act on **the held set**, across the
    /// stack, and all of them report the clips as they now stand: there is one
    /// payload here and a verb does not get to invent a second.
    ///
    /// **A verb that finds nothing to act on says so**, out loud, in the same
    /// `"refused" <verb> <why>` an unwritable body already answers a press
    /// with. A hand holding one box, two halves put back in the other order, a
    /// selection already on the grid: all of them were correct and silent, and
    /// a correct refusal nobody is told about is indistinguishable from a key
    /// that does not work.
    ///
    /// **The letters are the ones a clip already answered to on a lane.** Which
    /// keys they are is not settled -- see `clients/gui/PLAN.md`, "A shortcut is
    /// the application's, not the widget's".
    pub(super) fn keyed(&mut self, key: &Key, input: &mut KeyInput) -> Option<Events> {
        // **Delete acts on what is in hand, and a track can be in hand.** The
        // header is what puts one there, so with a track selected Delete is the
        // track's -- it and everything on it -- and with none it is the held
        // boxes', which is what it has always been. The ordinary rule, and the
        // reason the selected track is not merely decoration.
        if matches!(key, Key::Delete | Key::Backspace)
            && self.selected.is_empty()
            && self.track.is_some()
        {
            return self.remove_lane();
        }
        if self.selected.is_empty() && !matches!(key, Key::Char('v') | Key::Char('V')) {
            return None;
        }
        match key {
            Key::Char('q') | Key::Char('Q') if !input.mods.ctrl => {
                let held = self.selected.clone();
                Some(if boxes::quantize(self, &held, self.snap) {
                    self.clips_event()
                } else {
                    Events::refused("quantize", "these boxes are already on the grid")
                })
            }
            // **At the window's cursor**: a key gesture has no pointer to read a
            // position from, and the window has one cursor for exactly that.
            Key::Char('e') | Key::Char('E') if !input.mods.ctrl => {
                let at = boxes::snap(input.cursor.unwrap_or(0.0), self.snap).max(0.0);
                Some(if self.split_held(at) {
                    self.clips_event()
                } else {
                    Events::refused("split", "the cursor is not inside a held box")
                })
            }
            Key::Char('j') | Key::Char('J') if !input.mods.ctrl => Some(self.join_event()),
            Key::Delete | Key::Backspace => {
                let held = std::mem::take(&mut self.selected);
                boxes::discard(self, &held).then(|| self.clips_event())
            }
            // The clipboard is the host's one string, so a block travels between
            // multitracks and windows -- and rides it in the same JSON form a
            // `/gui_set clips` accepts, which is the carrier every non-scalar
            // here uses.
            Key::Char('c') | Key::Char('C') | Key::Char('x') | Key::Char('X')
                if input.mods.ctrl =>
            {
                let block: Vec<Clip> = self
                    .selected
                    .iter()
                    .filter_map(|&i| self.clips.get(i).cloned())
                    .collect();
                if block.is_empty() {
                    return None;
                }
                input
                    .clipboard
                    .set_text(&model::clips_json(&block).to_string());
                if !matches!(key, Key::Char('x') | Key::Char('X')) {
                    // A copy changed nothing, so it reports nothing -- but it
                    // consumed the key.
                    return Some(Events::none());
                }
                let held = std::mem::take(&mut self.selected);
                boxes::discard(self, &held);
                Some(self.clips_event())
            }
            Key::Char('v') | Key::Char('V') if input.mods.ctrl => {
                let mut props = Map::new();
                props.insert(
                    "clips".into(),
                    serde_json::from_str(&input.clipboard.text()).ok()?,
                );
                let block = parse_clips(&props);
                if block.is_empty() {
                    return None;
                }
                // **At the cursor**, and keeping the block's own shape: the
                // earliest pasted clip lands there and the rest keep their
                // distances, which is what makes a pasted block the same block.
                let at = boxes::snap(input.cursor.unwrap_or(0.0), self.snap).max(0.0);
                let offsets: Vec<f64> = block.iter().map(|c| c.place.offset).collect();
                let placed = boxes::rebased(&offsets, at)?;
                // **A paste needs two coordinates**, and the second is the
                // selected track: the position cursor says *when* and the
                // header says *where*. A block pasted onto a track is the same
                // block, so what is kept is its shape and not the row numbers
                // it was cut from -- and the two coordinates anchor it at
                // **two different boxes**, which is the part that was wrong.
                //
                // In time the anchor is the **earliest** box: that is what "it
                // starts here" means on an axis that runs one way. In rows it
                // is the **topmost**, and for the same reason: a track selected
                // for a paste is where the block *begins*, so everything lands
                // on it or below it, keeping whatever gaps the block had. Rows
                // 2, 4 and 1 pasted onto track 3 are tracks 4, 6 and 3.
                //
                // Anchoring the rows at the earliest box instead -- which is
                // what this did -- made the paste follow a rule nobody could
                // state: which track the block landed on depended on which of
                // its boxes happened to be first in *time*, so the same block
                // pasted onto the same track went up or down according to the
                // order it was recorded in, and part of it landed above the
                // track the hand had pointed at.
                //
                // With no track selected the rows are the ones it came from,
                // which is what a paste back into the same multitrack means.
                let rows: Vec<usize> = block.iter().map(|c| self.lane_of(c).unwrap_or(0)).collect();
                let base = rows.iter().copied().min().unwrap_or(0);
                let depth = rows.iter().copied().max().unwrap_or(0) - base;
                let onto = self.track.unwrap_or(base);
                // **A block that does not fit is refused, not flattened.** It
                // used to clamp every row past the last track onto that track,
                // which silently made a block of four tracks into a pile on
                // one -- the one thing a paste promises not to do. The multitrack
                // gains no track here either: making one is a verb of its own
                // (a double click on a header), reported as `lanes`, and a
                // paste is not the place to grow the thing it is pasting into.
                if onto + depth >= self.lanes.len() {
                    let need = onto + depth + 1;
                    return Some(Events::refused(
                        "paste",
                        &format!(
                            "this block is {} track(s) tall and needs {need} here; the multitrack has {}",
                            depth + 1,
                            self.lanes.len()
                        ),
                    ));
                }
                self.selected.clear();
                for (i, mut clip) in block.into_iter().enumerate() {
                    clip.place.offset = placed[i];
                    let row = rows[i] - base + onto;
                    if let Some(lane) = self.lanes.get(row) {
                        clip.lane = lane.name.clone();
                    }
                    clip.name = self.fresh_name(&clip.name);
                    self.clips.push(clip);
                    self.selected.push(self.clips.len() - 1);
                }
                Some(self.clips_event())
            }
            _ => None,
        }
    }
}
