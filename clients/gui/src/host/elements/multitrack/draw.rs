//! **The picture**: the stack, the boxes on it, the curves over them.
//!
//! Pure composition. What each thing looks like is `graphics::track`'s and
//! `graphics::multitrack`'s -- this says which of them to draw, where, and in
//! what order, and hands each body its own rectangle and its own local axis.

use super::*;

impl Multitrack {
    /// **Which of clip `n`'s grips is lit**, and where — the affordance for the
    /// resize gesture, or `None` for a box nobody is reaching for.
    ///
    /// Two answers in one, and the order is the whole of the fix. **A held edge
    /// draws its grip wherever the pointer has got to**: pulling an edge takes
    /// the pointer off the box within a pixel or two — that is what pulling an
    /// edge *is* — so asking where the pointer is made the mark blink out under
    /// the hand that was using it. What the hand is holding is known here, so it
    /// is asked first. With nothing held it is the pointer's own side, which is
    /// where an affordance belongs: lit always, every box carries two marks
    /// nobody is reaching for.
    pub(super) fn lit_grip(
        &self,
        n: usize,
        cr: Rect,
        ends: (bool, bool),
        m: &Metrics,
        cursor: Option<(f64, f64)>,
    ) -> Option<(Rect, track::ClipSide)> {
        let held = self
            .grab
            .filter(|g| g.clip == n)
            .and_then(|g| match g.part {
                Part::Start => Some(track::ClipSide::Start),
                Part::End => Some(track::ClipSide::End),
                Part::Body => None,
            })
            .and_then(|side| track::clip_grip_on(cr, ends, m, side));
        held.or_else(|| {
            cursor
                .filter(|(_, cy)| *cy as f32 >= cr.y && (*cy as f32) < cr.y + cr.h)
                .and_then(|(cx, _)| track::clip_grip_at(cr, ends, m, cx as f32))
        })
    }

    /// **Where each box is on screen, and the slice of its own span it shows** —
    /// the geometry the drawing and the texture pass both read, so a picture
    /// drawn on the mesh and one uploaded to the GPU land on the same pixels.
    ///
    /// A box whose lane is gone, whose lane is scrolled off, or which is off
    /// the window is absent rather than reported at zero size: what a caller
    /// wants is what it can draw.
    pub(super) fn boxes_on_screen(
        &self,
        rect: Rect,
        indent: f32,
        metrics: &Metrics,
        time: Option<TimeSpace>,
    ) -> Vec<(usize, Rect, View)> {
        let nav = self.view(time);
        let at = self.lane_rects(rect);
        let shown = |r: Rect| r.y + r.h >= rect.y && r.y <= rect.y + rect.h;
        let mut out = Vec::new();
        for (n, clip) in self.clips.iter().enumerate() {
            let Some(i) = self.lane_of(clip).filter(|i| shown(at[*i])) else {
                continue;
            };
            let body = track::lane_body(at[i], false, indent, metrics);
            if body.w <= 0.0 || body.h <= 0.0 {
                continue;
            }
            let Some((x0, x1)) = stack::clip_x(clip, body, &nav, MIN_CLIP_W) else {
                continue;
            };
            let cr = track::clip_rect(body, x0, x1);
            let local = track::clip_local_view(body, &nav, clip.place.offset, clip.place.dur, cr);
            out.push((n, cr, local));
        }
        out
    }

