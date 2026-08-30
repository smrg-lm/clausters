//! The module's own suite: it engraves a page from a display list, then asks
//! the questions the fronts ask — what is under this point, where does the
//! playhead sit, what does a drag of this many pixels mean in steps — and
//! checks the geometry that comes back.

use lyon::tessellation::FillTessellator;
use serde_json::{Map, Value};

use super::glyphs::build_path;
use super::tess::fill_path;
use super::*;
use crate::host::layout::Rect;
use crate::host::paint::Mesh;

/// One opaque palette for the render tests: they assert that geometry
/// lands, not which role painted it.
const INK: ScoreColors = ScoreColors {
    ink: [1.0; 4],
    playhead: [1.0; 4],
    selection: [1.0; 4],
};

#[test]
fn affine_composition_matches_manual() {
    let a = Affine {
        tx: 10.0,
        ty: 20.0,
        sx: 2.0,
        sy: -2.0,
    };
    let b = Affine {
        tx: 1.0,
        ty: 1.0,
        sx: 0.5,
        sy: 0.5,
    };
    let c = a.then(b);
    // c should map (4,4) the same as a(b(4,4))
    let manual = a.apply(b.apply(4.0, 4.0)[0], b.apply(4.0, 4.0)[1]);
    assert_eq!(c.apply(4.0, 4.0), manual);
}

#[test]
fn fit_centres_and_preserves_aspect() {
    let data = ScoreData {
        vb_w: 100.0,
        vb_h: 50.0,
        ..Default::default()
    };
    let fit = data.fit(Rect::new(0.0, 0.0, 200.0, 200.0));
    // width-bound: scale 2, page height 100 centred in 200 -> ty = 50
    assert_eq!(fit.sx, 2.0);
    assert_eq!(fit.sy, 2.0);
    assert_eq!(fit.ty, 50.0);
    assert_eq!(fit.tx, 0.0);
}

#[test]
fn parses_absolute_moveto_lineto_close() {
    // a unit triangle
    let p = build_path("M0 0 L10 0 L0 10 Z").expect("path");
    // lyon builds it; a filled triangle tessellates to >=1 triangle
    let mut mesh = Mesh::new();
    let mut tess = FillTessellator::new();
    fill_path(
        &mut mesh,
        &mut tess,
        "M0 0 L10 0 L0 10 Z",
        Affine::IDENTITY,
        0.1,
        [1.0; 4],
    );
    assert!(
        !mesh.is_empty(),
        "closed triangle should tessellate to geometry"
    );
    drop(p);
}

#[test]
fn parses_relative_cubic_like_a_glyph_outline() {
    // the notehead U+E0A4 outline shape (relative cubics), from verovio
    let d = "M0 -39c0 68 73 172 200 172c66 0 114 -37 114 -95c0 -84 -106 -171 -218 -171c-58 0 -96 34 -96 93Z";
    let mut mesh = Mesh::new();
    let mut tess = FillTessellator::new();
    fill_path(&mut mesh, &mut tess, d, Affine::IDENTITY, 0.5, [1.0; 4]);
    assert!(
        !mesh.is_empty(),
        "glyph outline should tessellate to geometry"
    );
}

#[test]
fn malformed_path_is_skipped_not_panicked() {
    let mut mesh = Mesh::new();
    let mut tess = FillTessellator::new();
    fill_path(
        &mut mesh,
        &mut tess,
        "M0 0 Q nonsense",
        Affine::IDENTITY,
        0.5,
        [1.0; 4],
    );
    assert!(mesh.is_empty());
}

