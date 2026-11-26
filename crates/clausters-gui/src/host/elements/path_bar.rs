//! `path_bar` -- where a folder is: one button per folder of its path, from
//! the root to it, as a platform's file dialog shows the folder it lists.
//!
//! A press on a folder reports `"go" <path>`, the path up to it; a press past
//! the last one reports `"edit"`, which a dialog answers by giving the path
//! to a field to type in. The bar lists nothing and goes nowhere itself: its
//! `path` is set by whoever shows a folder (a file dialog, beside its
//! chooser), and the root is named by its `root` prop (`/` by default; a
//! page's storage in a tab).
//!
//! **It owns the room it is given**: a path wider than that is cut from the
//! left, the folders nearest the root giving way first behind a `...` that
//! goes to the one before the first shown, so the folder shown is always
//! there -- and alone too wide, its own name is cut short.

use serde_json::{Map, Value};

use clausters_core::osc::OscType;

use crate::host::graphics::controls;
use crate::host::widget::element::{Claim, Ctx, Element, Events, Input};
use crate::host::widget::size::Natural;
use clausters_gui_draw::draw::Draw;
use clausters_gui_draw::font::{self, Family};
use clausters_gui_draw::metrics::Metrics;
use clausters_gui_draw::rect::Rect;

/// What a press on a folder reports, and a press past the last one.
pub const GO: &str = "go";
pub const EDIT: &str = "edit";

/// What stands for the folders cut off on the left.
const CUT: &str = "...";

/// **One folder of a path**: what its button says and the path up to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub label: String,
    pub path: String,
}

/// **The folders of `path`**, the root first, named `root`: `/` and a
/// drive's letter (`C:\`) are roots, and a path that has none (`.`, one not
/// listed yet) is its folders alone. Either separator splits it.
#[must_use]
pub fn segments(path: &str, root: &str) -> Vec<Segment> {
    let bytes = path.as_bytes();
    let sep = |b: u8| b == b'/' || b == b'\\';
    let mut out = Vec::new();
    let mut at = 0;
    if bytes.first().copied().is_some_and(sep) {
        out.push(Segment {
            label: root.into(),
            path: path[..1].into(),
        });
        at = 1;
    } else if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
        let end = if bytes.get(2).copied().is_some_and(sep) {
            3
        } else {
            2
        };
        out.push(Segment {
            label: path[..end].into(),
            path: path[..end].into(),
        });
        at = end;
    }
    while at < bytes.len() {
        let end = bytes[at..]
            .iter()
            .position(|b| sep(*b))
            .map_or(bytes.len(), |k| at + k);
        if end > at {
            out.push(Segment {
                label: path[at..end].into(),
                path: path[..end].into(),
            });
        }
        at = end + 1;
    }
    out
}

/// **Where each button goes in a strip `room` wide**, from widths `widths`
/// of the folders and `cut` of the `...`, a gap `between` two: the first
/// folder shown (folders before it are behind the `...`), and the x and the
/// width of every button from there, the `...` first where there is one.
/// The last folder is always shown, as wide as the room leaves it.
fn fit(widths: &[f32], cut: f32, between: f32, room: f32) -> (usize, Vec<(f32, f32)>) {
    let Some(&last) = widths.last() else {
        return (0, Vec::new());
    };
    let total: f32 = widths.iter().sum::<f32>() + between * widths.len().saturating_sub(1) as f32;
    let mut first = 0;
    if total > room {
        // from the last back, as many as fit after the `...`
        first = widths.len() - 1;
        let mut used = cut + between + last;
        while first > 0 && used + between + widths[first - 1] <= room {
            first -= 1;
            used += between + widths[first];
        }
    }
    let mut x = 0.0;
    let mut out = Vec::new();
    if first > 0 {
        out.push((x, cut));
        x += cut + between;
    }
    for (i, &w) in widths.iter().enumerate().skip(first) {
        let w = if i + 1 == widths.len() {
            w.min((room - x).max(0.0))
        } else {
            w
        };
        out.push((x, w));
        x += w + between;
    }
    (first, out)
}

