//! The timemap -> cursor-track fold: musical time turned into page geometry.
//!
//! The engraver reports *when* each element sounds (a timemap: onset ms -> the
//! ids starting and stopping then) and, separately, *where* it was drawn (the
//! display list, each primitive carrying its `xml:id`). The playhead needs both
//! at once, and the id is the join: for every onset, take its leftmost note's
//! page-x and the vertical span of the system that note sits on.
//!
//! This is pure geometry over data the engraver already produced, so it lives
//! here rather than beside the binding — a wasm client driving verovio-wasm gets
//! the identical track from the identical fold.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::svg::{DisplayList, Prim, r, staff_line_ys};

/// One timemap entry as the engraver emits it: the onset in milliseconds and
/// the ids that start sounding there. The other fields verovio writes (`qstamp`,
/// `off`, `tempo`, ...) are ignored — only onsets place a cursor.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TimemapEntry {
    /// Onset in milliseconds. Absent on entries that only stop notes.
    pub tstamp: Option<f64>,
    /// The MEI ids that begin sounding at `tstamp`.
    #[serde(default)]
    pub on: Vec<String>,
}

/// Where the playhead stands at one onset: `t` milliseconds, page-x `x`, and the
/// `y0..y1` span of the system the sounding note is on.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Cursor {
    pub t: f64,
    pub x: f64,
    pub y0: f64,
    pub y1: f64,
}

/// Fold `timemap` together with the placed geometry of `dl` into a cursor track,
/// sorted by time and ready for the host's `playhead`.
///
/// An onset whose ids were never drawn (an element off this page) places no
/// cursor, so the track only ever names positions the host can actually reach.
pub fn cursor_track(dl: &DisplayList, timemap: &[TimemapEntry]) -> Vec<Cursor> {
    let positions = id_positions(&dl.prims);
    let systems = staff_systems(&dl.prims);
    let mut track: Vec<Cursor> = timemap
        .iter()
        .filter_map(|entry| {
            let t = entry.tstamp?;
            // The leftmost of the notes starting here: the one the cursor sits on.
            let lead = entry
                .on
                .iter()
                .filter_map(|id| positions.get(id.as_str()).copied())
                .min_by(|a, b| a.0.total_cmp(&b.0))?;
            let (y0, y1) = system_bounds(&systems, lead.1);
            Some(Cursor {
                t: r(t, 1),
                x: r(lead.0, 1),
                y0: r(y0, 1),
                y1: r(y1, 1),
            })
        })
        .collect();
    track.sort_by(|a, b| a.t.total_cmp(&b.t));
    track
}

/// Each MEI id's page position, preferring the glyph (notehead) placement — its
/// transform origin — and falling back to a line's first point. First primitive
/// per id wins, which is the notehead in verovio's draw order.
fn id_positions(prims: &[Prim]) -> BTreeMap<&str, (f64, f64)> {
    let mut out = BTreeMap::new();
    for p in prims {
        let Some(id) = p.id() else { continue };
        let xy = match p {
            Prim::Glyph { xf, .. } => (xf[0], xf[1]),
            Prim::Line { pts, .. } if !pts.is_empty() => (pts[0][0], pts[0][1]),
            _ => continue,
        };
        out.entry(id).or_insert(xy);
    }
    out
}

/// Cluster the horizontal staff lines into systems, each a `(y_top, y_bottom)`
/// pair. A large vertical gap between lines starts a new system.
fn staff_systems(prims: &[Prim]) -> Vec<(f64, f64)> {
    let ys = staff_line_ys(prims);
    let Some(&first) = ys.first() else {
        return Vec::new();
    };
    let mut systems = Vec::new();
    let mut group = vec![first];
    for &y in &ys[1..] {
        if y - group[group.len() - 1] > 500.0 {
            // a gap between systems is far wider than one between staff lines
            systems.push((group[0], group[group.len() - 1]));
            group = vec![y];
        } else {
            group.push(y);
        }
    }
    systems.push((group[0], group[group.len() - 1]));
    systems
}

/// The `(y0, y1)` cursor span for a note at page-y `y`: its system's staff
/// extent, padded for the stems that reach above and below it.
fn system_bounds(systems: &[(f64, f64)], y: f64) -> (f64, f64) {
    let nearest = systems
        .iter()
        .min_by(|a, b| distance(y, **a).total_cmp(&distance(y, **b)));
    match nearest {
        None => (y - 400.0, y + 400.0),
        Some(&(top, bot)) => {
            let pad = (bot - top) * 0.6 + 100.0;
            (top - pad, bot + pad)
        }
    }
}

