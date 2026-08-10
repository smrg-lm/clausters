//! What a **press** does: the containers' plans over the hit chain, then the
//! element under the cursor.
//!
//! The press is the phase that decides *which* gesture a pointer-down starts —
//! every [`Drag`] in the machine is opened here, and nowhere else.
//! It runs in two layers, and the split is the reason a widget's own behaviour
//! stays small: the **containers** over the point declare what a modifier means
//! on them ([`GestureStep`]), innermost first, and only when every step has
//! declined does the press reach the **element** — which is the one arm per
//! widget kind in [`Gestures::element_press`].
//!
//! Both layers are one match apiece rather than a function apiece, deliberately:
//! the arms are short (a hit-test, a snapshot, a `Drag`), and the exhaustive
//! match is what makes a new widget kind impossible to forget here.

use clausters_core::osc::OscType;

use super::super::interact::{self, Hit, slider_t};
use super::super::textedit;
use super::super::widget::{Claim, GestureStep, WidgetKind};
use super::super::{Host, bpf, controls, patch, piano, pianoroll};
use super::effects::*;
use super::nav::*;
use super::{Drag, GestureCtx, GestureEffect, Gestures, MenuOpen};
use crate::viewport::View;

impl Gestures {
    /// Press: run the **containers' gesture plans** over the hit, innermost
    /// first, until one of their steps consumes it.
    ///
    /// The order is the containers', not the widget's. Each container over the
    /// point declares what a modifier does on it ([`GestureMap`](super::super::widget::GestureMap)) — pan its axis,
    /// sweep a selection, locate the transport, or hand the press to the
    /// element under the cursor — and a step that declines passes the press on,
    /// outward through the chain. That is why Shift+drag pans the same way over
    /// a waveform, a lane and a piano-roll (their axis claims it before any of
    /// them sees it), and why Shift on a patcher's empty canvas still pans the
    /// workspace *around* the patcher: the canvas declines and the plane
    /// outside it takes over.
    ///
    /// `grab` is the front's pointer-grab attempt for a knob/number drag
    /// (returns whether the pointer was *locked*); a front without pointer lock
    /// returns `false`.
    pub fn press(
        &mut self,
        host: &mut Host,
        ctx: &GestureCtx,
        cx: f64,
        cy: f64,
        grab: &mut dyn FnMut() -> bool,
    ) -> Vec<GestureEffect> {
        let mut out = Vec::new();
        // An **open list is modal**: it is over everything, so it is tested
        // before the tree, and it swallows the press either way — on a row it
        // picks that option, anywhere else it just closes, the way a menu
        // everywhere else behaves.
        if let Some(open) = self.menu.take() {
            out.push(GestureEffect::Redraw(ctx.def_id));
            if let Some(row) = self.menu_row(host, ctx, open, cx, cy) {
                interact::set_menu_index(host, ctx.def_id, open.id, row);
                emit_value(host, &mut out, ctx.def_id, open.id);
            }
            return out;
        }
        let Some(hit) = hit(host, ctx, cx, cy) else {
            // A press on empty space drops the text focus (the caret disappears).
            if let Some(old) = host.clear_text_focus() {
                out.push(GestureEffect::Redraw(old));
            }
            self.pan_sole_axis(host, ctx, cx);
            return out;
        };
        // A press on anything other than the focused text field defocuses it.
        if !matches!(hit.kind, WidgetKind::Text { .. })
            && let Some(old) = host.clear_text_focus()
        {
            out.push(GestureEffect::Redraw(old));
        }
        // The vertical axis is grabbed on its own strip, before any modifier: a
        // press on a y-ruler or a piano-roll's keyboard gutter means *that*
        // axis, whatever the container maps the drag to elsewhere.
        if let Some((id, axis)) = interact::time_of(&hit.chain)
            && let Some(y) = axis.y
            && y.strip.contains(cx, cy)
        {
            self.drag = Some(Drag::PanY {
                id,
                origin_y: cy,
                y_start: y.start,
                lane_h: y.lane_h,
            });
            return out;
        }
        // A spectrum's **frequency** axis is grabbed on the axis itself, before
        // any modifier and before the chain: the element is nobody's container,
        // so there is no coordinate system over it to offer a pan, and dragging
        // its curve sideways can mean nothing else.
        if let Some(axis) = freq_axis(host, ctx, &hit)
            && axis.surface.contains(cx, cy)
        {
            self.drag = Some(Drag::PanX {
                id: hit.id,
                origin_x: cx,
                x_start: axis.start,
                body_w: axis.body.w.max(1.0) as f64,
            });
            return out;
        }
        let mut element_ran = false;
        for frame in hit.chain.iter().rev() {
            for step in frame.map.plan(ctx.shift, ctx.ctrl, ctx.alt).steps() {
                let consumed = match step {
                    // The element gets exactly one turn, wherever the first
                    // container that offers it sits.
                    GestureStep::Element if !element_ran => {
                        element_ran = true;
                        self.element_press(host, ctx, &hit, cx, cy, grab, &mut out)
                    }
                    GestureStep::Element => false,
                    action => self.container_press(host, ctx, frame, action, cx, cy, &mut out),
                };
                if consumed {
                    return out;
                }
            }
        }
        // Nobody took it.
        self.pan_sole_axis(host, ctx, cx);
        out
    }