#[test]
fn parses_display_list_props_and_renders() {
    // the shape the Python `notation.engrave` builder sends: a glyph table,
    // a placed glyph (flip folded into a negative sy) and a staff line.
    let props: Map<String, Value> = serde_json::from_str(
        r#"{
            "vb": [1000, 400],
            "glyphs": {"E0A4": "M0 -39c0 68 73 172 200 172c66 0 114 -37 114 -95c0 -84 -106 -171 -218 -171c-58 0 -96 34 -96 93Z"},
            "prims": [
                {"k": "glyph", "cp": "E0A4", "xf": [500, 200, 0.72, -0.72], "id": "n1"},
                {"k": "line", "pts": [[0, 100], [1000, 100]], "w": 13, "id": "s1"}
            ]
        }"#,
    )
    .unwrap();
    let data = ScoreData::parse(&props);
    assert_eq!(data.glyphs.len(), 1);
    assert_eq!(data.prims.len(), 2);
    assert_eq!(data.prims[0].id(), Some("n1"));
    let mut mesh = Mesh::new();
    data.render(
        &mut mesh,
        Rect::new(0.0, 0.0, 500.0, 200.0),
        None,
        -1.0,
        INK,
    );
    assert!(
        !mesh.is_empty(),
        "a parsed score should tessellate to geometry"
    );
}

#[test]
fn parses_and_renders_text_prim() {
    let props: Map<String, Value> = serde_json::from_str(
        r#"{"vb":[1000,400],"glyphs":{},
            "prims":[{"k":"text","s":"mf","x":500,"y":200,"size":80,"id":"d1"}]}"#,
    )
    .unwrap();
    let data = ScoreData::parse(&props);
    assert_eq!(data.prims.len(), 1);
    assert_eq!(data.prims[0].id(), Some("d1"));
    let mut mesh = Mesh::new();
    data.render(
        &mut mesh,
        Rect::new(0.0, 0.0, 500.0, 200.0),
        None,
        -1.0,
        INK,
    );
    assert!(
        !mesh.is_empty(),
        "score text should paint through the host font"
    );
}

#[test]
fn playhead_selects_the_active_cursor_and_draws() {
    let props: Map<String, Value> = serde_json::from_str(
        r#"{"vb":[1000,400],"glyphs":{},"prims":[],
            "cursors":[{"t":0,"x":100,"y0":50,"y1":150},
                       {"t":500,"x":300,"y0":50,"y1":150},
                       {"t":1000,"x":500,"y0":50,"y1":150}],
            "playhead":600}"#,
    )
    .unwrap();
    let data = ScoreData::parse(&props);
    assert_eq!(data.cursors.len(), 3);
    assert_eq!(data.playhead, 600.0);
    // active cursor at t=600 is the one at t=500 (x=300)
    let fit = data.fit(Rect::new(0.0, 0.0, 1000.0, 400.0));
    let idx = data
        .cursors
        .partition_point(|c| c.t <= data.playhead)
        .saturating_sub(1);
    assert_eq!(data.cursors[idx].x, 300.0);
    let mut mesh = Mesh::new();
    data.render(
        &mut mesh,
        Rect::new(0.0, 0.0, 1000.0, 400.0),
        None,
        data.head_ms(0.0, 0.0),
        INK,
    );
    assert!(!mesh.is_empty(), "the playhead line should paint");
    let _ = fit;
}

#[test]
fn playhead_off_when_negative() {
    let mut data = ScoreData {
        vb_w: 1000.0,
        vb_h: 400.0,
        ..Default::default()
    };
    data.cursors.push(Cursor {
        t: 0.0,
        x: 100.0,
        y0: 50.0,
        y1: 150.0,
    });
    data.playhead = -1.0;
    let mut mesh = Mesh::new();
    data.render(
        &mut mesh,
        Rect::new(0.0, 0.0, 1000.0, 400.0),
        None,
        data.head_ms(0.0, 0.0),
        INK,
    );
    assert!(mesh.is_empty(), "no playhead when time is negative");
}

