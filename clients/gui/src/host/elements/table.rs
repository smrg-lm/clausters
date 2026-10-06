//! `table` -- rows of data: a list, a table and a tree, as one element.
//!
//! What it holds is the rows it was sent, which are open, which are selected
//! and which column the owner says they are ordered by. What a hand does to it
//! is reported in the owner's terms: `"select"` with the indices now selected,
//! `"activate"` on a double click or Enter, `"open"` when a branch folds or
//! unfolds, `"sort"` when a column's header is pressed. Ordering the rows is the
//! owner's -- the host holds no document -- so a header press moves the mark
//! and asks; folding a branch is drawn at once and reported, like a check in a
//! menu, because which branches are open is a prop.
//!
//! The rows and their gestures are [`Rows`], which the file chooser
//! ([`super::files`]) is built over too.

use std::cell::Cell;

use serde_json::{Map, Value};

use clausters_core::osc::OscType;

use crate::host::graphics::controls;
use crate::host::graphics::table::{self, Column, Geometry, Look, Row};
use crate::host::layout::Rect;
use crate::host::metrics::Metrics;
use crate::host::paint::Draw;
use crate::host::widget::element::{
    Claim, Ctx, Element, Events, HitArea, Input, Key, KeyInput, Mods,
};
use crate::host::widget::parse;
use crate::host::widget::size::Natural;

/// What a press landed on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Hit {
    /// A column's header.
    Header(usize),
    /// A branch's disclosure mark (the row, by index into the rows).
    Disclosure(usize),
    /// A row.
    Row(usize),
}

/// What a gesture on the rows changed.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct Changed {
    pub selection: bool,
    /// A row the hand asked to act on.
    pub activate: Option<usize>,
    /// A branch folded (`false`) or unfolded.
    pub open: Option<(usize, bool)>,
}

/// The rows and the hand's state over them.
#[derive(Debug, Clone, Default)]
pub(crate) struct Rows {
    pub columns: Vec<Column>,
    /// The rows, written through [`Rows::replace_rows`] alone, so the list of
    /// the ones a fold leaves showing is always theirs.
    rows: Vec<Row>,
    /// **The rows a reader sees**, by index into the rows: every one no
    /// folded branch hides ([`table::visible`]). Kept rather than walked for,
    /// because a draw, a press and a key each ask it, and walking the rows
    /// for each made a frame cost the length of the list and not of the
    /// window.
    shown: Vec<usize>,
    pub selected: Vec<usize>,
    pub multiple: bool,
    pub sort: Option<(usize, bool)>,
    pub text_size: f32,
    /// How far the list is scrolled, in pixels. A `Cell` because it is
    /// settled where the list's height is known -- at the draw -- the way an
    /// open list keeps its hovered row in view.
    scroll: Cell<f32>,
    /// A row to bring into view at the next draw, by index into the rows.
    reveal: Cell<Option<usize>>,
    /// Where a Shift range is counted from, by index into the rows.
    anchor: Option<usize>,
}

/// A column off the wire: a title, or `{"title", "w"}`.
fn column_of(v: &Value) -> Option<Column> {
    match v {
        Value::String(s) => Some(Column {
            title: s.clone(),
            w: None,
        }),
        Value::Object(o) => Some(Column {
            title: o
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            w: o.get("w").and_then(Value::as_f64).map(|w| w as f32),
        }),
        _ => None,
    }
}

/// A cell off the wire: a string, or a number written as one.
fn cell_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// A row off the wire: its cells, or `{"cells", "depth", "open"}`.
fn row_of(v: &Value) -> Option<Row> {
    match v {
        Value::Array(cells) => Some(Row {
            cells: cells.iter().map(cell_of).collect(),
            ..Row::default()
        }),
        Value::String(s) => Some(Row {
            cells: vec![s.clone()],
            ..Row::default()
        }),
        Value::Object(o) => Some(Row {
            cells: match o.get("cells") {
                Some(Value::Array(cells)) => cells.iter().map(cell_of).collect(),
                Some(one) => vec![cell_of(one)],
                None => Vec::new(),
            },
            depth: o.get("depth").and_then(Value::as_u64).unwrap_or(0) as u32,
            open: o.get("open").and_then(parse::truthy),
        }),
        _ => None,
    }
}

