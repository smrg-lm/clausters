//! Drawing the shared-memory-backed views: the level `meter` and the `scope`.
//!
//! These are the cheap counterparts of the heavy GPU views: their *data* is a
//! single control bus read straight from the shared-memory segment each frame
//! (see [`crate::host::shm`]), so they need no buffer, no analysis and no dedicated
//! pipeline -- just the flat-geometry painter ([`crate::host::paint`]) plus bitmap text,
//! exactly like the standard controls. The drawing lives here as pure functions
//! over a [`Draw`]; the windowed front supplies the live value(s) read from
//! shared memory and keeps the scope's rolling history. Keeping it GPU- and
//! shm-free makes it unit-testable without a window.

use clausters_core::measure;

use crate::spectrogram::FreqScale;

use super::controls::body_rect;
use super::signal::{layers, trace};
use crate::host::font;
use crate::host::frame::channel_rect;
use crate::host::layout::Rect;
use crate::host::live::TapWindow;
use crate::host::paint::{Color, Draw};
use crate::host::ruler;
use crate::host::theme::with_alpha;
use crate::host::widget::RulerDir;
use crate::viewport::{Axis, Unit};

/// The 0..1 position of `value` in `[min, max]`, clamped. A degenerate range
/// (min == max) maps to 0.
///
/// The value axis this expresses is a [`Axis::ranged`]; this stays as the
/// `f32` door the drawing code calls, so a paint site keeps naming a range
/// rather than building an axis per mark.
pub fn fraction(value: f32, min: f32, max: f32) -> f32 {
    Axis::ranged(min as f64, max as f64, Unit::Norm).fraction_clamped(value as f64) as f32
}

/// **The scale a meter's column is coloured on**: the two heights, as fractions
/// of the column, where it stops being green and where it is red.
///
/// The levels themselves are the shared core's
/// ([`measure::METER_WARN_DB`], [`measure::METER_HOT_DB`]) -- this is only where
/// they land on *this* column, which depends on what its height measures. A
/// strip drawn in decibels and a widget drawn over a plain amplitude range put
/// the same two levels at different heights, and both are then read the same
/// way: green is headroom, amber is using it, red is about to run out.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scale {
    /// Where the alignment level falls, as a fraction of the height: green
    /// below it, and the ramp to amber begins.
    pub warn: f32,
    /// Where the column is **fully** amber, and stays so up to `hot`.
    pub amber: f32,
    /// Where the hot end falls: the ramp to red begins.
    pub hot: f32,
}

impl Scale {
    /// The decibel strip, from [`measure::METER_FLOOR_DB`] up to full scale --
    /// what a channel's meter stands on.
    pub fn decibels() -> Self {
        Self::decibels_from(measure::METER_FLOOR_DB)
    }

    /// The same three levels on a decibel strip bottoming out at `floor_db` --
    /// the widget's own floor, which may be the dynamic range of a resolution
    /// rather than the mixing strip's sixty.
    pub fn decibels_from(floor_db: f32) -> Self {
        Self {
            warn: measure::meter_fraction_db(measure::METER_WARN_DB, floor_db),
            amber: measure::meter_fraction_db(measure::METER_AMBER_DB, floor_db),
            hot: measure::meter_fraction_db(measure::METER_HOT_DB, floor_db),
        }
    }

    /// The same two levels on a column whose height is an **amplitude** placed
    /// in `min..max` -- the `meter` widget's own axis, whatever range it was
    /// given.
    pub fn amplitude(min: f32, max: f32) -> Self {
        let at = |db| fraction(measure::amplitude_of_db(db), min, max);
        Self {
            warn: at(measure::METER_WARN_DB),
            amber: at(measure::METER_AMBER_DB),
            hot: at(measure::METER_HOT_DB),
        }
    }
}

/// The colour a column has at `height` (0 at the bottom of the well, 1 at the
/// top): green up to the alignment level, then through amber to red.
///
/// A function of the **height** and not of the current level, which is what
/// makes the strip readable at a glance: a colour that moved with the signal
/// would say only "loud", while a fixed scale says *how* loud by where the
/// colour changes.
pub fn column_color(theme: &crate::host::theme::Theme, scale: Scale, height: f32) -> Color {
    Paint::scale(scale, theme).color(height)
}

/// **How a colour passes to the next one**, from a stop up to the stop above.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shape {
    /// It does not: the colour holds to the next stop, which is an edge.
    Step,
    /// A straight blend of the two colours, as written.
    Lin,
    /// A blend bent by `curve`, the same bend a `knob`'s travel and an
    /// envelope segment take (`clausters_core::warp`): negative spends most of
    /// the change early, positive late.
    Curve(f32),
}

impl Shape {
    /// `"step"`, `"lin"`/`"linear"`, or a curve amount; `None` for anything
    /// else.
    pub fn parse(v: &serde_json::Value) -> Option<Self> {
        match v {
            serde_json::Value::String(name) => match name.as_str() {
                "step" => Some(Shape::Step),
                "lin" | "linear" => Some(Shape::Lin),
                _ => None,
            },
            serde_json::Value::Number(n) => Some(Shape::Curve(n.as_f64()? as f32)),
            _ => None,
        }
    }

    /// How far along the blend the colour is at `t` of the way to the next
    /// stop.
    fn weight(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Shape::Step => 0.0,
            Shape::Lin => t,
            Shape::Curve(curve) => clausters_core::warp::curve_value(t, 0.0, 1.0, curve),
        }
    }
}

/// One stop of a [`Paint`]: a height, the colour there, and how it passes to
/// the next stop's.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stop {
    pub at: f32,
    pub color: Color,
    pub shape: Shape,
}

/// **How a column is coloured**, as the drawing needs it: stops at heights
/// (fractions of the column), sorted, each colour already resolved against the
/// theme. Below the first stop its colour holds. What a widget *asks for* is
/// [`Zones`]; this is what that becomes on a given axis -- and the level
/// scale is one of these too, rather than a second way of colouring.
#[derive(Debug, Clone, PartialEq)]
pub struct Paint(pub Vec<Stop>);

impl Paint {
    /// One colour, the whole height.
    pub fn solid(color: Color) -> Self {
        Paint(vec![Stop {
            at: 0.0,
            color,
            shape: Shape::Step,
        }])
    }

    /// **The level scale**: green up to the alignment level, then a blend at
    /// every step -- into amber, reached at `METER_AMBER_DB` so a column using
    /// its headroom reads amber, and on into red, reached at the hot end and
    /// held to the top, where a peak that grows any further clips.
    pub fn scale(scale: Scale, theme: &crate::host::theme::Theme) -> Self {
        let (warn, hot) = (scale.warn.clamp(0.0, 1.0), scale.hot.clamp(0.0, 1.0));
        let amber = scale.amber.clamp(warn, hot);
        let stop = |at, color, shape| Stop { at, color, shape };
        Paint(vec![
            stop(0.0, theme.meter_low, Shape::Step),
            stop(warn, theme.meter_low, Shape::Lin),
            stop(amber, theme.meter_mid, Shape::Lin),
            stop(hot, theme.meter_high, Shape::Step),
        ])
    }

    /// The level scale **by distance from a zero** at `origin`, each side over
    /// its own reach: `above` from the zero up, `below` mirrored from the zero
    /// down, so the same distance either way is the same colour.
    pub fn mirrored(
        origin: f32,
        below: Scale,
        above: Scale,
        theme: &crate::host::theme::Theme,
    ) -> Self {
        let o = origin.clamp(0.0, 1.0);
        // Below the zero a distance `d` is the height `o - d * o`, so the
        // scale is read top-down: red at the bottom, blending up through amber
        // into the green that reaches the zero.
        let depth = |d: f32| o - d.clamp(0.0, 1.0) * o;
        let (warn, hot) = (below.warn.clamp(0.0, 1.0), below.hot.clamp(0.0, 1.0));
        let amber = below.amber.clamp(warn, hot);
        let stop = |at, color, shape| Stop { at, color, shape };
        let mut stops = vec![
            stop(0.0, theme.meter_high, Shape::Step),
            stop(depth(hot), theme.meter_high, Shape::Lin),
            stop(depth(amber), theme.meter_mid, Shape::Lin),
            stop(depth(warn), theme.meter_low, Shape::Step),
        ];
        stops.extend(Paint::scale(above, theme).0.into_iter().map(|s| Stop {
            at: o + s.at * (1.0 - o),
            ..s
        }));
        stops.sort_by(|a, b| a.at.total_cmp(&b.at));
        Paint(stops)
    }