    /// **Where every drawn curve is, and the space it is drawn against** — the
    /// rows under their lanes and the layers inside their boxes, in the order
    /// they are drawn.
    ///
    /// One answer for the drawing and for the hit test, which is what keeps a
    /// break-point grabbed on the pixels it was painted on. A hidden layer is
    /// absent: what is not drawn is not edited either.
    ///
    /// The two placements differ in exactly two facts, and this is where they
    /// are decided. A **row** spans the whole timeline — a track's gain does not
    /// begin and end with a box — so it is handed the shared window over the
    /// piece's own extent. A **layer** spans its box, so it is handed the box's
    /// local window over the box's own duration, the same [`TimeSpace`] the base
    /// view under it draws through.
    pub(super) fn curves_on_screen(
        &self,
        rect: Rect,
        indent: f32,
        metrics: &Metrics,
        time: Option<TimeSpace>,
    ) -> Vec<(&str, Rect, TimeSpace)> {
        let nav = self.view(time);
        let stack = self.stack();
        let rows = stack.rects(rect, self.scroll);
        let shown = |r: Rect| r.y + r.h >= rect.y && r.y <= rect.y + rect.h;
        let span = model::extent(&self.clips).max(nav.start + nav.len);
        let mut out = Vec::new();
        for (i, row) in rows.iter().enumerate() {
            let Some(stack::Row::Curve(n)) = stack.row(i) else {
                continue;
            };
            let Some(curve) = self.curves.get(n).filter(|c| !self.is_hidden(&c.name)) else {
                continue;
            };
            if !shown(*row) {
                continue;
            }
            let body = track::lane_body(*row, false, indent, metrics);
            if body.w <= 0.0 || body.h <= 0.0 {
                continue;
            }
            out.push((
                curve.name.as_str(),
                body,
                self.space(nav, span, &curve.name),
            ));
        }
        for (n, cr, local) in self.boxes_on_screen(rect, indent, metrics, time) {
            let clip = &self.clips[n];
            for curve in &self.layers {
                if curve.owner != clip.name || self.is_hidden(&curve.name) {
                    continue;
                }
                let mut space = self.space(local, clip.place.dur, &curve.name);
                space.window = SourceWindow {
                    start: clip.place.start,
                    looping: self.wraps(&clip.name),
                    ..SourceWindow::default()
                };
                out.push((curve.name.as_str(), cr, space));
            }
        }
        out
    }
}

