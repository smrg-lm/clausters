//! **A roll's curves**: the sequence's under its plane and each note's own.
//!
//! A roll is a plane of events, and it holds curves the way a multitrack
//! holds automation: the **sequence's** is a curve over the whole sequence
//! (a CC, a bend, a pressure, a control), drawn as a row of its own under the
//! plane; a **note's** is a curve over that note alone, drawn as a layer
//! inside its box -- and past the box's end when it runs into the release,
//! the box itself staying the span between the note's on and its off. A bend
//! is the one note curve drawn in the plane rather than normalized inside the
//! box: its layer spans the pitches its range covers, so
//! the line is the trajectory the note's pitch takes.
//!
//! Each curve is a `curve` element's body -- the same one a multitrack's rows
//! and layers are -- so its break-points are pressed, dragged and bent the way
//! they are everywhere, and the whole gesture reports once, on release, as a
//! `points` message listing every curve's points (a curve names itself).
//!
//! The wire:
//!
//! - `curves`: flat `name label min max height` quintuples, one row each;
//! - `layers`: flat `name note label min max pitch` sextuples -- `note` the id
//!   of the note it is over, `pitch` true for a bend drawn in the plane;
//! - `points`: flat `name at value shape curve` quintuples, `at` in the roll's
//!   own units, a layer's measured from its note's start.

use std::collections::HashMap;

use super::*;
use crate::host::elements::curve;
use crate::host::graphics::choice::{self, View as ChoiceView};
use crate::host::graphics::track;

/// A curve of the sequence, as a row under the plane.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Row {
    pub(super) name: String,
    pub(super) label: String,
    pub(super) min: f64,
    pub(super) max: f64,
    pub(super) height: f32,
}

/// A curve over one note.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Layer {
    pub(super) name: String,
    pub(super) note: u64,
    pub(super) label: String,
    pub(super) min: f64,
    pub(super) max: f64,
    /// A bend, drawn in the plane over the pitches `min..max` semitones from
    /// the note, rather than normalized inside its box.
    pub(super) pitch: bool,
}

/// The rows' share of a roll's height is at most this much of it, so a roll
/// with many rows still has a plane.
const MAX_ROWS_SHARE: f32 = 0.5;

/// The size the tabs over the lane are written at: a control's, at the
/// window's scale.
fn tab_size(m: &Metrics) -> f32 {
    crate::host::font::DEFAULT_SIZE * m.ui_scale
}

/// **The share of a roll one lane takes** when its owner shows one at a time
/// (`lane`): room for a value ruler beside it and a hand to draw in it.
const LANE_SHARE: f32 = 0.28;

/// The lane of the notes' velocities, by its name.
pub(super) const VELOCITY: &str = "velocity";

/// How wide a velocity bar is drawn, in pixels.
const BAR_W: f32 = 3.0;

/// The velocity the height `y` is in the lane `body`: its bottom 1, its top
/// 127.
fn velocity_at(y: f64, body: Rect) -> i32 {
    let frac = 1.0 - (y - f64::from(body.y)) / f64::from(body.h.max(1.0));
    (frac * 127.0).round().clamp(1.0, 127.0) as i32
}

