use super::draw::label_tops;
use super::scale::*;
use super::*;
use crate::host::widget::RulerY;
use crate::spectrogram::FreqScale;
use crate::waveform::AMP_MARGIN;
use clausters_core::tempomap::TempoMap;
use clausters_gui_draw::draw::Draw;
use clausters_gui_draw::font;
use clausters_gui_draw::metrics::Metrics;
use clausters_gui_draw::rect::Rect;

/// **A file of no samples has no duration to show**: its ruler is its
/// start alone, `0` in seconds and in samples.
#[test]
fn an_empty_axis_is_ruled_at_its_start_alone() {
    let m = Metrics::default();
    for unit in [TimeUnit::Seconds, TimeUnit::Samples] {
        let ticks = origin_ticks(48_000.0, unit, &m);
        assert_eq!(ticks.len(), 1, "{unit:?}");
        assert_eq!(ticks[0].frac, 0.0);
        assert_eq!(ticks[0].label.as_deref(), Some("0"));
    }
}
use clausters_gui_draw::paint::Mesh;
use clausters_gui_draw::theme::Theme;

/// A value strip left of a body at `body_x`, from `strip_x`: the view's own
/// strip, the form most of these tests were written against.
fn draw_ticks_v(d: &mut Draw, body_x: f32, strip_x: f32, row: Rect, ticks: &[Tick]) {
    let strip = Rect::new(strip_x, row.y, body_x - strip_x, row.h);
    Ruler::new(Scale::Value { lo: 0.0, hi: 1.0 }, RulerDir::Right).draw_ticks(
        d,
        strip,
        row,
        ticks,
        Shared::default(),
    );
}

// The generators as they were called before a scale knew its axis: each
// over the axis it was written for.
fn time_ticks(
    start: f64,
    len: f64,
    width_px: f64,
    sample_rate: f64,
    unit: TimeUnit,
    metrics: &Metrics,
) -> Vec<Tick> {
    time_ticks_on(Gaps::of(metrics), start, len, width_px, sample_rate, unit)
}

fn amp_ticks(
    unit: RulerY,
    height_px: f64,
    bit_depth: u32,
    y_start: f64,
    y_len: f64,
    metrics: &Metrics,
) -> Vec<Tick> {
    amp_ticks_on(
        Gaps::along(metrics, Along::Vertical),
        unit,
        height_px,
        bit_depth,
        y_start,
        y_len,
    )
}

fn value_ticks(lo: f64, hi: f64, height_px: f64, metrics: &Metrics) -> Vec<Tick> {
    value_ticks_on(Gaps::along(metrics, Along::Vertical), lo, hi, height_px)
}

fn labels(ticks: &[Tick]) -> Vec<&str> {
    ticks.iter().filter_map(|t| t.label.as_deref()).collect()
}

/// **Two stacked channel rows keep the space between their labels that
/// one row keeps**: eight channels at the row floor drew the bottom
/// label of one row (`-1`) and the top label of the next (`1`) a few
/// pixels apart. On a shared edge the label that cannot keep clear of it
/// is dropped, and the stack's own edges keep theirs.
#[test]
fn stacked_rows_keep_their_labels_clear_across_the_edge_they_share() {
    let m = Metrics::default();
    let line = font::height(m.caption_scale);
    let g = Gaps::of(&m);
    let rows = 3;
    let h = (line * 3.0).round();
    for (unit, label) in [(RulerY::Norm, "normalized"), (RulerY::Db, "dB")] {
        let ticks = amp_ticks(unit, h as f64, 16, 0.0, 1.0, &m);
        // each row's labels, top to bottom
        let rows_of: Vec<Vec<f32>> = (0..rows)
            .map(|index| {
                let row = Rect::new(0.0, 100.0 + h * index as f32, 300.0, h);
                let mut tops: Vec<f32> =
                    label_tops(row, &ticks, line, m.gap, Shared::of(index, rows))
                        .into_iter()
                        .flatten()
                        .collect();
                tops.sort_by(f32::total_cmp);
                tops
            })
            .collect();
        assert!(
            rows_of.iter().all(|tops| !tops.is_empty()),
            "{label}: every row keeps a label"
        );
        // across each shared edge, the space one row keeps between labels
        for pair in rows_of.windows(2) {
            let above = pair[0].last().unwrap() + line;
            let below = pair[1][0];
            assert!(
                below - above >= g.gap as f32 - 0.5,
                "{label}: labels across the edge are {} apart",
                below - above
            );
        }
    }
    // a row alone keeps its end labels against its own edges
    let ticks = amp_ticks(RulerY::Norm, h as f64, 16, 0.0, 1.0, &m);
    let alone = label_tops(
        Rect::new(0.0, 0.0, 300.0, h),
        &ticks,
        line,
        m.gap,
        Shared::default(),
    );
    assert_eq!(
        alone.iter().flatten().count(),
        labels(&ticks).len(),
        "nothing is dropped where no row meets it"
    );
}

/// **A row too short for a line of text still draws its ticks, and drops
/// their labels** -- and above all does not panic.
///
/// It used to. The label's y was `clamp`ed into the band from `row.y` to
/// `row.y + row.h` less one line of text, whose upper bound falls *below*
/// the lower one as soon as the row is shorter than one caption, and
/// `f32::clamp` panics on an inverted range. The guard above it tested `row.h` against a literal
/// four, a number that predates the text and is not the height the
/// arithmetic needs, so every row between it and a caption's height was a
/// crash -- reached by dragging a window's corner in, which is how it was
/// found.
#[test]
fn a_row_shorter_than_its_own_caption_drops_the_label_and_does_not_panic() {
    let m = Metrics::default();
    let ticks = value_ticks(-1.0, 1.0, 200.0, &m);
    assert!(!labels(&ticks).is_empty(), "the case needs labelled ticks");
    let line = font::height(m.caption_scale);

    // The band that used to panic: taller than the old guard, shorter than
    // a line. Every height in it draws, and none of it draws a caption.
    let mut h = 4.5;
    while h < line {
        let mut mesh = Mesh::new();
        let row = Rect::new(10.0, 356.0, 300.0, h);
        draw_ticks_v(
            &mut Draw::new(&mut mesh, &m, &Theme::default()),
            row.x + 40.0,
            row.x,
            row,
            &ticks,
        );
        assert!(!mesh.is_empty(), "the tick marks are drawn at h = {h}");
        h += 0.5;
    }

    // One line of room is where the captions come back.
    let mut mesh = Mesh::new();
    let row = Rect::new(10.0, 356.0, 300.0, line + 1.0);
    let mut bare = Mesh::new();
    draw_ticks_v(
        &mut Draw::new(&mut bare, &m, &Theme::default()),
        row.x + 40.0,
        row.x,
        row,
        &ticks
            .iter()
            .map(|t| Tick {
                label: None,
                ..t.clone()
            })
            .collect::<Vec<_>>(),
    );
    draw_ticks_v(
        &mut Draw::new(&mut mesh, &m, &Theme::default()),
        row.x + 40.0,
        row.x,
        row,
        &ticks,
    );
    assert!(
        mesh.vertex_count() > bare.vertex_count(),
        "a row with room for a line draws its labels"
    );
}