/// A list off the wire, or its JSON string (the scalar wire).
fn list_of<T>(v: &Value, each: fn(&Value) -> Option<T>) -> Vec<T> {
    match v {
        Value::Array(items) => items.iter().filter_map(each).collect(),
        Value::String(s) => serde_json::from_str::<Value>(s)
            .ok()
            .filter(Value::is_array)
            .map(|v| list_of(&v, each))
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// `[column, "up"|"down"]` off the wire.
fn sort_of(v: &Value) -> Option<(usize, bool)> {
    let a = v.as_array()?;
    let c = a.first()?.as_u64()? as usize;
    let up = a.get(1).and_then(Value::as_str) != Some("down");
    Some((c, up))
}

fn row_to_json(r: &Row) -> Value {
    let mut o = Map::new();
    o.insert("cells".into(), Value::from(r.cells.clone()));
    if r.depth > 0 {
        o.insert("depth".into(), Value::from(r.depth));
    }
    if let Some(open) = r.open {
        o.insert("open".into(), Value::from(open));
    }
    Value::Object(o)
}

impl Rows {
    pub(crate) fn from_props(props: &Map<String, Value>) -> Rows {
        let mut rows = Rows {
            columns: props
                .get("columns")
                .map(|v| list_of(v, column_of))
                .unwrap_or_default(),
            rows: props
                .get("rows")
                .map(|v| list_of(v, row_of))
                .unwrap_or_default(),
            multiple: props
                .get("multiple")
                .and_then(parse::truthy)
                .unwrap_or(false),
            sort: props.get("sort").and_then(sort_of),
            text_size: parse::text_size(props),
            ..Rows::default()
        };
        rows.shown = table::visible(&rows.rows);
        table::warn_past_the_wire(rows.rows.len());
        if let Some(v) = props.get("selected") {
            rows.set_selected(v);
        }
        rows
    }

    /// Replaces the rows, and with them which are showing.
    pub(crate) fn replace_rows(&mut self, rows: Vec<Row>) {
        self.rows = rows;
        self.shown = table::visible(&self.rows);
    }

    fn set_selected(&mut self, v: &Value) {
        let n = self.rows.len();
        let mut picked: Vec<usize> = match v {
            Value::Array(items) => items
                .iter()
                .filter_map(Value::as_u64)
                .map(|i| i as usize)
                .collect(),
            Value::Number(i) => i.as_u64().map(|i| vec![i as usize]).unwrap_or_default(),
            _ => Vec::new(),
        };
        picked.retain(|&i| i < n);
        if !self.multiple {
            picked.truncate(1);
        }
        self.selected = picked;
    }

    /// Applies one `/gui_set` key to the rows.
    pub(crate) fn set(&mut self, key: &str, v: &Value) -> bool {
        match key {
            "columns" => {
                self.columns = list_of(v, column_of);
                true
            }
            "rows" => {
                self.replace_rows(list_of(v, row_of));
                let n = self.rows.len();
                table::warn_past_the_wire(n);
                self.selected.retain(|&i| i < n);
                true
            }
            "selected" => {
                self.set_selected(v);
                if let Some(&i) = self.selected.first() {
                    self.reveal.set(Some(i));
                }
                true
            }
            "multiple" => parse::truthy(v).map(|b| self.multiple = b).is_some(),
            "sort" => {
                self.sort = sort_of(v);
                true
            }
            "text_size" => parse::set_size(&mut self.text_size, v),
            _ => false,
        }
    }

    pub(crate) fn info(&self) -> Vec<(String, Value)> {
        vec![
            ("selected".into(), Value::from(self.selected.clone())),
            (
                "rows".into(),
                Value::from(Value::Array(self.rows.iter().map(row_to_json).collect()).to_string()),
            ),
        ]
    }

    /// The body the rows are drawn in: the cell, inset by the control's pad.
    fn body(rect: Rect, m: &Metrics) -> Rect {
        Rect::new(
            rect.x + m.pad,
            rect.y + m.pad,
            (rect.w - 2.0 * m.pad).max(0.0),
            (rect.h - 2.0 * m.pad).max(0.0),
        )
    }

    pub(crate) fn geometry(&self, rect: Rect, scale: f32, m: &Metrics) -> Geometry {
        let shown = self.shown.len();
        Geometry::new(
            Self::body(rect, m),
            &self.columns,
            shown,
            self.text_size * scale,
            scale,
            m,
        )
    }

    /// The body the press is filtered by.
    pub(crate) fn hit_area(&self, input: &Input) -> HitArea {
        HitArea::Rect(Self::body(input.rect, input.metrics))
    }

    /// What is under `at`.
    pub(crate) fn hit(&self, at: (f64, f64), input: &Input) -> Option<Hit> {
        let g = self.geometry(input.rect, input.scale, input.metrics);
        if let Some(c) = g.header_at(at.0, at.1) {
            return Some(Hit::Header(c));
        }
        let shown = &self.shown;
        let scroll = self.scroll.get().clamp(0.0, g.max_scroll());
        let k = g.row_at(shown.len(), scroll, at.0, at.1)?;
        let i = shown[k];
        let row = &self.rows[i];
        if row.open.is_some() {
            let size = self.text_size * input.scale;
            let mark = g.disclosure(g.row(k, scroll), row.depth, size, input.metrics);
            if mark.contains(at.0, at.1) {
                return Some(Hit::Disclosure(i));
            }
        }
        Some(Hit::Row(i))
    }

    /// A press on row `i`, with the modifiers and the click count it came with.
    pub(crate) fn click(&mut self, i: usize, mods: Mods, clicks: u32) -> Changed {
        let before = self.selected.clone();
        if self.multiple && mods.ctrl {
            if let Some(at) = self.selected.iter().position(|&s| s == i) {
                self.selected.remove(at);
            } else {
                self.selected.push(i);
                self.selected.sort_unstable();
            }
            self.anchor = Some(i);
        } else if self.multiple && mods.shift && self.anchor.is_some() {
            self.select_range(self.anchor.unwrap_or(i), i);
        } else {
            self.selected = vec![i];
            self.anchor = Some(i);
        }
        Changed {
            selection: self.selected != before,
            activate: (clicks >= 2).then_some(i),
            open: None,
        }
    }

    /// Selects the visible rows from `a` to `b` (indices into the rows).
    fn select_range(&mut self, a: usize, b: usize) {
        let shown = &self.shown;
        let (pa, pb) = (
            shown.iter().position(|&s| s == a),
            shown.iter().position(|&s| s == b),
        );
        if let (Some(pa), Some(pb)) = (pa, pb) {
            let (lo, hi) = (pa.min(pb), pa.max(pb));
            self.selected = shown[lo..=hi].to_vec();
        }
    }

    /// Folds or unfolds branch `i`; a selected row that folding hides is let go
    /// of, since a selection a reader cannot see is a selection they will act
    /// on by accident.
    pub(crate) fn toggle(&mut self, i: usize) -> Changed {
        let Some(open) = self.rows.get(i).and_then(|r| r.open) else {
            return Changed::default();
        };
        self.rows[i].open = Some(!open);
        self.shown = table::visible(&self.rows);
        let shown = &self.shown;
        let before = self.selected.len();
        self.selected.retain(|s| shown.contains(s));
        Changed {
            selection: self.selected.len() != before,
            activate: None,
            open: Some((i, !open)),
        }
    }

    /// A key, while the list holds the focus. `None` is a key it does not
    /// answer.
    pub(crate) fn key(&mut self, key: &Key, mods: Mods) -> Option<Changed> {
        let shown = self.shown.clone();
        if shown.is_empty() {
            return None;
        }
        let current = self
            .selected
            .last()
            .and_then(|s| shown.iter().position(|v| v == s));
        let to = |k: usize| shown[k.min(shown.len() - 1)];
        let target = match key {
            Key::Down => to(current.map_or(0, |k| k + 1)),
            Key::Up => to(current.map_or(0, |k| k.saturating_sub(1))),
            Key::Home => shown[0],
            Key::End => shown[shown.len() - 1],
            Key::Enter => {
                return Some(Changed {
                    activate: self.selected.last().copied(),
                    ..Changed::default()
                });
            }
            Key::Right | Key::Left => {
                let i = self.selected.last().copied()?;
                let row = &self.rows[i];
                return match (key, row.open) {
                    (Key::Right, Some(false)) | (Key::Left, Some(true)) => Some(self.toggle(i)),
                    (Key::Left, _) => {
                        let p = table::parent(&self.rows, i)?;
                        Some(self.click(p, Mods::default(), 1))
                    }
                    _ => Some(Changed::default()),
                };
            }
            _ => return None,
        };
        let changed = if self.multiple && mods.shift {
            let before = self.selected.clone();
            let anchor = self.anchor.unwrap_or(target);
            self.select_range(anchor, target);
            Changed {
                selection: self.selected != before,
                ..Changed::default()
            }
        } else {
            self.click(target, Mods::default(), 1)
        };
        self.reveal.set(Some(target));
        Some(changed)
    }

    /// The wheel: a notch is three rows. Answers whether the list moved.
    pub(crate) fn wheel(&mut self, steps: f64, input: &Input) -> bool {
        let g = self.geometry(input.rect, input.scale, input.metrics);
        let from = self.scroll.get().clamp(0.0, g.max_scroll());
        let to = (from - steps as f32 * 3.0 * g.row_h).clamp(0.0, g.max_scroll());
        self.scroll.set(to);
        (to - from).abs() > f32::EPSILON
    }

    /// Whether the list has more rows than fit, so the wheel is its own.
    pub(crate) fn scrolls(&self, input: &Input) -> bool {
        self.geometry(input.rect, input.scale, input.metrics)
            .max_scroll()
            > 0.0
    }

    /// The events a change reports.
    pub(crate) fn events(&self, changed: &Changed) -> Events {
        let mut msgs: Vec<Vec<OscType>> = Vec::new();
        if let Some((i, open)) = changed.open {
            msgs.push(vec![
                OscType::String("open".into()),
                OscType::Int(i as i32),
                OscType::Int(i32::from(open)),
            ]);
        }
        if changed.selection {
            let mut args = vec![OscType::String("select".into())];
            args.extend(self.selected.iter().map(|&i| OscType::Int(i as i32)));
            msgs.push(args);
        }
        if let Some(i) = changed.activate {
            msgs.push(vec![
                OscType::String("activate".into()),
                OscType::Int(i as i32),
            ]);
        }
        let mut msgs = msgs.into_iter();
        match msgs.next() {
            None => Events::none(),
            Some(first) => msgs.fold(Events::message(first), Events::and),
        }
    }

    pub(crate) fn draw(&self, d: &mut Draw, ctx: &Ctx, empty: Option<&str>) {
        let g = self.geometry(ctx.rect, ctx.scale, ctx.metrics);
        let shown = &self.shown;
        let mut scroll = self.scroll.get().clamp(0.0, g.max_scroll());
        if let Some(i) = self.reveal.take()
            && let Some(k) = shown.iter().position(|&s| s == i)
        {
            scroll = g.reveal(k, scroll);
        }
        self.scroll.set(scroll);
        let hover = ctx
            .hovered
            .then_some(ctx.world.cursor)
            .flatten()
            .and_then(|(x, y)| g.row_at(shown.len(), scroll, x, y))
            .map(|k| shown[k]);
        table::draw(
            d,
            &g,
            &Look {
                columns: &self.columns,
                rows: &self.rows,
                shown,
                selected: &self.selected,
                hover,
                sort: self.sort,
                scroll,
                size: self.text_size * ctx.scale,
                empty,
            },
        );
    }
}

/// A list, a table or a tree.
#[derive(Debug, Clone)]
pub struct Table {
    rows: Rows,
    label: Option<String>,
}

pub(super) fn build(
    props: &Map<String, Value>,
    _blobs: &[Vec<u8>],
) -> Result<Box<dyn Element>, String> {
    Ok(Box::new(Table {
        rows: Rows::from_props(props),
        label: parse::label(props),
    }))
}

impl Table {
    /// The cell under the label strip, which is what the rows are placed in.
    fn under_label(&self, rect: Rect, scale: f32, m: &Metrics) -> Rect {
        let size = self.rows.text_size * scale;
        let strip = controls::label_height(rect.h, self.label.is_some(), size, m);
        Rect::new(rect.x, rect.y + strip, rect.w, (rect.h - strip).max(0.0))
    }

    fn placed<'a>(&self, input: &Input<'a>) -> Input<'a> {
        Input {
            rect: self.under_label(input.rect, input.scale, input.metrics),
            ..*input
        }
    }
}