/// A path bar: the folder it shows, and what its root is called.
#[derive(Debug, Clone)]
pub struct PathBar {
    pub path: String,
    pub root: String,
    /// The button the hand holds, among those placed, while it does.
    held: Option<usize>,
}

pub(super) fn build(
    props: &Map<String, Value>,
    _blobs: &[Vec<u8>],
) -> Result<Box<dyn Element>, String> {
    Ok(Box::new(PathBar {
        path: props
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .into(),
        root: props
            .get("root")
            .and_then(Value::as_str)
            .filter(|r| !r.is_empty())
            .unwrap_or("/")
            .into(),
        held: None,
    }))
}

/// **The buttons placed in `rect`**: for each, where it goes (`None` for the
/// press past them all) and its rect, the `...` first where there is one.
struct Placed {
    buttons: Vec<(Segment, Rect)>,
    /// Where the arrow between two buttons is drawn: after each but the last.
    arrows: Vec<Rect>,
    size: f32,
}

impl PathBar {
    fn place(&self, rect: Rect, m: &Metrics) -> Placed {
        let size = controls::ui_size(Family::UiMedium, m);
        let segs = segments(&self.path, &self.root);
        let wide = |s: &str| font::width_in(Family::UiMedium, s, size) + 2.0 * m.button_x;
        let widths: Vec<f32> = segs.iter().map(|s| wide(&s.label)).collect();
        let arrow = font::width(&arrow_glyph().to_string(), size) + m.button_gap;
        let (first, at) = fit(&widths, wide(CUT), arrow, rect.w);
        let h = m.button_sm_h.min(rect.h);
        let y = rect.y + (rect.h - h) / 2.0;
        let mut shown: Vec<Segment> = Vec::new();
        if first > 0 {
            shown.push(Segment {
                label: CUT.into(),
                path: segs[first - 1].path.clone(),
            });
        }
        shown.extend(segs.into_iter().skip(first));
        let mut buttons = Vec::new();
        let mut arrows = Vec::new();
        let count = shown.len();
        for (i, (seg, (x, w))) in shown.into_iter().zip(at).enumerate() {
            let r = Rect::new(rect.x + x, y, w, h);
            if i + 1 < count {
                arrows.push(Rect::new(r.x + r.w, y, arrow, h));
            }
            buttons.push((seg, r));
        }
        Placed {
            buttons,
            arrows,
            size,
        }
    }

    /// The button at `at`, among those placed.
    fn hit(&self, at: (f64, f64), input: &Input) -> Option<(usize, Segment)> {
        let placed = self.place(input.rect, input.metrics);
        let (x, y) = (at.0 as f32, at.1 as f32);
        placed
            .buttons
            .into_iter()
            .enumerate()
            .find(|(_, (_, r))| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h)
            .map(|(i, (seg, _))| (i, seg))
    }
}

/// The arrow drawn between two folders.
fn arrow_glyph() -> char {
    controls::icon("chevron-right", '>')
}

impl Element for PathBar {
    fn set(&mut self, key: &str, v: &Value) -> bool {
        match key {
            "path" => v
                .as_str()
                .map(|p| {
                    self.path = p.into();
                    self.held = None;
                })
                .is_some(),
            "root" => v
                .as_str()
                .map(|r| self.root = if r.is_empty() { "/".into() } else { r.into() })
                .is_some(),
            _ => false,
        }
    }

    fn draw(&self, d: &mut Draw, ctx: &Ctx) {
        let placed = self.place(ctx.rect, ctx.metrics);
        let last = placed.buttons.len().saturating_sub(1);
        // a button cuts its own words to its box: the folder shown, alone
        // too wide, ends in an ellipsis
        for (i, (seg, r)) in placed.buttons.iter().enumerate() {
            controls::button(
                d,
                Some(&seg.label),
                None,
                *r,
                controls::ButtonLook {
                    // the folder shown is the one that reads as on
                    active: i == last,
                    held: self.held == Some(i),
                    flat: true,
                    ..controls::ButtonLook::default()
                },
                placed.size,
            );
        }
        let (mesh, _, theme) = d.parts();
        for r in &placed.arrows {
            font::text_centered(
                mesh,
                &arrow_glyph().to_string(),
                *r,
                placed.size,
                theme.text_dim,
            );
        }
    }