/// **A vertical strip is the role's width whatever it writes, and its
/// labels shrink to fit it**: a zoom that writes `-0.0625` where `-1.00` was
/// written neither widens the strip nor runs a label past its edge, so the
/// picture beside it never moves.
#[test]
fn a_value_strip_keeps_its_width_and_its_labels_fit_it() {
    let m = Metrics::default();
    for (lo, hi) in [
        (-1.0, 1.0),
        (-0.1, 0.1),
        (-0.0001, 0.0001),
        (-12_000.0, 90_000.0),
    ] {
        let ruler = Ruler::new(Scale::Value { lo, hi }, RulerDir::Right);
        assert_eq!(ruler.thickness(&m), m.ruler_w, "[{lo}, {hi}]");
        let row = Rect::new(100.0, 20.0, 300.0, 240.0);
        let strip = Rect::new(row.x - m.ruler_w, row.y, m.ruler_w, row.h);
        let mut mesh = Mesh::new();
        ruler.draw(
            &mut Draw::new(&mut mesh, &m, &Theme::default()),
            strip,
            row,
            (0.0, 1.0),
            Shared::default(),
        );
        assert!(!mesh.is_empty(), "[{lo}, {hi}] is drawn");
        for (x, _) in mesh.positions() {
            assert!(
                x >= strip.x - 0.5 && x <= strip.x + strip.w + 0.5,
                "[{lo}, {hi}]: ink at {x} outside the strip {strip:?}"
            );
        }
    }
}

#[test]
fn scale_tags_are_three_letters() {
    // The corner read-out slot is sized once: every tag is exactly three
    // characters, so switching scales never moves the chrome.
    for scale in [
        FreqScale::Linear,
        FreqScale::Log,
        FreqScale::Mel,
        FreqScale::Bark,
    ] {
        assert_eq!(scale_tag(scale).len(), 3, "{scale:?}");
    }
}

/// Recomputes the drawn label intervals of a horizontal ruler the way the
/// frame renderer draws them (centered, edge-clamped) and asserts none
/// overlap -- the acceptance property.
fn assert_no_h_collisions(ticks: &[Tick], width_px: f64, ctx: &str) {
    let mut spans: Vec<(f64, f64)> = ticks
        .iter()
        .filter_map(|t| {
            let label = t.label.as_deref()?;
            let w = font::width(label, Metrics::default().caption_scale) as f64;
            let x = t.frac * width_px;
            let lx = (x - w * 0.5).clamp(0.0, (width_px - w).max(0.0));
            Some((lx, lx + w))
        })
        .collect();
    spans.sort_by(|a, b| a.0.total_cmp(&b.0));
    for w in spans.windows(2) {
        assert!(
            w[1].0 >= w[0].1,
            "{ctx}: labels overlap at {:?} vs {:?}",
            w[0],
            w[1]
        );
    }
}

/// Asserts no two labels of a vertical ruler sit closer than one line of
/// text (they are drawn at the tick's height).
fn assert_no_v_collisions(ticks: &[Tick], height_px: f64, ctx: &str) {
    let mut ys: Vec<f64> = ticks
        .iter()
        .filter(|t| t.label.is_some())
        .map(|t| t.frac * height_px)
        .collect();
    ys.sort_by(f64::total_cmp);
    let line = font::height(Metrics::default().caption_scale) as f64;
    for w in ys.windows(2) {
        assert!(w[1] - w[0] >= line - 1e-9, "{ctx}: {w:?}");
    }
}

#[test]
fn time_ticks_pick_the_smallest_fitting_step() {
    // 10 s visible over 800 px at 48 kHz: labels in milliseconds
    // ("0.000".."10.000"), at the finest step whose labels clear.
    let ticks = time_ticks(
        0.0,
        480_000.0,
        800.0,
        48_000.0,
        TimeUnit::Seconds,
        &Metrics::default(),
    );
    assert!(!ticks.is_empty());
    for t in &ticks {
        assert!((0.0..=1.0).contains(&t.frac));
    }
    let l = labels(&ticks);
    assert!(l.contains(&"0.000") && l.contains(&"10.000"), "{l:?}");
    assert_no_h_collisions(&ticks, 800.0, "10s/800px");
    // The same window on a narrow strip climbs the ladder.
    let narrow = time_ticks(
        0.0,
        480_000.0,
        160.0,
        48_000.0,
        TimeUnit::Seconds,
        &Metrics::default(),
    );
    assert!(labels(&narrow).len() < l.len());
    assert_no_h_collisions(&narrow, 160.0, "10s/160px");
}

#[test]
fn time_ticks_fall_back_to_sample_counts() {
    let ticks = time_ticks(
        0.0,
        100_000.0,
        500.0,
        0.0,
        TimeUnit::Seconds,
        &Metrics::default(),
    );
    // No rate: labels are sample counts (compacted), on the 1-2-5 ladder.
    let l = labels(&ticks);
    assert!(l.contains(&"0") && l.contains(&"20K"), "{l:?}");
    let explicit = time_ticks(
        0.0,
        100_000.0,
        500.0,
        48_000.0,
        TimeUnit::Samples,
        &Metrics::default(),
    );
    assert_eq!(
        labels(&explicit),
        labels(&ticks),
        "samples mode ignores the rate"
    );
}

#[test]
fn sample_steps_stay_integral() {
    // Zoomed to 30 samples over a wide strip: the step cannot go below 1,
    // so every label is a whole sample number.
    let ticks = time_ticks(
        100.0,
        30.0,
        1200.0,
        0.0,
        TimeUnit::Samples,
        &Metrics::default(),
    );
    let l = labels(&ticks);
    assert!(l.contains(&"100") && l.contains(&"130"), "{l:?}");
    for label in &l {
        assert!(label.parse::<i64>().is_ok(), "integral labels: {l:?}");
    }
}

#[test]
fn time_labels_use_clock_form_and_millis() {
    // three decimals, the field's default, and more only past a millisecond
    assert_eq!(fmt_time(0.0, 1.0), "0.000");
    assert_eq!(fmt_time(75.0, 5.0), "1:15.000");
    assert_eq!(fmt_time(3723.0, 60.0), "1:02:03.000");
    assert_eq!(fmt_time(1.25, 0.05), "1.250");
    assert_eq!(fmt_time(0.005, 0.005), "0.005");
    assert_eq!(fmt_time(61.5, 0.5), "1:01.500");
    assert_eq!(fmt_time(0.0005, 0.0001), "0.0005");
}

#[test]
fn zoomed_view_offsets_and_subdivides() {
    // A 1 s window starting at 2.5 s: sub-second majors from 2.5.
    let ticks = time_ticks(
        120_000.0,
        48_000.0,
        800.0,
        48_000.0,
        TimeUnit::Seconds,
        &Metrics::default(),
    );
    let l = labels(&ticks);
    assert!(l.contains(&"2.600") && l.contains(&"3.400"), "{l:?}");
    // Minors exist between majors (more ticks than labels).
    assert!(ticks.len() > l.len());
    assert_no_h_collisions(&ticks, 800.0, "zoomed 1s/800px");
}

