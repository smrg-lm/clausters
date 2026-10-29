//! **The picture**: the stack, the boxes on it, the curves over them.
//!
//! Pure composition. What each thing looks like is `graphics::track`'s and
//! `graphics::multitrack`'s -- this says which of them to draw, where, and in
//! what order, and hands each body its own rectangle and its own local axis.

use super::*;

impl Multitrack {
    /// **Which of clip `n`'s grips is lit**, and where -- the affordance for the
    /// resize gesture, or `None` for a box nobody is reaching for.
    ///
    /// Two answers in one, and the order is the whole of the fix. **A held edge
    /// draws its grip wherever the pointer has got to**: pulling an edge takes
    /// the pointer off the box within a pixel or two -- that is what pulling an
    /// edge *is* -- so asking where the pointer is made the mark blink out under
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

    /// **Where each box is on screen, and the slice of its own span it shows** --
    /// the geometry the drawing and the texture pass both read, so a picture
    /// drawn on the mesh and one uploaded to the GPU land on the same pixels.
    ///
    /// A box whose track is gone, whose track is scrolled off, or which is off
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
        let at = self.track_rects(rect);
        let shown = |r: Rect| r.y + r.h >= rect.y && r.y <= rect.y + rect.h;
        let mut out = Vec::new();
        for (n, clip) in self.clips.iter().enumerate() {
            let Some(i) = self.track_of(clip).filter(|i| shown(at[*i])) else {
                continue;
            };
            let body = track::track_body(at[i], false, indent, metrics);
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

    /// **Where every drawn curve is, and the space it is drawn against** -- the
    /// rows under their tracks and the layers inside their boxes, in the order
    /// they are drawn.
    ///
    /// One answer for the drawing and for the hit test, which is what keeps a
    /// break-point grabbed on the pixels it was painted on. A hidden layer is
    /// absent: what is not drawn is not edited either.
    ///
    /// The two placements differ in exactly two facts, and this is where they
    /// are decided. A **row** spans the whole timeline -- a track's gain does not
    /// begin and end with a box -- so it is handed the shared window over the
    /// multitrack's own extent. A **layer** spans its box, so it is handed the box's
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
            let body = track::track_body(*row, false, indent, metrics);
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
    /// **What moves of a multitrack**: each track's meters, over the level the
    /// track is making, and the playhead across the stack. Everything else it
    /// draws is the description, which a tick does not change.
    pub(super) fn paint_live(&self, d: &mut Draw, ctx: &Ctx) {
        let at = self.track_rects(ctx.rect);
        let shown = |r: Rect| r.y + r.h >= ctx.rect.y && r.y <= ctx.rect.y + ctx.rect.h;
        for (i, track) in self.tracks.iter().enumerate() {
            if shown(at[i]) && self.meters.contains_key(&track.name) {
                track::draw_meters(d, at[i], &self.live_header(track, ctx), ctx.indent);
            }
        }
        if let Some(time) = ctx.time
            && let Some(pos) = time.head
        {
            let nav = self.view(ctx.time);
            let over = track::track_body(ctx.rect, false, ctx.indent, ctx.metrics);
            if let Some(x) = track::playhead_x(over, &nav, pos) {
                let (mesh, m, theme) = d.parts();
                mesh.rect(Rect::new(x, over.y, m.trace_w, over.h), theme.playhead);
            }
        }
    }

    pub(super) fn paint(&self, d: &mut Draw, ctx: &Ctx) {
        let at = self.track_rects(ctx.rect);
        // A track scrolled off either end is skipped rather than drawn and
        // clipped: `stack` reports every track so a hit test reads the same
        // rects, and the drawing is what decides it has nothing to do.
        let shown = |r: Rect| r.y + r.h >= ctx.rect.y && r.y <= ctx.rect.y + ctx.rect.h;
        for (i, track) in self.tracks.iter().enumerate() {
            if shown(at[i]) {
                // The header as it stands with nothing sounding: its meters
                // are a level's to move, and are drawn again over these on
                // the live layer ([`Self::paint_live`]).
                track::draw(
                    d,
                    at[i],
                    Some(track.shown()),
                    &self.header(track, ctx.indent),
                    false,
                    ctx.indent,
                    self.track == Some(i),
                );
            }
        }
        // **A track automation is a row of its own**, under the track it names
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
        // **One pass over the clips, each onto the track it names.** A clip
        // whose track is gone draws nowhere and is still held, which is what
        // lets it come back in a report to be re-homed.
        for (n, cr, local) in self.boxes_on_screen(ctx.rect, ctx.indent, ctx.metrics, ctx.time) {
            let clip = &self.clips[n];
            track::draw_clip(d, cr, self.selected.contains(&n));
            // **The base view is what its contents are.** Samples are the
            // audio editor's view of the take, drawn in the box's own slot
            // over the box's window onto it ([`Self::view_slots`]) -- so they
            // are not drawn here. Notes draw as the roll's body, handed the
            // box's own axis and drawing no chrome of their own. A box with
            // neither draws its frame and nothing in it, which is the honest
            // picture of a window onto something nobody loaded.
            let Some(roll) = self.rolls.get(&clip.name) else {
                continue;
            };
            let space = TimeSpace::of(local, clip.place.dur).with_window(SourceWindow {
                start: clip.place.start,
                // Frames of the sequence per sample of the box: one, unless
                // the multitrack said this box plays at another speed.
                rate: self.rate_of(&clip.name),
                looping: self.wraps(&clip.name),
                ..SourceWindow::default()
            });
            // **A box of notes has two presentations**: its roll, and its
            // page -- drawn on the box's own axis, each note at its time. A
            // box with no page, or a row too low for a staff to be read, is
            // drawn as its roll.
            #[cfg(feature = "notation")]
            let paged = self.notes_view == NotesView::Score
                && self.draw_page(d, cr, ctx.rect, ctx.indent, &space, n);
            #[cfg(not(feature = "notation"))]
            let paged = false;
            if !paged {
                roll.draw_body(d, cr, &space);
            }
        }
    }

    /// **What is drawn over the boxes' pictures**: each box's fades, label
    /// and grip, the curves, the selection and the position cursor -- over
    /// the views of the takes, which the frame draws after the boxes.
    pub(super) fn paint_over(&self, d: &mut Draw, ctx: &Ctx) {
        let nav = self.view(ctx.time);
        for (n, cr, local) in self.boxes_on_screen(ctx.rect, ctx.indent, ctx.metrics, ctx.time) {
            let clip = &self.clips[n];
            // **The silenced part of its fades, veiled**, read off the box's
            // own fade layer -- the envelope a hand drags -- so the veil moves
            // with the curve while it is dragged, rather than waiting for
            // what the owner answers.
            if let Some((fade_in, fade_out)) = self.fade_edges(&clip.name) {
                track::draw_clip_fades(d, cr, &local, clip.place.dur, fade_in, fade_out);
            }
            track::draw_clip_label(d, cr, clip.shown());
            // **The grips are drawn where they are grabbed.** An end that is
            // off screen has no grip, because a handle for an edge nobody can
            // see is a handle for nothing -- the same rule a track's clip keeps.
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
        // and the playhead. A track widget gets these drawn for it by the frame;
        // an element draws its own, from the same facts (`Ctx::time`).
        if let Some(time) = ctx.time {
            let over = track::track_body(ctx.rect, false, ctx.indent, ctx.metrics);
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
            // is. The cursor is drawn here; the playhead goes over it, on the
            // live layer, since a clock moves it.
            if let Some(pos) = time.cursor
                && let Some(x) = track::playhead_x(over, &nav, pos)
            {
                let (mesh, m, theme) = d.parts();
                mesh.rect(Rect::new(x, over.y, m.trace_w, over.h), theme.cursor);
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
        self.draw_readout(d, ctx);
    }

    /// **What the hover reads over a curve**: its label and its value, in the
    /// bottom-right corner of the tracks -- a break-point's own value when the
    /// pointer is on one. A row answers anywhere over its body, a clip
    /// envelope on its line or its points, as a press does.
    fn draw_readout(&self, d: &mut Draw, ctx: &Ctx) {
        let Some(at) = ctx.world.cursor else {
            return;
        };
        let Some(text) = self.curve_readout(at, ctx.rect, ctx.indent, ctx.metrics, ctx.time) else {
            return;
        };
        let over = track::track_body(ctx.rect, false, ctx.indent, ctx.metrics);
        let (_, m, theme) = d.parts();
        let room = over.w - 2.0 * m.pad;
        let w = font::width(&text, m.caption_scale).min(room);
        let (x, y) = (
            over.x + over.w - w - m.pad,
            over.y + over.h - font::height(m.caption_scale) - 2.0,
        );
        let (scale, color) = (m.caption_scale, theme.ruler_text);
        crate::host::graphics::plate_text(d, &text, x, y, room, scale, color);
    }

    /// The text [`Self::draw_readout`] draws, or `None` off every curve.
    pub(super) fn curve_readout(
        &self,
        at: (f64, f64),
        rect: Rect,
        indent: f32,
        m: &Metrics,
        time: Option<TimeSpace>,
    ) -> Option<String> {
        let drawn = self.curves_on_screen(rect, indent, m, time);
        for (name, place, space) in drawn.iter().rev() {
            let (curve, row) = match self.curves.iter().find(|c| c.name == *name) {
                Some(c) => (c, true),
                None => (self.layers.iter().find(|c| c.name == *name)?, false),
            };
            let Some((value, on_point)) = self
                .bodies
                .get(*name)?
                .hover_value(at, *place, *space, m, row)
            else {
                continue;
            };
            let value = crate::host::graphics::bpf::readout_value(
                value as f64,
                curve.min as f64,
                curve.max as f64,
            );
            let label = curve.shown();
            return Some(if on_point {
                format!("{label}  point {value}")
            } else {
                format!("{label}  {value}")
            });
        }
        None
    }
}

#[cfg(feature = "notation")]
impl Multitrack {
    /// **A box's page, on the box's axis**: the engraving of what it holds
    /// with each note at its time, cut to the box -- and its clef to the row,
    /// since it stands before the first note. Answers whether it drew one.
    fn draw_page(
        &self,
        d: &mut Draw,
        cr: Rect,
        body: Rect,
        indent: f32,
        space: &TimeSpace,
        n: usize,
    ) -> bool {
        use crate::host::graphics::pianoroll::to_x;
        use crate::host::graphics::score::{TimeColors, TimeFrame};

        let selected = self.selected.contains(&n);
        let Some(score) = self
            .clips
            .get(n)
            .and_then(|clip| self.scores.get(&clip.name))
        else {
            return false;
        };
        let (mesh, _, theme) = d.parts();
        // the row the box is on, as far across as the tracks are drawn
        let row = Rect::new(body.x + indent, cr.y, (body.w - indent).max(0.0), cr.h);
        let view = space.view;
        // a note's time is a frame of the sequence, read through the box's
        // window as its roll is
        let window = space.window;
        let rate = if window.rate > 0.0 { window.rate } else { 1.0 };
        score.page.render_on_time(
            mesh,
            TimeFrame { body: cr, row },
            &score.anchors,
            &|start| to_x((start - window.start) / rate, &view, cr) as f32,
            // inked as a take is in a box, so a page reads as a picture of its
            // contents and a selection is drawn over it as over a take
            TimeColors {
                ink: theme.selection,
                backdrop: if selected {
                    theme.selected_fill
                } else {
                    theme.object_fill
                },
            },
        )
    }
}