    fn natural(&self, m: &Metrics, _scale: f32) -> Natural {
        (None, Some(m.button_sm_h))
    }

    /// As narrow as the folder shown cut to an ellipsis.
    fn floor(&self, m: &Metrics, _scale: f32) -> Natural {
        let size = controls::ui_size(Family::UiMedium, m);
        (
            Some(controls::caption_floor(CUT, size, m)),
            Some(m.button_sm_h),
        )
    }

    fn value(&self) -> Option<OscType> {
        Some(OscType::String(self.path.clone()))
    }

    fn info(&self) -> Vec<(String, Value)> {
        vec![("path".into(), Value::from(self.path.clone()))]
    }

    fn press(&mut self, at: (f64, f64), input: &Input) -> Claim {
        self.held = self.hit(at, input).map(|(i, _)| i);
        Claim::take()
    }

    /// **A click goes**: to the folder of the button let go on, or, past the
    /// last one, to the field the path is typed in.
    fn release(&mut self, at: (f64, f64), inside: bool, input: &Input) -> Events {
        let held = self.held.take();
        if !inside {
            return Events::none();
        }
        let tell = |args: Vec<&str>| {
            Events::none().and_interface(
                args.into_iter()
                    .map(|a| OscType::String(a.into()))
                    .collect(),
            )
        };
        match (held, self.hit(at, input)) {
            (Some(i), Some((j, seg))) if i == j => tell(vec![GO, &seg.path]),
            (None, None) => tell(vec![EDIT]),
            _ => Events::none(),
        }
    }

    /// Like a row of tools, it takes no focus: the keys stay where the hand
    /// was typing.
    fn accepts_focus(&self) -> bool {
        false
    }

    fn clone_box(&self) -> Box<dyn Element> {
        Box::new(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(path: &str, root: &str) -> Vec<String> {
        segments(path, root).into_iter().map(|s| s.label).collect()
    }

    /// **A path is its folders from the root**, each going to the path up
    /// to it, whichever separator and root it has.
    #[test]
    fn a_path_is_its_folders_from_the_root() {
        assert_eq!(
            labels("/home/ana/takes", "/"),
            ["/", "home", "ana", "takes"]
        );
        assert_eq!(segments("/home/ana", "/")[1].path, "/home");
        assert_eq!(labels("/", "Storage"), ["Storage"]);
        assert_eq!(labels("/sessions/", "Storage"), ["Storage", "sessions"]);
        let drive = segments(r"C:\Users\ana", "/");
        assert_eq!(drive[0].label, r"C:\");
        assert_eq!(drive[2].path, r"C:\Users\ana");
        assert_eq!(labels(".", "/"), ["."]);
    }

    /// **Too wide, it is cut from the left**: the `...` and the folders
    /// nearest the one shown, which is always there, cut to the room left.
    #[test]
    fn a_path_too_wide_is_cut_from_the_left() {
        let widths = [20.0, 50.0, 50.0, 50.0];
        let (first, at) = fit(&widths, 20.0, 10.0, 500.0);
        assert_eq!(first, 0);
        assert_eq!(at.len(), 4);
        let (first, at) = fit(&widths, 20.0, 10.0, 150.0);
        assert_eq!(first, 2, "the ... and the last two");
        assert_eq!(at[0], (0.0, 20.0));
        assert_eq!(at[1], (30.0, 50.0));
        assert_eq!(at[2], (90.0, 50.0), "the last, whole where it fits");
        let (first, at) = fit(&widths, 20.0, 10.0, 60.0);
        assert_eq!(first, 3);
        assert_eq!(at[1], (30.0, 30.0), "the last alone, cut to the room");
    }
}
