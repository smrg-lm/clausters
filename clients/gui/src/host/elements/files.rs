//! `files` -- a directory's entries, to choose a file from.
//!
//! The chooser is drawn by the host, the same in a window and in a page: its
//! rows are a directory listed by the front -- the disk natively, the page's
//! own storage in a tab ([`crate::host::files`]) -- so neither client lists
//! anything. It is the [`Rows`] of a `table` over that listing: a `..` row when
//! there is a directory above, the directories, then the files that pass the
//! `filter`.
//!
//! What it reports: its **value** is the path of the file selected, so a
//! `text` field bound to it follows the selection; a double click or Enter on a
//! file reports `"pick" <path>`, and on a directory it goes into it, reported
//! as `"path" <directory>`. Those two are what the hand did, so they reach the
//! script whether or not the value is bound elsewhere.

use serde_json::{Map, Value};

use clausters_core::osc::OscType;

use crate::host::files::{self, DirEntry, Listing};
use crate::host::graphics::table::{Column, Row};
use crate::host::metrics::Metrics;
use crate::host::paint::Draw;
use crate::host::widget::element::{Claim, Ctx, Element, Events, HitArea, Input, Key, KeyInput};
use crate::host::widget::size::Natural;

use super::table::{Hit, Rows};

/// The room around the size column's text, in logical pixels: the pad on
/// either side of a cell and the hairline between columns.
const SIZE_PAD: f32 = 16.0;

/// A file chooser.
#[derive(Debug, Clone)]
pub struct Files {
    /// The directory shown -- as asked until its listing arrives, then as the
    /// listing resolved it.
    path: String,
    /// The extensions a file must have to be listed, without the dot; empty
    /// lists every file.
    filter: Vec<String>,
    entries: Vec<DirEntry>,
    /// Whether the rows start with `..`.
    up: bool,
    /// Why the directory could not be listed, shown in its place.
    error: Option<String>,
    /// Whether the directory still has to be asked for.
    ask: bool,
    rows: Rows,
}

pub(super) fn build(
    props: &Map<String, Value>,
    _blobs: &[Vec<u8>],
) -> Result<Box<dyn Element>, String> {
    Ok(Box::new(from_props(props)))
}

fn from_props(props: &Map<String, Value>) -> Files {
    let mut rows = Rows::from_props(props);
    rows.columns = vec![
        Column {
            title: "name".into(),
            w: None,
        },
        // As wide as the widest size it writes, at the text size it is drawn
        // at -- a fixed number cut "70.2 kB" short at the default size.
        Column {
            title: "size".into(),
            w: Some(crate::host::font::width("000.0 kB", rows.text_size) + SIZE_PAD),
        },
    ];
    let mut files = Files {
        path: props
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or(".")
            .to_string(),
        filter: filter_of(props.get("filter")),
        entries: Vec::new(),
        up: false,
        error: None,
        ask: true,
        rows,
    };
    files.refill();
    files
}

fn filter_of(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .map(|e| e.trim_start_matches('.').to_lowercase())
            .collect(),
        Some(Value::String(one)) if !one.is_empty() => {
            vec![one.trim_start_matches('.').to_lowercase()]
        }
        _ => Vec::new(),
    }
}

/// A size as a reader reads one.
fn size_text(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 30 => format!("{:.1} GB", b as f64 / (1u64 << 30) as f64),
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / (1u64 << 20) as f64),
        b if b >= 1 << 10 => format!("{:.1} kB", b as f64 / (1u64 << 10) as f64),
        b => format!("{b} B"),
    }
}

impl Files {
    /// The entries shown, after the filter.
    fn shown(&self) -> Vec<&DirEntry> {
        self.entries
            .iter()
            .filter(|e| {
                e.dir
                    || self.filter.is_empty()
                    || e.name
                        .rsplit_once('.')
                        .is_some_and(|(_, ext)| self.filter.contains(&ext.to_lowercase()))
            })
            .collect()
    }

    /// Rebuilds the rows from the entries.
    fn refill(&mut self) {
        // A path the listing has resolved is absolute, so it has a parent
        // unless it is a root; one still as asked (`.`) has none to show yet.
        self.up = files::parent(&self.path).is_some();
        let mut rows = Vec::new();
        if self.up {
            rows.push(Row {
                cells: vec!["..".into(), String::new()],
                ..Row::default()
            });
        }
        rows.extend(self.shown().iter().map(|e| Row {
            cells: if e.dir {
                vec![format!("{}/", e.name), String::new()]
            } else {
                vec![e.name.clone(), size_text(e.size)]
            },
            ..Row::default()
        }));
        self.rows.rows = rows;
        self.rows.selected.clear();
    }