    /// Shift+drag means "pan the axis" wherever it starts, so in a window with
    /// **one** navigation group it means that off the lanes too — the gap
    /// between them, the slack under the last one, a container's margin, the
    /// window's own edge. Returns whether it grabbed.
    fn pan_sole_axis(&mut self, host: &Host, ctx: &GestureCtx, cx: f64) -> bool {
        if !ctx.shift {
            return false;
        }
        let Some(sole) =
            interact::sole_time_axis(host, ctx.def_id, ctx.fb_w, ctx.fb_h, &|id, kind| {
                ctx.lanes(id, kind)
            })
        else {
            return false;
        };
        self.drag = Some(Drag::Pan {
            id: sole.id,
            origin_x: cx,
            start: sole.axis.nav.start,
            body_w: sole.axis.body.w.max(1.0) as f64,
        });
        true
    }

    /// The option row an open list has under `(cx, cy)` — `None` when the press
    /// landed outside the list (which closes it and picks nothing). The option
    /// count comes from the tree, so a list left open over a widget that is
    /// gone resolves to nothing rather than to a stale row.
    fn menu_row(
        &self,
        host: &Host,
        ctx: &GestureCtx,
        open: MenuOpen,
        cx: f64,
        cy: f64,
    ) -> Option<usize> {
        let options = host
            .window_def(ctx.def_id)
            .and_then(|tree| tree.find(open.id))
            .and_then(|w| match &w.kind {
                WidgetKind::Menu { options, .. } => Some(options.len()),
                _ => None,
            })?;
        controls::menu_row_at(open.popup, options, cx, cy)
    }

