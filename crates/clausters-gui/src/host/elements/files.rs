//! `files` -- a directory's entries, to choose a file from.
//!
//! The chooser is drawn by the host, the same in a window and in a page: its
//! rows are a directory listed by the front -- the disk natively, the page's
//! own storage in a tab ([`crate::host::files`]) -- so neither client lists
//! anything. It is the [`Rows`] of a `table` over that listing: a `..` row when
//! there is a directory above, the directories, then the files that pass the
//! `filter`.
//!
//! What the chooser is doing -- the directory, the ask it waits on, an error
//! -- is a [`Chooser`]; this element is that state drawn as rows, with the
//! table's selection, focus and scrolling.
//!
//! What it reports: its **value** is the path of the file selected, so a
//! `text` field bound to it follows the selection; a double click or Enter on a
//! file reports `"pick" <path>`, and on a directory it goes into it, reported
//! as `"path" <directory>`. Those two are what the hand did, so they reach the
//! script whether or not the value is bound elsewhere.

use serde_json::{Map, Value};

use clausters_core::osc::OscType;

use crate::host::files::{self, Ask, Chooser, DirEntry, Listing};
use crate::host::graphics::table::{Column, Row};
use crate::host::metrics::Metrics;
use crate::host::paint::Draw;
use crate::host::widget::element::{Claim, Ctx, Element, Events, HitArea, Input, Key, KeyInput};
use crate::host::widget::size::Natural;

use super::table::{Hit, Rows};

/// The room around the size column's text, in logical pixels: the pad on
/// either side of a cell and the hairline between columns.
const SIZE_PAD: f32 = 16.0;

/// A file chooser: what it is doing ([`Chooser`]) and the rows it is drawn as.
#[derive(Debug, Clone)]
pub struct Files {
    chooser: Chooser,
    /// Whether the rows start with `..`.
    up: bool,
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
        chooser: Chooser::new(
            props.get("path").and_then(Value::as_str).unwrap_or("."),
            filter_of(props.get("filter")),
        ),
        up: false,
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
            .map(str::to_string)
            .collect(),
        Some(Value::String(one)) => vec![one.clone()],
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
    /// Rebuilds the rows from what the chooser shows; nothing stays selected.
    fn refill(&mut self) {
        self.up = self.chooser.up().is_some();
        let mut rows = Vec::new();
        if self.up {
            rows.push(Row {
                cells: vec!["..".into(), String::new()],
                ..Row::default()
            });
        }
        rows.extend(self.chooser.shown().iter().map(|e| Row {
            cells: if e.dir {
                vec![format!("{}/", e.name), String::new()]
            } else {
                vec![e.name.clone(), size_text(e.size)]
            },
            ..Row::default()
        }));
        self.rows.replace_rows(rows);
        self.rows.selected.clear();
    }

    /// The entry row `i` stands for, or `None` for the `..` row.
    fn entry(&self, i: usize) -> Option<DirEntry> {
        let k = i.checked_sub(usize::from(self.up))?;
        self.chooser.shown().get(k).map(|e| (*e).clone())
    }

    /// The path of the file selected, if a file is.
    fn selected_file(&self) -> Option<String> {
        let i = *self.rows.selected.last()?;
        self.entry(i)
            .filter(|e| !e.dir)
            .map(|e| files::join(self.chooser.path(), &e.name))
    }

    fn selected_files(&self) -> Vec<OscType> {
        self.rows
            .selected
            .iter()
            .filter_map(|&i| self.entry(i))
            .filter(|e| !e.dir)
            .map(|e| OscType::String(files::join(self.chooser.path(), &e.name)))
            .collect()
    }

    /// Goes into directory `to`, which is asked for and reported.
    fn go(&mut self, to: String) -> Events {
        self.chooser.go(to.clone());
        self.refill();
        Events::none().and_interface(vec![OscType::String("path".into()), OscType::String(to)])
    }

