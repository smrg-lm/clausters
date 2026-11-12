//! **The loudness layer**: the curve of what a meter would have read along the
//! take, on a scale of its own.
//!
//! Every other measure of a signal is an amplitude and shares the picture's
//! vertical -- a level body sits inside the envelope it is a reading of, the
//! reconstruction passes through the samples it is drawn over. A loudness
//! reading is in LUFS, so this layer measures **a different quantity on the
//! same time axis**, and the three things that follow are the whole of this
//! module: it brings its own scale, it brings its own ruler, and it is read
//! against a target rather than against the samples it crosses.
//!
//! The scale is EBU Tech 3341's, named by its top: `+9` runs from 18 LU under
//! the target to 9 over it, `+18` from 36 under to 18 over, and the document
//! makes `+9` the default. The target is EBU R 128's -23 LUFS unless a script
//! says otherwise, drawn as a line the way a loudness track draws it.
//!
//! **The curve is not measured here.** It is read off the
//! [`Profile`] the element measured at its
//! mutation points, through the one renderer every signal against time is drawn
//! by ([`trace::draw_channel`]) -- with a vertical map in LU instead of the
//! picture's own, which is the only difference between drawing this and drawing
//! a waveform.

use clausters_core::loudness::Profile;

use crate::host::elements::signal::LoudnessFrame;
use crate::host::graphics::signal::trace::{self, Trace, TraceStyle};
use crate::host::layout::Rect;
use crate::host::paint::Draw;
use crate::host::ruler;
use crate::viewport::View;

/// **A live loudness curve**: the readings a streaming meter has taken, with
/// the scale they are read on.
///
/// The stored layer reads its curve out of a profile, which can answer for any
/// point of a take; a live one has no past but the readings it kept, so what it
/// draws is the run of them -- the same drawing over a different memory, which
/// is the whole difference between a view of a file and a view of a bus.
pub(crate) struct LiveCurve {
    /// One run per loudness layer, oldest reading first, with the weight its
    /// layer is drawn at.
    pub readings: Vec<(trace::Measure, f32, Vec<f32>)>,
    /// The view's vertical window, as a normalized `(start, len)` -- see
    /// [`LoudnessParams::y`].
    pub y: (f64, f64),
    /// The scale's bounds in LUFS, bottom first.
    pub domain: (f32, f32),
    /// The target line, in LUFS.
    pub target: f64,
    /// Whether the layer draws its own LU ruler.
    pub ruler: bool,
}

/// Draws a live curve over `body`: the readings end to end, the way every live
/// view draws its window.
pub(crate) fn draw_live(d: &mut Draw, body: Rect, curve: &LiveCurve) {
    let (lo, hi) = curve.domain;
    if hi <= lo || body.w <= 0.0 || body.h <= 0.0 {
        return;
    }
    let y_at = window_map(body, (lo, hi), curve.y);
    let target_y = y_at(curve.target as f32);
    if target_y >= body.y && target_y <= body.y + body.h {
        let (mesh, m, theme) = d.parts();
        mesh.rect(
            Rect::new(body.x, target_y, body.w, m.divider_w),
            theme.trace_guide,
        );
    }
    for (measure, alpha, readings) in &curve.readings {
        if readings.len() < 2 {
            continue;
        }
        let span = (readings.len() - 1) as f64;
        let (mesh, m, theme) = d.parts();
        trace::draw_channel(
            mesh,
            body,
            &Trace::samples(readings, 1),
            0,
            |x| (x - body.x) as f64 / body.w.max(1.0) as f64 * span,
            |s| body.x + (s / span) as f32 * body.w,
            y_at,
            TraceStyle::new(
                crate::host::theme::with_alpha(
                    trace::measure_color(theme, *measure, theme.trace),
                    *alpha,
                ),
                m.trace_w,
            )
            // A reading is not a sample and is not marked as one: what is
            // drawn is the meter's own line.
            .with_measure(trace::Measure::Peak),
        );
    }
    if curve.ruler {
        lu_ruler(d, body, (lo, hi), curve.target, curve.y);
    }
}

/// What one loudness layer is drawn from.
pub(crate) struct LoudnessParams<'a> {
    /// The curve and the scale, as the element stated them.
    pub layer: &'a LoudnessFrame,
    /// **The view's whole stack** -- the loudness layers in it are what this
    /// draws, one curve each, at the weight each of them states.
    pub layers: &'a super::layers::Stack,
    /// Whether the read-out's span is a selection, which is the one thing the
    /// numbers have to say about themselves.
    pub selection: bool,
    /// **The view's vertical window**, as a normalized `(start, len)` -- the
    /// same one the picture under this layer is drawn through.
    ///
    /// The layer measures its own quantity, but it is drawn in the *box* the
    /// view gives it, and the box is what the vertical gesture zooms. So the
    /// scale normalizes into that box and the window slices it, exactly as the
    /// amplitude domain does: zooming the axis magnifies the LU scale with
    /// everything else, and the ruler's numbers follow. A layer pinned while
    /// the picture under it moved would read as a drawing that came loose.
    pub y: (f64, f64),
}