    /// One container-level step of a press: the gestures that belong to the
    /// coordinate system rather than to what is drawn in it. Each reads the
    /// frame the chain resolved — the axis' own body, window and view state —
    /// so a pan is one implementation for the five timeline views and a plane
    /// pan is one for every workspace. Returns whether the step consumed the
    /// press; a step that has nothing to act on (a locate outside the axis'
    /// body, a selection on a canvas with no marquee) declines, and the plan
    /// goes on.
    #[allow(clippy::too_many_arguments)] // one press: a container, a step, a cursor
    fn container_press(
        &mut self,
        host: &mut Host,
        ctx: &GestureCtx,
        frame: &interact::Frame,
        step: GestureStep,
        cx: f64,
        cy: f64,
        out: &mut Vec<GestureEffect>,
    ) -> bool {
        let def_id = ctx.def_id;
        let Some(id) = frame.id else {
            return false; // an unaddressable container navigates nothing
        };
        match (step, frame.coords) {
            (GestureStep::Pan, interact::Coords::Time(axis)) => {
                self.drag = Some(Drag::Pan {
                    id,
                    origin_x: cx,
                    start: axis.nav.start,
                    body_w: axis.body.w.max(1.0) as f64,
                });
                true
            }
            (GestureStep::Pan, interact::Coords::Plane(view)) => {
                // A plane with nowhere to go **declines**, the way its wheel
                // does: the slack under a short stack is not a surface with a
                // gesture of its own, and eating the press there is what left
                // Shift+drag dead everywhere except over a lane.
                if !interact::plane_can_pan(host, def_id, id, frame.rect, view) {
                    return false;
                }
                self.drag = Some(Drag::ScrollPan {
                    id,
                    area: frame.rect,
                    origin_x: cx,
                    origin_y: cy,
                    x0: view.view_x,
                    y0: view.view_y,
                });
                true
            }
            (GestureStep::Select, interact::Coords::Time(axis)) => {
                if !axis.spans(cx) {
                    return false;
                }
                // The press collapses the shared selection to the sample under
                // it; the drag sweeps from there. On an axis that measures a
                // value too (a roll's pitch), the sweep is a rectangle and the
                // container's own elements inside it become its selection --
                // but only when the press is on the body, since the strips
                // under it (a velocity lane, a ruler) read the time axis alone.
                let anchor = interact::sample_at(
                    axis.nav.start,
                    axis.nav.len,
                    axis.body.x as f64,
                    axis.body.w as f64,
                    cx,
                );
                let window = axis
                    .y
                    .filter(|_| axis.body.contains(cx, cy))
                    .and_then(|y| y.window);
                if window.is_some() {
                    // A fresh sweep drops the set the previous one left.
                    interact::clear_element_selection(host, def_id, id);
                }
                let value =
                    window.map(|(lo, hi)| (lo, hi, interact::value_at(axis.body, lo, hi, cy)));
                set_selection(host, out, def_id, id, anchor, anchor);
                self.drag = Some(Drag::Select {
                    id,
                    body: axis.body,
                    nav_start: axis.nav.start,
                    nav_len: axis.nav.len,
                    anchor,
                    value,
                });
                out.push(GestureEffect::Redraw(def_id));
                true
            }
            (GestureStep::Select, interact::Coords::Canvas) => {
                interact::graph_select(host, def_id, id, Vec::new());
                self.drag = Some(Drag::Marquee {
                    id,
                    area: frame.rect,
                    scale: frame.scale,
                    origin: (cx, cy),
                    cursor: (cx, cy),
                });
                out.push(GestureEffect::Redraw(def_id));
                true
            }
            (GestureStep::Locate, interact::Coords::Time(axis)) => {
                if !axis.spans(cx) {
                    return false; // beside the axis (a lane's header): no position
                }
                locate_timeline(host, out, def_id, id, axis.body, cx);
                true
            }
            _ => false,
        }
    }