    /// The colour at `height`.
    pub fn color(&self, height: f32) -> Color {
        let Some(first) = self.0.first() else {
            return [0.0; 4];
        };
        let Some(i) = self.0.iter().rposition(|s| height >= s.at) else {
            return first.color;
        };
        let here = self.0[i];
        match self.0.get(i + 1) {
            Some(next) if here.shape != Shape::Step => {
                let t = (height - here.at) / (next.at - here.at).max(1e-6);
                let w = here.shape.weight(t);
                std::array::from_fn(|k| here.color[k] + (next.color[k] - here.color[k]) * w)
            }
            _ => here.color,
        }
    }

    /// The spans a column from `from` to `to` is drawn in: `(low, high,
    /// shape)` with the colour at each end read off [`color`](Self::color).
    /// An edge is where a stop is, never where a band happened to end.
    fn spans(&self, from: f32, to: f32) -> Vec<(f32, f32, Shape)> {
        let mut out = Vec::new();
        let first = self.0.first().map_or(1.0, |s| s.at);
        if first > from {
            out.push((from, first.min(to), Shape::Step));
        }
        for (i, stop) in self.0.iter().enumerate() {
            let high = self.0.get(i + 1).map_or(1.0, |s| s.at);
            let (low, high) = (stop.at.max(from), high.min(to));
            if high > low {
                let shape = if i + 1 < self.0.len() {
                    stop.shape
                } else {
                    Shape::Step
                };
                out.push((low, high, shape));
            }
        }
        out
    }
}

/// How many pieces a bent blend is drawn in: each is a straight blend, and a
/// column is thin, so a handful is a curve.
const CURVE_PIECES: usize = 8;

/// **What a meter's colours are**, as a widget states them (the `zones` prop).
///
/// A level meter is coloured by where its column is: green is headroom, amber
/// is using it, red is about to run out. That reading belongs to a level, so it
/// is the default only where the meter measures one -- a decibel axis, or a
/// plain range that does not cross zero. A range across zero is a signed
/// value, and its default is one colour.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Zones {
    /// Whatever the axis calls for: the level scale, or one colour on a range
    /// across zero.
    #[default]
    Auto,
    /// The level scale on any axis; on a range across zero it is graded by
    /// distance from the zero, the same on both sides.
    On,
    /// One colour, the level scale's lowest.
    Off,
    /// The widget's own stops, in the axis' units -- decibels on a decibel
    /// axis, the value itself on a plain one: each colour from its value up
    /// to the next one's, passing to it by its [`Shape`]. Below the first
    /// value the first colour holds.
    Stops(Vec<(f32, ZoneColor, Shape)>),
}

/// A zone's colour: a fixed one, or a theme role, which follows the theme the
/// meter is drawn in.
#[derive(Debug, Clone, PartialEq)]
pub enum ZoneColor {
    Rgba(Color),
    Role(String),
}

impl ZoneColor {
    /// `#rrggbb[aa]`, or the name of a theme role; `None` for anything else.
    pub fn parse(text: &str) -> Option<Self> {
        if text.starts_with('#') {
            crate::host::theme::parse_hex(text).map(ZoneColor::Rgba)
        } else {
            crate::host::theme::Theme::default()
                .get(text)
                .map(|_| ZoneColor::Role(text.to_string()))
        }
    }

    fn resolve(&self, theme: &crate::host::theme::Theme) -> Color {
        match self {
            ZoneColor::Rgba(color) => *color,
            ZoneColor::Role(name) => theme.get(name).unwrap_or(theme.meter_low),
        }
    }
}

impl Zones {
    /// The `zones` prop: a boolean (`true`/`false`, or `1`/`0` as the clients
    /// send a flag), or an array of `[value, colour]` or `[value, colour,
    /// shape]` stops -- as JSON, or as the JSON text a `/gui_set` carries.
    /// `None` for anything else, including a stop whose colour is neither a
    /// hex nor a theme role or whose shape is not one.
    pub fn parse(v: &serde_json::Value) -> Option<Self> {
        use serde_json::Value;
        match v {
            Value::Bool(on) => Some(if *on { Zones::On } else { Zones::Off }),
            Value::Number(n) => Some(if n.as_f64()? != 0.0 {
                Zones::On
            } else {
                Zones::Off
            }),
            Value::String(text) => match text.as_str() {
                "auto" | "" => Some(Zones::Auto),
                "on" | "true" => Some(Zones::On),
                "off" | "false" => Some(Zones::Off),
                json => Self::parse(&serde_json::from_str(json).ok()?),
            },
            Value::Array(entries) => {
                let mut stops = entries
                    .iter()
                    .map(|entry| {
                        let entry = entry.as_array()?;
                        if entry.len() > 3 {
                            return None;
                        }
                        let value = entry.first()?.as_f64()? as f32;
                        let color = ZoneColor::parse(entry.get(1)?.as_str()?)?;
                        let shape = match entry.get(2) {
                            Some(shape) => Shape::parse(shape)?,
                            None => Shape::Step,
                        };
                        Some((value, color, shape))
                    })
                    .collect::<Option<Vec<_>>>()?;
                if stops.is_empty() {
                    return None;
                }
                stops.sort_by(|a, b| a.0.total_cmp(&b.0));
                Some(Zones::Stops(stops))
            }
            _ => None,
        }
    }

    /// What these zones are on `axis`, drawn in `theme`.
    pub fn paint(&self, axis: MeterAxis, theme: &crate::host::theme::Theme) -> Paint {
        match (self, axis) {
            (Zones::Auto, axis) if axis.centred() => Paint::solid(theme.meter_low),
            (Zones::Off, _) => Paint::solid(theme.meter_low),
            (Zones::On, MeterAxis::Linear { min, max }) if axis.centred() => Paint::mirrored(
                axis.origin(),
                Scale::amplitude(0.0, -min),
                Scale::amplitude(0.0, max),
                theme,
            ),
            (Zones::Auto | Zones::On, axis) => Paint::scale(axis.scale(), theme),
            (Zones::Stops(stops), axis) => Paint(
                stops
                    .iter()
                    .map(|(value, color, shape)| Stop {
                        at: axis.fraction_of(*value),
                        color: color.resolve(theme),
                        shape: *shape,
                    })
                    .collect(),
            ),
        }
    }
}

/// **One meter column**, and the only place this host draws one: the well, the
/// column standing in it up to `fill`, and the held peak as a hairline at
/// `mark` (a hairline rather than a second column, because it is the same axis
/// read at another moment). Both are fractions of the cell's height; a `mark`
/// at or below zero draws none.
///
/// Written once because a meter is about to exist in three places -- a track's
/// header, the `meter` widget, and the mixer that has not been built -- and
/// three columns drawn by three call sites is three answers to how loud a
/// signal is.
pub fn draw_column(
    mesh: &mut crate::host::paint::Mesh,
    m: &crate::host::metrics::Metrics,
    theme: &crate::host::theme::Theme,
    cell: Rect,
    fill: f32,
    mark: f32,
    scale: Scale,
) {
    let paint = Paint::scale(scale, theme);
    draw_column_from(mesh, m, theme, cell, 0.0, fill, mark, &paint);
}

