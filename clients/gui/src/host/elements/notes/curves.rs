//! **A roll's curves**: the lanes under its plane and each note's own.
//!
//! A roll is a plane of events, and it holds curves the way a multitrack
//! holds automation: a **lane** is a curve over the whole sequence (a CC, a
//! bend, a pressure, a control), drawn as a row of its own under the plane;
//! a note's **expression** is a curve over that note alone, drawn as a layer
//! inside its box. A bend is the one expression drawn in the plane rather than
//! normalized inside the box: its layer spans the pitches its range covers, so
//! the line is the trajectory the note's pitch takes.
//!
//! Each curve is a `curve` element's body -- the same one a multitrack's rows
//! and layers are -- so its break-points are pressed, dragged and bent the way
//! they are everywhere, and the whole gesture reports once, on release, as a
//! `points` message listing every curve's points (a curve names itself).
//!
//! The wire:
//!
//! - `curves`: flat `name label min max height` quintuples, one lane each;
//! - `layers`: flat `name note label min max pitch` sextuples -- `note` the id
//!   of the note it is over, `pitch` true for a bend drawn in the plane;
//! - `points`: flat `name at value shape curve` quintuples, `at` in the roll's
//!   own units, a layer's measured from its note's start.

use std::collections::HashMap;

use super::*;
use crate::host::elements::curve;
use crate::host::graphics::track;

/// A lane under the plane.
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
/// with many lanes still has a plane.
const MAX_ROWS_SHARE: f32 = 0.5;

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
    pub(super) fn rows_h(&self, height: f32) -> f32 {
        let want: f32 = self.rows.iter().map(|r| r.height).sum();
        want.min(height * MAX_ROWS_SHARE).max(0.0)
    }

    /// Each row's label cell (in the keyboard's column) and body, top to
    /// bottom under the plane, scaled into the share the rows get.
    pub(super) fn row_rects(&self, rect: Rect, indent: f32, m: &Metrics) -> Vec<(Rect, Rect)> {
        let full = pianoroll::regions(
            rect,
            self.editor.ruler != Ruler::Off,
            self.osc_lane,
            indent,
            m,
        );
        let room = self.rows_h(full.grid.h);
        let total: f32 = self.rows.iter().map(|r| r.height).sum();
        let scale = if total > 0.0 { room / total } else { 0.0 };
        let mut y = full.grid.y + full.grid.h - room;
        self.rows
            .iter()
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
    /// from the note's start.
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
        for (row, (_, body)) in self.rows.iter().zip(self.row_rects(rect, indent, m)) {
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
            let Some(boxed) = pianoroll::note_rect(grid, &nav, 0.0, note, axis) else {
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
            let local = track::clip_local_view(grid, &nav, note.start, note.dur, boxed);
            let mut space = TimeSpace::of(local, note.dur);
            space.active = active(&layer.name);
            out.push((layer.name.as_str(), place, space));
        }
        out
    }

    /// The rows' backgrounds and labels, then every curve over its place --
    /// drawn last of the contents, being the one thing a hand edits on top of
    /// what it shapes.
    pub(super) fn draw_curves(&self, d: &mut Draw, ctx: &Ctx) {
        for (row, (label, body)) in
            self.rows
                .iter()
                .zip(self.row_rects(ctx.rect, ctx.indent, ctx.metrics))
        {
            let (mesh, m, theme) = d.parts();
            mesh.rect(body, theme.osc_lane);
            mesh.rect(Rect::new(body.x, body.y, body.w, 1.0), theme.frame);
            font::text(
                mesh,
                &row.label,
                label.x + m.pad,
                label.y + 2.0,
                m.caption_scale,
                theme.ruler_text,
            );
        }
        for (name, rect, space) in
            self.curves_on_screen(ctx.rect, ctx.indent, ctx.metrics, ctx.time)
        {
            if let Some(body) = self.bodies.get(name) {
                body.draw_body(d, rect, &space);
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
        let rows = self.rows.len();
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