    /// Acts on row `i`: into a directory, up, or the pick of a file.
    fn open(&mut self, i: usize) -> Events {
        match self.entry(i) {
            None if self.up && i == 0 => match self.chooser.up() {
                Some(up) => self.go(up),
                None => Events::none(),
            },
            Some(e) if e.dir => {
                let to = files::join(self.chooser.path(), &e.name);
                self.go(to)
            }
            Some(e) => {
                let path = files::join(self.chooser.path(), &e.name);
                let picked = if self.rows.multiple {
                    self.selected_files()
                } else {
                    vec![OscType::String(path.clone())]
                };
                Events::value(OscType::String(path)).and_interface(
                    std::iter::once(OscType::String("pick".into()))
                        .chain(picked)
                        .collect(),
                )
            }
            None => Events::none(),
        }
    }

    /// What a selection change reports: the selected file's path as the value
    /// and, choosing several, every file selected as `"select" path...`.
    fn selection(&self) -> Events {
        let events = match self.selected_file().map(OscType::String) {
            Some(value) => Events::value(value),
            None => Events::none(),
        };
        if !self.rows.multiple {
            return events;
        }
        events.and_interface(
            std::iter::once(OscType::String("select".into()))
                .chain(self.selected_files())
                .collect(),
        )
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
                self.chooser.set_filter(filter_of(Some(v)));
                self.refill();
                true
            }
            "text_size" | "multiple" => self.rows.set(key, v),
            _ => false,
        }
    }

    fn draw(&self, d: &mut Draw, ctx: &Ctx) {
        self.rows.draw(d, ctx, self.chooser.placeholder());
    }

    fn natural(&self, _m: &Metrics, _scale: f32) -> Natural {
        (None, None)
    }

    fn value(&self) -> Option<OscType> {
        self.selected_file().map(OscType::String)
    }

    fn info(&self) -> Vec<(String, Value)> {
        let mut out = vec![("path".into(), Value::from(self.chooser.path()))];
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

    fn wants_listing(&mut self) -> Option<Ask> {
        self.chooser.ask()
    }

    fn listed(&mut self, ticket: u64, listing: Result<Listing, String>) -> bool {
        let taken = self.chooser.listed(ticket, listing);
        if taken {
            self.refill();
        }
        taken
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

    /// Answers the chooser's ask with `listing`, as a front does.
    fn answer(f: &mut Files, listing: Result<Listing, String>) -> bool {
        let ask = f.wants_listing().expect("an ask");
        f.listed(ask.ticket, listing)
    }

    #[test]
    fn it_asks_once_and_shows_what_the_listing_resolved() {
        let mut f = chooser(r#"{"path":"takes","filter":["wav"]}"#);
        let ask = f.wants_listing().expect("asked");
        assert_eq!(ask.path, "takes");
        assert_eq!(f.wants_listing(), None, "asked once");
        assert!(
            !f.listed(ask.ticket + 1, Ok(listing())),
            "an answer to another ask"
        );
        assert!(f.listed(ask.ticket, Ok(listing())));
        assert_eq!(f.chooser.path(), "/takes");
        let names: Vec<&str> = f.rows.rows().iter().map(|r| r.cells[0].as_str()).collect();
        assert_eq!(
            names,
            vec!["..", "old/", "kick.wav"],
            "filtered, with the way up"
        );
        assert_eq!(f.rows.rows()[2].cells[1], "2.0 kB");
    }

    #[test]
    fn a_click_selects_a_file_as_the_value_and_a_double_click_picks_it() {
        let m = Metrics::default();
        let mut f = chooser(r#"{"path":"takes"}"#);
        answer(&mut f, Ok(listing()));
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
        answer(&mut f, Ok(listing()));
        let Claim::Take(mut take) = f.press(on_row(&f, &m, 1), &double) else {
            panic!("taken");
        };
        assert_eq!(
            take.events.take_interface()[0][1],
            OscType::String("/takes/old".into())
        );
        let ask = f.wants_listing().expect("the directory gone into");
        assert_eq!(ask.path, "/takes/old");
        f.listed(
            ask.ticket,
            Ok(Listing::ordered("/takes/old".into(), Vec::new())),
        );
        f.press(on_row(&f, &m, 0), &double);
        assert_eq!(f.wants_listing().map(|a| a.path).as_deref(), Some("/takes"));
    }

    /// **Navigation while loading**: the way up is there before the listing
    /// is, and the listing of the directory left behind, coming back after,
    /// neither replaces the rows nor selects anything.
    #[test]
    fn climbing_while_a_directory_is_listed_leaves_its_answer_stale() {
        let m = Metrics::default();
        let double = Input {
            clicks: 2,
            ..input(&m)
        };
        let mut f = chooser(r#"{"path":"/takes/old"}"#);
        let slow = f.wants_listing().unwrap();
        assert_eq!(
            f.rows.rows()[0].cells[0],
            "..",
            "the way up, before the listing"
        );
        f.press(on_row(&f, &m, 0), &double);
        let up = f.wants_listing().unwrap();
        assert_eq!(up.path, "/takes");
        assert!(!f.listed(slow.ticket, Ok(listing())), "stale");
        assert_eq!(f.chooser.path(), "/takes");
        assert_eq!(f.rows.rows().len(), 1, "only the way up while it waits");
        assert!(f.listed(up.ticket, Ok(listing())));
        assert_eq!(f.rows.rows().len(), 4);
    }

    /// **A chooser opened again**: its `path` set again reads the directory
    /// afresh, with nothing selected, and an answer to the ask before it is
    /// stale.
    #[test]
    fn setting_the_path_again_reads_it_afresh_with_nothing_selected() {
        let m = Metrics::default();
        let mut f = chooser(r#"{"path":"takes"}"#);
        answer(&mut f, Ok(listing()));
        f.press(on_row(&f, &m, 2), &input(&m));
        assert!(f.value().is_some());
        let before = f.wants_listing();
        assert_eq!(before, None, "nothing asked while nothing moved");
        assert!(f.set("path", &Value::from("/takes")));
        assert_eq!(f.value(), None, "the selection went with the listing");
        let fresh = f.wants_listing().expect("asked afresh");
        assert!(f.listed(fresh.ticket, Ok(listing())));
        assert_eq!(f.value(), None);
    }

    #[test]
    fn a_directory_that_cannot_be_read_says_why() {
        let mut f = chooser(r#"{"path":"/nowhere"}"#);
        assert!(answer(&mut f, Err("/nowhere: not found".into())));
        assert_eq!(f.chooser.placeholder(), Some("/nowhere: not found"));
        assert_eq!(f.rows.rows()[0].cells[0], "..", "the way up is still there");
    }

    #[test]
    fn the_size_column_holds_the_widest_size_it_writes() {
        let f = chooser(r#"{"path":"takes"}"#);
        let w = f.rows.columns[1].w.unwrap();
        assert!(w > crate::host::font::width(&size_text(999 << 10), f.rows.text_size));
    }

    #[test]
    fn multiple_selection_reports_all_paths_and_excludes_directories() {
        let mut f = chooser(r#"{"path":"takes","multiple":true}"#);
        answer(&mut f, Ok(listing()));
        f.rows.click(1, Mods::default(), 1);
        f.rows.click(
            2,
            Mods {
                ctrl: true,
                ..Mods::default()
            },
            1,
        );
        f.rows.click(
            3,
            Mods {
                ctrl: true,
                ..Mods::default()
            },
            1,
        );
        let last = || Events::value(OscType::String("/takes/notes.txt".into()));
        assert_eq!(
            f.selection(),
            last().and_interface(vec![
                OscType::String("select".into()),
                OscType::String("/takes/kick.wav".into()),
                OscType::String("/takes/notes.txt".into())
            ]),
            "the value is the last file, so a bound field follows it"
        );
        assert_eq!(
            f.open(3),
            last().and_interface(vec![
                OscType::String("pick".into()),
                OscType::String("/takes/kick.wav".into()),
                OscType::String("/takes/notes.txt".into())
            ])
        );
    }
}