/// [`draw_column`] standing on `origin` rather than on the floor, and coloured
/// by any [`Paint`]: the column runs from `origin` to `fill`, up or down. A
/// meter whose range crosses zero stands on the zero, so a value below it
/// reads as a column below it rather than as a shorter one above the floor.
///
/// **A span is one quad.** A held colour is a flat one, a straight blend is
/// one quad with a colour at each end that the rasterizer interpolates, and a
/// bent blend is [`CURVE_PIECES`] of those -- so a graded column is a handful
/// of quads however tall it is, and every edge lands on its stop.
#[allow(clippy::too_many_arguments)] // draw_column's six, and where it stands
pub fn draw_column_from(
    mesh: &mut crate::host::paint::Mesh,
    m: &crate::host::metrics::Metrics,
    theme: &crate::host::theme::Theme,
    cell: Rect,
    origin: f32,
    fill: f32,
    mark: f32,
    paint: &Paint,
) {
    if cell.w <= 0.0 || cell.h <= 0.0 {
        return;
    }
    mesh.rect(cell, theme.meter_field);
    let origin = origin.clamp(0.0, 1.0);
    let fill = fill.clamp(0.0, 1.0);
    let (from, to) = (origin.min(fill), origin.max(fill));
    let y = |height: f32| cell.y + cell.h * (1.0 - height);
    let mut blend = |low: f32, high: f32| {
        mesh.vgradient(
            Rect::new(cell.x, y(high), cell.w, y(low) - y(high)),
            paint.color(high - 1e-6),
            paint.color(low),
        );
    };
    if to > from {
        for (low, high, shape) in paint.spans(from, to) {
            match shape {
                Shape::Step | Shape::Lin => blend(low, high),
                Shape::Curve(_) => {
                    let step = (high - low) / CURVE_PIECES as f32;
                    for piece in 0..CURVE_PIECES {
                        let at = low + piece as f32 * step;
                        blend(at, at + step);
                    }
                }
            }
        }
    }
    if mark > 0.0 {
        let y = cell.y + cell.h * (1.0 - mark.clamp(0.0, 1.0));
        mesh.line([cell.x, y], [cell.x + cell.w, y], m.divider_w, theme.text);
    }
}

/// Draws a vertical level meter: a framed well with the column rising from the
/// bottom to `fraction` of the body height, plus the raw value as text. The
/// column is the shared one ([`draw_column`]), so this widget, a track's meter
/// strip and a mixer's all read alike.
pub fn draw_meter(d: &mut Draw, rect: Rect, value: f32, fraction: f32, label: Option<&str>) {
    draw_meter_scaled(d, rect, value, fraction, label, Scale::amplitude(0.0, 1.0));
}

/// [`draw_meter`] told which scale its height is on, which only the widget
/// knows: its `min`..`max` is its axis and the colours follow it.
pub fn draw_meter_scaled(
    d: &mut Draw,
    rect: Rect,
    value: f32,
    fraction: f32,
    label: Option<&str>,
    scale: Scale,
) {
    label_strip(d, label, rect);
    let (mesh, m, theme) = d.parts();
    let body = body_rect(rect, label.is_some(), m);
    if body.w <= 0.0 || body.h <= 0.0 {
        return;
    }
    draw_column(mesh, m, theme, body, fraction, 0.0, scale);
    mesh.border(body, m.divider_w, theme.accent);
    super::corner_text(d, &fmt(value), body);
}

/// Draws a time-domain scope: a framed field with a polyline through `history`
/// (oldest sample at the left, newest at the right), each sample normalized into
/// `[min, max]`. Fewer than two samples draw just the frame.
pub fn draw_scope(
    d: &mut Draw,
    rect: Rect,
    history: &[f32],
    min: f32,
    max: f32,
    label: Option<&str>,
    layers: &layers::Stack,
) {
    label_strip(d, label, rect);
    let (mesh, m, theme) = d.parts();
    let body = body_rect(rect, label.is_some(), m);
    if body.w <= 0.0 || body.h <= 0.0 {
        return;
    }
    mesh.rect(body, theme.field);
    mesh.border(body, m.divider_w, theme.accent);
    // A control bus's history is one channel of a live source: the same
    // renderer, so a history longer than the body's pixels summarizes instead
    // of aliasing -- which a polyline of its own never did. One pass per
    // measure, the envelope under the level body.
    let measures = layers.measures();
    for (measure, alpha) in layers.drawn_measures() {
        let color = with_alpha(trace::measure_color(d.theme, measure, d.theme.trace), alpha);
        trace_row(d, body, history, 1, 0, (min, max), color, measure, measures);
    }
}

/// The display parameters of one audio-rate oscilloscope draw, alongside its
/// aligned [`TapWindow`].
pub(crate) struct WaveParams<'a> {
    pub window: &'a TapWindow,
    pub min: f32,
    pub max: f32,
    /// The display window in ms (places the x ruler's ticks).
    pub window_ms: f32,
    pub trigger: f32,
    pub overlay: bool,
    pub ruler: bool,
    pub ruler_y: bool,
    pub label: Option<&'a str>,
    /// **The layer stack**, back to front -- a live view reads it like a stored
    /// one: the same pictures, over a window that happens to be arriving.
    pub layers: &'a layers::Stack,
    /// The live loudness curve, when a measure asks for one -- drawn over the
    /// whole body, since a loudness is the channels summed and has no lane of
    /// its own.
    pub loudness: Option<super::signal::loudness::LiveCurve>,
}

/// Draws an audio-rate oscilloscope: the [`TapWindow`]'s channels as stacked
/// rows, one per channel (or color-coded `overlay` traces in one field), each an
/// already-aligned display window (see `clausters_core::oscil`) over `[min, max]` --
/// a polyline while the data fits the width, a per-column min/max envelope
/// when it does not (never resolving finer than the screen). The chrome names
/// what the trigger did: a faint line marks the `trigger` level in the first
/// channel's row (where the alignment is searched) and a `lock`/`free`
/// read-out says whether it fired. `ruler` is the x strip in milliseconds of
/// the window, `ruler_y` the per-row value strip. An empty window draws just
/// the framed field.
pub(crate) fn draw_wave(d: &mut Draw, rect: Rect, p: &WaveParams) {
    label_strip(d, p.label, rect);
    let m = d.m;
    let mut body = body_rect(rect, p.label.is_some(), m);
    // **The rows are the lanes, not the channels**, and the two differ exactly
    // when the traces are overlaid: one lane holding every channel. Keeping the
    // distinction in one name is what this drawing got wrong -- a second
    // `channels` further down shadowed this one, so an overlaid scope reserved
    // one lane's worth of value axis and then placed its traces in row 0 *of
    // the channel count*, which is the top half of the body with the rest left
    // empty.
    let rows = if p.overlay {
        1
    } else {
        p.window.channels.max(1)
    };
    // Height first: the x strip takes it, and it is what decides how finely the
    // value axis steps and therefore how wide the labels the y strip holds are.
    let takes_x = p.ruler && body.h > m.ruler_h * 2.0;
    let row_h = (if takes_x { body.h - m.ruler_h } else { body.h }) / rows as f32;
    let want_w = ruler::value_strip_w(p.min as f64, p.max as f64, row_h, m);
    let strip_x = (p.ruler_y && body.w > want_w * 2.0).then(|| {
        let x = body.x;
        body.x += want_w;
        body.w -= want_w;
        x
    });
    let x_strip = takes_x.then(|| {
        body.h -= m.ruler_h;
        Rect::new(body.x, body.y + body.h, body.w, m.ruler_h)
    });
    if body.w <= 0.0 || body.h <= 0.0 {
        return;
    }
    d.mesh.rect(body, d.theme.field);
    d.mesh.border(body, m.divider_w, d.theme.accent);
    if let Some(strip) = x_strip {
        let ticks = ruler::hz_ticks_h(
            p.window_ms.max(0.1) as f64,
            FreqScale::Linear,
            1e-4,
            strip.w as f64,
            0.0,
            1.0,
            m,
        );
        ruler::draw_ticks_h(d, strip, &ticks, RulerDir::Up);
    }
    let channels = p.window.channels.max(1);
    let frames = p.window.frames();
    for ch in 0..channels {
        let row = channel_rect(body, rows, if p.overlay { 0 } else { ch });
        if ch > 0 && !p.overlay {
            d.mesh.rect(
                Rect::new(body.x, row.y, body.w, m.divider_w),
                d.theme.channel_divider,
            );
        }
        if (ch == 0 || !p.overlay)
            && let Some(strip_x) = strip_x
        {
            let ticks = ruler::value_ticks(p.min as f64, p.max as f64, row.h as f64, m);
            ruler::draw_ticks_v(d, body.x, strip_x, row, &ticks);
        }
        if ch == 0 && frames > 0 {
            // The trigger level, in the channel the alignment is searched in.
            let y = row.y + row.h * (1.0 - fraction(p.trigger, p.min, p.max));
            d.mesh
                .rect(Rect::new(body.x, y, body.w, m.divider_w), d.theme.trigger);
        }
        let color = if channels > 1 {
            d.theme.series(ch)
        } else {
            d.theme.trace
        };
        let measures = p.layers.measures();
        for (measure, alpha) in p.layers.drawn_measures() {
            trace_row(
                d,
                row,
                &p.window.samples,
                channels,
                ch,
                (p.min, p.max),
                with_alpha(trace::measure_color(d.theme, measure, color), alpha),
                measure,
                measures,
            );
        }
    }
    if let Some(curve) = &p.loudness {
        super::signal::loudness::draw_live(d, body, curve);
    }
    if frames > 0 {
        super::corner_text(d, if p.window.locked { "lock" } else { "free" }, body);
    }
}