/// Draws the layer over `body`, on the time window `nav`.
pub(crate) fn draw(d: &mut Draw, body: Rect, nav: &View, p: &LoudnessParams) {
    let drawn: Vec<(trace::Measure, f32)> = p
        .layers
        .drawn_measures()
        .filter(|(m, _)| m.is_loudness())
        .collect();
    if drawn.is_empty() || body.w <= 0.0 || body.h <= 0.0 {
        return;
    }
    let (lo, hi) = p.layer.domain;
    if hi <= lo {
        return;
    }
    let y_at = window_map(body, (lo, hi), p.y);
    // The guides first, under the curve: a reference is read *behind* what it
    // is a reference for.
    guides(d, body, p, &y_at);
    let Some(profile) = p.layer.profile.as_deref() else {
        return;
    };
    let src = |x: f32| nav.start + nav.len * ((x - body.x) as f64 / body.w.max(1.0) as f64);
    let x_of = |s: f64| body.x + ((s - nav.start) / nav.len.max(f64::MIN_POSITIVE)) as f32 * body.w;
    let measures = p.layers.measures();
    for (measure, alpha) in drawn {
        let Some(window) = measure.loudness_window() else {
            continue;
        };
        let (mesh, m, theme) = d.parts();
        trace::draw_channel(
            mesh,
            body,
            &Trace::Loudness {
                profile,
                window,
                floor: lo,
            },
            0,
            src,
            x_of,
            y_at,
            TraceStyle::new(
                crate::host::theme::with_alpha(
                    trace::measure_color(theme, measure, theme.trace),
                    alpha,
                ),
                m.trace_w,
            )
            .with_measure(measure)
            .with_layers(measures),
        );
    }
    if p.layer.ruler {
        lu_ruler(d, body, (lo, hi), p.layer.target, p.y);
    }
    if p.layer.stats {
        numbers(d, body, p, profile);
    }
}

/// The target line and the edges of the scale.
fn guides(d: &mut Draw, body: Rect, p: &LoudnessParams, y_at: &impl Fn(f32) -> f32) {
    let y = y_at(p.layer.target as f32);
    // Zoomed in past it, the target is off the box: a line drawn at the edge
    // would say the target is *there*, which is the one thing a reference line
    // must never say.
    if y < body.y || y > body.y + body.h {
        return;
    }
    let (mesh, m, theme) = d.parts();
    mesh.rect(Rect::new(body.x, y, body.w, m.divider_w), theme.trace_guide);
}

/// The vertical map a loudness layer is drawn through: its own domain
/// normalized into the box, then sliced by the view's vertical window.
fn window_map(body: Rect, domain: (f32, f32), y: (f64, f64)) -> impl Fn(f32) -> f32 + Copy {
    let (lo, hi) = domain;
    let (y0, y_len) = y;
    move |v: f32| {
        let frac = ((v - lo) / (hi - lo)) as f64;
        let rel = (frac - y0) / y_len.max(f64::MIN_POSITIVE);
        body.y + body.h * (1.0 - rel as f32)
    }
}

/// The part of the layer's scale the window shows, in LUFS.
fn visible(domain: (f32, f32), y: (f64, f64)) -> (f32, f32) {
    let (lo, hi) = domain;
    let (y0, y_len) = y;
    let span = (hi - lo) as f64;
    (
        (lo as f64 + span * y0) as f32,
        (lo as f64 + span * (y0 + y_len)) as f32,
    )
}