/// A ritardando, drawn: the multitrack your hand writes as
/// `d=60 --> d=30` spreads its beat ticks as it slows, over an axis that
/// never stopped measuring samples. The marks move; the axis does not.
#[test]
fn a_slowing_tempo_spreads_its_beat_ticks() {
    // Eight beats at 1 beat/s, decelerating to half that over the last
    // four -- one bar at tempo, then a bar of ritardando.
    let mut map = TempoMap::new(1.0);
    map.ramp(4.0, 8.0, 1.0, 0.5).unwrap();
    let secs = map.secs_at(8.0);
    let unit = TimeUnit::Beats {
        tempo: 1.0,
        beat_at: 0.0,
        quant: 4.0,
        map: Some(&map),
        bars: &[],
    };
    let ticks = time_ticks(
        0.0,
        secs * 48_000.0,
        900.0,
        48_000.0,
        unit,
        &Metrics::default(),
    );
    let labelled: Vec<f64> = ticks
        .iter()
        .filter(|t| t.label.is_some())
        .map(|t| t.frac)
        .collect();
    assert!(labelled.len() >= 8, "a tick a beat: {labelled:?}");
    let gaps: Vec<f64> = labelled.windows(2).map(|w| w[1] - w[0]).collect();
    // The first bar is at one tempo, so its beats are evenly spaced...
    for g in &gaps[..3] {
        assert!((g - gaps[0]).abs() < 1e-9, "even before the ramp: {gaps:?}");
    }
    // ...and every beat of the ritardando is wider than the one before it.
    for w in gaps[3..].windows(2) {
        assert!(w[1] > w[0] + 1e-9, "spreading through the ramp: {gaps:?}");
    }
    // And each one is worth exactly the seconds the map gives that beat --
    // which is the whole claim, and the reason the last gap is not twice
    // the first: the ramp is linear in tempo, so the eighth beat is played
    // *through* the deceleration rather than after it.
    for (i, gap) in gaps.iter().enumerate() {
        let want = map.span_secs(i as f64, i as f64 + 1.0) / secs;
        assert!((gap - want).abs() < 1e-9, "beat {i}: {gap} vs {want}");
    }
}

/// The placement is the map's own answer, not a proportion of the window:
/// with the tempo doubled at beat 2, beat 8 falls at 5 seconds, and that is
/// the sample its tick sits on. This is the example's own arithmetic.
#[test]
fn a_beat_tick_sits_on_the_sample_the_map_puts_it_on() {
    let mut map = TempoMap::new(1.0);
    map.push(2.0, 2.0).unwrap();
    assert_eq!(map.secs_at(8.0), 5.0, "2 s of slow, then 6 beats at 2/s");
    let len = 10.0 * 48_000.0; // a ten-second window
    let unit = TimeUnit::Beats {
        tempo: 1.0,
        beat_at: 0.0,
        quant: 4.0,
        map: Some(&map),
        bars: &[],
    };
    let ticks = time_ticks(0.0, len, 900.0, 48_000.0, unit, &Metrics::default());
    // Bar 3 beat 1 is beat 8: half way across a ten-second window.
    let bar3 = ticks
        .iter()
        .find(|t| t.label.as_deref() == Some("3:1"))
        .expect("beat 8 is labelled");
    assert!(
        (bar3.frac - 0.5).abs() < 1e-9,
        "beat 8 at 5 s: {}",
        bar3.frac
    );
    // A frozen tempo would have drawn it at 8 s, four fifths across --
    // the defect this fixes, and three seconds of it.
    let frozen = TimeUnit::Beats {
        tempo: 1.0,
        beat_at: 0.0,
        quant: 4.0,
        map: None,
        bars: &[],
    };
    let was = time_ticks(0.0, len, 900.0, 48_000.0, frozen, &Metrics::default());
    let then = was
        .iter()
        .find(|t| t.label.as_deref() == Some("3:1"))
        .expect("beat 8 is labelled either way");
    assert!((then.frac - 0.8).abs() < 1e-9, "{}", then.frac);
}

/// Every tick goes through the map, minors included. Inside a ramp the
/// tempo moves *within* a beat, so the half-beat mark is not the midpoint
/// of the two beats around it -- placing the majors and halving between
/// them would put it where nothing sounds.
#[test]
fn a_subdivision_inside_a_ramp_is_placed_by_the_map_too() {
    let mut map = TempoMap::new(1.0);
    map.ramp(0.0, 4.0, 1.0, 4.0).unwrap();
    let unit = TimeUnit::Beats {
        tempo: 1.0,
        beat_at: 0.0,
        quant: 4.0,
        map: Some(&map),
        bars: &[],
    };
    let len = map.secs_at(4.0) * 48_000.0;
    let ticks = time_ticks(0.0, len, 1400.0, 48_000.0, unit, &Metrics::default());
    assert!(ticks.len() > ticks.iter().filter(|t| t.label.is_some()).count());
    // The tick between beat 0 and beat 1 is where beat 0.5 sounds, which
    // in an accelerando is later than half way between them.
    let half = map.secs_at(0.5) / map.secs_at(4.0);
    let midpoint = map.secs_at(1.0) / map.secs_at(4.0) / 2.0;
    assert!(half > midpoint + 1e-6, "the ramp bends inside the beat");
    assert!(
        ticks.iter().any(|t| (t.frac - half).abs() < 1e-9),
        "the half-beat mark is the map's"
    );
}

#[test]
fn beat_ticks_label_bars_and_beats_on_the_quant_grid() {
    // 8 beats visible over 800 px at 48 kHz, 2 beats/s (tempo 2.0):
    // one-beat labels fit. quant 4 -> bar:beat.
    let unit = TimeUnit::Beats {
        tempo: 2.0,
        beat_at: 0.0,
        quant: 4.0,
        map: None,
        bars: &[],
    };
    let ticks = time_ticks(0.0, 192_000.0, 800.0, 48_000.0, unit, &Metrics::default());
    let l = labels(&ticks);
    assert_eq!(l[0], "1:1");
    assert!(l.contains(&"1:3") && l.contains(&"2:1"), "{l:?}");
    // Minors (the binary subdivision) exist between majors.
    assert!(ticks.len() > l.len());
    for t in &ticks {
        assert!((0.0..=1.0).contains(&t.frac));
    }
    assert_no_h_collisions(&ticks, 800.0, "8 beats/800px");
}

/// **The page's bars count as the page does**: a pickup is bar 0, a
/// meter that changes changes the bar's length, and a strip too narrow
/// for beats labels bars.
#[test]
fn the_pages_bars_number_the_ruler_as_the_page_does() {
    // a quarter's pickup, 4/4 from bar 1, 3/4 from bar 3 (beat 9)
    let bars = [[0.0, 1.0, 0.0], [1.0, 4.0, 1.0], [9.0, 3.0, 3.0]];
    assert_eq!(
        bars_in(&bars, 0.0, 13.0),
        vec![
            (0.0, 1.0, 0),
            (1.0, 4.0, 1),
            (5.0, 4.0, 2),
            (9.0, 3.0, 3),
            (12.0, 3.0, 4)
        ]
    );
    // 16 beats a second at 48 kHz over 1600 px: beats fit
    let unit = TimeUnit::Beats {
        tempo: 1.0,
        beat_at: 0.0,
        quant: 4.0,
        map: None,
        bars: &bars,
    };
    let ticks = time_ticks(
        0.0,
        16.0 * 48_000.0,
        1600.0,
        48_000.0,
        unit,
        &Metrics::default(),
    );
    let l = labels(&ticks);
    assert_eq!(l[0], "0:1");
    assert!(
        l.contains(&"1:4") && l.contains(&"3:3") && !l.contains(&"3:4"),
        "{l:?}"
    );
    assert_no_h_collisions(&ticks, 1600.0, "page bars/1600px");
    // narrow: bars only, by their numbers
    let narrow = time_ticks(
        0.0,
        16.0 * 48_000.0,
        90.0,
        48_000.0,
        unit,
        &Metrics::default(),
    );
    let l = labels(&narrow);
    assert!(
        !l.is_empty() && l.iter().all(|label| !label.contains(':')),
        "{l:?}"
    );
    assert_no_h_collisions(&narrow, 90.0, "page bars/90px");
}