    /// The entry row `i` stands for, or `None` for the `..` row.
    fn entry(&self, i: usize) -> Option<DirEntry> {
        let k = i.checked_sub(usize::from(self.up))?;
        self.shown().get(k).map(|e| (*e).clone())
    }

    /// The path of the file selected, if a file is.
    fn selected_file(&self) -> Option<String> {
        let i = *self.rows.selected.last()?;
        self.entry(i)
            .filter(|e| !e.dir)
            .map(|e| files::join(&self.path, &e.name))
    }

    /// Goes into directory `to`, which is asked for and reported.
    fn go(&mut self, to: String) -> Events {
        self.path = to.clone();
        self.ask = true;
        self.entries.clear();
        self.error = None;
        self.refill();
        Events::none().and_interface(vec![OscType::String("path".into()), OscType::String(to)])
    }

    /// Acts on row `i`: into a directory, up, or the pick of a file.
    fn open(&mut self, i: usize) -> Events {
        match self.entry(i) {
            None if self.up && i == 0 => match files::parent(&self.path) {
                Some(up) => self.go(up),
                None => Events::none(),
            },
            Some(e) if e.dir => {
                let to = files::join(&self.path, &e.name);
                self.go(to)
            }
            Some(e) => {
                let path = files::join(&self.path, &e.name);
                Events::value(OscType::String(path.clone()))
                    .and_interface(vec![OscType::String("pick".into()), OscType::String(path)])
            }
            None => Events::none(),
        }
    }

    /// What a selection change reports: the selected file's path as the value.
    fn selection(&self) -> Events {
        match self.selected_file() {
            Some(path) => Events::value(OscType::String(path)),
            None => Events::none(),
        }
    }
}

impl Element for Files {
    fn set(&mut self, key: &str, v: &Value) -> bool {
        match key {
            "path" => match v.as_str() {
                Some(p) => {
                    self.go(p.to_string());
                    true
                }
                None => false,
            },
            "filter" => {
                self.filter = filter_of(Some(v));
                self.refill();
                true
            }
            "text_size" => self.rows.set(key, v),
            _ => false,
        }
    }

    fn draw(&self, d: &mut Draw, ctx: &Ctx) {
        let empty = match (&self.error, self.ask, self.entries.is_empty()) {
            (Some(e), _, _) => Some(e.as_str()),
            (None, true, _) => Some("listing..."),
            (None, false, true) => Some("(empty)"),
            _ => None,
        };
        self.rows.draw(d, ctx, empty);
    }

    fn natural(&self, _m: &Metrics, _scale: f32) -> Natural {
        (None, None)
    }

    fn value(&self) -> Option<OscType> {
        self.selected_file().map(OscType::String)
    }

    fn info(&self) -> Vec<(String, Value)> {
        let mut out = vec![("path".into(), Value::from(self.path.clone()))];
        if let Some(file) = self.selected_file() {
            out.push(("selected".into(), Value::from(file)));
        }
        out
    }

    fn hit_area(&self, input: &Input) -> HitArea {
        self.rows.hit_area(input)
    }

    fn press(&mut self, at: (f64, f64), input: &Input) -> Claim {
        match self.rows.hit(at, input) {
            Some(Hit::Row(i)) => {
                let changed = self.rows.click(i, input.mods, input.clicks);
                if changed.activate.is_some() {
                    return Claim::events(self.open(i));
                }
                Claim::events(if changed.selection {
                    self.selection()
                } else {
                    Events::none()
                })
            }
            _ => Claim::take(),
        }
    }

    fn accepts_focus(&self) -> bool {
        true
    }

    fn key(&mut self, key: &Key, input: &mut KeyInput) -> Option<Events> {
        let changed = self.rows.key(key, input.mods)?;
        if let Some(i) = changed.activate {
            return Some(self.open(i));
        }
        Some(if changed.selection {
            self.selection()
        } else {
            Events::none()
        })
    }

    fn wheel(&mut self, _at: (f64, f64), delta: (f64, f64), input: &Input) -> Option<Events> {
        if !self.rows.scrolls(input) {
            return None;
        }
        self.rows.wheel(delta.1, input);
        Some(Events::none())
    }

    fn wants_listing(&mut self) -> Option<String> {
        std::mem::take(&mut self.ask).then(|| self.path.clone())
    }

    fn listed(&mut self, asked: &str, listing: Result<Listing, String>) -> bool {
        if asked != self.path {
            return false;
        }
        match listing {
            Ok(Listing { path, entries }) => {
                self.path = path;
                self.entries = entries;
                self.error = None;
            }
            Err(why) => {
                self.entries.clear();
                self.error = Some(why);
            }
        }
        self.refill();
        true
    }