/// One channel of a live source into `row`, through the **one** column source
/// and mesh renderer every view of a signal against time reads
/// ([`trace::draw_channel`]): a per-column min/max envelope while the frames
/// outnumber the pixels, a polyline once they do not.
///
/// It used to be this module's own loop -- the copy the signal element's
/// collapse left outside, with a regime rule of its own (`frames > columns *
/// 2`), a column inked one hairline wide however wide the pixel column was,
/// and no baseline. A live view is the same drawing of the same signal as a
/// stored one; only where the samples come from differs.
#[allow(clippy::too_many_arguments)]
fn trace_row(
    d: &mut Draw,
    row: Rect,
    samples: &[f32],
    channels: usize,
    ch: usize,
    domain: (f32, f32),
    color: Color,
    measure: trace::Measure,
    layers: trace::Measures,
) {
    let (min, max) = domain;
    let (mesh, m, theme) = d.parts();
    let frames = samples.len() / channels.max(1);
    if frames < 2 {
        return;
    }
    // A live window is drawn whole: its span is the row, end to end.
    let span = (frames - 1) as f64;
    trace::draw_channel(
        mesh,
        row,
        &trace::Trace::samples(samples, channels),
        ch,
        |x| (x - row.x) as f64 / row.w.max(1.0) as f64 * span,
        |s| row.x + (s / span) as f32 * row.w,
        |v| row.y + row.h * (1.0 - fraction(v, min, max)),
        trace::TraceStyle::new(color, m.trace_w)
            .with_dots(m.point_radius)
            .with_measure(measure)
            .with_layers(layers)
            .with_overs(theme.meter_clip, m.caption_scale),
    );
}

/// Draws the label strip above a view body, if it has a label.
///
/// Every view that reserves a strip with [`super::controls::body_rect`] draws it
/// with this: the height and the drawing are the same fact, and they were once
/// two -- three signal views carried a copy of these four lines, agreeing because
/// they had been copied rather than because anything held them together.
pub(crate) fn label_strip(d: &mut Draw, label: Option<&str>, rect: Rect) {
    let (mesh, m, theme) = d.parts();
    if let Some(text) = label {
        // **Truncated to its own widget**, the way a control's caption already
        // was (`controls::label_strip`): a live view is often narrow -- a meter
        // is a column -- and a caption drawn at full length runs across the
        // neighbour, which reads as one unintelligible word rather than as two
        // labels.
        font::text_ellipsis(
            mesh,
            text,
            rect.x + m.pad,
            rect.y + m.pad,
            (rect.w - 2.0 * m.pad).max(0.0),
            m.text_scale,
            theme.text,
        );
    }
}

/// Formats a value compactly (drops trailing zeros within 2 decimals).
fn fmt(v: f32) -> String {
    if v.fract() == 0.0 && v.abs() < 1e6 {
        format!("{v:.0}")
    } else {
        format!("{v:.2}")
    }
}

#[cfg(test)]
mod tests {
    /// **Overlaid traces share the whole lane, not its top half.** The rows a
    /// live view places into are the *lanes*, and with `overlay` there is one
    /// of them -- a second count of the *channels* shadowed that here, so an
    /// overlaid scope drew both traces into row 0 of two, leaving the bottom
    /// half of the body empty with the value ruler squeezed into the top.
    /// Found by eye on `views/loudness`, the first example to overlay a scope.
    #[test]
    fn an_overlaid_live_trace_fills_its_lane() {
        use super::*;
        use crate::host::live::TapWindow;
        use crate::host::metrics::Metrics;
        use crate::host::paint::Mesh;
        use crate::host::theme::Theme;

        let (m, theme) = (Metrics::default(), Theme::default());
        let rect = Rect::new(0.0, 0.0, 200.0, 120.0);
        let frames = 128;
        // Two channels of a full-scale ramp: what is drawn reaches both ends of
        // the value axis, so the ink's extent is the lane's.
        let samples: Vec<f32> = (0..frames)
            .flat_map(|i| {
                let v = (i as f32 / (frames - 1) as f32) * 2.0 - 1.0;
                [v, v]
            })
            .collect();
        let window = TapWindow {
            samples,
            channels: 2,
            locked: false,
        };
        let draw_with = |overlay: bool| {
            let mut mesh = Mesh::default();
            let mut d = Draw::new(&mut mesh, &m, &theme);
            draw_wave(
                &mut d,
                rect,
                &WaveParams {
                    window: &window,
                    min: -1.0,
                    max: 1.0,
                    window_ms: 10.0,
                    trigger: 0.0,
                    overlay,
                    ruler: false,
                    ruler_y: false,
                    label: None,
                    layers: &layers::Stack::of(&[trace::Measure::Peak]),
                    loudness: None,
                },
            );
            mesh.extent().expect("something was drawn")
        };
        let overlaid = draw_with(true);
        let stacked = draw_with(false);
        // Both pictures reach the same bottom: the overlaid one is one lane as
        // tall as the two stacked ones together.
        assert!(
            (overlaid.y + overlaid.h - (stacked.y + stacked.h)).abs() < 2.0,
            "overlaid {overlaid:?} against stacked {stacked:?}"
        );
        // And it is not the top half: the ink is taller than one of two lanes.
        assert!(
            overlaid.h > rect.h * 0.6,
            "an overlaid lane is the body, not a row of it: {overlaid:?}"
        );
    }

    use super::*;
    use crate::host::metrics::Metrics;
    use crate::host::paint::Mesh;
    use crate::host::theme::Theme;

    #[test]
    fn fraction_clamps_and_handles_degenerate_range() {
        assert_eq!(fraction(0.5, 0.0, 1.0), 0.5);
        assert_eq!(fraction(-1.0, 0.0, 1.0), 0.0, "below min clamps to 0");
        assert_eq!(fraction(2.0, 0.0, 1.0), 1.0, "above max clamps to 1");
        assert_eq!(fraction(0.0, 0.0, 2.0), 0.0);
        assert_eq!(fraction(5.0, 3.0, 3.0), 0.0, "min == max maps to 0");
    }

    /// **A meter at its own natural width still carries its ladder.** The rule
    /// that decides whether the numbers fit used to be "the strip may not take
    /// half the body", which was right while a meter stretched to its cell and
    /// wrong the moment it asked for a width of its own -- a strip of numbers
    /// is wider than two thin columns, so the meter dropped the very ladder its
    /// width had been computed to include, and the numbers vanished.
    #[test]
    fn a_meter_at_its_natural_width_keeps_its_numbers() {
        let m = Metrics::default();
        let theme = Theme::default();
        let channels = [ChannelRead::default(); 2];
        let draw_at = |mesh: &mut Mesh, ruler| {
            draw_meter_view(
                &mut Draw::new(mesh, &m, &theme),
                // The width a two-channel meter asks for: two thin columns
                // plus the `ruler_w` role and the padding.
                Rect::new(
                    0.0,
                    0.0,
                    (m.box_side * 0.5).max(3.0) * 2.0 + m.ruler_w + m.pad,
                    160.0,
                ),
                &MeterView {
                    channels: &channels,
                    axis: MeterAxis::Decibels { floor_db: -60.0 },
                    readout: true,
                    label: None,
                    ruler,
                    zones: &Zones::Auto,
                },
            );
        };
        let mut bare = Mesh::new();
        draw_at(&mut bare, None);
        let mut ruled = Mesh::new();
        draw_at(&mut ruled, Some(crate::host::ruler::Side::Left));
        assert!(
            ruled.vertex_count() > bare.vertex_count(),
            "the ladder is drawn: {} vs {}",
            ruled.vertex_count(),
            bare.vertex_count()
        );
    }