#[test]
fn beat_ladder_narrow_strip_climbs_to_bars() {
    // The same 8 beats on a strip too narrow for per-beat labels: the
    // ladder climbs to a bar-aligned step, so bar lines stay majors.
    let unit = TimeUnit::Beats {
        tempo: 2.0,
        beat_at: 0.0,
        quant: 4.0,
        map: None,
        bars: &[],
    };
    let ticks = time_ticks(0.0, 192_000.0, 120.0, 48_000.0, unit, &Metrics::default());
    let l = labels(&ticks);
    assert!(!l.is_empty());
    for label in &l {
        assert!(label.ends_with(":1"), "bar-aligned majors, got {l:?}");
    }
    assert_no_h_collisions(&ticks, 120.0, "8 beats/120px");
}

#[test]
fn beat_steps_climb_the_musical_ladder() {
    // Binary fractions below a beat, then beats, then bar multiples;
    // every rung divides the next (a 3-beat bar skips the 2-beat rung).
    let steps = beat_steps(16.0, 800.0, 4.0, Gaps::of(&Metrics::default()));
    for w in steps.windows(2) {
        let ratio = w[1] / w[0];
        assert!(
            (ratio - ratio.round()).abs() < 1e-9,
            "every rung divides the next: {steps:?}"
        );
    }
    assert!(steps.contains(&1.0) && steps.contains(&2.0) && steps.contains(&4.0));
    let three = beat_steps(16.0, 800.0, 3.0, Gaps::of(&Metrics::default()));
    assert!(!three.contains(&2.0), "{three:?}");
    assert!(three.contains(&3.0), "{three:?}");
}

#[test]
fn beats_without_rate_or_tempo_fall_back_to_samples() {
    let unit = TimeUnit::Beats {
        tempo: 0.0,
        beat_at: 0.0,
        quant: 4.0,
        map: None,
        bars: &[],
    };
    let ticks = time_ticks(0.0, 100_000.0, 500.0, 48_000.0, unit, &Metrics::default());
    assert_eq!(labels(&ticks)[0], "0");
    assert!(labels(&ticks).contains(&"20K"));
}

#[test]
fn beat_offset_shifts_the_grid() {
    // beat_at 2 with quant 4: sample 0 is beat 2 (0-based) = bar 1 beat 3.
    let unit = TimeUnit::Beats {
        tempo: 2.0,
        beat_at: 2.0,
        quant: 4.0,
        map: None,
        bars: &[],
    };
    let ticks = time_ticks(0.0, 192_000.0, 800.0, 48_000.0, unit, &Metrics::default());
    let l = labels(&ticks);
    assert_eq!(l[0], "1:3", "{l:?}");
    assert!(l.contains(&"2:1"), "{l:?}");
}

#[test]
fn readout_beats_reads_bar_beat() {
    // 48k samples at 2 beats/s = 2 beats in; quant 4 -> bar 1, beat 3.
    assert_eq!(
        readout_beats(48_000.0, 48_000.0, 2.0, 0.0, 4.0, None, 0.005),
        "1:3.00"
    );
    assert_eq!(
        readout_beats(48_000.0, 48_000.0, 2.0, 0.0, 0.0, None, 0.005),
        "2.00"
    );
    assert_eq!(
        readout_beats(48_000.0, 0.0, 2.0, 0.0, 4.0, None, 0.005),
        "48000"
    );
    // Deep zoom (a pixel spans well under a hundredth of a beat): the
    // decimals refine with the view, up to the ruler's four-decimal cap.
    assert_eq!(
        readout_beats(48_000.0, 48_000.0, 2.0, 0.0, 4.0, None, 5e-4),
        "1:3.000"
    );
    assert_eq!(
        readout_beats(48_000.0, 48_000.0, 2.0, 0.0, 4.0, None, 5e-7),
        "1:3.0000"
    );
    // No pixel resolution known: the two-decimal floor.
    assert_eq!(
        readout_beats(48_000.0, 48_000.0, 2.0, 0.0, 4.0, None, 0.0),
        "1:3.00"
    );
}

#[test]
fn readout_time_refines_with_the_pixel_resolution() {
    // Normal zoom (a pixel spans >= 1 ms): millisecond precision.
    assert_eq!(readout_time(24_000.0, 48_000.0, 0.01), "0.500");
    // Deep zoom (one 440 Hz cycle over ~600 px): finer than the ruler's
    // 4-decimal labels, never coarser.
    let secs_per_px = (1.0 / 440.0) / 600.0;
    assert_eq!(readout_time(48.0, 48_000.0, secs_per_px), "0.001000");
    // No pixel resolution known: the millisecond floor.
    assert_eq!(readout_time(24_000.0, 48_000.0, 0.0), "0.500");
    // No rate known: the sample count.
    assert_eq!(readout_time(24_000.0, 0.0, 0.01), "24000");
}

#[test]
fn amp_ticks_norm_and_percent_share_the_geometry() {
    let norm = amp_ticks(RulerY::Norm, 400.0, 16, 0.0, 1.0, &Metrics::default());
    let pct = amp_ticks(RulerY::Percent, 400.0, 16, 0.0, 1.0, &Metrics::default());
    assert_eq!(norm.len(), pct.len());
    for (n, p) in norm.iter().zip(&pct) {
        assert!((n.frac - p.frac).abs() < 1e-12, "same positions");
    }
    // The extremes respect the waveform's vertical margin.
    let top = norm.iter().map(|t| t.frac).fold(0.0, f64::max);
    assert!((top - (1.0 + AMP_MARGIN as f64) / 2.0).abs() < 1e-9);
    let l = labels(&norm);
    // amplitude in two decimals
    assert!(
        l.contains(&"1.00") && l.contains(&"0.00") && l.contains(&"-1.00"),
        "{l:?}"
    );
    // Percent is signed: 0..100% above the zero line, 0..-100% below it.
    let p = labels(&pct);
    assert!(p.contains(&"100%") && p.contains(&"-100%"), "{p:?}");
    assert_no_v_collisions(&norm, 400.0, "norm/400px");
}

#[test]
fn amp_ticks_bits_step_in_sample_values_and_keep_the_endpoints() {
    let bits = amp_ticks(RulerY::Bits, 400.0, 16, 0.0, 1.0, &Metrics::default());
    let l = labels(&bits);
    // 1-2-5 steps in integer sample values, compacted...
    assert!(l.contains(&"0") && l.contains(&"10K"), "{l:?}");
    // ...plus the full-scale endpoints the decimal step misses.
    assert!(l.contains(&"-32768") && l.contains(&"32768"), "{l:?}");
    // A different bit depth rescales the axis.
    let eight = amp_ticks(RulerY::Bits, 400.0, 8, 0.0, 1.0, &Metrics::default());
    assert!(labels(&eight).contains(&"-128"), "{:?}", labels(&eight));
}