    fn clone_box(&self) -> Box<dyn Element> {
        Box::new(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::layout::Rect;
    use crate::host::widget::element::Mods;

    fn input(m: &Metrics) -> Input<'_> {
        Input {
            metrics: m,
            indent: 0.0,
            rect: Rect::new(0.0, 0.0, 300.0, 300.0),
            scale: 1.0,
            mods: Mods::default(),
            viewport: (400.0, 400.0),
            clicks: 1,
            time: None,
        }
    }

    fn chooser(json: &str) -> Files {
        let props: Map<String, Value> = serde_json::from_str(json).unwrap();
        assert!(build(&props, &[]).is_ok());
        from_props(&props)
    }

    fn listing() -> Listing {
        Listing::ordered(
            "/takes".into(),
            vec![
                DirEntry {
                    name: "old".into(),
                    dir: true,
                    size: 0,
                },
                DirEntry {
                    name: "kick.wav".into(),
                    dir: false,
                    size: 2048,
                },
                DirEntry {
                    name: "notes.txt".into(),
                    dir: false,
                    size: 10,
                },
            ],
        )
    }

    fn on_row(f: &Files, m: &Metrics, k: usize) -> (f64, f64) {
        let g = f.rows.geometry(input(m).rect, 1.0, m);
        let r = g.row(k, 0.0);
        ((r.x + r.w * 0.5) as f64, (r.y + r.h * 0.5) as f64)
    }

    #[test]
    fn it_asks_once_and_shows_what_the_listing_resolved() {
        let mut f = chooser(r#"{"path":"takes","filter":["wav"]}"#);
        assert_eq!(f.wants_listing().as_deref(), Some("takes"));
        assert_eq!(f.wants_listing(), None, "asked once");
        assert!(
            !f.listed("elsewhere", Ok(listing())),
            "an answer to another ask"
        );
        assert!(f.listed("takes", Ok(listing())));
        assert_eq!(f.path, "/takes");
        let names: Vec<&str> = f.rows.rows.iter().map(|r| r.cells[0].as_str()).collect();
        assert_eq!(
            names,
            vec!["..", "old/", "kick.wav"],
            "filtered, with the way up"
        );
        assert_eq!(f.rows.rows[2].cells[1], "2.0 kB");
    }

    #[test]
    fn a_click_selects_a_file_as_the_value_and_a_double_click_picks_it() {
        let m = Metrics::default();
        let mut f = chooser(r#"{"path":"takes"}"#);
        f.wants_listing();
        f.listed("takes", Ok(listing()));
        let e = f.press(on_row(&f, &m, 2), &input(&m));
        assert_eq!(e, Claim::value(OscType::String("/takes/kick.wav".into())));
        assert_eq!(f.value(), Some(OscType::String("/takes/kick.wav".into())));
        let double = Input {
            clicks: 2,
            ..input(&m)
        };
        let Claim::Take(mut take) = f.press(on_row(&f, &m, 2), &double) else {
            panic!("taken");
        };
        assert_eq!(
            take.events.take_interface(),
            vec![vec![
                OscType::String("pick".into()),
                OscType::String("/takes/kick.wav".into())
            ]]
        );
    }

    #[test]
    fn a_double_click_on_a_directory_goes_into_it_and_up_goes_back() {
        let m = Metrics::default();
        let double = Input {
            clicks: 2,
            ..input(&m)
        };
        let mut f = chooser(r#"{"path":"takes"}"#);
        f.wants_listing();
        f.listed("takes", Ok(listing()));
        let Claim::Take(mut take) = f.press(on_row(&f, &m, 1), &double) else {
            panic!("taken");
        };
        assert_eq!(
            take.events.take_interface()[0][1],
            OscType::String("/takes/old".into())
        );
        assert_eq!(f.wants_listing().as_deref(), Some("/takes/old"));
        f.listed(
            "/takes/old",
            Ok(Listing::ordered("/takes/old".into(), Vec::new())),
        );
        f.press(on_row(&f, &m, 0), &double);
        assert_eq!(f.wants_listing().as_deref(), Some("/takes"));
    }

    #[test]
    fn a_directory_that_cannot_be_read_says_why() {
        let mut f = chooser(r#"{"path":"/nowhere"}"#);
        f.wants_listing();
        assert!(f.listed("/nowhere", Err("/nowhere: not found".into())));
        assert_eq!(f.error.as_deref(), Some("/nowhere: not found"));
        assert_eq!(f.rows.rows[0].cells[0], "..", "the way up is still there");
    }

    #[test]
    fn the_size_column_holds_the_widest_size_it_writes() {
        let f = chooser(r#"{"path":"takes"}"#);
        let w = f.rows.columns[1].w.unwrap();
        assert!(w > crate::host::font::width(&size_text(999 << 10), f.rows.text_size));
    }
}