    /// **`readout` off is a bare column**, which is what a strip of meters in a
    /// track header is: no ladder, no figures, and the height that would have
    /// held them given back to the column.
    #[test]
    fn a_meter_told_to_carry_no_numbers_carries_none() {
        let m = Metrics::default();
        let theme = Theme::default();
        let channels = [ChannelRead {
            level: 0.5,
            mark: 0.7,
            clipped: Some(1.4),
        }];
        let draw_at = |mesh: &mut Mesh, readout| {
            draw_meter_view(
                &mut Draw::new(mesh, &m, &theme),
                Rect::new(0.0, 0.0, 60.0, 160.0),
                &MeterView {
                    channels: &channels,
                    axis: MeterAxis::Decibels { floor_db: -60.0 },
                    readout,
                    label: None,
                    ruler: None,
                    zones: &Zones::Auto,
                },
            );
        };
        let mut numbered = Mesh::new();
        draw_at(&mut numbered, true);
        let mut bare = Mesh::new();
        draw_at(&mut bare, false);
        assert!(
            bare.vertex_count() < numbered.vertex_count(),
            "the glyphs and their plate are gone: {} vs {}",
            bare.vertex_count(),
            numbered.vertex_count()
        );
    }

    /// **The `zones` prop reads a flag or a list.** A boolean and the `1`/`0`
    /// the clients send a flag as; the words; an array of `[value, colour]`
    /// or `[value, colour, shape]` stops, sorted by value, as JSON or as the
    /// JSON text a `/gui_set` carries; and nothing else -- a colour that is
    /// neither a hex nor a theme role, or a shape that is not one, refuses the
    /// whole list rather than drawing a zone nobody asked for.
    #[test]
    fn zones_read_a_flag_or_a_list() {
        use serde_json::json;
        assert_eq!(Zones::parse(&json!(true)), Some(Zones::On));
        assert_eq!(Zones::parse(&json!(0)), Some(Zones::Off));
        assert_eq!(Zones::parse(&json!("auto")), Some(Zones::Auto));
        let list = json!([
            [0.5, "meter_high"],
            [-1, "#40c060", "lin"],
            [0, "meter_mid", -3]
        ]);
        let green = crate::host::theme::parse_hex("#40c060").unwrap();
        let want = Some(Zones::Stops(vec![
            (-1.0, ZoneColor::Rgba(green), Shape::Lin),
            (0.0, ZoneColor::Role("meter_mid".into()), Shape::Curve(-3.0)),
            (0.5, ZoneColor::Role("meter_high".into()), Shape::Step),
        ]));
        assert_eq!(Zones::parse(&list), want, "sorted by value");
        assert_eq!(Zones::parse(&json!(list.to_string())), want, "and as text");
        assert_eq!(Zones::parse(&json!([[0, "no_such_role"]])), None);
        assert_eq!(Zones::parse(&json!([[0, "#zz0000"]])), None);
        assert_eq!(Zones::parse(&json!([[0, "meter_low", "smooth"]])), None);
        assert_eq!(Zones::parse(&json!([[0, "meter_low", "lin", 1]])), None);
        assert_eq!(Zones::parse(&json!([])), None);
        assert_eq!(Zones::parse(&json!({"a": 1})), None);
    }

    /// **The level scale is stops too**: green to the alignment level, a
    /// straight blend into amber and another into red, red from the hot end.
    #[test]
    fn the_level_scale_is_stops_and_reads_as_before() {
        let theme = Theme::default();
        let scale = Scale::decibels();
        let paint = Paint::scale(scale, &theme);
        assert_eq!(paint.color(scale.warn * 0.5), theme.meter_low);
        let mid = (scale.warn + scale.amber) * 0.5;
        let blend = paint.color(mid);
        for ((got, low), mid) in blend.iter().zip(theme.meter_low).zip(theme.meter_mid) {
            assert!(
                (got - (low + mid) * 0.5).abs() < 1e-5,
                "half way is half of each"
            );
        }
        assert_ne!(
            paint.color((scale.amber + scale.hot) * 0.5),
            theme.meter_mid,
            "amber blends on into red"
        );
        assert_eq!(paint.color(1.0), theme.meter_high);
        // Drawn in a handful of quads, not a band per row.
        let m = Metrics::default();
        let mut mesh = Mesh::new();
        draw_column(
            &mut mesh,
            &m,
            &theme,
            Rect::new(0.0, 0.0, 8.0, 400.0),
            1.0,
            0.0,
            scale,
        );
        assert!(
            mesh.vertex_count() <= 6 * 5,
            "{} vertices",
            mesh.vertex_count()
        );
    }

    /// **A bend is the core's curve.** A negative amount spends most of the
    /// change early, so half way along the colour is past half way.
    #[test]
    fn a_curved_stop_bends_the_blend() {
        let (a, b) = ([0.0, 0.0, 0.0, 1.0], [1.0, 1.0, 1.0, 1.0]);
        let bent = |shape| {
            Paint(vec![
                Stop {
                    at: 0.0,
                    color: a,
                    shape,
                },
                Stop {
                    at: 1.0,
                    color: b,
                    shape: Shape::Step,
                },
            ])
        };
        assert!((bent(Shape::Lin).color(0.5)[0] - 0.5).abs() < 1e-6);
        assert!(bent(Shape::Curve(-4.0)).color(0.5)[0] > 0.8);
        assert!(bent(Shape::Curve(4.0)).color(0.5)[0] < 0.2);
    }

    /// **The default follows the axis.** A level is graded; a range across
    /// zero is one colour unless asked, and asked it is graded by distance
    /// from the zero, the same on both sides; off is one colour anywhere.
    #[test]
    fn the_default_zones_follow_the_axis() {
        let theme = Theme::default();
        let db = MeterAxis::Decibels { floor_db: -60.0 };
        let level = MeterAxis::Linear { min: 0.0, max: 1.0 };
        let signed = MeterAxis::Linear {
            min: -1.0,
            max: 1.0,
        };
        let solid = Paint::solid(theme.meter_low);
        assert_eq!(
            Zones::Auto.paint(db, &theme),
            Paint::scale(db.scale(), &theme)
        );
        assert_eq!(
            Zones::Auto.paint(level, &theme),
            Paint::scale(level.scale(), &theme)
        );
        assert_eq!(Zones::Auto.paint(signed, &theme), solid);
        assert_eq!(Zones::Off.paint(db, &theme), solid);
        let mirrored = Zones::On.paint(signed, &theme);
        for d in [0.01f32, 0.05, 0.2, 0.3, 0.35, 0.45, 0.49] {
            let (up, down) = (mirrored.color(0.5 + d), mirrored.color(0.5 - d));
            for (u, w) in up.iter().zip(down) {
                assert!(
                    (u - w).abs() < 1e-4,
                    "the same distance either side of zero is the same colour ({d}): {up:?} {down:?}"
                );
            }
        }
        assert_eq!(mirrored.color(0.5), theme.meter_low, "green at zero");
        assert_eq!(mirrored.color(0.0), theme.meter_high, "red at -1");
        assert_eq!(mirrored.color(1.0), theme.meter_high, "and at +1");
    }

    /// **A zone is written in the axis' units** and drawn with its edges where
    /// they are: decibels on a decibel axis, the signed value on a range
    /// across zero, and the first colour under the first value.
    #[test]
    fn zones_are_placed_in_the_axis_units() {
        let theme = Theme::default();
        let red = crate::host::theme::parse_hex("#d04040").unwrap();
        let green = crate::host::theme::parse_hex("#40c060").unwrap();
        let by_sign = Zones::Stops(vec![
            (-1.0, ZoneColor::Rgba(red), Shape::Step),
            (0.0, ZoneColor::Rgba(green), Shape::Step),
        ]);
        let signed = MeterAxis::Linear {
            min: -1.0,
            max: 1.0,
        };
        let paint = by_sign.paint(signed, &theme);
        assert_eq!(paint.color(0.25), red);
        assert_eq!(paint.color(0.75), green);
        let db = MeterAxis::Decibels { floor_db: -60.0 };
        let hot = Zones::Stops(vec![
            (-60.0, ZoneColor::Role("meter_low".into()), Shape::Step),
            (-6.0, ZoneColor::Role("meter_high".into()), Shape::Step),
        ]);
        let stops = hot.paint(db, &theme).0;
        assert!(
            (stops[1].at - 0.9).abs() < 1e-5,
            "-6 dB of 60 is nine tenths"
        );
        assert_eq!(stops[1].color, theme.meter_high, "a role follows the theme");

        // Drawn: a column from the zero down to -0.5 is one red quad, with its
        // edge at the zero and not at the nearest band.
        let m = Metrics::default();
        let cell = Rect::new(0.0, 0.0, 8.0, 100.0);
        let mut mesh = Mesh::new();
        draw_column(&mut mesh, &m, &theme, cell, 0.0, 0.0, Scale::decibels());
        let well = mesh.vertex_count();
        mesh.clear();
        draw_column_from(&mut mesh, &m, &theme, cell, 0.5, 0.25, 0.0, &paint);
        let (lo, hi) = mesh
            .positions()
            .skip(well as usize)
            .fold((f32::MAX, f32::MIN), |(lo, hi), (_, y)| {
                (lo.min(y), hi.max(y))
            });
        assert_eq!((lo, hi), (50.0, 75.0));
        assert_eq!(
            mesh.vertex_count() - well,
            well,
            "one quad, as many vertices as the well"
        );
        assert!(
            mesh.colors_by_y()
                .skip(well as usize)
                .all(|(_, c)| c == red)
        );
    }