fn distance(y: f64, system: (f64, f64)) -> f64 {
    (y - system.0).abs().min((y - system.1).abs())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glyph(id: &str, x: f64, y: f64) -> Prim {
        Prim::Glyph {
            cp: "E0A4".into(),
            xf: [x, y, 1.0, 1.0],
            id: Some(id.into()),
        }
    }

    /// A five-line staff at `top`, spaced 90 apart (verovio's default unit).
    fn staff(top: f64) -> Vec<Prim> {
        (0..5)
            .map(|i| Prim::Line {
                pts: vec![
                    [0.0, top + 90.0 * i as f64],
                    [2000.0, top + 90.0 * i as f64],
                ],
                w: 5.0,
                id: None,
            })
            .collect()
    }

    fn entry(t: f64, on: &[&str]) -> TimemapEntry {
        TimemapEntry {
            tstamp: Some(t),
            on: on.iter().map(|s| (*s).to_string()).collect(),
        }
    }

    #[test]
    fn an_onset_takes_the_x_of_its_leftmost_note() {
        let mut prims = staff(1000.0);
        prims.push(glyph("n2", 800.0, 1090.0));
        prims.push(glyph("n1", 400.0, 1180.0));
        let dl = DisplayList {
            prims,
            ..Default::default()
        };
        let track = cursor_track(&dl, &[entry(0.0, &["n2", "n1"])]);
        assert_eq!(track.len(), 1);
        assert_eq!(track[0].x, 400.0, "the leftmost of the chord");
    }

    #[test]
    fn the_span_covers_the_system_the_note_sits_on() {
        // Two systems 2000 apart; a note on the lower one spans the lower staff.
        let mut prims = staff(1000.0);
        prims.extend(staff(3000.0));
        prims.push(glyph("n1", 400.0, 3090.0));
        let dl = DisplayList {
            prims,
            ..Default::default()
        };
        let track = cursor_track(&dl, &[entry(0.0, &["n1"])]);
        // The lower staff runs 3000..3360; pad = 360 * 0.6 + 100 = 316.
        assert_eq!((track[0].y0, track[0].y1), (2684.0, 3676.0));
    }

    #[test]
    fn an_undrawn_onset_places_no_cursor() {
        let mut prims = staff(1000.0);
        prims.push(glyph("n1", 400.0, 1090.0));
        let dl = DisplayList {
            prims,
            ..Default::default()
        };
        let track = cursor_track(&dl, &[entry(0.0, &["elsewhere"]), entry(500.0, &["n1"])]);
        assert_eq!(track.len(), 1);
        assert_eq!(track[0].t, 500.0);
    }

    #[test]
    fn the_track_comes_out_sorted_by_time() {
        let mut prims = staff(1000.0);
        prims.push(glyph("a", 400.0, 1090.0));
        prims.push(glyph("b", 800.0, 1090.0));
        let dl = DisplayList {
            prims,
            ..Default::default()
        };
        let track = cursor_track(&dl, &[entry(500.0, &["b"]), entry(0.0, &["a"])]);
        assert_eq!(
            track.iter().map(|c| c.t).collect::<Vec<_>>(),
            vec![0.0, 500.0]
        );
    }

    #[test]
    fn without_a_staff_the_span_is_a_fixed_pad() {
        let dl = DisplayList {
            prims: vec![glyph("n1", 400.0, 1000.0)],
            ..Default::default()
        };
        let track = cursor_track(&dl, &[entry(0.0, &["n1"])]);
        assert_eq!((track[0].y0, track[0].y1), (600.0, 1400.0));
    }

    #[test]
    fn a_timemap_entry_without_an_onset_is_skipped() {
        let dl = DisplayList {
            prims: vec![glyph("n1", 400.0, 1000.0)],
            ..Default::default()
        };
        let off_only = TimemapEntry {
            tstamp: None,
            on: vec!["n1".into()],
        };
        assert!(cursor_track(&dl, &[off_only]).is_empty());
    }

    #[test]
    fn a_verovio_timemap_deserializes() {
        let json = r#"[{"tstamp": 0, "qstamp": 0, "on": ["note-1"]},
                       {"tstamp": 500, "off": ["note-1"]}]"#;
        let map: Vec<TimemapEntry> = serde_json::from_str(json).expect("parses");
        assert_eq!(map.len(), 2);
        assert_eq!(map[0].on, ["note-1"]);
        assert!(map[1].on.is_empty(), "an off-only entry carries no onsets");
    }
}