/// **The layer's own ruler**, along the **inside** of the body's right edge.
///
/// Two calls, and they are the axis-claim question in miniature. It is on the
/// *right* because the left strip is the picture's own axis, and two axes on
/// one strip are two numbers where a reader expects one. It is **inside** the
/// body because a strip beside it would have to be reserved by the container,
/// for a layer that is optional and live -- turning the measure on would then
/// relayout the view, and a picture that jumps when a reading is asked for is
/// worse than one whose numbers sit over it. Text over a picture is what the
/// text plate is for, and this uses it.
///
/// It reads the **visible** part of the scale, so it says what the window is
/// showing rather than what the layer measures.
///
/// It is labelled in **LU relative to the target**, which is the unit R 128
/// states a scale in: 0 is the target itself, so a curve is read as "three over"
/// rather than as an absolute figure the reader has to subtract.
fn lu_ruler(d: &mut Draw, body: Rect, domain: (f32, f32), target: f64, y: (f64, f64)) {
    let (lo, hi) = visible(domain, y);
    let target = target as f32;
    let m = d.m;
    let ticks = ruler::value_ticks((lo - target) as f64, (hi - target) as f64, body.h as f64, m);
    let scale = m.caption_scale;
    let line = crate::host::font::height(scale);
    for tick in &ticks {
        let Some(label) = tick.label.as_ref() else {
            continue;
        };
        let y = body.y + body.h * (1.0 - tick.frac as f32);
        {
            let (mesh, m, theme) = d.parts();
            mesh.rect(
                Rect::new(body.x + body.w - m.pad, y, m.pad, m.divider_w),
                theme.trace_guide,
            );
        }
        let w = crate::host::font::width(label, scale);
        let x = body.x + body.w - m.pad * 2.0 - w;
        let ty = (y - line * 0.5).clamp(body.y, body.y + body.h - line);
        crate::host::graphics::plate_text(d, label, x, ty, w, scale, d.theme.trace_loudness);
    }
}