/// The lanes offered as tabs (`lane_tabs`): flat `name label` pairs.
pub(super) fn parse_lane_tabs(v: &Value) -> Vec<(String, String)> {
    let flat: Vec<String> = v
        .as_array()
        .map(|a| {
            a.iter()
                .map(|x| x.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();
    flat.as_chunks::<2>()
        .0
        .iter()
        .map(|[name, label]| (name.clone(), label.clone()))
        .collect()
}

pub(super) fn parse_rows(props: &Map<String, Value>) -> Vec<Row> {
    let Some(Value::Array(items)) = props.get("curves") else {
        return Vec::new();
    };
    items
        .as_chunks::<5>()
        .0
        .iter()
        .filter_map(|c| {
            Some(Row {
                name: c[0].as_str()?.to_string(),
                label: c[1].as_str().unwrap_or("").to_string(),
                min: c[2].as_f64().unwrap_or(0.0),
                max: c[3].as_f64().unwrap_or(1.0),
                height: c[4].as_f64().unwrap_or(40.0) as f32,
            })
        })
        .collect()
}

pub(super) fn parse_layers(props: &Map<String, Value>) -> Vec<Layer> {
    let Some(Value::Array(items)) = props.get("layers") else {
        return Vec::new();
    };
    items
        .as_chunks::<6>()
        .0
        .iter()
        .filter_map(|c| {
            Some(Layer {
                name: c[0].as_str()?.to_string(),
                note: c[1]
                    .as_f64()
                    .or_else(|| c[1].as_str()?.parse().ok())?
                    .max(0.0) as u64,
                label: c[2].as_str().unwrap_or("").to_string(),
                min: c[3].as_f64().unwrap_or(0.0),
                max: c[4].as_f64().unwrap_or(1.0),
                pitch: truthy(&c[5]).unwrap_or(false),
            })
        })
        .collect()
}

/// The `points` prop, by curve: each curve's own flat `at value shape curve`
/// list, as a `curve` body reads it.
pub(super) fn parse_points(props: &Map<String, Value>) -> HashMap<String, Vec<f64>> {
    let Some(Value::Array(items)) = props.get("points") else {
        return HashMap::new();
    };
    let mut out: HashMap<String, Vec<f64>> = HashMap::new();
    for p in items.as_chunks::<5>().0 {
        let Some(name) = p[0].as_str() else {
            continue;
        };
        out.entry(name.to_string())
            .or_default()
            .extend(p[1..].iter().map(|v| v.as_f64().unwrap_or(0.0)));
    }
    out
}

impl Notes {
    /// Every curve's body, built from the rows, the layers and the points the
    /// owner last sent.
    pub(super) fn rebuild_bodies(&mut self) {
        let bodies = self
            .rows
            .iter()
            .map(|r| (&r.name, r.min, r.max))
            .chain(self.layers.iter().map(|l| (&l.name, l.min, l.max)))
            .map(|(name, min, max)| {
                let mut props = Map::new();
                props.insert("min".into(), Value::from(min));
                props.insert("max".into(), Value::from(max));
                if let Some(flat) = self.curve_points.get(name) {
                    props.insert("points".into(), Value::from(flat.clone()));
                }
                (name.clone(), curve::body(&props))
            })
            .collect();
        self.bodies = bodies;
    }

    /// How tall the rows are drawn under a plane of `height`: their own
    /// heights, together at most half of it.
    ///
    /// A row's height is logical, as the wire states it, and `height` is in
    /// device pixels, so the rows are resolved at the window's scale first.
    ///
    /// **A roll whose owner shows one lane at a time** (`lane`) gives the lane
    /// shown a share of its height, a row's or the velocities', and none
    /// when nothing is shown.
    pub(super) fn rows_h(&self, height: f32, m: &Metrics) -> f32 {
        if let Some(lane) = &self.lane {
            let shown = lane == VELOCITY || self.rows.iter().any(|r| &r.name == lane);
            return if shown {
                (height * LANE_SHARE).max(0.0)
            } else {
                0.0
            };
        }
        let want: f32 = self.rows.iter().map(|r| r.height).sum::<f32>() * m.ui_scale;
        want.min(height * MAX_ROWS_SHARE).max(0.0)
    }

    /// **How tall the tabs over the lane are**: one row of tabs where the
    /// owner shows one lane at a time and names the lanes it offers
    /// (`lane_tabs`), none otherwise.
    pub(super) fn tabs_h(&self, m: &Metrics) -> f32 {
        if self.lane.is_none() || self.lane_tabs.is_empty() {
            return 0.0;
        }
        choice::body_h(ChoiceView::Tabs, self.lane_tabs.len(), tab_size(m), m)
    }

    /// **The tabs that choose the lane, right over it**, across the plane's
    /// column: where a hand looking at the lane finds what it shows.
    pub(super) fn tabs_rect(&self, rect: Rect, indent: f32, m: &Metrics) -> Option<Rect> {
        let h = self.tabs_h(m);
        if h <= 0.0 {
            return None;
        }
        let full = pianoroll::regions(rect, self.editor.strip(), self.osc_markers, indent, m);
        let room = self.rows_h(full.grid.h, m);
        let y = full.grid.y + full.grid.h - room - h;
        Some(Rect::new(full.grid.x, y, full.grid.w, h))
    }

    /// The tabs' labels, and which one is the lane shown.
    pub(super) fn tabs_said(&self) -> (Vec<String>, usize) {
        let shown = self.lane.as_deref().unwrap_or_default();
        let index = self
            .lane_tabs
            .iter()
            .position(|(name, _)| name == shown)
            .unwrap_or(0);
        let labels = self.lane_tabs.iter().map(|(_, l)| l.clone()).collect();
        (labels, index)
    }

    /// Draws the tabs over the lane, the one under the pointer lit.
    pub(super) fn draw_lane_tabs(&self, d: &mut Draw, ctx: &Ctx) {
        let Some(rect) = self.tabs_rect(ctx.rect, ctx.indent, ctx.metrics) else {
            return;
        };
        let size = tab_size(ctx.metrics);
        let (labels, index) = self.tabs_said();
        let hover = ctx
            .hovered
            .then_some(ctx.world.cursor)
            .flatten()
            .and_then(|(x, y)| {
                let parts = choice::parts(ChoiceView::Tabs, &labels, rect, size, ctx.metrics);
                choice::part_at(&parts, x, y)
            });
        let (mesh, _, theme) = d.parts();
        mesh.rect(rect, theme.lane);
        choice::draw(d, ChoiceView::Tabs, &labels, index, None, rect, size, hover);
    }

    /// **A tab pressed**: the owner asked to show its lane (`lane` and the
    /// lane's name), which it answers with the lane shown. The tabs that did
    /// not fit gather behind one, which steps through them.
    pub(super) fn press_lane_tabs(&mut self, at: (f64, f64), input: &Input) -> Option<Claim> {
        let rect = self.tabs_rect(input.rect, input.indent, input.metrics)?;
        if !rect.contains(at.0, at.1) {
            return None;
        }
        let (labels, index) = self.tabs_said();
        let parts = choice::parts(
            ChoiceView::Tabs,
            &labels,
            rect,
            tab_size(input.metrics),
            input.metrics,
        );
        let chosen = match choice::part_at(&parts, at.0, at.1) {
            Some(choice::Part::Option(i)) => Some(i),
            Some(choice::Part::More(fit)) => Some(if index >= fit && index + 1 < labels.len() {
                index + 1
            } else {
                fit
            }),
            _ => None,
        };
        let events =
            chosen
                .and_then(|i| self.lane_tabs.get(i))
                .map_or_else(Events::none, |(name, _)| {
                    Events::message(vec![
                        OscType::String("lane".into()),
                        OscType::String(name.clone()),
                    ])
                });
        Some(Claim::Take(Take {
            events,
            ..Take::default()
        }))
    }

    /// **The rows drawn**: every one, or the lane shown where the owner shows
    /// one at a time.
    pub(super) fn shown_rows(&self) -> Vec<&Row> {
        match &self.lane {
            None => self.rows.iter().collect(),
            Some(lane) => self.rows.iter().filter(|r| &r.name == lane).collect(),
        }
    }

    /// **The velocities' lane**, label cell and body, when it is the lane
    /// shown.
    pub(super) fn velocity_rects(
        &self,
        rect: Rect,
        indent: f32,
        m: &Metrics,
    ) -> Option<(Rect, Rect)> {
        if self.lane.as_deref() != Some(VELOCITY) {
            return None;
        }
        let full = pianoroll::regions(rect, self.editor.strip(), self.osc_markers, indent, m);
        let room = self.rows_h(full.grid.h, m);
        let y = full.grid.y + full.grid.h - room;
        Some((
            Rect::new(full.keyboard.x, y, full.keyboard.w, room),
            Rect::new(full.grid.x, y, full.grid.w, room),
        ))
    }

    /// Each row's label cell (in the keyboard's column) and body, top to
    /// bottom under the plane, scaled into the share the rows get.
    pub(super) fn row_rects(&self, rect: Rect, indent: f32, m: &Metrics) -> Vec<(Rect, Rect)> {
        let full = pianoroll::regions(rect, self.editor.strip(), self.osc_markers, indent, m);
        let room = self.rows_h(full.grid.h, m);
        let shown = self.shown_rows();
        let total: f32 = shown.iter().map(|r| r.height).sum();
        let scale = if total > 0.0 { room / total } else { 0.0 };
        let mut y = full.grid.y + full.grid.h - room;
        shown
            .into_iter()
            .map(|row| {
                let h = row.height * scale;
                let label = Rect::new(full.keyboard.x, y, full.keyboard.w, h);
                let body = Rect::new(full.grid.x, y, full.grid.w, h);
                y += h;
                (label, body)
            })
            .collect()
    }

    /// **Where every drawn curve is, and the space it is drawn against**: the
    /// rows on the roll's own axis, and each layer over its note -- a bend's
    /// over the pitches its range spans, the rest inside the box -- measured
    /// from the note's start and reaching its last point.
    pub(super) fn curves_on_screen(
        &self,
        rect: Rect,
        indent: f32,
        m: &Metrics,
        time: Option<TimeSpace>,
    ) -> Vec<(&str, Rect, TimeSpace)> {
        let nav = self.view(time);
        let span = self.span().max(nav.start + nav.len);
        let active = |name: &str| self.layer.as_deref() == Some(name);
        let mut out = Vec::new();
        for (row, (_, body)) in self
            .shown_rows()
            .into_iter()
            .zip(self.row_rects(rect, indent, m))
        {
            if body.w > 0.0 && body.h > 0.0 {
                let mut space = TimeSpace::of(nav, span);
                space.active = active(&row.name);
                out.push((row.name.as_str(), body, space));
            }
        }
        let grid = self.regions(rect, indent, m).grid;
        let axis = self.axis(m);
        for layer in &self.layers {
            let Some(note) = self.notes.iter().find(|n| n.id == layer.note && n.id != 0) else {
                continue;
            };
            // **A note's curve may run past its off**, into the release: the
            // layer reaches its last point when that is later than the note's
            // end. The box itself stays the span between the on and the off --
            // it draws the message, not what sounds.
            let reach = self
                .bodies
                .get(&layer.name)
                .and_then(|b| b.points().last())
                .map_or(0.0, |p| p.time)
                .max(note.dur);
            let tail = Note {
                dur: reach,
                ..*note
            };
            let Some(boxed) = pianoroll::note_rect(grid, &nav, 0.0, &tail, axis) else {
                continue;
            };
            let place = if layer.pitch {
                // The bend's range, in the plane: the curve's `max` at the top.
                let top = axis.y(note.pitch + layer.max as f32, grid).max(grid.y);
                let bottom = axis
                    .y(note.pitch + layer.min as f32, grid)
                    .min(grid.y + grid.h);
                Rect::new(boxed.x, top, boxed.w, (bottom - top).max(1.0))
            } else {
                boxed
            };
            // A point is kept from the note's start to the roll's end, so a
            // hand dragging the last one out lengthens the tail.
            let local = track::clip_local_view(grid, &nav, note.start, reach, boxed);
            let mut space = TimeSpace::of(local, (span - note.start).max(reach));
            space.active = active(&layer.name);
            out.push((layer.name.as_str(), place, space));
        }
        out
    }

    /// The rows' backgrounds and labels, then every curve over its place --
    /// drawn last of the contents, being the one thing a hand edits on top of
    /// what it shapes.
    pub(super) fn draw_curves(&self, d: &mut Draw, ctx: &Ctx) {
        let one = self.lane.is_some();
        for (row, (label, body)) in
            self.shown_rows()
                .into_iter()
                .zip(self.row_rects(ctx.rect, ctx.indent, ctx.metrics))
        {
            let (mesh, m, theme) = d.parts();
            mesh.rect(body, theme.osc_markers);
            mesh.rect(Rect::new(body.x, body.y, body.w, 1.0), theme.frame);
            if one {
                // **one lane at a time is named by its tab**: its cell holds
                // its value ruler
                value_ruler(mesh, m, theme, label, row.min, row.max);
            } else {
                font::text(
                    mesh,
                    &row.label,
                    label.x + m.pad,
                    label.y + 2.0,
                    m.caption_scale,
                    theme.ruler_text,
                );
            }
        }
        self.draw_velocities(d, ctx);
        self.draw_lane_tabs(d, ctx);
        for (name, rect, space) in
            self.curves_on_screen(ctx.rect, ctx.indent, ctx.metrics, ctx.time)
        {
            if let Some(body) = self.bodies.get(name) {
                body.draw_body(d, rect, &space);
            }
        }
    }

    /// **What the hover reads over a curve**, or `None` when the pointer is on
    /// none: the curve's whole label -- the row's cell is too narrow to hold it
    /// -- and its value, a break-point's own when the pointer is on one. A
    /// row answers anywhere over itself, a layer on its line or its points,
    /// the active layer before the rest.
    pub(super) fn curve_readout(
        &self,
        at: (f64, f64),
        rect: Rect,
        indent: f32,
        m: &Metrics,
        time: Option<TimeSpace>,
    ) -> Option<String> {
        // A row's label cell is too narrow for its label: over it, the whole
        // one is read.
        for (row, (label, _)) in self
            .shown_rows()
            .into_iter()
            .zip(self.row_rects(rect, indent, m))
        {
            if label.contains(at.0, at.1) {
                return Some(row.label.clone());
            }
        }
        if let Some(reading) = self.velocity_readout(at, rect, indent, m, time) {
            return Some(reading);
        }
        let drawn = self.curves_on_screen(rect, indent, m, time);
        let active = drawn
            .iter()
            .filter(|(name, ..)| self.layer.as_deref() == Some(*name));
        for (name, place, space) in active.chain(drawn.iter().rev()) {
            let row = self.rows.iter().find(|r| r.name == *name);
            let layer = self.layers.iter().find(|l| l.name == *name);
            let (label, min, max, pitch) = match (row, layer) {
                (Some(r), _) => (&r.label, r.min, r.max, false),
                (None, Some(l)) => (&l.label, l.min, l.max, l.pitch),
                (None, None) => continue,
            };
            let Some(body) = self.bodies.get(*name) else {
                continue;
            };
            let Some((value, on_point)) = body.hover_value(at, *place, *space, m, row.is_some())
            else {
                continue;
            };
            // A bend reads in semitones, signed; anything else to its range.
            let value = if pitch {
                format!("{value:+.2} st")
            } else {
                crate::host::graphics::bpf::readout_value(value as f64, min, max)
            };
            return Some(if on_point {
                format!("{label}  point {value}")
            } else {
                format!("{label}  {value}")
            });
        }
        None
    }

    /// The notes the velocity lane shows: the ones the roll takes, so in
    /// notation mode the voice edited and in MIDI mode the channel edited.
    pub(super) fn velocity_shown(&self) -> Vec<usize> {
        (0..self.notes.len())
            .filter(|i| !self.written.dimmed(*i))
            .collect()
    }

    /// Where note `i`'s bar stands in the lane `body`, and how tall it is.
    fn bar_of(&self, i: usize, body: Rect, nav: &View) -> Option<Rect> {
        let note = self.notes.get(i)?;
        let x = pianoroll::to_x(note.start, nav, body) as f32;
        if x < body.x - 1.0 || x > body.x + body.w + 1.0 {
            return None;
        }
        let h = body.h * (note.velocity.clamp(0, 127) as f32 / 127.0);
        Some(Rect::new(x - BAR_W * 0.5, body.y + body.h - h, BAR_W, h))
    }

    /// **The velocity lane**: a bar at each note's start, as tall as its
    /// velocity, the selected ones in the selection's ink, and its value
    /// ruler in the cell beside it.
    pub(super) fn draw_velocities(&self, d: &mut Draw, ctx: &Ctx) {
        let Some((cell, body)) = self.velocity_rects(ctx.rect, ctx.indent, ctx.metrics) else {
            return;
        };
        let nav = self.view(ctx.time);
        let (mesh, m, theme) = d.parts();
        mesh.rect(body, theme.osc_markers);
        mesh.rect(Rect::new(body.x, body.y, body.w, 1.0), theme.frame);
        value_ruler(mesh, m, theme, cell, 0.0, 127.0);
        for i in self.velocity_shown() {
            let Some(bar) = self.bar_of(i, body, &nav) else {
                continue;
            };
            let ink = if self.selected.contains(&i) {
                theme.cursor
            } else {
                theme.note_edge
            };
            mesh.rect(bar, ink);
            mesh.rect(Rect::new(bar.x - 1.0, bar.y, BAR_W + 2.0, 1.0), ink);
        }
    }

    /// What the hover reads over the velocity lane: the velocity of the bar
    /// under the pointer, or the value the pointer is at.
    pub(super) fn velocity_readout(
        &self,
        at: (f64, f64),
        rect: Rect,
        indent: f32,
        m: &Metrics,
        time: Option<TimeSpace>,
    ) -> Option<String> {
        let (_, body) = self.velocity_rects(rect, indent, m)?;
        if !body.contains(at.0, at.1) {
            return None;
        }
        let nav = self.view(time);
        if let Some(i) = self.bar_under(at, body, &nav, m) {
            return Some(format!("velocity  {}", self.notes[i].velocity));
        }
        Some(format!("velocity  {}", velocity_at(at.1, body)))
    }

    /// The note whose bar is under `at`, the nearest one.
    fn bar_under(&self, at: (f64, f64), body: Rect, nav: &View, m: &Metrics) -> Option<usize> {
        let reach = f64::from(BAR_W * 0.5 + m.hit_slop);
        self.velocity_shown()
            .into_iter()
            .filter_map(|i| {
                let x = pianoroll::to_x(self.notes[i].start, nav, body);
                let off = (x - at.0).abs();
                (off <= reach).then_some((i, off))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    }

    /// **A press in the velocity lane**: on a bar it takes that note's
    /// velocity, or the selection's when the note is selected; anywhere else
    /// it starts a ramp.
    pub(super) fn press_velocity(&mut self, at: (f64, f64), input: &Input) -> Option<Claim> {
        let (_, body) = self.velocity_rects(input.rect, input.indent, input.metrics)?;
        if !body.contains(at.0, at.1) {
            return None;
        }
        if !self.editable {
            return Some(Claim::take());
        }
        // **the pencil draws the heights freehand**, from the press on
        if self.tool == Tool::Draw {
            self.paint_velocities(at, at, input);
            self.drag = Some(Drag::Paint { last: at });
            return Some(Claim::take());
        }
        let nav = self.view(input.time);
        let orig_of = |notes: &[Note], which: &[usize]| -> Vec<(usize, i32)> {
            which
                .iter()
                .filter_map(|&i| notes.get(i).map(|n| (i, n.velocity)))
                .collect()
        };
        self.drag = Some(match self.bar_under(at, body, &nav, input.metrics) {
            Some(i) => {
                let which = if self.selected.contains(&i) {
                    self.selected.clone()
                } else {
                    vec![i]
                };
                // the bar goes to where it was pressed, and the rest with it
                let to = velocity_at(at.1, body);
                let orig = orig_of(&self.notes, &which);
                let lead = self.notes[i].velocity;
                for (j, v) in &orig {
                    self.notes[*j].velocity = (v + to - lead).clamp(1, 127);
                }
                Drag::Bars {
                    press_y: at.1,
                    orig: orig
                        .into_iter()
                        .map(|(j, v)| (j, (v + to - lead).clamp(1, 127)))
                        .collect(),
                }
            }
            None => Drag::Ramp {
                from: at,
                orig: orig_of(&self.notes, &self.velocity_shown()),
            },
        });
        Some(Claim::take())
    }

    /// The bars in hand, moved by the height the pointer travelled.
    pub(super) fn drag_bars(
        &mut self,
        press_y: f64,
        orig: &[(usize, i32)],
        at: (f64, f64),
        input: &Input,
    ) {
        let Some((_, body)) = self.velocity_rects(input.rect, input.indent, input.metrics) else {
            return;
        };
        let dv = ((press_y - at.1) / f64::from(body.h.max(1.0)) * 127.0).round() as i32;
        for (i, v) in orig {
            if let Some(n) = self.notes.get_mut(*i) {
                n.velocity = (v + dv).clamp(1, 127);
            }
        }
    }

    /// The ramp drawn so far: the notes that start between the press and the
    /// pointer on its line, every other note at what it had.
    pub(super) fn drag_ramp(
        &mut self,
        from: (f64, f64),
        orig: &[(usize, i32)],
        at: (f64, f64),
        input: &Input,
    ) {
        let Some((_, body)) = self.velocity_rects(input.rect, input.indent, input.metrics) else {
            return;
        };
        let nav = self.view(input.time);
        let (x0, x1) = (from.0.min(at.0), from.0.max(at.0));
        let (v0, v1) = (velocity_at(from.1, body), velocity_at(at.1, body));
        for (i, v) in orig {
            let x = pianoroll::to_x(self.notes[*i].start, &nav, body);
            let along = if (at.0 - from.0).abs() < 1e-9 {
                0.0
            } else {
                (x - from.0) / (at.0 - from.0)
            };
            self.notes[*i].velocity = if x >= x0 && x <= x1 {
                (f64::from(v0) + (f64::from(v1) - f64::from(v0)) * along)
                    .round()
                    .clamp(1.0, 127.0) as i32
            } else {
                *v
            };
        }
    }

    /// **The pencil's stroke from `from` to `to` over the velocity lane**:
    /// every note shown whose bar stands between the two takes the height the
    /// stroke is at over it; a press alone takes the bar under it.
    pub(super) fn paint_velocities(&mut self, from: (f64, f64), to: (f64, f64), input: &Input) {
        let Some((_, body)) = self.velocity_rects(input.rect, input.indent, input.metrics) else {
            return;
        };
        let nav = self.view(input.time);
        if (to.0 - from.0).abs() < 1e-9 {
            if let Some(i) = self.bar_under(to, body, &nav, input.metrics) {
                self.notes[i].velocity = velocity_at(to.1, body);
            }
            return;
        }
        let (x0, x1) = (from.0.min(to.0), from.0.max(to.0));
        for i in self.velocity_shown() {
            let x = pianoroll::to_x(self.notes[i].start, &nav, body);
            if x >= x0 && x <= x1 {
                let along = (x - from.0) / (to.0 - from.0);
                let y = from.1 + (to.1 - from.1) * along;
                self.notes[i].velocity = velocity_at(y, body);
            }
        }
    }

    fn on_curve<'a>(input: &Input<'a>, rect: Rect, space: TimeSpace) -> Input<'a> {
        Input {
            rect,
            indent: 0.0,
            time: Some(space),
            ..*input
        }
    }

    /// **A press a curve answers**: anywhere on a row (the row is the curve's
    /// own), or on a layer's line or break-points -- a layer's field is the
    /// note's, which stays draggable under it. Ctrl on a layer's field adds a
    /// point, as on a row. The active layer is asked first.
    pub(super) fn press_curve(&mut self, at: (f64, f64), input: &Input) -> Option<Claim> {
        if !self.editable {
            return None;
        }
        let drawn = self.curves_on_screen(input.rect, input.indent, input.metrics, input.time);
        let rows = self.shown_rows().len();
        let wants = |i: usize, name: &str, rect: Rect, space: TimeSpace| {
            if !rect.contains(at.0, at.1) {
                return false;
            }
            if i < rows || input.mods.ctrl {
                return true;
            }
            self.bodies
                .get(name)
                .is_some_and(|b| b.layer_hit(at, &Self::on_curve(input, rect, space)))
        };
        let (name, rect, mut space) = drawn
            .iter()
            .enumerate()
            .find(|(i, (name, rect, space))| {
                self.layer.as_deref() == Some(*name) && wants(*i, name, *rect, *space)
            })
            .or_else(|| {
                drawn
                    .iter()
                    .enumerate()
                    .rev()
                    .find(|(i, (name, rect, space))| wants(*i, name, *rect, *space))
            })
            .map(|(_, (name, rect, space))| (name.to_string(), *rect, *space))?;
        space.active = true;
        self.layer = Some(name.clone());
        let before = self.points_of(&name);
        let sub = Self::on_curve(input, rect, space);
        let Claim::Take(take) = self.bodies.get_mut(&name)?.press(at, &sub) else {
            // Nothing of the curve's after all: the press goes on to the note
            // under it, and the layer it moved to stays where it moved.
            return None;
        };
        // A point added or removed at the press is already the edit: it is
        // reported now, and the release reports only what moves after it.
        let now = self.points_of(&name);
        let events = if now != before {
            Events::message(self.points_args())
        } else {
            Events::none()
        };
        self.holding = Some((name, now));
        Some(Claim::Take(Take {
            events,
            edge_scroll: true,
            ..take
        }))
    }

    /// Where a curve by name was drawn, and against what.
    fn curve_place(&self, name: &str, input: &Input) -> Option<(Rect, TimeSpace)> {
        self.curves_on_screen(input.rect, input.indent, input.metrics, input.time)
            .into_iter()
            .find(|(n, ..)| *n == name)
            .map(|(_, rect, space)| (rect, space))
    }

    /// A curve in hand follows it, reporting nothing on the way.
    pub(super) fn drag_curve(&mut self, at: (f64, f64), input: &Input) -> bool {
        let Some((name, _)) = self.holding.clone() else {
            return false;
        };
        if let Some((rect, space)) = self.curve_place(&name, input) {
            let sub = Self::on_curve(input, rect, space);
            if let Some(body) = self.bodies.get_mut(&name) {
                body.drag(at, &sub);
            }
        }
        true
    }

    /// The curve's gesture ends: its points report once, if they moved.
    pub(super) fn release_curve(
        &mut self,
        at: (f64, f64),
        inside: bool,
        input: &Input,
    ) -> Option<Events> {
        let (name, before) = self.holding.take()?;
        if let Some((rect, space)) = self.curve_place(&name, input) {
            let sub = Self::on_curve(input, rect, space);
            if let Some(body) = self.bodies.get_mut(&name) {
                body.release(at, inside, &sub);
            }
        }
        Some(if self.points_of(&name) == before {
            Events::none()
        } else {
            self.points_event()
        })
    }

    fn points_of(&self, name: &str) -> Value {
        self.bodies
            .get(name)
            .map(|b| crate::host::structures::points::points_json(b.points()))
            .unwrap_or(Value::Null)
    }

    pub(super) fn points_event(&self) -> Events {
        Events::message(self.points_args())
    }

    /// The `"points"` report: every curve's break-points, each naming its
    /// curve, in the roll's units (a layer's from its note's start).
    fn points_args(&self) -> Vec<OscType> {
        let mut args = vec![OscType::String("points".into())];
        let names = self
            .rows
            .iter()
            .map(|r| &r.name)
            .chain(self.layers.iter().map(|l| &l.name));
        for name in names {
            let Some(body) = self.bodies.get(name) else {
                continue;
            };
            for p in body.points() {
                args.push(OscType::String(name.clone()));
                args.push(OscType::Double(p.time));
                args.push(OscType::Float(p.value));
                args.push(OscType::Int(p.shape));
                args.push(OscType::Float(p.curve));
            }
        }
        args
    }
}

/// **A lane's value ruler**, in its label cell: the top of its range at the
/// top and the bottom at the bottom.
pub(super) fn value_ruler(
    mesh: &mut crate::host::paint::Mesh,
    m: &Metrics,
    theme: &crate::host::theme::Theme,
    cell: Rect,
    min: f64,
    max: f64,
) {
    let scale = m.caption_scale;
    let say = |v: f64| crate::host::graphics::bpf::readout_value(v, min, max);
    font::text(
        mesh,
        &say(max),
        cell.x + m.pad,
        cell.y + 2.0,
        scale,
        theme.ruler_text,
    );
    let low = cell.y + cell.h - font::line_advance(scale) - 2.0;
    if low > cell.y + font::line_advance(scale) {
        font::text(
            mesh,
            &say(min),
            cell.x + m.pad,
            low,
            scale,
            theme.ruler_text,
        );
    }
}