#[test]
fn amp_ticks_db_ladder_is_mirrored_and_placed_by_amplitude() {
    let ticks = amp_ticks(RulerY::Db, 600.0, 16, 0.0, 1.0, &Metrics::default());
    // The center line is the -inf mark.
    assert_eq!(ticks[0].frac, 0.5);
    assert_eq!(ticks[0].label.as_deref(), Some("-INF"));
    // 0 dB sits at full scale (frac = (1 + margin)/2), mirrored below.
    let full = (1.0 + AMP_MARGIN as f64) / 2.0;
    let zero: Vec<&Tick> = ticks
        .iter()
        .filter(|t| t.label.as_deref() == Some("0"))
        .collect();
    assert_eq!(zero.len(), 2);
    assert!((zero[0].frac - full).abs() < 1e-9);
    assert!((zero[1].frac - (1.0 - full)).abs() < 1e-9);
    // -6 dB sits at amplitude 10^(-6/20) of the margin, not halfway.
    let m6 = ticks
        .iter()
        .find(|t| t.label.as_deref() == Some("-6"))
        .unwrap();
    let expected = (10f64.powf(-6.0 / 20.0) * AMP_MARGIN as f64 + 1.0) / 2.0;
    assert!((m6.frac - expected).abs() < 1e-9);
    assert_no_v_collisions(&ticks, 600.0, "db/600px");
}

#[test]
fn amp_zoom_window_reveals_finer_steps_and_names_whats_on_screen() {
    // Zoom into the top of the norm axis: the visible amplitudes span
    // roughly [0.6, 1.09]; the step refines below the full-view 0.1 and
    // every tick names an amplitude inside the window.
    let full_view = amp_ticks(RulerY::Norm, 400.0, 16, 0.0, 1.0, &Metrics::default());
    let zoomed = amp_ticks(RulerY::Norm, 400.0, 16, 0.8, 0.2, &Metrics::default());
    let parse = |t: &Tick| t.label.as_deref().unwrap().parse::<f64>().unwrap();
    let step_full = parse(&full_view[1]) - parse(&full_view[0]);
    let z: Vec<f64> = zoomed.iter().map(parse).collect();
    assert!(!z.is_empty());
    let step_zoom = z[1] - z[0];
    assert!(step_zoom < step_full, "{step_zoom} vs {step_full}");
    let margin = AMP_MARGIN as f64;
    for (tick, v) in zoomed.iter().zip(&z) {
        // Each tick sits exactly where its amplitude maps in the window.
        let expected = ((v * margin + 1.0) / 2.0 - 0.8) / 0.2;
        assert!((tick.frac - expected).abs() < 1e-9, "{v} at {}", tick.frac);
    }
    assert_no_v_collisions(&zoomed, 400.0, "norm zoomed");
    // The dB axis zoomed into the top reveals the fine (-1, -2) rungs the
    // full view drops on a short row.
    let db_short = amp_ticks(RulerY::Db, 150.0, 16, 0.0, 1.0, &Metrics::default());
    let db_zoom = amp_ticks(RulerY::Db, 150.0, 16, 0.85, 0.15, &Metrics::default());
    assert!(
        !labels(&db_short).contains(&"-1"),
        "{:?}",
        labels(&db_short)
    );
    assert!(labels(&db_zoom).contains(&"-1"), "{:?}", labels(&db_zoom));
    assert_no_v_collisions(&db_zoom, 150.0, "db zoomed");
}

/// **A note axis names the notes**: every C where an octave has room
/// for a label, the naturals as it is zoomed in, never two labels over
/// each other; a tuning moves every note by its ratio, and the readout
/// names the nearest note and how far off it is.
#[test]
fn a_note_axis_names_the_notes_at_its_tuning() {
    let nyq = 24_000.0;
    for a4 in [440.0, 415.0] {
        let floor = axis_floor(FreqScale::Note, a4, nyq);
        assert!((tuning_of(nyq, floor) - a4).abs() < 1e-6);
        // **MIDI 0 at the bottom, 127 at the top, an even step apiece**
        for (m, d) in [(0.0, 0.0), (69.0, 69.0 / 127.0), (127.0, 1.0)] {
            let f = clausters_core::scale::midi_to_hz_tuned(m, a4);
            assert!((hz_to_display(f, nyq, FreqScale::Note, floor) - d).abs() < 1e-9);
            assert!((display_to_hz(d, nyq, FreqScale::Note, floor) - f).abs() < 1e-6);
        }
        let whole = hz_ticks(
            nyq,
            FreqScale::Note,
            floor,
            600.0,
            0.0,
            1.0,
            &Metrics::default(),
        );
        let names: Vec<&str> = whole.iter().filter_map(|t| t.label.as_deref()).collect();
        assert!(names.contains(&"C4"), "{names:?}");
        // every octave's C, and what else has room
        for c in ["C1", "C2", "C3", "C5", "C6", "C7", "C8"] {
            assert!(names.contains(&c), "{c} in {names:?}");
        }
        assert_no_v_collisions(&whole, 600.0, "notes");
        // A4 sits where the tuning puts it
        let a = hz_to_display(a4, nyq, FreqScale::Note, floor);
        let near = |f: f64| (display_to_hz(a, nyq, FreqScale::Note, floor) - f).abs() < 1e-6;
        assert!(near(a4));
        // zoomed in to an octave around A4, the naturals are named
        let c4 = hz_to_display(
            clausters_core::scale::midi_to_hz_tuned(60.0, a4),
            nyq,
            FreqScale::Note,
            floor,
        );
        let c5 = hz_to_display(
            clausters_core::scale::midi_to_hz_tuned(72.0, a4),
            nyq,
            FreqScale::Note,
            floor,
        );
        let octave = hz_ticks(
            nyq,
            FreqScale::Note,
            floor,
            600.0,
            c4,
            c5 - c4,
            &Metrics::default(),
        );
        let names: Vec<&str> = octave.iter().filter_map(|t| t.label.as_deref()).collect();
        for n in ["D4", "E4", "F4", "G4", "A4", "B4"] {
            assert!(names.contains(&n), "{n} in {names:?}");
        }
        assert_no_v_collisions(&octave, 600.0, "an octave of notes");
    }
    assert_eq!(note_readout(440.0, 440.0), "A4");
    assert_eq!(note_readout(445.0, 440.0), "A4 +20");
    assert_eq!(note_readout(430.0, 440.0), "A4 -40");
    assert_eq!(note_readout(261.63, 440.0), "C4");
    assert_eq!(note_readout(415.0, 415.0), "A4");
    assert_eq!(scale_tag(FreqScale::Note), "TET");
}

#[test]
fn amp_readout_formats_per_unit() {
    assert_eq!(readout_amp(0.5, RulerY::Norm, 16, 0.01), "+0.50");
    assert_eq!(readout_amp(0.5, RulerY::Db, 16, 0.01), "-6.0 DB");
    assert_eq!(readout_amp(0.0, RulerY::Db, 16, 0.01), "-INF DB");
    assert_eq!(readout_amp(-0.5, RulerY::Bits, 16, 0.01), "-16384");
    assert_eq!(readout_amp(0.5, RulerY::Percent, 16, 0.01), "50%");
    assert_eq!(readout_amp(-0.5, RulerY::Percent, 16, 0.01), "-50%");
}

