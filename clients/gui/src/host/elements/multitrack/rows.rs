//! **The stack**: which row is where, how tall it is, and the header beside it.
//!
//! The vertical model, apart from what is drawn on it and from what a hand does
//! to it. A row is a lane or one of its automation rows, and everything here
//! answers one of three questions -- where a row is on screen, which row a `y`
//! is in, and how tall each is once a reader has zoomed one. The time axis'
//! half is here too, since a box's rectangle needs both: where a sample lands,
//! and what a box's edges may reach.

use super::*;

impl Multitrack {
    /// The axis this draws against: the navigation group's window when it is on
    /// one, else its own whole extent.
    pub(super) fn view(&self, time: Option<TimeSpace>) -> View {
        match time {
            Some(t) => t.view,
            None => View::full(model::extent(&self.clips).ceil().max(1.0) as usize),
        }
    }

    /// The lane a clip sits on, by index — `None` for a clip naming a lane that
    /// is not here.
    ///
    /// **A clip is kept rather than dropped**, because what cannot be placed can
    /// still be reported: a script that renamed a lane gets its clips back to
    /// re-home rather than silently losing them.
    pub(super) fn lane_of(&self, clip: &Clip) -> Option<usize> {
        self.lanes.iter().position(|l| l.name == clip.lane)
    }

    /// The lane a pointer is **on**, or `None` off the stack — the *press*'
    /// question, over the same bands the drawing used.
    pub(super) fn lane_at(&self, rect: Rect, y: f64) -> Option<usize> {
        self.stack().lane_at(rect, self.scroll, y)
    }

    /// **The vertical axis**: the lanes and the automation rows under them, in
    /// the order they are drawn. Built per ask rather than kept, because it is
    /// derived from two lists a `/gui_set` replaces whole.
    pub(super) fn stack(&self) -> stack::Stack {
        stack::Stack::shown(&self.lanes, &self.curves, self.gap, |c| {
            !self.is_hidden(&c.name)
        })
    }

    /// Where each **lane** lands, by lane index.
    pub(super) fn lane_rects(&self, rect: Rect) -> Vec<Rect> {
        self.stack().lane_rects(rect, self.scroll, self.lanes.len())
    }

    /// The lane a hand **is heading for**, always — the *drag*'s question, and
    /// the sweep's. It answers for the gaps between lanes and clamps past
    /// either end, which is the whole of why a dragged clip neither jumps nor
    /// oscillates.
    pub(super) fn lane_toward(&self, rect: Rect, y: f64) -> usize {
        self.stack().lane_toward(rect, self.scroll, y)
    }

    /// The time a pointer x names on the shared axis.
    ///
    /// **Never on an axis this drag is moving.** A widget on no navigation
    /// group rules itself by its own extent, which a drag changes, so during
    /// one the axis is the press'; on a group it is read live, which is what an
    /// edge-scrolled pan needs.
    pub(super) fn time_at(&self, input: &Input, x: f64) -> f64 {
        let nav = match (input.time, self.grab) {
            (None, Some(grab)) => grab.axis,
            (time, _) => self.view(time),
        };
        let body = track::lane_body(input.rect, false, input.indent, input.metrics);
        if body.w <= 0.0 {
            return nav.start;
        }
        nav.start + (x - f64::from(body.x)) / f64::from(body.w) * nav.len
    }

    /// What bounds a clip's drag here: the lane's grid, and a floor no shorter
    /// than a box a hand can still find.
    pub(super) fn bounds(&self) -> Bounds {
        Bounds {
            grid: self.snap,
            ..Bounds::default()
        }
    }

    /// The lane header band `y` falls in, and the part of it `(x, y)` hit.
    pub(super) fn header_at(
        &self,
        input: &Input,
        at: (f64, f64),
    ) -> Option<(usize, track::HeaderPart)> {
        let i = self.lane_at(input.rect, at.1)?;
        let rect = self.lane_rects(input.rect)[i];
        let band = crate::host::timeline::gutter_band(rect, input.indent);
        let header = self.header(&self.lanes[i], input.indent);
        let part = track::header_hit(band, &header, input.metrics, at.0, at.1)?;
        Some((i, part))
    }

    /// **Lays the hand's own row heights back over what a payload says.**
    ///
    /// How tall a track is drawn is this window's and the wire carries none of
    /// it — but a `lanes` payload states a height on every row, because the
    /// prop has always had one — so a fader moved or a track added would
    /// otherwise take a reader's vertical zoom away with it. The same rule the
    /// scroll and the box selection follow, applied where the payload lands.
    pub(super) fn zoom_rows(&mut self) {
        for lane in &mut self.lanes {
            if let Some(h) = self.zoom.get(&lane.name) {
                lane.height = *h;
            }
        }
        for curve in &mut self.curves {
            if let Some(h) = self.curve_zoom.get(&curve.name) {
                curve.height = *h;
            }
        }
        // A row that is gone takes its height with it, the way every other
        // table here is pruned by what the multitrack now holds.
        self.zoom
            .retain(|name, _| self.lanes.iter().any(|l| &l.name == name));
        self.curve_zoom
            .retain(|name, _| self.curves.iter().any(|c| &c.name == name));
    }