    /// **A centred meter has no held peak.** The mark a level meter keeps is
    /// the loudest an amplitude has been; on a signed value it was the
    /// magnitude, drawn above the zero for a value below it. The same view
    /// with a mark and without one draws the same, and a level meter still
    /// draws its mark.
    #[test]
    fn a_centred_meter_draws_no_mark() {
        let m = Metrics::default();
        let theme = Theme::default();
        let draw_with = |axis, mark| {
            let mut mesh = Mesh::new();
            draw_meter_view(
                &mut Draw::new(&mut mesh, &m, &theme),
                Rect::new(0.0, 0.0, 60.0, 160.0),
                &MeterView {
                    channels: &[ChannelRead {
                        level: -0.5,
                        mark,
                        clipped: None,
                    }],
                    axis,
                    readout: false,
                    label: None,
                    ruler: None,
                    zones: &Zones::Auto,
                },
            );
            mesh.vertex_count()
        };
        let signed = MeterAxis::Linear {
            min: -1.0,
            max: 1.0,
        };
        assert_eq!(draw_with(signed, 0.8), draw_with(signed, 0.0));
        let level = MeterAxis::Linear { min: 0.0, max: 1.0 };
        assert!(draw_with(level, 0.8) > draw_with(level, 0.0));
    }

    #[test]
    fn meter_emits_fill_geometry() {
        let mut m = Mesh::new();
        draw_meter(
            &mut Draw::new(&mut m, &Metrics::default(), &Theme::default()),
            Rect::new(0.0, 0.0, 40.0, 120.0),
            0.5,
            0.5,
            Some("out"),
        );
        assert!(!m.is_empty(), "a meter with a positive fill draws geometry");
    }

    #[test]
    fn scope_draws_a_polyline_for_history() {
        let mut empty = Mesh::new();
        draw_scope(
            &mut Draw::new(&mut empty, &Metrics::default(), &Theme::default()),
            Rect::new(0.0, 0.0, 80.0, 60.0),
            &[0.0],
            -1.0,
            1.0,
            None,
            &layers::Stack::default(),
        );
        let with_one = empty.vertex_count();

        let mut many = Mesh::new();
        draw_scope(
            &mut Draw::new(&mut many, &Metrics::default(), &Theme::default()),
            Rect::new(0.0, 0.0, 80.0, 60.0),
            &[0.0, 0.5, -0.5, 1.0],
            -1.0,
            1.0,
            None,
            &layers::Stack::default(),
        );
        assert!(
            many.vertex_count() > with_one,
            "more history points add line segments"
        );
    }

    /// The live views read the **one** trace renderer now, so a history longer
    /// than the body's pixels summarizes into min/max columns instead of
    /// drawing a segment per sample. This module used to have a polyline of its
    /// own that stepped `row.w / (frames - 1)` however many frames there were,
    /// which aliases and costs the data rather than the screen -- the rule every
    /// other view of a signal has always followed.
    #[test]
    fn a_long_history_costs_the_body_not_its_samples() {
        let body = Rect::new(0.0, 0.0, 80.0, 60.0);
        let history: Vec<f32> = (0..20_000).map(|i| (i as f32 * 0.01).sin()).collect();
        let mut mesh = Mesh::new();
        draw_scope(
            &mut Draw::new(&mut mesh, &Metrics::default(), &Theme::default()),
            body,
            &history,
            -1.0,
            1.0,
            None,
            &layers::Stack::default(),
        );
        // The field, its border and at most one six-vertex column per pixel.
        let columns = (mesh.vertex_count() as f32 - 60.0) / 6.0;
        assert!(
            columns <= body.w + 2.0,
            "a screenful of columns, not 20000 segments: {columns}"
        );
        assert!(!mesh.is_empty());
    }

    /// A live row is drawn by the shared renderer, so it inks what the other
    /// two ink: an offset signal is a band at its own level (never a fill from
    /// the baseline) and a signal that swings across zero is the solid body.
    /// Before the fold this module drew a hairline envelope of its own, so a
    /// live view never quite matched the stored view beside it.
    #[test]
    fn a_live_row_inks_what_every_other_trace_inks() {
        let row = Rect::new(0.0, 0.0, 100.0, 100.0);
        let draw = |samples: &[f32], min: f32, max: f32| {
            let mut mesh = Mesh::new();
            trace_row(
                &mut Draw::new(&mut mesh, &Metrics::default(), &Theme::default()),
                row,
                samples,
                1,
                0,
                (min, max),
                [1.0, 1.0, 1.0, 1.0],
                trace::Measure::Peak,
                trace::Measures::of(trace::Measure::Peak),
            );
            mesh.extent().expect("the row drew").h
        };
        let offset = vec![0.8f32; 4_000];
        assert!(
            draw(&offset, -1.0, 1.0) < row.h * 0.05,
            "an offset signal is a band where the samples are"
        );
        let swinging: Vec<f32> = (0..4_000)
            .map(|i| if i % 2 == 0 { 0.9 } else { -0.9 })
            .collect();
        assert!(
            draw(&swinging, -1.0, 1.0) > row.h * 0.8,
            "and a swinging one is the body its own data fills"
        );
    }

    /// **The colour changes where the level says, on whichever axis the column
    /// has.** The two levels are the core's; a decibel strip and a plain
    /// amplitude range put them at different heights and both are read the same
    /// way.
    #[test]
    fn a_columns_colour_is_placed_by_the_level_and_not_by_the_pixel() {
        let db = Scale::decibels();
        assert!(
            (db.warn - 0.7).abs() < 0.01,
            "-18 of 60 is seven tenths up: {}",
            db.warn
        );
        assert!(
            db.warn < db.amber && db.amber < db.hot && db.hot < 1.0,
            "{db:?}"
        );

        // **Every step is a blend, reached at its own mark.** A column at -12
        // dB is using its headroom and reads amber, and it blends on into red,
        // which it reaches at -6 and holds to the top: the last six decibels
        // are where a peak that grows any further clips.
        let theme = crate::host::theme::Theme::default();
        let at = |db_level: f32| {
            column_color(
                &theme,
                db,
                measure::meter_fraction_db(db_level, measure::METER_FLOOR_DB),
            )
        };
        assert_eq!(at(measure::METER_AMBER_DB), theme.meter_mid);
        let between = at(-9.0);
        for ((got, mid), high) in between.iter().zip(theme.meter_mid).zip(theme.meter_high) {
            assert!(
                (got - (mid + high) * 0.5).abs() < 1e-5,
                "half way from -12 to -6 is half way from amber to red: {between:?}"
            );
        }
        assert_eq!(at(measure::METER_HOT_DB), theme.meter_high, "red at -6");
        assert_eq!(at(-3.0), theme.meter_high);
        assert_eq!(at(0.0), theme.meter_high);

        let linear = Scale::amplitude(0.0, 1.0);
        assert!(
            linear.warn < 0.2 && (linear.hot - 0.5).abs() < 0.01,
            "on an amplitude axis -6 dB is half of unity: {linear:?}"
        );

        let theme = crate::host::theme::Theme::default();
        assert_eq!(
            column_color(&theme, db, db.warn * 0.5),
            theme.meter_low,
            "headroom is green all the way up to the alignment level"
        );
        let top = column_color(&theme, db, 1.0);
        assert!(
            top.iter()
                .zip(theme.meter_high)
                .all(|(a, b)| (a - b).abs() < 1e-5),
            "and red at the top: {top:?}"
        );
        let edge = column_color(&theme, db, (db.warn + db.amber) * 0.5);
        assert!(
            edge != theme.meter_low && edge != theme.meter_mid,
            "the step into the amber is a blend: {edge:?}"
        );
    }