#[test]
fn playhead_at_sweeps_off_the_engine_clock() {
    let props: Map<String, Value> = serde_json::from_str(
        r#"{"vb":[1000,400],"glyphs":{},"prims":[],
            "cursors":[{"t":0,"x":100,"y0":50,"y1":150},
                       {"t":500,"x":300,"y0":50,"y1":150}],
            "playhead":250,"playhead_at":48000,"sample_rate":48000}"#,
    )
    .unwrap();
    let data = ScoreData::parse(&props);
    // one second of clock past the origin is one second of musical time
    assert_eq!(data.head_ms(96_000.0, 44_100.0), 1000.0);
    // the widget's own rate wins over the server's; without one, the
    // server's places the time
    let host_rate = ScoreData {
        sample_rate: 0.0,
        ..data.clone()
    };
    assert_eq!(host_rate.head_ms(96_000.0, 24_000.0), 2000.0);
    // not playing: the static time stands still whatever the clock says
    let stopped = ScoreData {
        playhead_at: -1.0,
        ..data.clone()
    };
    assert_eq!(stopped.head_ms(96_000.0, 48_000.0), 250.0);
    // a rate nobody knows leaves the static time in place too
    let no_rate = ScoreData {
        sample_rate: 0.0,
        ..data
    };
    assert_eq!(no_rate.head_ms(96_000.0, 0.0), 250.0);
}

/// A page with a notehead glyph at (500, 200) sitting on a full-width staff
/// line — the two overlapping hit targets a click has to choose between.
fn indexed_page() -> ScoreData {
    let props: Map<String, Value> = serde_json::from_str(
        r#"{
            "vb": [1000, 400],
            "glyphs": {"E0A4": "M0 -39c0 68 73 172 200 172c66 0 114 -37 114 -95c0 -84 -106 -171 -218 -171c-58 0 -96 34 -96 93Z"},
            "prims": [
                {"k": "line", "pts": [[0, 200], [1000, 200]], "w": 13, "id": "staff"},
                {"k": "glyph", "cp": "E0A4", "xf": [500, 200, 0.72, -0.72], "id": "n1"}
            ]
        }"#,
    )
    .unwrap();
    ScoreData::parse(&props)
}

#[test]
fn a_click_names_the_smallest_element_under_it() {
    let data = indexed_page();
    assert_eq!(data.hits.len(), 2);
    // fitted 1:1 (a 1000x400 page into a 1000x400 rect), so page == screen
    let rect = Rect::new(0.0, 0.0, 1000.0, 400.0);
    assert_eq!(data.fit(rect).sx, 1.0);
    // over the notehead, where both boxes overlap: the note wins
    assert_eq!(data.hit(rect, 550.0, 190.0), Some("n1"));
    // on the staff line away from the note: only the line is there
    assert_eq!(data.hit(rect, 100.0, 200.0), Some("staff"));
    // blank paper names nothing
    assert_eq!(data.hit(rect, 100.0, 380.0), None);
}

/// **A notehead is an oval, and the corners of the box around it are paper.**
/// The index measures extents, so every entry arrives as a rectangle; a
/// notehead's is a quarter larger than the glyph and that quarter is where the
/// stem, the beam and the note on the next line live. The entry carries the
/// shape it stands for, so the test is the ellipse inscribed in the box.
#[test]
fn a_notehead_is_hit_as_the_oval_it_is_drawn_as() {
    let data = indexed_page();
    let rect = Rect::new(0.0, 0.0, 1000.0, 400.0);
    let head = data
        .hits
        .iter()
        .find(|h| h.id == "n1")
        .expect("the notehead is indexed");
    assert_eq!(head.shape, HitShape::Ellipse);
    let b = head.bounds;
    // Inside the oval, off the staff line that crosses it (the smaller box
    // wins where two overlap, and that rule is untouched).
    assert_eq!(data.hit(rect, 550.0, 190.0), Some("n1"), "the head itself");
    // A hair inside the box's top-left corner: inside the rectangle, outside
    // the oval — and it used to name the note.
    assert_ne!(data.hit(rect, b.x0 + 1.0, b.y0 + 1.0), Some("n1"));
    // The staff line, whose own extent is a stroke, is unaffected.
    assert_eq!(data.hit(rect, 100.0, 200.0), Some("staff"));
}