impl Multitrack {
    pub(super) fn paint(&self, d: &mut Draw, ctx: &Ctx) {
        let nav = self.view(ctx.time);
        let at = self.lane_rects(ctx.rect);
        // A lane scrolled off either end is skipped rather than drawn and
        // clipped: `stack` reports every lane so a hit test reads the same
        // rects, and the drawing is what decides it has nothing to do.
        let shown = |r: Rect| r.y + r.h >= ctx.rect.y && r.y <= ctx.rect.y + ctx.rect.h;
        for (i, lane) in self.lanes.iter().enumerate() {
            if shown(at[i]) {
                track::draw(
                    d,
                    at[i],
                    Some(lane.shown()),
                    &self.live_header(lane, ctx),
                    false,
                    ctx.indent,
                    self.track == Some(i),
                );
            }
        }
        // **A track automation is a row of its own**, under the lane it names
        // and with no boxes on it: what it draws runs the whole timeline the
        // track does, so it is a row and not a layer.
        let stack = self.stack();
        for (i, row) in stack.rects(ctx.rect, self.scroll).iter().enumerate() {
            let Some(stack::Row::Curve(n)) = stack.row(i) else {
                continue;
            };
            let Some(curve) = self.curves.get(n) else {
                continue;
            };
            if shown(*row) {
                track::draw(
                    d,
                    *row,
                    Some(curve.shown()),
                    &track::Header {
                        w: (ctx.indent > 0.0).then_some(ctx.indent),
                        mute: None,
                        solo: None,
                        level: None,
                        curves: None,
                        meters: Vec::new(),
                    },
                    false,
                    ctx.indent,
                    // A curve's row is the track's picture, not the track: what
                    // is selected is the track, and its own header says so.
                    false,
                );
            }
        }
        // **One pass over the clips, each onto the lane it names.** A clip
        // whose lane is gone draws nowhere and is still held, which is what
        // lets it come back in a report to be re-homed.
        for (n, cr, local) in self.boxes_on_screen(ctx.rect, ctx.indent, ctx.metrics, ctx.time) {
            let clip = &self.clips[n];
            track::draw_clip(d, cr, self.selected.contains(&n));
            // **The take, drawn from the source per visible pixel**, mapped
            // back through the clip's own window onto it — which is what makes
            // the picture scroll and trim *with* the box instead of squashing
            // into whatever rectangle it currently has. One pyramid however
            // many clips read it.
            // **The base view is what its contents are.** Samples draw as the
            // signal element's body, notes as the roll's — the very elements
            // that stand on their own elsewhere, handed the box's own axis and
            // drawing no chrome of their own. A box with neither draws its
            // frame and nothing in it, which is the honest picture of a window
            // onto something nobody loaded.
            let space = TimeSpace::of(local, clip.place.dur).with_window(SourceWindow {
                start: clip.place.start,
                // A box longer than its samples **wraps** where the piece says
                // it loops, and shows nothing past their end where it does not:
                // the picture is what the box reads, and it reads this.
                looping: self.wraps(&clip.name),
                ..SourceWindow::default()
            });
            if let Some(take) = self.takes.get(&clip.source) {
                take.draw_body(d, cr, &space);
            } else if let Some(roll) = self.rolls.get(&clip.name) {
                roll.draw_body(d, cr, &space);
            }
            track::draw_clip_label(d, cr, clip.shown());
            // **The grips are drawn where they are grabbed.** An end that is
            // off screen has no grip, because a handle for an edge nobody can
            // see is a handle for nothing — the same rule a lane's clip keeps.
            // **A grip is an affordance, so it is shown where the hand is.**
            // Drawn always, every clip carries two marks nobody is reaching
            // for; drawn on the side the pointer is over, it says *this edge
            // moves* at the moment that is worth saying.
            //
            // **And a held edge draws its grip wherever the pointer has got
            // to.** Pulling an edge takes the pointer off the box within a
            // pixel or two -- that is what pulling an edge *is* -- so asking
            // where the pointer is made the mark disappear under the hand that
            // was using it. What the hand is holding is known here, so it is
            // asked first: the affordance stops lying about being reachable.
            let ends = track::clip_ends_on_screen(&local, clip.place.dur);
            if let Some((grip, side)) = self.lit_grip(n, cr, ends, ctx.metrics, ctx.world.cursor) {
                track::draw_clip_grip(d, grip, side);
            }
        }
        // **The light views, over the base ones.** A curve is drawn last of
        // the contents, whether it is a row of its own or a layer inside a box:
        // it is the one thing here a hand may edit, and it has to be on top of
        // what it shapes to be reached.
        for (name, rect, space) in
            self.curves_on_screen(ctx.rect, ctx.indent, ctx.metrics, ctx.time)
        {
            if let Some(body) = self.bodies.get(name) {
                body.draw_body(d, rect, &space);
            }
        }
        // **The axis' own chrome, over the clips**: the shared selection band
        // and the playhead. A lane widget gets these drawn for it by the frame;
        // an element draws its own, from the same facts (`Ctx::time`).
        if let Some(time) = ctx.time {
            let over = track::lane_body(ctx.rect, false, ctx.indent, ctx.metrics);
            crate::host::graphics::selection::draw_span(
                d,
                over,
                &nav,
                time.sel,
                1,
                None,
                crate::host::graphics::selection::Vertical::Whole,
            );
            // **Two lines, and they mean two things**: the position cursor is
            // where the reader put the mark, the playhead is where the music
            // is. The cursor goes down first, so where they coincide it is the
            // playhead that reads.
            for (pos, role) in [(time.cursor, false), (time.head, true)] {
                if let Some(pos) = pos
                    && let Some(x) = track::playhead_x(over, &nav, pos)
                {
                    let (mesh, m, theme) = d.parts();
                    let color = if role { theme.playhead } else { theme.cursor };
                    mesh.rect(Rect::new(x, over.y, m.trace_w, over.h), color);
                }
            }
        }
        if let Some(text) = &self.label {
            let (mesh, m, theme) = d.parts();
            font::text(
                mesh,
                text,
                ctx.rect.x + ctx.indent + m.pad,
                ctx.rect.y + 2.0,
                m.caption_scale,
                theme.ruler_text,
            );
        }
    }
}