#[test]
fn amp_readout_refines_with_the_pixel_resolution() {
    // Deep vertical zoom: the linear units refine to what a pixel
    // resolves, so the readout never shows fewer decimals than the
    // zoomed ruler labels.
    assert_eq!(readout_amp(0.5, RulerY::Norm, 16, 1e-4), "+0.5000");
    assert_eq!(readout_amp(0.5, RulerY::Percent, 16, 1e-4), "50.00%");
    // No pixel resolution known: the units' base precision.
    assert_eq!(readout_amp(0.5, RulerY::Norm, 16, 0.0), "+0.50");
    assert_eq!(readout_amp(0.5, RulerY::Percent, 16, 0.0), "50%");
    // dB keeps its tenth regardless (its ruler labels whole rungs).
    assert_eq!(readout_amp(0.5, RulerY::Db, 16, 1e-4), "-6.0 DB");
}

#[test]
fn log_hz_ticks_match_the_shader_geometry() {
    // 48 kHz => Nyquist 24 kHz, f_lo = 20/24000.
    let nyq = 24_000.0;
    let f_lo = 20.0 / nyq;
    let ticks = hz_ticks(
        nyq,
        FreqScale::Log,
        f_lo,
        600.0,
        0.0,
        1.0,
        &Metrics::default(),
    );
    let l = labels(&ticks);
    for expected in ["100", "1K", "10K", "2K", "50"] {
        assert!(l.contains(&expected), "missing {expected} in {l:?}");
    }
    // The 1 kHz tick sits where the shader puts 1 kHz: d solves
    // f_lo^(1 - d) = f/nyq.
    let one_k = ticks
        .iter()
        .find(|t| t.label.as_deref() == Some("1K"))
        .unwrap();
    let expected_d = 1.0 - (1000.0f64 / nyq).ln() / f_lo.ln();
    assert!((one_k.frac - expected_d).abs() < 1e-9);
    // Everything stays inside the axis.
    for t in &ticks {
        assert!((0.0..=1.0).contains(&t.frac));
    }
    assert_no_v_collisions(&ticks, 600.0, "log/600px");
}

#[test]
fn linear_hz_ticks_are_evenly_spaced() {
    let ticks = hz_ticks(
        24_000.0,
        FreqScale::Linear,
        20.0 / 24_000.0,
        600.0,
        0.0,
        1.0,
        &Metrics::default(),
    );
    let l = labels(&ticks);
    assert_eq!(l.first(), Some(&"0"));
    let majors: Vec<&Tick> = ticks.iter().filter(|t| t.label.is_some()).collect();
    let d01 = majors[1].frac - majors[0].frac;
    let d12 = majors[2].frac - majors[1].frac;
    assert!((d01 - d12).abs() < 1e-9, "even spacing on the linear axis");
    assert_no_v_collisions(&ticks, 600.0, "linear/600px");
}

#[test]
fn mel_and_bark_ticks_sit_on_the_perceptual_mapping() {
    let nyq = 24_000.0;
    for scale in [FreqScale::Mel, FreqScale::Bark] {
        let ticks = hz_ticks(nyq, scale, 20.0 / nyq, 600.0, 0.0, 1.0, &Metrics::default());
        let l = labels(&ticks);
        assert_eq!(l.first(), Some(&"0"));
        assert!(l.contains(&"1K"), "{scale:?}: {l:?}");
        // The 1 kHz tick inverts back to 1 kHz through the shared
        // display mapping (the shader's geometry).
        let one_k = ticks
            .iter()
            .find(|t| t.label.as_deref() == Some("1K"))
            .unwrap();
        let f = display_to_hz(one_k.frac, nyq, scale, 20.0 / nyq);
        assert!((f - 1000.0).abs() < 1e-6, "{scale:?}: {f}");
        for t in &ticks {
            assert!((0.0..=1.0).contains(&t.frac));
        }
        assert_no_v_collisions(&ticks, 600.0, &format!("{scale:?}/600px"));
    }
}

#[test]
fn hz_zoom_window_reveals_sub_decade_ticks() {
    // Zoom the log axis into the octave around 1-2 kHz: the window is
    // narrower than a decade, so the ladder switches to round hertz steps
    // finer than the decade scheme's 1/2/5.
    let nyq = 24_000.0;
    let f_lo = 20.0 / nyq;
    let d1 = hz_to_display(1_000.0, nyq, FreqScale::Log, f_lo);
    let d2 = hz_to_display(2_000.0, nyq, FreqScale::Log, f_lo);
    let ticks = hz_ticks(
        nyq,
        FreqScale::Log,
        f_lo,
        600.0,
        d1,
        d2 - d1,
        &Metrics::default(),
    );
    let l = labels(&ticks);
    assert!(l.contains(&"1.1K") || l.contains(&"1.2K"), "{l:?}");
    // Every label names a frequency inside the window, exactly on the
    // shader mapping.
    for t in ticks.iter().filter(|t| t.label.is_some()) {
        let f = display_to_hz(d1 + t.frac * (d2 - d1), nyq, FreqScale::Log, f_lo);
        assert!((999.0..=2001.0).contains(&f), "{f}");
    }
    assert_no_v_collisions(&ticks, 600.0, "log zoomed");
}

#[test]
fn mel_display_mapping_round_trips() {
    // display_to_hz and hz_to_display invert each other on every scale.
    let nyq = 22_050.0;
    for scale in [
        FreqScale::Linear,
        FreqScale::Log,
        FreqScale::Mel,
        FreqScale::Bark,
    ] {
        for d in [0.1, 0.5, 0.9] {
            let f = display_to_hz(d, nyq, scale, 20.0 / nyq);
            let back = hz_to_display(f, nyq, scale, 20.0 / nyq);
            assert!((back - d).abs() < 1e-9, "{scale:?} at {d}");
        }
    }
}

#[test]
fn no_labels_collide_at_any_window_zoom_unit_or_strip_size() {
    // The acceptance property, swept: every unit, at several strip sizes,
    // windows and zooms (including degenerate slivers), never lays out
    // two overlapping labels.
    let widths = [40.0, 90.0, 160.0, 320.0, 800.0, 2000.0];
    let windows = [
        (0.0, 480_000.0),
        (120_000.0, 48_000.0),
        (999.0, 77.0),
        (1_234_567.0, 3_456_789.0),
        (47_999.0, 2.0),
    ];
    for &w in &widths {
        for &(start, len) in &windows {
            for unit in [
                TimeUnit::Seconds,
                TimeUnit::Samples,
                TimeUnit::Beats {
                    map: None,
                    bars: &[],
                    tempo: 2.5,
                    beat_at: 1.0,
                    quant: 3.0,
                },
            ] {
                let ticks = time_ticks(start, len, w, 48_000.0, unit, &Metrics::default());
                assert_no_h_collisions(&ticks, w, &format!("{unit:?} {start}+{len} @{w}px"));
            }
        }
    }
    let heights = [30.0, 80.0, 150.0, 400.0, 1200.0];
    let y_windows = [(0.0, 1.0), (0.5, 0.5), (0.8, 0.2), (0.45, 0.02), (0.0, 0.3)];
    for &h in &heights {
        for &(y0, ylen) in &y_windows {
            for unit in [RulerY::Norm, RulerY::Db, RulerY::Bits, RulerY::Percent] {
                let ticks = amp_ticks(unit, h, 16, y0, ylen, &Metrics::default());
                assert_no_v_collisions(&ticks, h, &format!("{unit:?} y{y0}+{ylen} @{h}px"));
            }
            for scale in [
                FreqScale::Linear,
                FreqScale::Log,
                FreqScale::Mel,
                FreqScale::Bark,
            ] {
                let ticks = hz_ticks(
                    24_000.0,
                    scale,
                    20.0 / 24_000.0,
                    h,
                    y0,
                    ylen,
                    &Metrics::default(),
                );
                assert_no_v_collisions(&ticks, h, &format!("{scale:?} y{y0}+{ylen} @{h}px"));
                for t in &ticks {
                    assert!((0.0..=1.0).contains(&t.frac));
                }
            }
        }
    }
}