    /// **A range across zero is a signed value, not a level.** Its column
    /// stands on the zero; a range that starts at or above zero, and a
    /// decibel axis, stand on the floor as before.
    #[test]
    fn a_range_across_zero_stands_on_the_zero() {
        let signed = MeterAxis::Linear {
            min: -1.0,
            max: 1.0,
        };
        assert!(signed.centred());
        assert_eq!(signed.origin(), 0.5);
        let level = MeterAxis::Linear { min: 0.0, max: 1.0 };
        assert!(!level.centred());
        assert_eq!(level.origin(), 0.0);
        assert!(!MeterAxis::Decibels { floor_db: -60.0 }.centred());
    }

    /// **A value below zero is a column below it.** At zero there is nothing
    /// but the well; at -0.5 on a -1..1 axis the column fills the quarter under
    /// the middle and nothing above it.
    #[test]
    fn a_centred_column_runs_down_from_the_zero() {
        let m = Metrics::default();
        let theme = crate::host::theme::Theme::default();
        let cell = Rect::new(0.0, 0.0, 8.0, 100.0);
        let mut mesh = crate::host::paint::Mesh::new();
        draw_column(&mut mesh, &m, &theme, cell, 0.0, 0.0, Scale::decibels());
        let well = mesh.vertex_count();
        mesh.clear();
        draw_column_from(
            &mut mesh,
            &m,
            &theme,
            cell,
            0.5,
            0.5,
            0.0,
            &Paint::scale(Scale::decibels(), &theme),
        );
        assert_eq!(
            mesh.vertex_count(),
            well,
            "a value at zero draws only the well"
        );
        mesh.clear();
        draw_column_from(
            &mut mesh,
            &m,
            &theme,
            cell,
            0.5,
            0.25,
            0.0,
            &Paint::scale(Scale::decibels(), &theme),
        );
        let (lo, hi) = mesh
            .positions()
            .skip(well as usize)
            .fold((f32::MAX, f32::MIN), |(lo, hi), (_, y)| {
                (lo.min(y), hi.max(y))
            });
        assert!(
            (lo - 50.0).abs() < 1e-3 && (hi - 75.0).abs() < 1e-3,
            "the column spans the zero down to -0.5: y {lo}..{hi}"
        );
    }

    /// **A column is drawn against its own empty space**, so the well is always
    /// there and the mark is drawn over it even when nothing is sounding.
    #[test]
    fn an_empty_meter_is_still_a_well() {
        let m = Metrics::default();
        let theme = crate::host::theme::Theme::default();
        let cell = Rect::new(0.0, 0.0, 8.0, 60.0);
        let mut mesh = crate::host::paint::Mesh::new();
        draw_column(&mut mesh, &m, &theme, cell, 0.0, 0.0, Scale::decibels());
        let empty = mesh.vertex_count();
        assert!(empty > 0, "the well is drawn");
        mesh.clear();
        draw_column(&mut mesh, &m, &theme, cell, 0.5, 0.9, Scale::decibels());
        assert!(
            mesh.vertex_count() > empty,
            "and a level and a mark are more than the well"
        );
    }
}

// ---- the whole meter: columns, held peaks, the clip lamp and the ladder ----

/// **What a meter's height measures.** A level is an amplitude and is read in
/// decibels; anything else a bus carries is read over the range the widget was
/// given, and the two are different axes rather than two settings of one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MeterAxis {
    /// Decibels, from `floor_db` up to full scale -- what a meter of a signal
    /// stands on. The floor is the reader's question: the 60 dB strip a mix is
    /// read on, or the dynamic range of the resolution the multitrack is rendered
    /// at (`clausters_core::measure::floor_db_for_bits`).
    Decibels { floor_db: f32 },
    /// A plain value over `min..max` -- a control bus carrying something that is
    /// not an amplitude, where a decibel would be a reading of nothing.
    Linear { min: f32, max: f32 },
}

impl MeterAxis {
    /// How high a column stands for `value`, over `0..1`.
    pub fn fraction(self, value: f32) -> f32 {
        match self {
            MeterAxis::Decibels { floor_db } => measure::meter_fraction(value, floor_db),
            MeterAxis::Linear { min, max } => fraction(value, min, max),
        }
    }

    /// **Whether the axis has a zero in the middle**: a plain range that
    /// crosses it, which is a signed value rather than a level. Its column
    /// stands on the zero and it has no held peak -- a peak is the loudest an
    /// amplitude has been, and a value below zero is not a quiet one.
    pub fn centred(self) -> bool {
        matches!(self, MeterAxis::Linear { min, max } if min < 0.0 && max > 0.0)
    }

    /// Where a value **in the axis' own units** falls: decibels on a decibel
    /// axis (what a zone is written in), the value itself on a plain one.
    /// [`fraction`](Self::fraction) takes a level, which on a decibel axis is
    /// an amplitude.
    pub fn fraction_of(self, value: f32) -> f32 {
        match self {
            MeterAxis::Decibels { floor_db } => measure::meter_fraction_db(value, floor_db),
            MeterAxis::Linear { min, max } => fraction(value, min, max),
        }
    }

    /// Where a column stands, as a fraction of the height: the floor, or the
    /// zero of a [`centred`](Self::centred) axis.
    pub fn origin(self) -> f32 {
        if self.centred() {
            self.fraction(0.0)
        } else {
            0.0
        }
    }

    /// Where the two colour levels fall on this axis.
    pub fn scale(self) -> Scale {
        match self {
            MeterAxis::Decibels { floor_db } => Scale::decibels_from(floor_db),
            MeterAxis::Linear { min, max } => Scale::amplitude(min, max),
        }
    }

    /// The reading, as the corner says it: decibels below full scale, or the
    /// value itself.
    pub fn readout(self, value: f32) -> String {
        match self {
            MeterAxis::Decibels { floor_db } => {
                let db = db_of(value);
                if db <= floor_db {
                    "-INF".to_string()
                } else {
                    format!("{db:.1}")
                }
            }
            MeterAxis::Linear { .. } => fmt(value),
        }
    }
}

/// An amplitude in decibels below full scale; `-INF` reads as a very negative
/// number rather than as one nothing can compare.
pub fn db_of(amplitude: f32) -> f32 {
    let a = amplitude.abs();
    if a <= 0.0 {
        f32::NEG_INFINITY
    } else {
        20.0 * a.log10()
    }
}

/// **What one channel of a meter reads this frame.**
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ChannelRead {
    /// The level now, in the axis' own units.
    pub level: f32,
    /// The held peak -- the mark that waits to be read.
    pub mark: f32,
    /// The latched over, if the lamp is lit: the loudest level seen since it
    /// lit, which is what the number in the lamp says.
    pub clipped: Option<f32>,
}

/// One meter, whole: the columns, their held peaks, the lamp over each and the
/// decibel ladder beside them.
pub(crate) struct MeterView<'a> {
    pub channels: &'a [ChannelRead],
    pub axis: MeterAxis,
    /// Whether the meter writes **numbers over its columns**: the reading at
    /// the foot and the figure a lit lamp raises. Off is a bare column, which
    /// is what a strip of meters in a header is.
    pub readout: bool,
    pub label: Option<&'a str>,
    /// Which side the numbers fall on, or `None` for a meter with no ladder.
    pub ruler: Option<crate::host::ruler::Side>,
    /// What the columns are coloured by.
    pub zones: &'a Zones,
}

/// **The share of a meter's height the clip lamp takes.** A lamp is a mark over
/// a column, so it is a proportion of the column and not a fixed strip: a tall
/// meter with a hairline for a lamp has a mark nobody notices, and a short one
/// with a fixed strip loses a chunk of its scale to it.
const LAMP_SHARE: f32 = 0.04;

/// **The narrowest a column may be squeezed to** before the drawing gives
/// something else up instead. Three device pixels still reads as a column;
/// below that it is a line.
const MIN_COLUMN: f32 = 3.0;