impl Element for Table {
    fn set(&mut self, key: &str, v: &Value) -> bool {
        match key {
            "label" => parse::set_label(&mut self.label, v),
            _ => self.rows.set(key, v),
        }
    }

    fn draw(&self, d: &mut Draw, ctx: &Ctx) {
        let size = self.rows.text_size * ctx.scale;
        controls::label_strip(d, self.label.as_deref(), ctx.rect, size);
        let rect = self.under_label(ctx.rect, ctx.scale, ctx.metrics);
        let inner = Ctx { rect, ..*ctx };
        self.rows.draw(d, &inner, None);
    }

    /// A surface: the rows are data, and data never sizes a widget.
    fn natural(&self, _m: &Metrics, _scale: f32) -> Natural {
        (None, None)
    }

    fn info(&self) -> Vec<(String, Value)> {
        self.rows.info()
    }

    fn hit_area(&self, input: &Input) -> HitArea {
        self.rows.hit_area(&self.placed(input))
    }

    fn press(&mut self, at: (f64, f64), input: &Input) -> Claim {
        let placed = self.placed(input);
        let changed = match self.rows.hit(at, &placed) {
            Some(Hit::Header(c)) => {
                // The owner orders the rows; the host moves the mark and asks.
                let up = !matches!(self.rows.sort, Some((sc, true)) if sc == c);
                self.rows.sort = Some((c, up));
                return Claim::events(Events::message(vec![
                    OscType::String("sort".into()),
                    OscType::Int(c as i32),
                    OscType::String(if up { "up" } else { "down" }.into()),
                ]));
            }
            Some(Hit::Disclosure(i)) => self.rows.toggle(i),
            Some(Hit::Row(i)) => self.rows.click(i, input.mods, input.clicks),
            None => return Claim::take(),
        };
        Claim::events(self.rows.events(&changed))
    }