#[test]
fn value_ticks_fit_any_range() {
    // A non-normalized sequence range (say pwhite over [40, 4700]): round
    // 1-2-5 steps, labels inside the range, no collisions.
    let ticks = value_ticks(40.0, 4700.0, 400.0, &Metrics::default());
    let l = labels(&ticks);
    assert!(!l.is_empty());
    assert!(l.contains(&"1000") || l.contains(&"500"), "{l:?}");
    for t in &ticks {
        assert!((0.0..=1.0).contains(&t.frac));
    }
    assert_no_v_collisions(&ticks, 400.0, "value 40..4700");
    // A tiny fractional range refines below a hundredth, in scientific
    // notation on the whole ladder.
    let fine = value_ticks(-0.02, 0.03, 400.0, &Metrics::default());
    let fl = labels(&fine);
    assert!(
        fl.contains(&"0") && fl.iter().filter(|s| **s != "0").all(|s| s.contains('e')),
        "{fl:?}"
    );
    // Two decimals over a fraction of a unit, whole numbers over whole steps.
    let unit = labels(&value_ticks(0.0, 1.0, 400.0, &Metrics::default())).join(" ");
    assert!(unit.contains("0.50"), "{unit}");
    assert!(l.iter().all(|s| !s.contains('.')), "{l:?}");
    assert_no_v_collisions(&fine, 400.0, "value -0.02..0.03");
    // Degenerate span: nothing.
    assert!(value_ticks(1.0, 1.0, 400.0, &Metrics::default()).is_empty());
    assert!(value_ticks(0.0, 1.0, 0.0, &Metrics::default()).is_empty());
}

#[test]
fn readout_value_resolves_a_thousandth_of_the_span() {
    assert_eq!(readout_value(0.5, 2.0), "0.5");
    assert_eq!(readout_value(1234.0, 5000.0), "1234");
    assert_eq!(readout_value(0.1234567, 0.001), "0.123457");
    assert_eq!(readout_value(-0.25, 2.0), "-0.25");
}

#[test]
fn horizontal_hz_ticks_fit_by_label_width() {
    let nyq = 24_000.0;
    let f_lo = 20.0 / nyq;
    for scale in [
        FreqScale::Linear,
        FreqScale::Log,
        FreqScale::Mel,
        FreqScale::Bark,
    ] {
        let ticks = hz_ticks_h(nyq, scale, f_lo, 800.0, 0.0, 1.0, &Metrics::default());
        let l = labels(&ticks);
        assert!(l.contains(&"1K") || l.contains(&"2K"), "{scale:?}: {l:?}");
        // Every label sits exactly on the shared display mapping.
        for t in ticks.iter().filter(|t| t.label.is_some()) {
            let f = display_to_hz(t.frac, nyq, scale, f_lo);
            let named = t.label.as_deref().unwrap();
            let parsed = if let Some(k) = named.strip_suffix('K') {
                k.parse::<f64>().unwrap() * 1000.0
            } else {
                named.parse::<f64>().unwrap()
            };
            assert!(
                (f - parsed).abs() <= parsed.max(1.0) * 1e-3 + 0.5,
                "{scale:?}: tick {named} at {f} Hz"
            );
        }
        assert_no_h_collisions(&ticks, 800.0, &format!("{scale:?} horizontal"));
        // A narrow strip keeps fewer labels, still collision-free.
        let narrow = hz_ticks_h(nyq, scale, f_lo, 120.0, 0.0, 1.0, &Metrics::default());
        assert!(labels(&narrow).len() <= l.len());
        assert_no_h_collisions(&narrow, 120.0, &format!("{scale:?} narrow"));
    }
    assert!(
        hz_ticks_h(
            0.0,
            FreqScale::Log,
            0.001,
            800.0,
            0.0,
            1.0,
            &Metrics::default()
        )
        .is_empty()
    );
}

/// Zooming a frequency x axis reveals **finer** round frequencies inside
/// the window and drops everything outside it -- the property already fixed for
/// every other axis, now that this one navigates too. The vertical twin
/// has had it since the spectrogram's frequency window; this is the same
/// rule read across the strip instead of up it.
#[test]
fn a_zoomed_horizontal_hz_ruler_reveals_finer_rungs() {
    let (nyq, f_lo, m) = (24_000.0, 20.0 / 24_000.0, Metrics::default());
    for scale in [
        FreqScale::Linear,
        FreqScale::Log,
        FreqScale::Mel,
        FreqScale::Bark,
    ] {
        let full = hz_ticks_h(nyq, scale, f_lo, 800.0, 0.0, 1.0, &m);
        // A tenth of the axis around the middle: whatever hertz that is
        // per scale, the ruler must name frequencies inside it and only
        // inside it, without colliding.
        let (x0, x_len) = (0.45, 0.1);
        let zoomed = hz_ticks_h(nyq, scale, f_lo, 800.0, x0, x_len, &m);
        assert!(!zoomed.is_empty(), "{scale:?}: a zoomed window has ticks");
        assert_no_h_collisions(&zoomed, 800.0, &format!("{scale:?} zoomed"));
        let lo = display_to_hz(x0, nyq, scale, f_lo);
        let hi = display_to_hz(x0 + x_len, nyq, scale, f_lo);
        for t in &zoomed {
            let f = display_to_hz(x0 + t.frac * x_len, nyq, scale, f_lo);
            assert!(
                f >= lo - (hi - lo) * 1e-6 && f <= hi + (hi - lo) * 1e-6,
                "{scale:?}: {f} Hz drawn outside [{lo}, {hi}]"
            );
        }
        // Finer, measured where the comparison is fair -- inside the
        // window, which is the only place both rulers describe. (A
        // nonlinear axis packs its bottom decade tightly, so the smallest
        // gap over the *whole* ruler says nothing about the zoom.)
        let named = |ticks: &[Tick], start: f64, len: f64| -> Vec<f64> {
            ticks
                .iter()
                .filter(|t| t.label.is_some())
                .map(|t| display_to_hz(start + t.frac * len, nyq, scale, f_lo))
                .filter(|f| *f >= lo && *f <= hi)
                .collect()
        };
        let before = named(&full, 0.0, 1.0);
        let after = named(&zoomed, x0, x_len);
        assert!(
            after.len() > before.len(),
            "{scale:?}: the window names {} frequencies zoomed in, {} zoomed out",
            after.len(),
            before.len()
        );
    }
    // An empty window draws nothing rather than dividing by zero.
    assert!(hz_ticks_h(24_000.0, FreqScale::Log, 1e-3, 800.0, 0.0, 0.0, &m).is_empty());
}