/// The height the clip lamp takes off the top of the body: [`LAMP_SHARE`] of
/// it, never thinner than a few hairlines and **never taller than one line of
/// caption**. A lamp is a *mark*, not a panel: what it has to say it says by
/// being lit, and the number it raises is written over the column on the plate
/// every overlaid line in this host is written on.
fn lamp_h(body_h: f32, m: &crate::host::metrics::Metrics) -> f32 {
    let floor = m.divider_w * 3.0;
    (body_h * LAMP_SHARE).clamp(floor, font::height(m.caption_scale).max(floor))
}

/// Draws a [`MeterView`] into `rect`.
///
/// The order the body is cut in is the drawing's whole argument. The **lamp
/// strip comes off the top first** and is reserved whether or not anything is
/// lit, because a lamp that pushed the column down as it lit would move the
/// picture at exactly the moment a reader is looking at it. The **ladder comes
/// off its side next**, and only if what is left is still wider than the strip
/// it took -- an element owns its space, and a meter squeezed to a few pixels
/// drops its numbers and stays a meter rather than becoming a ruler with no
/// column. What remains is shared by the channels, one column each.
pub(crate) fn draw_meter_view(d: &mut Draw, rect: Rect, view: &MeterView) {
    label_strip(d, view.label, rect);
    let m = d.m;
    let mut body = body_rect(rect, view.label.is_some(), m);
    if body.w <= 0.0 || body.h <= 0.0 || view.channels.is_empty() {
        return;
    }
    let lamps = matches!(view.axis, MeterAxis::Decibels { .. });
    let lamp = lamps.then(|| {
        let h = lamp_h(body.h, m).min(body.h * 0.5);
        body.y += h;
        body.h -= h;
        Rect::new(body.x, body.y - h, body.w, h)
    });
    // **The reading gets a strip of its own, at the bottom.** It used to be
    // written in the body's top corner, where the lamp's number also goes, so
    // on a narrow meter the two landed on each other. The bottom is also where
    // a meter's reading belongs: the loud end of the scale is the end a mark is
    // drawn at, and the number is read after it.
    // The strip's **height** is taken here, before anything else is laid out,
    // so the ladder's ticks and the columns end where the reading begins. Where
    // it is written is settled later, once the ladder has taken its side: the
    // number belongs under the **columns**, not under the ladder's last label.
    let value = {
        let h = super::controls::readout_h(m.caption_scale, m);
        (view.readout && body.h > h * 4.0).then(|| {
            body.h -= h;
            body.y + body.h
        })
    };
    let strip = view.ruler.and_then(|side| {
        let floor = match view.axis {
            MeterAxis::Decibels { floor_db } => floor_db as f64,
            MeterAxis::Linear { .. } => return None,
        };
        let want = crate::host::ruler::db_strip_w(floor, body.h, m);
        // **The columns keep their minimum and the ladder takes the rest.** The
        // rule was half the body, which was right while a meter stretched to
        // its cell and wrong the moment it asked for a width of its own: a
        // strip of numbers is wider than a few thin columns, so a meter at its
        // own natural width dropped the very ladder that width included. What a
        // column cannot give up is its own thin column.
        let least = view.channels.len() as f32 * MIN_COLUMN
            + (view.channels.len().saturating_sub(1)) as f32 * m.divider_w;
        if body.w - want < least {
            return None;
        }
        let at = match side {
            crate::host::ruler::Side::Left => {
                let x = body.x;
                body.x += want;
                x
            }
            crate::host::ruler::Side::Right => body.x + body.w - want,
        };
        body.w -= want;
        Some((Rect::new(at, body.y, want, body.h), side))
    });
    if body.w <= 0.0 || body.h <= 0.0 {
        return;
    }
    let paint = view.zones.paint(view.axis, d.theme);
    let n = view.channels.len();
    let gaps = (n - 1) as f32 * m.divider_w;
    let column = ((body.w - gaps) / n as f32).max(1.0);
    for (i, ch) in view.channels.iter().enumerate() {
        let x = body.x + i as f32 * (column + m.divider_w);
        let cell = Rect::new(x, body.y, column, body.h);
        let (mesh, m, theme) = d.parts();
        let mark = if view.axis.centred() {
            0.0
        } else {
            view.axis.fraction(ch.mark)
        };
        draw_column_from(
            mesh,
            m,
            theme,
            cell,
            view.axis.origin(),
            view.axis.fraction(ch.level),
            mark,
            &paint,
        );
        if let Some(lamp) = lamp {
            draw_lamp(
                d,
                Rect::new(x, lamp.y, column, lamp.h),
                ch.clipped.is_some(),
            );
        }
    }
    let (mesh, m, theme) = d.parts();
    mesh.border(body, m.divider_w, theme.accent);
    if let (Some((strip, side)), MeterAxis::Decibels { floor_db }) = (strip, view.axis) {
        let ticks = crate::host::ruler::db_ticks(floor_db as f64, body.h as f64, d.m);
        let edge = match side {
            crate::host::ruler::Side::Left => body.x,
            crate::host::ruler::Side::Right => body.x + body.w,
        };
        crate::host::ruler::draw_ticks_v_side(d, edge, strip, body, &ticks, side);
    }
    // **The lamps are per channel; the number is one, and it is written over
    // the columns.** Which channel was flattened is worth a lamp of its own,
    // but *how far past full scale* is a question about the signal -- and a
    // column is a few pixels wide, so a number per column would be a number
    // nobody can read and a column wide enough for one would be width spent on
    // nothing.
    if view.readout && lamp.is_some() {
        let worst = view
            .channels
            .iter()
            .filter_map(|c| c.clipped)
            .fold(f32::NEG_INFINITY, f32::max);
        draw_lamp_number(d, body, worst);
    }
    // The reading is the loudest channel's: one number for the widget, which is
    // the question a glance asks of a stereo pair. On a centred axis that is
    // the one furthest from zero, sign and all.
    if let Some(top) = value {
        let loudest = if view.axis.centred() {
            view.channels
                .iter()
                .map(|ch| ch.level)
                .fold(0.0f32, |acc, v| if v.abs() > acc.abs() { v } else { acc })
        } else {
            view.channels
                .iter()
                .fold(0.0f32, |acc, ch| acc.max(ch.level))
        };
        let text = view.axis.readout(loudest);
        let (mesh, m, theme) = d.parts();
        let w = font::width(&text, m.caption_scale).min(body.w);
        font::text_ellipsis(
            mesh,
            &text,
            body.x + (body.w - w) * 0.5,
            top + m.pad * 0.5,
            body.w,
            m.caption_scale,
            theme.text,
        );
    }
}

/// The lamp over one column: dark while nothing has clipped, lit red once
/// something has, and left lit until a hand puts it out.
fn draw_lamp(d: &mut Draw, cell: Rect, lit: bool) {
    let (mesh, _, theme) = d.parts();
    if cell.w <= 0.0 || cell.h <= 0.0 {
        return;
    }
    mesh.rect(
        cell,
        if lit {
            theme.meter_clip
        } else {
            theme.meter_field
        },
    );
}

/// **How far past full scale it went**, written over the columns -- and nothing
/// at all while nothing is lit.
///
/// In decibels *over* full scale rather than the level itself, because that is
/// the question a lit lamp raises: the server works in floating point, so a
/// signal that passed unity is not lost -- it is a signal that has to come down
/// by this much before anything converts it.
fn draw_lamp_number(d: &mut Draw, body: Rect, peak: f32) {
    let over = db_of(peak);
    if !over.is_finite() {
        return;
    }
    let text = format!("{:+.1}", over.max(0.0));
    // The caption scale on the translucent plate, centred at the top of the
    // column where the loud end of the scale is: the same line every other
    // drawing in this host writes over a picture. Not inside the lamp -- a lamp
    // big enough to hold a number is a panel, and a number scaled to a lamp is
    // the one part of a meter that changes size for no reason a reader could
    // name.
    let (scale, w, pad) = {
        let m = d.m;
        (
            m.caption_scale,
            font::width(&text, m.caption_scale),
            m.divider_w * 2.0,
        )
    };
    if w > body.w || font::height(scale) > body.h {
        return;
    }
    let x = body.x + (body.w - w) * 0.5;
    let color = d.theme.text;
    super::plate_text(d, &text, x, body.y + pad, body.w, scale, color);
}