    fn accepts_focus(&self) -> bool {
        true
    }

    fn key(&mut self, key: &Key, input: &mut KeyInput) -> Option<Events> {
        let changed = self.rows.key(key, input.mods)?;
        Some(self.rows.events(&changed))
    }

    fn wheel(&mut self, _at: (f64, f64), delta: (f64, f64), input: &Input) -> Option<Events> {
        let placed = self.placed(input);
        if !self.rows.scrolls(&placed) {
            return None;
        }
        self.rows.wheel(delta.1, &placed);
        Some(Events::none())
    }

    fn clone_box(&self) -> Box<dyn Element> {
        Box::new(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl Rows {
        /// The rows -- what a test reads; everything else draws them. Here
        /// and not beside the field, so the build carries no reader nothing
        /// calls.
        pub(crate) fn rows(&self) -> &[Row] {
            &self.rows
        }
    }

    fn props(json: &str) -> Map<String, Value> {
        serde_json::from_str(json).unwrap()
    }

    fn input(m: &Metrics) -> Input<'_> {
        Input {
            metrics: m,
            indent: 0.0,
            rect: Rect::new(0.0, 0.0, 300.0, 200.0),
            scale: 1.0,
            mods: Mods::default(),
            viewport: (400.0, 300.0),
            clicks: 1,
            time: None,
        }
    }

    fn table(json: &str) -> Table {
        assert!(build(&props(json), &[]).is_ok());
        Table {
            rows: Rows::from_props(&props(json)),
            label: None,
        }
    }

    /// The point a test presses to land on visible row `k`.
    fn on_row(t: &Table, m: &Metrics, k: usize) -> (f64, f64) {
        let g = t.rows.geometry(input(m).rect, 1.0, m);
        let r = g.row(k, 0.0);
        ((r.x + r.w * 0.5) as f64, (r.y + r.h * 0.5) as f64)
    }

    const TREE: &str = r#"{"columns":["name","size"],"rows":[
        {"cells":["sounds"],"open":true},
        {"cells":["kick.wav","12"],"depth":1},
        {"cells":["loops"],"depth":1,"open":false},
        {"cells":["a.wav","3"],"depth":2},
        ["notes.txt","1"]],"multiple":true}"#;

    #[test]
    fn the_wire_reads_columns_rows_and_a_tree() {
        let t = table(TREE);
        assert_eq!(t.rows.columns.len(), 2);
        assert_eq!(t.rows.rows().len(), 5);
        assert_eq!(t.rows.rows()[2].open, Some(false));
        assert_eq!(table::visible(t.rows.rows()), vec![0, 1, 2, 4]);
        assert_eq!(t.rows.rows()[4].cells, vec!["notes.txt", "1"]);
        // ...and the same list on the scalar wire.
        let mut s = table("{}");
        assert!(s.set("rows", &Value::from(r#"[["a"],["b"]]"#)));
        assert_eq!(s.rows.rows().len(), 2);
    }

    #[test]
    fn a_click_selects_ctrl_adds_shift_ranges_and_a_double_click_activates() {
        let m = Metrics::default();
        let mut t = table(TREE);
        let e = t.press(on_row(&t, &m, 1), &input(&m));
        assert_eq!(
            e,
            Claim::events(Events::message(vec![
                OscType::String("select".into()),
                OscType::Int(1)
            ]))
        );
        let ctrl = Input {
            mods: Mods {
                ctrl: true,
                ..Mods::default()
            },
            ..input(&m)
        };
        t.press(on_row(&t, &m, 3), &ctrl);
        assert_eq!(
            t.rows.selected,
            vec![1, 4],
            "the fourth visible row is row 4"
        );
        let shift = Input {
            mods: Mods {
                shift: true,
                ..Mods::default()
            },
            ..input(&m)
        };
        t.press(on_row(&t, &m, 0), &shift);
        assert_eq!(t.rows.selected, vec![0, 1, 2, 4]);
        let double = Input {
            clicks: 2,
            ..input(&m)
        };
        let Claim::Take(take) = t.press(on_row(&t, &m, 1), &double) else {
            panic!("taken");
        };
        assert!(
            take.events
                .clone()
                .into_messages()
                .contains(&vec![OscType::String("activate".into()), OscType::Int(1)])
        );
    }

    #[test]
    fn a_press_on_a_branchs_mark_folds_it_and_reports_it() {
        let m = Metrics::default();
        let mut t = table(TREE);
        let g = t.rows.geometry(input(&m).rect, 1.0, &m);
        let mark = g.disclosure(g.row(0, 0.0), 0, 2.0, &m);
        let at = (
            (mark.x + mark.w * 0.5) as f64,
            (mark.y + mark.h * 0.5) as f64,
        );
        let Claim::Take(take) = t.press(at, &input(&m)) else {
            panic!("taken");
        };
        assert_eq!(
            take.events.into_messages()[0],
            vec![
                OscType::String("open".into()),
                OscType::Int(0),
                OscType::Int(0)
            ]
        );
        assert_eq!(table::visible(t.rows.rows()), vec![0, 4]);
        assert_eq!(
            t.rows.shown,
            vec![0, 4],
            "and the list kept of them follows"
        );
        let info = t.info();
        assert!(info[1].1.as_str().unwrap().contains(r#""open":false"#));
    }

    #[test]
    fn a_header_press_moves_the_mark_and_asks_the_owner_to_order_the_rows() {
        let m = Metrics::default();
        let mut t = table(TREE);
        let g = t.rows.geometry(input(&m).rect, 1.0, &m);
        let (x, w) = g.columns[1];
        let at = ((x + w * 0.5) as f64, (g.header.y + 2.0) as f64);
        let sort = |dir: &str| {
            Claim::events(Events::message(vec![
                OscType::String("sort".into()),
                OscType::Int(1),
                OscType::String(dir.into()),
            ]))
        };
        assert_eq!(t.press(at, &input(&m)), sort("up"));
        assert_eq!(t.press(at, &input(&m)), sort("down"));
        assert_eq!(
            t.rows.rows()[1].cells[0],
            "kick.wav",
            "the rows are the owner's"
        );
    }

    #[test]
    fn the_keys_walk_the_visible_rows_and_fold_and_unfold() {
        let mut t = table(TREE);
        let none = Mods::default();
        t.rows.key(&Key::Down, none);
        assert_eq!(t.rows.selected, vec![0]);
        t.rows.key(&Key::Down, none);
        t.rows.key(&Key::Down, none);
        assert_eq!(t.rows.selected, vec![2], "the folded branch");
        let opened = t.rows.key(&Key::Right, none).unwrap();
        assert_eq!(opened.open, Some((2, true)));
        t.rows.key(&Key::Down, none);
        assert_eq!(t.rows.selected, vec![3], "into it");
        t.rows.key(&Key::Left, none);
        assert_eq!(t.rows.selected, vec![2], "Left goes to the branch");
        assert_eq!(t.rows.key(&Key::Enter, none).unwrap().activate, Some(2));
        t.rows.key(&Key::End, none);
        assert_eq!(t.rows.selected, vec![4]);
    }

    #[test]
    fn the_wheel_is_the_lists_only_when_it_has_more_rows_than_fit() {
        let m = Metrics::default();
        let few = table(r#"{"rows":[["a"],["b"]]}"#);
        assert!(!few.rows.scrolls(&input(&m)));
        let rows: Vec<String> = (0..200).map(|i| format!("[\"{i}\"]")).collect();
        let mut many = table(&format!(r#"{{"rows":[{}]}}"#, rows.join(",")));
        assert!(many.rows.scrolls(&input(&m)));
        assert!(many.rows.wheel(-1.0, &input(&m)));
        assert!(!many.rows.wheel(10_000.0, &input(&m)) || many.rows.scroll.get() == 0.0);
        assert_eq!(many.natural(&m, 1.0), (None, None), "data never sizes it");
    }
}