#[test]
fn degenerate_inputs_yield_no_ticks() {
    assert!(
        time_ticks(
            0.0,
            0.0,
            800.0,
            48_000.0,
            TimeUnit::Seconds,
            &Metrics::default()
        )
        .is_empty()
    );
    assert!(
        time_ticks(
            0.0,
            100.0,
            0.0,
            48_000.0,
            TimeUnit::Seconds,
            &Metrics::default()
        )
        .is_empty()
    );
    assert!(
        hz_ticks(
            0.0,
            FreqScale::Log,
            0.001,
            600.0,
            0.0,
            1.0,
            &Metrics::default()
        )
        .is_empty()
    );
    assert!(
        hz_ticks(
            24_000.0,
            FreqScale::Linear,
            0.001,
            0.0,
            0.0,
            1.0,
            &Metrics::default()
        )
        .is_empty()
    );
    assert!(
        hz_ticks(
            24_000.0,
            FreqScale::Log,
            0.001,
            600.0,
            0.5,
            0.0,
            &Metrics::default()
        )
        .is_empty()
    );
    assert!(amp_ticks(RulerY::Norm, 0.0, 16, 0.0, 1.0, &Metrics::default()).is_empty());
    assert!(amp_ticks(RulerY::Norm, 400.0, 16, 0.5, 0.0, &Metrics::default()).is_empty());
    assert!(amp_ticks(RulerY::Off, 400.0, 16, 0.0, 1.0, &Metrics::default()).is_empty());
}

/// **Every scale answers for either axis**: side by side its labels keep
/// their measured widths apart, one over the other a line of text apart --
/// the one thing the axis changes about a scale.
#[test]
fn every_scale_rules_either_axis_with_its_labels_clear() {
    let m = Metrics::default();
    let scales = [
        (
            Scale::Time {
                rate: 48_000.0,
                unit: TimeUnit::Seconds,
                empty: false,
            },
            (0.0, 480_000.0),
        ),
        (
            Scale::Amplitude {
                unit: RulerY::Norm,
                bit_depth: 16,
            },
            (0.0, 1.0),
        ),
        (
            Scale::Amplitude {
                unit: RulerY::Db,
                bit_depth: 16,
            },
            (0.0, 1.0),
        ),
        (
            Scale::Value {
                lo: -3.0,
                hi: 120.0,
            },
            (0.0, 1.0),
        ),
        (
            Scale::Hertz {
                nyquist: 24_000.0,
                scale: FreqScale::Log,
                f_lo: 20.0 / 24_000.0,
            },
            (0.0, 1.0),
        ),
        (Scale::Decibels { floor: -60.0 }, (0.0, 1.0)),
    ];
    let scale = m.caption_scale;
    let line = font::height(scale);
    for (s, window) in scales {
        for along in [Along::Horizontal, Along::Vertical] {
            let px = 600.0;
            let ticks = s.ticks(along, window, px, &m);
            let mut spans: Vec<(f32, f32)> = ticks
                .iter()
                .filter_map(|t| {
                    let label = t.label.as_deref()?;
                    let at = t.frac as f32 * px as f32;
                    let half = match along {
                        Along::Horizontal => font::width(label, scale) * 0.5,
                        Along::Vertical => line * 0.5,
                    };
                    Some((at - half, at + half))
                })
                .collect();
            assert!(spans.len() >= 2, "{s:?} along {along:?} labels its axis");
            spans.sort_by(|a, b| a.0.total_cmp(&b.0));
            for pair in spans.windows(2) {
                assert!(
                    pair[1].0 >= pair[0].1 - 0.5,
                    "{s:?} along {along:?}: labels at {pair:?} overlap"
                );
            }
        }
    }
}

/// **The strip on a view's right is the mirror of the one on its left**: the
/// same ticks, their marks against the edge that faces the view, at the same
/// heights -- so which side a ruler stands on is the layout's choice.
#[test]
fn a_value_strip_on_the_right_is_the_mirror_of_one_on_the_left() {
    let m = Metrics::default();
    let row = Rect::new(100.0, 20.0, 300.0, 240.0);
    let scale = Scale::Value { lo: -1.0, hi: 1.0 };
    let w = Ruler::new(scale, RulerDir::Right).thickness(&m);
    let left = Rect::new(row.x - w, row.y, w, row.h);
    let right = Rect::new(row.x + row.w, row.y, w, row.h);
    let ticks = Ruler::new(scale, RulerDir::Right).ticks((0.0, 1.0), row.h as f64, &m);
    let bare: Vec<Tick> = ticks
        .iter()
        .map(|t| Tick {
            label: None,
            ..t.clone()
        })
        .collect();
    let draw = |strip: Rect, dir: RulerDir, ticks: &[Tick]| {
        let mut mesh = Mesh::new();
        Ruler::new(scale, dir).draw_ticks(
            &mut Draw::new(&mut mesh, &m, &Theme::default()),
            strip,
            row,
            ticks,
            Shared::default(),
        );
        mesh
    };
    assert_eq!(
        draw(left, RulerDir::Right, &ticks).vertex_count(),
        draw(right, RulerDir::Left, &ticks).vertex_count(),
        "the same marks and text"
    );
    // the marks alone, mirrored about the body's centre, are the other side's
    let (l, r) = (
        draw(left, RulerDir::Right, &bare),
        draw(right, RulerDir::Left, &bare),
    );
    let centre = row.x + row.w * 0.5;
    let sorted = |mesh: &Mesh, mirror: bool| {
        let mut out: Vec<(i64, i64)> = mesh
            .positions()
            .map(|(x, y)| {
                let x = if mirror { 2.0 * centre - x } else { x };
                ((x * 4.0).round() as i64, (y * 4.0).round() as i64)
            })
            .collect();
        // the corners, not the triangles: a mirror winds them the other way
        out.sort();
        out.dedup();
        out
    };
    assert_eq!(
        sorted(&l, true),
        sorted(&r, false),
        "each mark is the other's mirror"
    );
}

/// **A strip is reserved off the side it stands on**, and what is left is
/// the content's.
#[test]
fn a_strip_is_reserved_off_the_side_it_stands_on() {
    let body = Rect::new(10.0, 20.0, 300.0, 200.0);
    let value = Scale::Value { lo: 0.0, hi: 1.0 };
    let cases = [
        (RulerDir::Right, Rect::new(10.0, 20.0, 40.0, 200.0)),
        (RulerDir::Left, Rect::new(270.0, 20.0, 40.0, 200.0)),
        (RulerDir::Up, Rect::new(10.0, 180.0, 300.0, 40.0)),
        (RulerDir::Down, Rect::new(10.0, 20.0, 300.0, 40.0)),
    ];
    for (dir, want) in cases {
        let mut left = body;
        let strip = Ruler::new(value, dir).reserve(&mut left, 40.0);
        assert_eq!(strip, want, "{dir:?}");
        let joined = (left.w * left.h) + (strip.w * strip.h);
        assert_eq!(joined, body.w * body.h, "{dir:?}: nothing lost");
        assert!(
            left.intersect(strip).w * left.intersect(strip).h <= 0.0,
            "{dir:?}: nothing shared"
        );
    }
}