#[test]
fn hit_testing_follows_the_page_fit() {
    let data = indexed_page();
    // half scale, so the notehead's page x=500 lands at screen x=250
    let rect = Rect::new(0.0, 0.0, 500.0, 200.0);
    assert_eq!(data.fit(rect).sx, 0.5);
    assert_eq!(data.hit(rect, 275.0, 95.0), Some("n1"));
    // the same screen point on the unscaled page is blank paper
    assert_eq!(
        data.hit(Rect::new(0.0, 0.0, 1000.0, 400.0), 275.0, 95.0),
        None
    );
}

#[test]
fn a_selected_element_is_highlighted() {
    let props: Map<String, Value> = serde_json::from_str(
        r#"{"vb":[1000,400],"glyphs":{},
            "prims":[{"k":"line","pts":[[0,200],[1000,200]],"w":13,"id":"staff"}],
            "selected":"staff"}"#,
    )
    .unwrap();
    let mut data = ScoreData::parse(&props);
    assert_eq!(data.selected.as_deref(), Some("staff"));
    let rect = Rect::new(0.0, 0.0, 1000.0, 400.0);
    let mut with = Mesh::new();
    data.render(&mut with, rect, None, -1.0, INK);
    data.selected = None;
    let mut without = Mesh::new();
    data.render(&mut without, rect, None, -1.0, INK);
    assert!(
        with.positions().count() > without.positions().count(),
        "the highlight should add geometry over the engraving"
    );
}

#[test]
fn a_vertical_drag_counts_whole_diatonic_steps() {
    let data = indexed_page();
    // fitted 1:1, so a step is the page's own 90 units
    let rect = Rect::new(0.0, 0.0, 1000.0, 400.0);
    assert_eq!(data.step, STEP);
    // up the staff is up in pitch, and the count rounds to whole steps
    assert_eq!(data.steps_for(rect, -90.0), 1);
    assert_eq!(data.steps_for(rect, -44.0), 0);
    assert_eq!(data.steps_for(rect, -46.0), 1);
    assert_eq!(data.steps_for(rect, 270.0), -3);
    // half the scale halves the pixels a step takes
    assert_eq!(data.steps_for(Rect::new(0.0, 0.0, 500.0, 200.0), -90.0), 2);
}

#[test]
fn a_dragged_element_is_drawn_displaced() {
    let mut data = indexed_page();
    let rect = Rect::new(0.0, 0.0, 1000.0, 400.0);
    let fit = data.fit(rect);
    // the note travels a step up; the staff line it sat on does not move
    data.drag = Some(ScoreDrag {
        id: "n1".into(),
        steps: 1,
    });
    assert_eq!(data.prim_fit(fit, Some("n1")).ty, fit.ty - STEP);
    assert_eq!(data.prim_fit(fit, Some("staff")).ty, fit.ty);
    assert_eq!(data.prim_fit(fit, None).ty, fit.ty);
    // and down again the other way
    data.drag = Some(ScoreDrag {
        id: "n1".into(),
        steps: -2,
    });
    assert_eq!(data.prim_fit(fit, Some("n1")).ty, fit.ty + 2.0 * STEP);
}

#[test]
fn the_drag_moves_the_drawn_geometry_by_whole_steps() {
    // one notehead alone on the page, so every triangle drawn is its own
    let mut data = ScoreData {
        vb_w: 1000.0,
        vb_h: 400.0,
        ..indexed_page()
    };
    data.prims.retain(|p| p.id() == Some("n1"));
    let rect = Rect::new(0.0, 0.0, 1000.0, 400.0); // fitted 1:1
    let top = |data: &ScoreData| {
        let mut mesh = Mesh::new();
        data.render(&mut mesh, rect, None, -1.0, INK);
        mesh.positions().map(|p| p.1).fold(f32::MAX, f32::min)
    };
    let before = top(&data);
    // down the staff, which on the page is down in y (and keeps the whole
    // notehead inside the page, where the render clips)
    data.drag = Some(ScoreDrag {
        id: "n1".into(),
        steps: -2,
    });
    assert!((top(&data) - (before + 2.0 * STEP)).abs() < 0.01);
}