/// **The numbers**, over the span the read-out names: the selection where there
/// is one, the whole take where there is not.
///
/// The four a delivery specification asks for, in the order it asks for them:
/// the programme loudness, its range, the true peak, and the distance between
/// the last two -- the peak-to-loudness ratio, which is the number a reader
/// means by "how compressed is this".
fn numbers(d: &mut Draw, body: Rect, p: &LoudnessParams, _profile: &Profile) {
    let Some(summary) = p.layer.summary else {
        return;
    };
    let db = |v: f64| {
        if v.is_finite() {
            format!("{v:.1}")
        } else {
            "-inf".to_string()
        }
    };
    let plr = if summary.loudness.integrated.is_finite() && summary.true_peak_db.is_finite() {
        format!(
            "  PLR {}",
            db(summary.true_peak_db as f64 - summary.loudness.integrated)
        )
    } else {
        String::new()
    };
    let text = format!(
        "{}I {} LUFS  LRA {} LU  TP {} dBTP{}",
        if p.selection { "SEL  " } else { "" },
        db(summary.loudness.integrated),
        db(summary.loudness.range),
        db(summary.true_peak_db as f64),
        plr,
    );
    let m = d.m;
    let scale = m.caption_scale;
    let x = body.x + m.pad;
    let y = body.y + m.pad;
    crate::host::graphics::plate_text(
        d,
        &text,
        x,
        y,
        (body.w - 2.0 * m.pad).max(0.0),
        scale,
        d.theme.trace_loudness,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::guidef::GuiNode;
    use crate::host::metrics::Metrics;
    use crate::host::paint::Mesh;
    use crate::host::theme::Theme;
    use crate::host::widget::Widget;
    use crate::host::widget::element::Element;

    /// A **stereo** take of `seconds` at `db` dBFS per channel, interleaved --
    /// the EBU test tone, whose peak level in dBFS is its loudness in LUFS
    /// (a mono one reads 3 dB under, which is the calibration and not an
    /// error).
    fn tone(seconds: f64, db: f64, rate: f64) -> Vec<f32> {
        let n = (seconds * rate) as usize;
        let amp = 10f64.powf(db / 20.0);
        (0..n)
            .flat_map(|i| {
                let v = (amp * (core::f64::consts::TAU * 1000.0 * i as f64 / rate).sin()) as f32;
                [v, v]
            })
            .collect()
    }

    fn element(json: &str) -> crate::host::elements::signal::SignalElement {
        let w = Widget::from_node(1, &GuiNode::parse(json.as_bytes()).unwrap(), &[]).unwrap();
        w.signal().expect("a signal element").clone()
    }

    fn def(measure: &str, samples: &[f32], rate: f64) -> String {
        let data: Vec<String> = samples.iter().map(|v| format!("{v}")).collect();
        format!(
            r#"{{"id":1,"type":"signal","view":"trace","navigable":1,"measure":"{measure}",
                "channels":2,"sample_rate":{rate},"data":[{}]}}"#,
            data.join(",")
        )
    }

    /// **The layer is measured when it is asked for, and not before.** A view
    /// that measures amplitude holds no profile at all -- a curve nobody draws
    /// should not cost a pass over the take, nor keep its blocks.
    #[test]
    fn a_profile_is_measured_only_where_a_measure_asks_for_one() {
        let samples = tone(1.0, -20.0, 8000.0);
        let plain = element(&def("peak", &samples, 8000.0));
        assert!(plain.loudness.profile.is_none(), "nothing asked for one");
        let measured = element(&def("peak momentary", &samples, 8000.0));
        assert!(measured.loudness.profile.is_some(), "a measure did");
        // And it is dropped again when the measure goes, live.
        let mut off = measured.clone();
        assert!(off.set("measure", &serde_json::json!("peak")));
        assert!(off.loudness.profile.is_none(), "and dropped when it goes");
    }

    /// **A loudness layer needs the source's rate**, since its windows are
    /// counted in samples: a view that states none draws no curve rather than
    /// a curve of the wrong length.
    #[test]
    fn a_take_with_no_rate_measures_nothing() {
        let samples = tone(1.0, -20.0, 8000.0);
        let e = element(&def("momentary", &samples, 0.0));
        assert!(e.loudness.profile.is_none());
    }

    /// The numbers name the span they measured, and the span is the selection
    /// where there is one -- which is what an editor's statistics window does,
    /// and what makes the figures answer the question a reader asked.
    #[test]
    fn the_read_out_measures_the_selection_where_there_is_one() {
        let rate = 8000.0;
        let mut samples = tone(4.0, -30.0, rate);
        samples.extend(tone(4.0, -14.0, rate));
        let frames = (samples.len() / 2) as u64;
        let mut e = element(&def("momentary", &samples, rate));
        let whole = e.loudness.summary.expect("the take is measured").loudness;
        assert!(
            (whole.integrated - -14.0).abs() < 1.5,
            "the take is dominated by its loud half: {whole:?}"
        );
        // The quiet half, selected.
        assert!(e.set("sel_start", &serde_json::json!(0.0)));
        assert!(e.set("sel_len", &serde_json::json!(4.0 * rate)));
        let selected = e.loudness.summary.expect("a selection is measured");
        assert_eq!(selected.span, (0, (4.0 * rate) as u64));
        assert_eq!(frames, (8.0 * rate) as u64, "the take is both halves");
        assert!(
            (selected.loudness.integrated - -30.0).abs() < 0.5,
            "the quiet half reads its own loudness: {:?}",
            selected.loudness
        );
    }

    /// **The curve is drawn, and on its own scale.** The proof it is not the
    /// picture's axis: the same take at two targets draws the curve at two
    /// heights, because a scale is a scale and not a normalization.
    #[test]
    fn the_curve_is_drawn_on_the_scale_the_layer_states() {
        let rate = 8000.0;
        let samples = tone(4.0, -20.0, rate);
        let rect = Rect::new(0.0, 0.0, 200.0, 100.0);
        let height = |target: f64| {
            let mut e = element(&def("momentary", &samples, rate));
            assert!(e.set("loudness_target", &serde_json::json!(target)));
            let (m, theme) = (Metrics::default(), Theme::default());
            let mut mesh = Mesh::default();
            let mut d = Draw::new(&mut mesh, &m, &theme);
            draw(
                &mut d,
                rect,
                &View::full(samples.len()),
                &LoudnessParams {
                    layer: &e.loudness.frame(),
                    layers: &e.layers,
                    selection: false,
                    y: (0.0, 1.0),
                },
            );
            let ys: Vec<f32> = mesh.positions().map(|(_, y)| y).collect();
            assert!(!ys.is_empty(), "something was drawn");
            ys.iter().copied().fold(f32::INFINITY, f32::min)
        };
        let (low, high) = (height(-23.0), height(-40.0));
        assert!(
            high < low,
            "a target further under the signal draws it higher: {high} against {low}"
        );
    }

    /// A layer with nothing measured draws its guides and no curve -- the
    /// picture says "this is the scale", not "the loudness is zero".
    #[test]
    fn a_layer_with_no_profile_draws_no_curve() {
        let (m, theme) = (Metrics::default(), Theme::default());
        let mut mesh = Mesh::default();
        let mut d = Draw::new(&mut mesh, &m, &theme);
        let layer = crate::host::elements::signal::LoudnessLayer::default();
        draw(
            &mut d,
            Rect::new(0.0, 0.0, 200.0, 100.0),
            &View::full(1000),
            &LoudnessParams {
                layer: &layer.frame(),
                layers: &crate::host::graphics::signal::layers::Stack::parse(&serde_json::json!(
                    "momentary"
                ))
                .unwrap(),
                selection: false,
                y: (0.0, 1.0),
            },
        );
        // The target line and nothing else: one quad, which is two triangles.
        assert_eq!(mesh.vertex_count(), 6, "the guide, and no curve");
    }
}