    /// The press the containers handed down: what the widget under the cursor
    /// does with it — a control's value, a note, a break-point, a clip, a piano
    /// key, a cord. Returns whether it was consumed; declining (empty space in
    /// a lane, a patch's bare canvas) hands the press back to the chain.
    #[allow(clippy::too_many_arguments)] // one press: a hit, the context, a cursor
    fn element_press(
        &mut self,
        host: &mut Host,
        ctx: &GestureCtx,
        hit: &Hit,
        cx: f64,
        cy: f64,
        grab: &mut dyn FnMut() -> bool,
        out: &mut Vec<GestureEffect>,
    ) -> bool {
        let Hit {
            id, rect, scale, ..
        } = *hit;
        let (chain, kind) = (&hit.chain, hit.kind.clone());
        let def_id = ctx.def_id;
        let effects_before = out.len();
        match kind {
            WidgetKind::Slider { range: r, vertical } => {
                // The track area, not the whole body: the grab has to agree with
                // the groove the renderer drew (`controls::slider_track`) — at
                // the placement's own table, which is what the renderer used.
                let body = controls::slider_track(
                    rect,
                    r.label.is_some(),
                    r.text_size * scale,
                    &host.metrics_for(def_id).at(scale),
                );
                let t = slider_t(body, cx, cy, vertical);
                interact::set_fraction(host, def_id, id, t);
                emit_value(host, out, def_id, id);
                self.drag = Some(Drag::Slider { id, body, vertical });
                out.push(GestureEffect::Redraw(def_id));
            }
            WidgetKind::Knob(r) | WidgetKind::Number(r) => {
                let body = controls::body_rect_at(
                    rect,
                    r.label.is_some(),
                    r.text_size * scale,
                    &host.metrics_for(def_id).at(scale),
                );
                let locked = grab();
                self.drag = Some(Drag::Vertical {
                    id,
                    last_y: cy,
                    body_h: body.h,
                    locked,
                });
            }
            WidgetKind::Button { .. } => {
                deliver(host, out, def_id, id, OscType::Int(1));
                self.drag = Some(Drag::Button { id });
                out.push(GestureEffect::Redraw(def_id));
            }
            WidgetKind::Toggle { .. } => {
                interact::flip_toggle(host, def_id, id);
                emit_value(host, out, def_id, id);
                out.push(GestureEffect::Redraw(def_id));
            }
            WidgetKind::Menu {
                ref options,
                ref label,
                text_size,
                ..
            } => {
                // The list hangs off the menu's **body** (the field the chosen
                // option is drawn in), not off the whole cell, so it lines up
                // with what it is replacing rather than with the label over it.
                let m = host.metrics_for(def_id).at(scale);
                let size = text_size * scale;
                let body = controls::body_rect_at(rect, label.is_some(), size, &m);
                self.menu = Some(MenuOpen {
                    id,
                    popup: controls::menu_popup(body, options.len(), size, ctx.fb_h as f32, &m),
                });
                out.push(GestureEffect::Redraw(def_id));
            }
            WidgetKind::Text { .. } => {
                // Focus the field and drop the caret where the press landed; a
                // drag from here extends a selection.
                host.focus_text(def_id, id);
                let pos =
                    interact::text_caret_at(host, def_id, id, rect, scale, cx, cy).unwrap_or(0);
                interact::text_edit(host, def_id, id, |value, caret, _| {
                    textedit::clamp(value, caret); // guard a stale caret
                    caret.pos = pos;
                    caret.anchor = None;
                });
                self.drag = Some(Drag::TextSelect {
                    id,
                    rect,
                    scale,
                    anchor: pos,
                });
                out.push(GestureEffect::Redraw(def_id));
            }
            WidgetKind::Bpf {
                ref points,
                min,
                max,
                duration,
                exp,
                ref label,
                ..
            } => {
                let body = bpf::body(rect, label.is_some(), host.metrics_for(def_id));
                let hit_pt = bpf::hit_point(
                    points,
                    body,
                    duration,
                    min,
                    max,
                    exp,
                    cx,
                    cy,
                    host.metrics_for(def_id),
                );
                if ctx.ctrl {
                    // Ctrl+click on a point removes it; elsewhere it adds one
                    // at the cursor (which then drags until release).
                    // `None` = nothing changed, `Some(None)` = removed,
                    // `Some(Some(i))` = added at index `i`.
                    let edited: Option<Option<usize>> = match hit_pt {
                        Some(i) => interact::bpf_edit(host, def_id, id, |p, _, _, _, _| {
                            bpf::remove_point(p, i)
                        })
                        .and_then(|removed| removed.then_some(None)),
                        None => interact::bpf_edit(host, def_id, id, |p, duration, lo, hi, exp| {
                            bpf::add_point(p, body, duration, lo, hi, exp, cx, cy)
                        })
                        .map(Some),
                    };
                    if let Some(added) = edited {
                        if let Some(index) = added {
                            self.drag = Some(Drag::BpfPoint { id, index, body });
                        }
                        emit_points(host, out, def_id, id);
                        out.push(GestureEffect::Redraw(def_id));
                    }
                } else if let Some(index) = hit_pt {
                    self.drag = Some(Drag::BpfPoint { id, index, body });
                } else if let Some(segment) = bpf::hit_segment(points, body, duration, cx) {
                    self.drag = Some(Drag::BpfCurve {
                        id,
                        segment,
                        last_y: cy,
                        body_h: body.h.max(1.0) as f64,
                    });
                }
            }
            WidgetKind::Patch {
                ref patch,
                ref selected,
                ..
            } => {
                // A port wins: the cord drag. Then a box: select it and start a
                // move (a press on an already-selected box keeps the set, so the
                // drag moves the whole selection). The empty canvas is not the
                // element's: the press goes back to the canvas' own plan, which
                // sweeps the marquee on a plain drag and leaves Shift to the
                // workspace outside it.
                if let Some(port) = patch::port_hit(rect, patch, cx, cy, scale) {
                    self.drag = Some(Drag::Wire {
                        id,
                        port,
                        area: rect,
                        scale,
                    });
                } else if let Some(hit_box) = patch::box_hit(rect, patch, cx, cy, scale) {
                    let set = if selected.contains(&hit_box) {
                        selected.clone()
                    } else {
                        vec![hit_box]
                    };
                    let grabbed = set
                        .iter()
                        .map(|&i| {
                            let (x, y) = patch::box_pos(rect, patch, i, scale);
                            (i, x, y)
                        })
                        .collect();
                    interact::graph_select(host, def_id, id, set);
                    self.drag = Some(Drag::Box {
                        id,
                        scale,
                        origin: (cx, cy),
                        grabbed,
                        moved: false,
                    });
                    out.push(GestureEffect::Redraw(def_id));
                }
            }
            // A lane's **header** is the element: the band beside the axis
            // carries the controls, so a press there is a mute, a solo or a
            // fader rather than a position. A press on the band's empty space
            // still means nothing (it names no sample), which is what it has
            // meant since the axis stopped locating from the header.
            WidgetKind::Track { .. } => {
                let Some((_, axis)) = interact::time_of(chain) else {
                    return false;
                };
                let Some(h) = interact::header_hit(host, def_id, id, rect, axis.body.x, cx, cy)
                else {
                    return false;
                };
                interact::header_set(host, def_id, id, h.part, h.fader.map(|r| (r, cx)));
                if let Some(r) = h.fader.filter(|_| h.part == interact::HeaderPart::Fader) {
                    self.drag = Some(Drag::LaneLevel { id, rect: r });
                }
                emit_lane(host, out, def_id, id, h.part);
                out.push(GestureEffect::Redraw(def_id));
            }
            // A **clip** is the element now: the layout places it on its lane's
            // axis, so the hit lands on it directly and the press reads the
            // rectangle that was drawn. Empty lane space and the ruler strip
            // are not a clip at all — the press falls back to the chain, where
            // the lane's plan locates the transport.
            WidgetKind::Clip { .. } => {
                let Some(lane) = interact::time_of(chain) else {
                    return false;
                };
                // The lane's own grid, from the container the axis came from.
                let snap = match host.widget_kind(def_id, lane.0) {
                    Some(WidgetKind::Track { snap, .. }) => *snap,
                    _ => 0.0,
                };
                // The clip's own axis, resolved by the layout and carried down
                // the hit chain — not re-derived from the lane's window here.
                let Some(local) = interact::local_time_of(chain) else {
                    return false;
                };
                if let Some(h) = interact::clip_hit(host, def_id, lane, local, cx, cy) {
                    // An automation clip: a break-point wins over the clip body
                    // (as it wins over a segment in the `bpf` view), and Ctrl+click
                    // adds one - or removes the one under the cursor. The same
                    // gestures, now on a lane.
                    if h.point.is_some() || (ctx.ctrl && h.has_curve) {
                        if ctx.ctrl {
                            if interact::clip_point_edit(
                                host,
                                def_id,
                                h.id,
                                h.point,
                                interact::ClipAt {
                                    rect: h.rect,
                                    local: h.local,
                                    cx,
                                    cy,
                                },
                            ) {
                                emit_points(host, out, def_id, h.id);
                                out.push(GestureEffect::Redraw(def_id));
                            }
                        } else if let Some(index) = h.point {
                            self.drag = Some(Drag::ClipPoint {
                                id: h.id,
                                index,
                                rect: h.rect,
                                nav_start: h.local.start,
                                nav_len: h.local.len,
                            });
                        }
                        return true;
                    }
                    let press_sample = interact::sample_at(
                        h.nav.start,
                        h.nav.len,
                        h.body.x as f64,
                        h.body.w as f64,
                        cx,
                    );
                    self.drag = Some(Drag::Clip {
                        id: h.id,
                        lane: h.lane,
                        part: h.part,
                        body_x: h.body.x as f64,
                        body_w: h.body.w as f64,
                        nav_start: h.nav.start,
                        nav_len: h.nav.len,
                        press_sample,
                        orig_offset: h.offset,
                        orig_dur: h.dur,
                        grid: snap,
                    });
                }
            }
            WidgetKind::Piano {
                min,
                max,
                active_min,
                active_max,
                pan,
                overview,
                velocity,
                channel,
                ref label,
                ..
            } => {
                let l = piano::layout(
                    rect,
                    min,
                    max,
                    overview,
                    label.is_some(),
                    host.metrics_for(def_id),
                );
                // A press on the overview strip grabs the visible window: the
                // drag pans it (relative, from the press snapshot). Gated by
                // `pan` — a fixed-range piano ignores the strip.
                if let Some(strip) = l.overview
                    && strip.contains(cx, cy)
                {
                    if pan {
                        self.drag = Some(Drag::PianoView {
                            id,
                            strip,
                            min0: l.min,
                            max0: l.max,
                            anchor: piano::overview_hit(strip, cx as f32),
                        });
                    }
                    return true;
                }
                // A press on a key plays it — inert outside the active range.
                if let Some(p) = piano::hit(&l, cx as f32, cy as f32) {
                    if !(active_min..=active_max).contains(&p) {
                        return true;
                    }
                    let vel = velocity.unwrap_or_else(|| piano::velocity_at(&l, p, cy as f32));
                    piano_note(host, out, def_id, id, p, vel, 1, channel);
                    self.drag = Some(Drag::PianoKey {
                        id,
                        layout: l,
                        pitch: p,
                        fixed_vel: velocity,
                        channel,
                    });
                    out.push(GestureEffect::Redraw(def_id));
                }
            }
            WidgetKind::Score(ref data) => {
                // A press names the engraved element under it by its MEI id —
                // the same id the client engraved from, so a driver resolves it
                // in its own score. Pressing blank paper clears the selection.
                let picked = data.hit(rect, cx as f32, cy as f32).map(str::to_string);
                if interact::score_select(host, def_id, id, picked.as_deref()) {
                    out.push(GestureEffect::Emit {
                        def_id,
                        widget_id: id,
                        args: interact::score_element_args(picked.as_deref()),
                    });
                    out.push(GestureEffect::Redraw(def_id));
                }
                // ...and, on an editable score, holding it drags the element's
                // pitch. A press that does not move stays a plain selection: the
                // release emits nothing more. A read-only page (the default)
                // still selects and reports the element above, but a drag does
                // nothing — the host holds no score, so an edit the client will
                // not apply is a gesture it cannot fulfil.
                if data.editable
                    && let Some(element) = picked
                {
                    self.drag = Some(Drag::ScoreStep {
                        id,
                        element,
                        rect,
                        origin_y: cy,
                        steps: 0,
                    });
                }
            }
            WidgetKind::PianoRoll { .. } => {
                let Some((_, axis)) = interact::time_of(chain) else {
                    return false;
                };
                let Some(h) = interact::pianoroll_hit(host, def_id, (id, rect, axis), cx, cy)
                else {
                    return false;
                };
                self.pianoroll_press(host, out, ctx, id, &h, cx, cy);
            }
            // A registered element gets the press on the live widget (the `kind`
            // matched above is the hit's copy), and answers the same way a
            // built-in arm does by hand: it consumed it, or it declines and the
            // press goes back up the chain. The claim is taken before anything
            // is delivered, so the element's borrow of the tree is over by the
            // time the event leaves.
            WidgetKind::Custom(_) => {
                let claim = match host.widget_kind_mut(def_id, id) {
                    Some(WidgetKind::Custom(el)) => el.press((cx, cy), rect),
                    _ => Claim::Decline,
                };
                match claim {
                    Claim::Decline => return false,
                    Claim::Take { value } => {
                        if let Some(v) = value {
                            deliver(host, out, def_id, id, v);
                        }
                        out.push(GestureEffect::Redraw(def_id));
                    }
                }
            }
            _ => {}
        }
        // Nothing the element wanted: the press goes back to the chain.
        self.drag.is_some() || out.len() > effects_before
    }