    /// **A scroll that cannot lose the stack**: clamped to what there is below
    /// the window, and pinned at the top when the whole thing fits.
    ///
    /// The floor is zero and the ceiling is how much of the stack is off the
    /// bottom, so the last row can always be brought into view and never past
    /// it. A stack shorter than its window has a ceiling of zero, which is the
    /// same statement: there is nothing to scroll to.
    pub(super) fn clamped_scroll(&self, rect: Rect, want: f32) -> f32 {
        let over = (self.stack().content_height() - rect.h).max(0.0);
        want.clamp(0.0, over)
    }

    /// **What kind of row a y is on** — a lane, an automation row, or nothing
    /// at all past either end of the stack.
    pub(super) fn row_kind(&self, input: &Input, y: f64) -> Option<stack::Row> {
        let stack = self.stack();
        stack.row(stack.row_at(input.rect, self.scroll, y)?)
    }

    /// **How far a snap reaches**, in the axis' own units: a few device pixels
    /// crossed to time, so it feels the same at every zoom.
    ///
    /// A **screen** distance and not a musical one, because what it does is
    /// screen work: it is the allowance a hand gets for meaning *this edge*,
    /// the same kind of number as the hit slop, and a tolerance in samples
    /// would be unreachable zoomed out and enormous zoomed in.
    pub(super) fn snap_reach(&self, input: &Input) -> f64 {
        (self.time_at(input, f64::from(SNAP_PX)) - self.time_at(input, 0.0)).abs()
    }

    /// **The correction that lands a moving edge on a neighbour's**, or zero
    /// when nothing is near enough.
    ///
    /// A snap to **content**, which is what makes two boxes meetable at the
    /// sample: with no quantization a hand never lands one box exactly where
    /// another ends, so `j` never had two boxes to join. It stands beside
    /// `snap` rather than replacing it — a grid says where a beat is, this says
    /// where the music already is — and a hand that keeps pulling past the
    /// tolerance goes on through and overlaps them, which is a crossfade and
    /// legal.
    ///
    /// `moving` are the edges the hand is carrying, `held` what it is carrying
    /// them on (a box does not snap to itself), and `row` the lane whose boxes
    /// are the neighbours: an edge on another lane is another lane's business.
    pub(super) fn pull_to_edge(
        &self,
        input: &Input,
        row: f32,
        held: &[usize],
        moving: &[f64],
    ) -> f64 {
        let reach = self.snap_reach(input);
        if reach <= 0.0 {
            return 0.0;
        }
        let mut best = 0.0;
        let mut nearest = f64::INFINITY;
        for (i, clip) in self.clips.iter().enumerate() {
            if held.contains(&i) || (self.row(i) - row).abs() > f32::EPSILON {
                continue;
            }
            for edge in [clip.place.offset, clip.place.offset + clip.place.dur] {
                for m in moving {
                    let d = edge - m;
                    if d.abs() <= reach && d.abs() < nearest {
                        nearest = d.abs();
                        best = d;
                    }
                }
            }
        }
        best
    }

    /// The space a curve is drawn against, with the one fact a container
    /// decides for its layers: **whether this is the active one**.
    pub(super) fn space(&self, view: View, span: f64, name: &str) -> TimeSpace {
        let mut space = TimeSpace::of(view, span);
        space.active = self.layer.as_deref() == Some(name);
        space
    }

    /// Whether a layer is one of the ones that are not drawn.
    pub(super) fn is_hidden(&self, name: &str) -> bool {
        self.hidden.iter().any(|h| h == name)
    }

    /// The lane header a lane's own props ask for. Presence-driven, like every
    /// header here: a lane that carries no mixer state offers no controls.
    pub(super) fn header(&self, lane: &Lane, indent: f32) -> track::Header {
        track::Header {
            w: (indent > 0.0).then_some(indent),
            mute: Some(lane.mute),
            solo: Some(lane.solo),
            level: Some(lane.gain),
            // **Offered on every track, including the ones with nothing to
            // show** *(asked for by the user 2026-09-12)*. The first press on a
            // bare track is what **adds** its gain automation, the way a double
            // click on a header adds a track: the owner reads "show me this
            // track's automation" and makes one where there is none. So the
            // button is not a view of something that exists, it is the verb
            // that brings it into being and then hides and shows it.
            //
            // **A facility, and it says so**: what a track may automate is its
            // own question, and this is the smallest thing that makes a multitrack
            // with automation editable while that is worked out. See
            // `clients/gui/PLAN.md`, "The whole interaction vocabulary is
            // provisional".
            curves: Some(lane.curves),
            // **Silent, and the right length**: the strip's width follows the
            // channel count and nothing else, so a hit test lays the header out
            // exactly where the drawing did without reading a bus.
            meters: self
                .meters
                .get(&lane.name)
                .map_or_else(Vec::new, |m| vec![(0.0, 0.0); m.channels]),
        }
    }

    /// The same header with the **levels read**, which only a draw can do: the
    /// values are in the shared segment and are one atomic load each, so a
    /// meter costs a frame's read rather than a message.
    pub(super) fn live_header(&self, lane: &Lane, ctx: &Ctx) -> track::Header {
        let mut header = self.header(lane, ctx.indent);
        let Some(meter) = self.meters.get(&lane.name) else {
            return header;
        };
        for (channel, slot) in header.meters.iter_mut().enumerate() {
            let channel = channel as i32;
            *slot = (
                ctx.world.level(meter.level + channel, Rate::Control),
                if meter.mark < 0 {
                    0.0
                } else {
                    ctx.world.level(meter.mark + channel, Rate::Control)
                },
            );
        }
        header
    }
}