/// A treble staff with a middle C below it, in verovio's own numbers (the
/// engraved page of `4CDEF/` at scale 40): five lines 180 apart, the
/// notehead a whole line position below the last, and the ledger line the
/// engraver drew for it — tagged with the staff, as verovio tags it.
fn staffed_page() -> ScoreData {
    let props: Map<String, Value> = serde_json::from_str(
        r#"{
            "vb": [11000, 3000], "step": 90,
            "glyphs": {"E0A4": "M0 -39c0 68 73 172 200 172c66 0 114 -37 114 -95c0 -84 -106 -171 -218 -171c-58 0 -96 34 -96 93Z"},
            "prims": [
                {"k": "line", "pts": [[600, 1040], [10400, 1040]], "w": 13, "id": "staff"},
                {"k": "line", "pts": [[600, 1220], [10400, 1220]], "w": 13, "id": "staff"},
                {"k": "line", "pts": [[600, 1400], [10400, 1400]], "w": 13, "id": "staff"},
                {"k": "line", "pts": [[600, 1580], [10400, 1580]], "w": 13, "id": "staff"},
                {"k": "line", "pts": [[600, 1760], [10400, 1760]], "w": 13, "id": "staff"},
                {"k": "glyph", "cp": "E0A4", "xf": [1783, 1940, 0.72, -0.72], "id": "n1"},
                {"k": "line", "pts": [[1735, 1940], [2057, 1940]], "w": 22, "id": "staff"}
            ]
        }"#,
    )
    .unwrap();
    ScoreData::parse(&props)
}

#[test]
fn the_staves_are_read_back_out_of_the_engraving() {
    let data = staffed_page();
    assert_eq!(
        data.staves,
        vec![Staff {
            y0: 1040.0,
            y1: 1760.0,
            width: 13.0
        }]
    );
    // the ledger line is too short to be a staff line, and the notehead is
    // not a line at all
    assert_eq!(data.staff_at(1940.0), data.staves.first().copied());
}

#[test]
fn ledger_lines_follow_the_engraving_rule() {
    let data = staffed_page();
    let staff = data.staves[0];
    let ys = |y: f32| data.ledger_ys(staff, y);
    // on the staff, and in the space just outside it: nothing to draw
    assert!(ys(1400.0).is_empty());
    assert!(ys(1850.0).is_empty()); // one step below the last line
    assert!(ys(950.0).is_empty()); // one step above the first
    // on the first ledger position, and in the space beyond it: one line
    assert_eq!(ys(1940.0), vec![1940.0]);
    assert_eq!(ys(2030.0), vec![1940.0]);
    // two positions out: two lines, and upward is symmetrical
    assert_eq!(ys(2120.0), vec![1940.0, 2120.0]);
    assert_eq!(ys(860.0), vec![860.0]);
    assert_eq!(ys(680.0), vec![860.0, 680.0]);
}

#[test]
fn dragging_off_the_staff_draws_ledger_lines_and_back_on_drops_them() {
    let mut data = staffed_page();
    let rect = Rect::new(0.0, 0.0, 1100.0, 300.0);
    let verts = |data: &ScoreData| {
        let mut mesh = Mesh::new();
        data.render(&mut mesh, rect, None, -1.0, INK);
        mesh.positions().count()
    };
    let engraved = verts(&data); // one ledger line, the engraved one
    // dragged up onto the staff: its ledger line goes with it, which means
    // it stops being drawn
    data.drag = Some(ScoreDrag {
        id: "n1".into(),
        steps: 2,
    });
    let on_staff = verts(&data);
    // dragged two positions below: the engraved one plus a second
    data.drag = Some(ScoreDrag {
        id: "n1".into(),
        steps: -2,
    });
    let below = verts(&data);
    assert!(on_staff < engraved && engraved < below);
    assert_eq!(engraved - on_staff, below - engraved);
    // and the drag that moves nothing redraws exactly what was engraved
    data.drag = Some(ScoreDrag {
        id: "n1".into(),
        steps: 0,
    });
    assert_eq!(verts(&data), engraved);
}