    /// Handles a plain (non-Shift) press on a `pianoroll`: start a note
    /// move/resize (a **selected** note moves the whole selection), a velocity
    /// drag (over a selected note, the whole selection's) or an OSC-marker
    /// drag; Ctrl+click adds or removes a note/marker; Alt+click toggles a note
    /// in/out of the multi-note selection; a press on empty grid drags the
    /// marquee — the shared time selection restricted in pitch, which fills the
    /// selected set.
    #[allow(clippy::too_many_arguments)] // one press: a hit, the context, a cursor
    fn pianoroll_press(
        &mut self,
        host: &mut Host,
        out: &mut Vec<GestureEffect>,
        ctx: &GestureCtx,
        id: i32,
        h: &interact::PianoRollHit,
        cx: f64,
        cy: f64,
    ) {
        let def_id = ctx.def_id;
        let nav = View {
            start: h.nav.start,
            len: h.nav.len,
        };
        match h.region {
            interact::PrRegion::Grid => {
                // Alt+click toggles a note in/out of the multi-note selection
                // (a non-rectangular selection, one note at a time).
                if ctx.alt {
                    if let Some(nh) = h.note {
                        interact::pianoroll_state_edit(host, def_id, id, |_, sel| {
                            pianoroll::toggle_selected(sel, nh.index);
                        });
                        out.push(GestureEffect::Redraw(def_id));
                    }
                    return;
                }
                if ctx.ctrl {
                    match h.note {
                        // Ctrl+click on a note removes it (the selection's
                        // indices shift down past it).
                        Some(nh) => {
                            interact::pianoroll_state_edit(host, def_id, id, |notes, sel| {
                                pianoroll::remove_note(notes, nh.index);
                                *sel = pianoroll::selection_after_removal(sel, nh.index);
                            });
                        }
                        // Ctrl+click on empty grid adds a note there, then drags
                        // its end to set the length until release.
                        None => {
                            let time = interact::snap(
                                pianoroll::time_at(h.grid, &nav, 0.0, cx as f32),
                                h.snap,
                            )
                            .max(0.0);
                            let pitch = pianoroll::y_to_pitch(cy as f32, h.lo, h.hi, h.grid)
                                .round()
                                .clamp(h.lo, h.hi);
                            let dur = if h.snap > 0.0 {
                                h.snap
                            } else {
                                (h.nav.len * 0.05).max(1.0)
                            };
                            let index = interact::pianoroll_notes_edit(host, def_id, id, |notes| {
                                pianoroll::insert_note(
                                    notes,
                                    pianoroll::Note::new(time, dur, pitch),
                                )
                            });
                            if let Some(index) = index {
                                self.drag = Some(Drag::Note {
                                    id,
                                    index,
                                    part: pianoroll::NotePart::End,
                                    grid: h.grid,
                                    nav_start: h.nav.start,
                                    nav_len: h.nav.len,
                                    lo: h.lo,
                                    hi: h.hi,
                                    press_time: time,
                                    orig_start: time,
                                    orig_dur: dur,
                                    snap: h.snap,
                                });
                            }
                        }
                    }
                    host.sync_track_totals();
                    emit_notes(host, out, def_id, id);
                    out.push(GestureEffect::Redraw(def_id));
                    return;
                }
                // Move (body) or resize (edge) the note under the cursor.
                // Grabbing the body of a **selected** note moves the whole
                // selection rigidly; grabbing an unselected one drops the
                // selection first (the single-note gesture, as before). Empty
                // grid is nothing of the element's: the press goes back to the
                // roll's own plan, whose plain drag sweeps the selection --
                // the shared time span, restricted in pitch.
                if let Some(nh) = h.note {
                    let press_time = pianoroll::time_at(h.grid, &nav, 0.0, cx as f32);
                    if nh.part == pianoroll::NotePart::Body {
                        let orig =
                            interact::pianoroll_state_edit(host, def_id, id, |notes, sel| {
                                if !sel.contains(&nh.index) {
                                    sel.clear();
                                    return Vec::new();
                                }
                                // The grabbed note's snapshot leads (the
                                // snap anchor).
                                let mut idx = sel.clone();
                                idx.retain(|&i| i != nh.index);
                                idx.insert(0, nh.index);
                                idx.iter()
                                    .filter_map(|&i| notes.get(i).map(|n| (i, n.start, n.pitch)))
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default();
                        if !orig.is_empty() {
                            let press_pitch = pianoroll::y_to_pitch(cy as f32, h.lo, h.hi, h.grid);
                            self.drag = Some(Drag::NoteBlock {
                                id,
                                grid: h.grid,
                                nav_start: h.nav.start,
                                nav_len: h.nav.len,
                                lo: h.lo,
                                hi: h.hi,
                                press_time,
                                press_pitch,
                                snap: h.snap,
                                orig,
                            });
                            return;
                        }
                    }
                    let (orig_start, orig_dur) =
                        note_at(host, def_id, id, nh.index).unwrap_or((0.0, 0.0));
                    self.drag = Some(Drag::Note {
                        id,
                        index: nh.index,
                        part: nh.part,
                        grid: h.grid,
                        nav_start: h.nav.start,
                        nav_len: h.nav.len,
                        lo: h.lo,
                        hi: h.hi,
                        press_time,
                        orig_start,
                        orig_dur,
                        snap: h.snap,
                    });
                }
            }
            interact::PrRegion::Velocity => {
                if let Some(nh) = h.note {
                    // Over a **selected** note the whole selection's velocities
                    // nudge together (relative, from a press snapshot); over an
                    // unselected one the single bar follows the cursor.
                    let orig = interact::pianoroll_state_edit(host, def_id, id, |notes, sel| {
                        if !sel.contains(&nh.index) {
                            return Vec::new();
                        }
                        sel.iter()
                            .filter_map(|&i| notes.get(i).map(|n| (i, n.velocity)))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                    if !orig.is_empty() {
                        let lane = h.region_rect;
                        self.drag = Some(Drag::VelocityBlock {
                            id,
                            lane,
                            press_velocity: pianoroll::velocity_at(lane, cy),
                            orig,
                        });
                        return;
                    }
                    self.drag = Some(Drag::Velocity {
                        id,
                        index: nh.index,
                        lane: h.region_rect,
                    });
                }
            }
            interact::PrRegion::Osc => {
                if ctx.ctrl {
                    match h.osc_index {
                        Some(index) => {
                            interact::pianoroll_osc_edit(host, def_id, id, |osc| {
                                if index < osc.len() {
                                    osc.remove(index);
                                }
                            });
                        }
                        None => {
                            let time = interact::snap(
                                pianoroll::time_at(h.grid, &nav, 0.0, cx as f32),
                                h.snap,
                            )
                            .max(0.0);
                            interact::pianoroll_osc_edit(host, def_id, id, |osc| {
                                osc.push(pianoroll::OscMark { time, label: None });
                            });
                        }
                    }
                    host.sync_track_totals();
                    emit_osc(host, out, def_id, id);
                    out.push(GestureEffect::Redraw(def_id));
                } else if let Some(index) = h.osc_index {
                    self.drag = Some(Drag::OscMark {
                        id,
                        index,
                        grid: h.grid,
                        nav_start: h.nav.start,
                        nav_len: h.nav.len,
                        snap: h.snap,
                    });
                }
            }
        }
    }
}