#[test]
fn the_page_carries_its_own_step() {
    // the step follows verovio's `unit`, not the staff scale, so the client
    // sends it with the page; a nonsensical one falls back to the default.
    let props: Map<String, Value> =
        serde_json::from_str(r#"{"vb":[1000,400],"step":120}"#).unwrap();
    assert_eq!(ScoreData::parse(&props).step, 120.0);
    let props: Map<String, Value> = serde_json::from_str(r#"{"vb":[1000,400],"step":0}"#).unwrap();
    assert_eq!(ScoreData::parse(&props).step, STEP);
}

#[test]
fn line_prim_renders_within_clip() {
    let mut data = ScoreData {
        vb_w: 100.0,
        vb_h: 100.0,
        ..Default::default()
    };
    data.prims.push(Prim::Line {
        pts: vec![[0.0, 50.0], [100.0, 50.0]],
        width: 2.0,
        id: None,
    });
    let mut mesh = Mesh::new();
    data.render(
        &mut mesh,
        Rect::new(0.0, 0.0, 200.0, 200.0),
        None,
        -1.0,
        INK,
    );
    assert!(!mesh.is_empty(), "a staff line should paint");
}

/// **A press on blank paper names a place, in the ids the client engraved.**
/// The host is the only one that can measure where the finger went; it says so
/// and stops there — no pitch, because that needs a clef and a key it does not
/// have, and no duration, because a click implies none.
#[test]
fn a_press_on_blank_paper_names_the_staff_the_step_and_what_it_follows() {
    let mut data = staffed_page();
    // The client says which ids are sounding elements. Without that a staff
    // line, which carries the staff's id, would be "the element to the left" of
    // everything -- to a renderer an id is an id.
    data.elements.insert("n1".to_string());
    let rect = Rect::new(0.0, 0.0, 11000.0, 3000.0);
    assert_eq!(data.fit(rect).sx, 1.0);

    // To the right of the note, on the staff: it follows the note.
    let entry = data
        .entry_at(rect, 4000.0, 1400.0)
        .expect("a staff is there");
    assert_eq!(entry.after.as_deref(), Some("n1"));
    assert_eq!(entry.staff, 0);
    // 1400 is two steps of 90... no: the staff's top line is 1040 and a step is
    // 90, so 1400 is four steps below it.
    assert_eq!(entry.position, -4);

    // Before everything: nothing to follow, which is where an empty score
    // begins rather than an error.
    let first = data.entry_at(rect, 700.0, 1040.0).expect("still the staff");
    assert_eq!(first.after, None);
    assert_eq!(first.position, 0, "the top line itself");
}

/// **The furniture is not an element.** A staff's lines carry the staff's id,
/// so without the client's list the nearest thing to the left of any press is
/// always a staff line -- and an insertion "after the staff" names nothing a
/// model can resolve.
#[test]
fn a_staff_line_is_never_what_a_new_note_follows() {
    let data = staffed_page();
    assert!(data.elements.is_empty(), "nothing was declared an element");
    let rect = Rect::new(0.0, 0.0, 11000.0, 3000.0);
    let entry = data
        .entry_at(rect, 4000.0, 1400.0)
        .expect("a staff is there");
    assert_eq!(entry.after, None, "the staff line is not a candidate");
}

/// The measurement follows the page fit, exactly as hit testing does: the same
/// screen point on a half-scale page is a different place on the page.
#[test]
fn an_insertion_point_follows_the_page_fit() {
    let data = staffed_page();
    let full = data.entry_at(Rect::new(0.0, 0.0, 11000.0, 3000.0), 4000.0, 1400.0);
    let half = data.entry_at(Rect::new(0.0, 0.0, 5500.0, 1500.0), 2000.0, 700.0);
    assert_eq!(full.map(|e| e.position), half.map(|e| e.position));
}
