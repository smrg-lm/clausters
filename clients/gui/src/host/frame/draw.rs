//! What one frame *draws*: the mesh passes over the collected items.
//!
//! Three passes, in the order the picture is built. The live one reads sources
//! that change every frame (a shared-memory bus, a rolling scope history, a tap
//! window); the timeline one draws the editor views, their rulers and their
//! overlay chrome; the static one draws what is already fixed in the tree
//! (plots, envelopes, node trees, and the menu popup on top of everything).
//! Each works from the items [`super::items`] copied out, never from the host
//! tree.

use super::*;

/// Draws the time-ruler strip under `body` for the visible `nav` window
/// (aligned with the body, so its ticks sit under the samples they label even
/// when a vertical ruler indents the body).
pub(crate) fn draw_time_ruler(
    d: &mut Draw,
    rect: Rect,
    body: Rect,
    nav: &View,
    rate: f64,
    editor: &EditorProps,
) {
    if editor.ruler == Ruler::Off {
        return;
    }
    let metrics = d.m;
    let strip = Rect::new(body.x, body.y + body.h, body.w, (rect.h - body.h).max(0.0));
    if strip.h <= 2.0 || strip.w <= 0.0 {
        return;
    }
    let ticks = ruler::time_ticks(
        nav.start,
        nav.len,
        strip.w as f64,
        rate,
        time_unit(editor),
        metrics,
    );
    ruler::draw_ticks_h(d, strip, &ticks);
}

/// The pixel domain a free-standing `timeruler` labels: its own rect, indented
/// on the left by its **group's** gutter so the ticks line up with the bodies
/// of whatever it is stacked with — a lane's clips, a roll's grid, a heavy
/// view's trace. Zero height, so [`draw_time_ruler`] lays the strip over the
/// widget's whole box.
pub(crate) fn ruler_strip_body(rect: Rect, indent: f32) -> Rect {
    let hw = indent.min(rect.w);
    Rect::new(rect.x + hw, rect.y, (rect.w - hw).max(0.0), 0.0)
}

/// The ticks of a trace's vertical strip, over the value domain its geometry
/// was built through and the visible window `(y0, y_len)`.
///
/// **The amplitude axis *is* the full-scale domain.** `dbfs`, `bits` and
/// `percent` are units of full scale — a rung at -6 dB or at 2^15 says nothing
/// over a range of, say, `[0, 400]` — so an element that names a domain of its
/// own is ruled as a plain value axis instead, over the slice its window shows.
/// The default domain keeps the amplitude ladders untouched, which is every
/// view that has ever been drawn.
fn amp_or_value_ticks(
    domain: (f32, f32),
    unit: RulerY,
    bit_depth: u32,
    lane_h: f64,
    (y0, y_len): (f64, f64),
    m: &Metrics,
) -> Vec<ruler::Tick> {
    if domain == crate::waveform::DEFAULT_DOMAIN {
        return ruler::amp_ticks(unit, lane_h, bit_depth, y0, y_len, m);
    }
    // The visible slice of the domain. `value_to_display` is affine, so a
    // value's fraction of this slice is exactly its fraction of the window —
    // the ticks land on the samples they name with no margin arithmetic here.
    let lo = crate::waveform::display_to_value(y0, domain.0, domain.1);
    let hi = crate::waveform::display_to_value(y0 + y_len, domain.0, domain.1);
    ruler::value_ticks(lo as f64, hi as f64, lane_h, m)
}

/// What a timeline view's **vertical** axis measures, which is what its cursor
/// readout names. It used to be an `Option<(f64, FreqScale, f64)>` where
/// `None` silently meant "a waveform, in amplitude" — and amplitude is the
/// default of a value domain, not the only one, so the absence had to become a
/// case that carries its own answer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Vertical {
    /// A trace over its value domain (`min`/`max`,
    /// [`crate::waveform::DEFAULT_DOMAIN`] for full-scale amplitude).
    Value((f32, f32)),
    /// A time-frequency texture: the Nyquist, the display scale and the log
    /// floor its shader maps through.
    Frequency(f64, FreqScale, f64),
}

/// Draws the selection overlay and playhead of one timeline view — both read
/// off `chrome`, its navigation group's shared state — plus its cursor readout
/// when the pointer is inside the body. `lanes` is the lane
/// count of the stacked layout (1 when overlaid), so the vertical readout is
/// computed within the lane under the cursor.
#[allow(clippy::too_many_arguments)] // one chrome pass, all inputs by value
pub(super) fn draw_editor_overlay(
    mesh: &mut Mesh,
    item: &TimelineItem,
    body: Rect,
    chrome: &GroupState,
    rate: f64,
    lanes: usize,
    inputs: &FrameInputs,
    vertical: Vertical,
    theme: &Theme,
) {
    let m = inputs.metrics;
    let nav = &chrome.nav;
    mesh.border(body, m.divider_w, theme.view_frame);
    // Selection: a translucent band with hard edges, clipped to the body.
    if let Some((start, len)) = chrome.selection() {
        let x0 = sample_to_x(start, nav, body).clamp(body.x, body.x + body.w);
        let x1 = sample_to_x(start + len, nav, body).clamp(body.x, body.x + body.w);
        if x1 > x0 {
            mesh.rect(
                Rect::new(x0, body.y, x1 - x0, body.h),
                with_alpha(theme.selection, 0.18),
            );
            mesh.rect(
                Rect::new(x0, body.y, m.divider_w, body.h),
                with_alpha(theme.selection, 0.75),
            );
            mesh.rect(
                Rect::new(x1 - m.divider_w, body.y, m.divider_w, body.h),
                with_alpha(theme.selection, 0.75),
            );
        }
    }
    // Playhead: the engine clock relative to the widget's origin while playing,
    // else the static cursor of a located, stopped transport.
    if let Some(pos) = chrome.head_at(inputs.world.sample_clock)
        && pos >= nav.start
        && pos <= nav.start + nav.len
    {
        let x = sample_to_x(pos, nav, body);
        mesh.rect(Rect::new(x, body.y, m.trace_w, body.h), theme.playhead);
    }
    // Cursor readout: time (per the ruler mode) plus value/frequency (per the
    // vertical unit / frequency scale), in the body's bottom-right corner —
    // pure math over the view mapping, within the lane under the cursor.
    if let Some((cx, cy)) = inputs.world.cursor
        && body.contains(cx, cy)
    {
        let s = nav.start + nav.len * ((cx - body.x as f64) / body.w.max(1.0) as f64);
        let editor = &item.editor;
        let time = match editor.ruler {
            Ruler::Samples => ruler::readout_samples(s),
            Ruler::Beats => {
                let beats_per_px = nav.len / rate * editor.tempo / body.w.max(1.0) as f64;
                ruler::readout_beats(
                    s,
                    rate,
                    editor.tempo,
                    editor.beat_at,
                    editor.quant,
                    beats_per_px,
                )
            }
            _ => ruler::readout_time(s, rate, nav.len / rate / body.w.max(1.0) as f64),
        };
        let lane = lane_rect(body, lanes.max(1), lane_at(body, lanes.max(1), cy));
        let rel = ((cy - lane.y as f64) / lane.h.max(1.0) as f64).clamp(0.0, 1.0);
        // The cursor's height mapped through the visible vertical window into
        // an absolute display coordinate (0 = axis bottom) — so the readout
        // names exactly what is under the cursor at any vertical zoom/pan.
        let (y0, y_len) = editor.y_view();
        let display = y0 + (1.0 - rel) * y_len;
        let text = match vertical {
            // Spectrogram: invert the shader's display→bin mapping at the
            // cursor's height for the frequency under it.
            Vertical::Frequency(nyquist, scale, f_lo) => {
                let f = ruler::display_to_hz(display, nyquist, scale, f_lo);
                format!("{time}  {} HZ", f.round() as i64)
            }
            // A trace: the value at the cursor's height within its lane, read
            // through the same domain the geometry was built with — so the
            // readout names what is under the pointer whatever range the
            // element declared, and not an amplitude it never drew.
            Vertical::Value(domain) => {
                let v = crate::waveform::display_to_value(display, domain.0, domain.1);
                let v = v.clamp(domain.0.min(domain.1), domain.0.max(domain.1));
                let per_px = crate::waveform::value_per_display(domain.0, domain.1) * y_len
                    / lane.h.max(1.0) as f64;
                let value = if domain == crate::waveform::DEFAULT_DOMAIN {
                    ruler::readout_amp(v as f64, editor.ruler_y, editor.bit_depth, per_px)
                } else {
                    // A named domain is a plain value axis (see
                    // `amp_or_value_ticks`), so its readout is the number.
                    ruler::fmt_decimal(v as f64, per_px)
                };
                format!("{time}  {value}")
            }
        };
        let w = font::width(&text, m.caption_scale);
        let x = (body.x + body.w - w - m.pad).max(body.x);
        let y = body.y + body.h - font::height(m.caption_scale) - 3.0;
        font::text(
            mesh,
            &text,
            x,
            y.max(body.y),
            m.caption_scale,
            with_alpha(theme.text, 0.9),
        );
    }
}

/// Draws the timeline views (waveform/spectrogram): the field, time ruler and
/// the vertical-ruler strip go into the base `mesh` (under the GPU view); the
/// border, lane dividers, selection, playhead and cursor readout into `over`
/// (drawn over it).
pub(super) fn draw_timeline_meshes(
    mesh: &mut Mesh,
    over: &mut Mesh,
    collected: &Collected,
    waveforms: &HashMap<i32, WaveformSlot>,
    spectrograms: &HashMap<i32, SpectrogramSlot>,
    inputs: &FrameInputs,
    theme: &Theme,
) {
    let m = inputs.metrics;
    // Timeline views (waveform/spectrogram): the field, time ruler and the
    // vertical-ruler strip go into the base mesh (under the GPU view); the
    // border, lane dividers, selection, playhead and cursor readout into the
    // overlay mesh (over it).
    for item in &collected.timeline_items {
        mesh.set_clip(item.clip);
        over.set_clip(item.clip);
        mesh.set_ink(item.ink);
        over.set_ink(item.ink);
        let th = item.theme.as_deref().unwrap_or(theme);
        // The body the element stated when it described its frame: one
        // rectangle, so the picture and the chrome around it agree.
        let body = item.body;
        mesh.rect(body, th.view_field);
        match &item.kind {
            TimelineKind::Waveform {
                overlay: overlaid,
                domain,
                amp,
            } => {
                let Some(slot) = waveforms.get(&item.id) else {
                    over.border(body, 1.0, th.view_frame);
                    continue;
                };
                let chrome = chrome_for(inputs, item.id, &item.editor, || {
                    View::full(slot.view.total_samples())
                });
                let nav = chrome.nav;
                let rate = if item.editor.sample_rate > 0.0 {
                    item.editor.sample_rate
                } else {
                    inputs.world.sample_rate
                };
                draw_time_ruler(
                    &mut Draw::new(mesh, m, th),
                    item.rect,
                    body,
                    &nav,
                    rate,
                    &item.editor,
                );
                let lanes = slot.view.num_channels();
                // Overlaid traces share one lane (and one amplitude axis).
                let draw_lanes = if *overlaid { 1 } else { lanes };
                if item.editor.ruler_y != RulerY::Off {
                    // The window the element stated for this frame, which is
                    // the one its picture was uploaded at.
                    let (y0, y_len) = (amp.0, amp.1);
                    for ch in 0..draw_lanes {
                        let lane = lane_rect(body, draw_lanes, ch);
                        let ticks = amp_or_value_ticks(
                            *domain,
                            item.editor.ruler_y,
                            item.editor.bit_depth,
                            lane.h as f64,
                            (y0, y_len),
                            m,
                        );
                        ruler::draw_ticks_v(
                            &mut Draw::new(mesh, m, th),
                            body.x,
                            item.rect.x,
                            lane,
                            &ticks,
                        );
                    }
                }
                for ch in 1..draw_lanes {
                    let lane = lane_rect(body, draw_lanes, ch);
                    over.rect(Rect::new(lane.x, lane.y, lane.w, 1.0), th.lane_divider);
                }
                draw_editor_overlay(
                    &mut *over,
                    item,
                    body,
                    &chrome,
                    rate,
                    draw_lanes,
                    inputs,
                    Vertical::Value(*domain),
                    th,
                );
            }
            TimelineKind::Spectrogram { freq, look } => {
                let Some(slot) = spectrograms.get(&item.id) else {
                    over.border(body, 1.0, th.view_frame);
                    continue;
                };
                let chrome = chrome_for(inputs, item.id, &item.editor, || {
                    View::full(slot.total_samples())
                });
                let nav = chrome.nav;
                let (nyquist, f_lo) = slot
                    .views
                    .first()
                    .map(|v| (v.stft().nyquist() as f64, v.log_floor() as f64))
                    .unwrap_or((24_000.0, 20.0 / 24_000.0));
                let rate = if item.editor.sample_rate > 0.0 {
                    item.editor.sample_rate
                } else {
                    nyquist * 2.0
                };
                draw_time_ruler(
                    &mut Draw::new(mesh, m, th),
                    item.rect,
                    body,
                    &nav,
                    rate,
                    &item.editor,
                );
                let lanes = slot.views.len();
                for ch in 0..lanes {
                    let lane = lane_rect(body, lanes, ch);
                    if ch > 0 {
                        over.rect(
                            Rect::new(lane.x, lane.y, lane.w, m.divider_w),
                            th.lane_divider,
                        );
                    }
                    if item.editor.ruler_y != RulerY::Off {
                        let ticks = ruler::hz_ticks(
                            nyquist,
                            look.freq_scale,
                            f_lo,
                            lane.h as f64,
                            freq.0,
                            freq.1,
                            m,
                        );
                        ruler::draw_ticks_v(
                            &mut Draw::new(mesh, m, th),
                            body.x,
                            item.rect.x,
                            lane,
                            &ticks,
                        );
                    }
                }
                // The active scale, named over the view (the live views'
                // corner slot) — log/mel/bark are not tellable apart from
                // the tick spacing at a glance.
                crate::host::graphics::corner_text(
                    &mut Draw::new(over, m, th),
                    ruler::scale_tag(look.freq_scale),
                    body,
                );
                draw_editor_overlay(
                    &mut *over,
                    item,
                    body,
                    &chrome,
                    rate,
                    lanes,
                    inputs,
                    Vertical::Frequency(nyquist, look.freq_scale, f_lo),
                    th,
                );
            }
        }
    }
}

/// Draws the remaining static views (plots, envelope editors, node trees,
/// multitrack lanes and piano rolls): pure mesh work with the host-tree borrow
/// already released. The flat geometry goes into `mesh`, the hover/selection/
/// playhead chrome into `over`.
pub(super) fn draw_static_meshes(
    mesh: &mut Mesh,
    over: &mut Mesh,
    collected: &Collected,
    inputs: &FrameInputs,
    theme: &Theme,
    tree: &Widget,
) {
    let m = inputs.metrics;
    // Where the shared time axis begins, per navigation group: a lane, a roll
    // and a free-standing ruler on one axis agree on it, whatever gutter each
    // would have reserved alone (see `timeline::group_indents`).
    // Static plots draw from their (already mapped) samples; node trees draw from
    // the model last read off the client leg. Both are pure mesh work with the
    // host-tree borrow already released.
    // Envelope editors are pure mesh work: the curve evaluated per pixel
    // column through the shared shape math, discs for the breakpoints.
    // Multitrack lanes: the window's tracks share one time axis (aligned
    // lanes), spanning the longest clip end; each lane's clips are placed on it.
    // The hit-test (`interact::clip_hit`) reads the same `window_nav`, so a clip
    // maps to the same pixels for drawing and dragging.
    // The free-standing time rulers: each labels its group's window, in a strip
    // of its own that no lane pays for. Drawn before the lanes so a lane's own
    // chrome still reads over it where both exist.
    for item in &collected.ruler_items {
        mesh.set_clip(item.clip);
        mesh.set_ink(item.ink);
        let th = item.theme.as_deref().unwrap_or(theme);
        let nav = chrome_for(inputs, item.id, &item.editor, || track::window_nav(tree)).nav;
        let rate = if item.editor.sample_rate > 0.0 {
            item.editor.sample_rate
        } else {
            inputs.world.sample_rate
        };
        // The strip is indented by its **group's** gutter, so its ticks stand
        // over the samples they label whatever it is stacked with -- the whole
        // point of a ruler that is not inside one.
        let body = ruler_strip_body(item.rect, item.indent);
        draw_time_ruler(
            &mut Draw::new(mesh, m, th),
            item.rect,
            body,
            &nav,
            rate,
            &item.editor,
        );
    }
    if !collected.track_items.is_empty() {
        // The lanes navigate as a group (linked by default across a window), so
        // the axis zooms and pans as one; the full span is the fallback for a
        // lane not yet in a group.
        let full = track::window_nav(tree);
        for item in &collected.track_items {
            mesh.set_clip(item.clip);
            over.set_clip(item.clip);
            mesh.set_ink(item.ink);
            over.set_ink(item.ink);
            let th = item.theme.as_deref().unwrap_or(theme);
            let chrome = chrome_for(inputs, item.id, &item.editor, || full);
            let nav = chrome.nav;
            let ruler_on = item.editor.ruler != Ruler::Off;
            let indent = item.indent;
            track::draw(
                &mut Draw::new(mesh, m, th),
                item.rect,
                item.label.as_deref(),
                &item.header,
                ruler_on,
                indent,
            );
            let body = track::lane_body(item.rect, ruler_on, indent, m);
            // The lane's own time ruler, in the strip the lane body reserved —
            // the same tick math the timeline views use, over the shared axis.
            if ruler_on {
                let rate = if item.editor.sample_rate > 0.0 {
                    item.editor.sample_rate
                } else {
                    inputs.world.sample_rate
                };
                draw_time_ruler(
                    &mut Draw::new(mesh, m, th),
                    item.rect,
                    body,
                    &nav,
                    rate,
                    &item.editor,
                );
            }
            // The playhead, over the clips: the engine clock as a timeline
            // position (`playhead_at` anchors timeline sample 0 to a clock
            // value), so it sweeps the lane as the composition plays.
            if let Some(pos) = chrome.head_at(inputs.world.sample_clock)
                && let Some(x) = track::playhead_x(body, &nav, pos)
            {
                over.rect(Rect::new(x, body.y, 1.5, body.h), th.playhead);
            }
        }
    }
    // The clips over their lanes, and their bodies over them: separate passes
    // rather than one nested loop, because that *is* the z order — every clip's
    // box is under every body, and the layout emitted them in that order.
    for item in &collected.clip_items {
        mesh.set_clip(item.clip);
        mesh.set_ink(item.ink);
        let th = item.theme.as_deref().unwrap_or(theme);
        track::draw_clip(&mut Draw::new(mesh, m, th), item.rect);
    }
    for item in &collected.clip_bodies {
        mesh.set_clip(item.clip);
        mesh.set_ink(item.ink);
        let th = item.theme.as_deref().unwrap_or(theme);
        match &item.kind {
            // An **element** body draws itself, told which coordinate system
            // it is in: the same element that stands on its own elsewhere,
            // handed the clip's axis instead of its own. The container decides
            // that here — once — rather than each element asking where it is.
            WidgetKind::Custom(el) => el.draw(
                &mut Draw::new(mesh, &item.metrics, th),
                &Ctx {
                    world: &inputs.world,
                    metrics: &item.metrics,
                    rect: item.rect,
                    // A body is drawn against its clip's axis, which starts at
                    // the clip's own left edge: no group gutter here.
                    indent: 0.0,
                    clip: item.clip,
                    scale: item.scale,
                    time: Some(TimeSpace::of(item.local, item.dur)),
                    // A body carries no id, so nothing can address the keyboard
                    // at it: the focus is the clip's or nobody's.
                    focused: false,
                },
            ),
            kind => track::draw_body_widget(
                &mut Draw::new(mesh, m, th),
                kind,
                item.rect,
                &item.local,
                item.dur,
            ),
        }
    }
    // A clip's **name** and its **grips**, last and into the overlay: a body
    // drawn over them would bury them (the take's trace does, and the
    // time-frequency texture — a GPU pass after every mesh — hides them
    // outright), and a clip nobody can read is a rectangle. Same reason the
    // playhead and the selection live here.
    // **One** clip carries the grip, and which one depends on whether anything
    // is already held. Free, it is the topmost clip under the pointer: clips
    // overlap and the overlay is painted after every clip's box, so a covered
    // clip lighting its edge would draw over the clip covering it, announcing a
    // grab the press — which takes the topmost — would not give. Held, it is
    // the clip in hand, wherever the pointer has got to: a clip moves in snap
    // steps and the pointer does not, so the two part company between steps,
    // and a grip that follows the pointer there blinks out mid-drag and lights
    // up whatever the pointer drifted over.
    let pointer = inputs.world.cursor;
    let topmost = pointer.and_then(|(x, y)| {
        collected
            .clip_items
            .iter()
            .rposition(|item| item.rect.contains(x, y))
    });
    for (i, item) in collected.clip_items.iter().enumerate() {
        let grip = match inputs.grab {
            // Something else has the pointer: no clip offers anything.
            Grab::Other => None,
            Grab::Clip(id, side) if item.id == Some(id) => {
                let cx = pointer.map_or(item.rect.x + item.rect.w * 0.5, |(x, _)| {
                    // Clamped into the clip: a held clip keeps its grip lit
                    // even while the pointer is outside it.
                    (x as f32).clamp(item.rect.x, item.rect.x + item.rect.w)
                });
                match side {
                    Some(side) => track::clip_grip_on(item.rect, item.ends, m, side),
                    None => track::clip_grip_at(item.rect, item.ends, m, cx),
                }
            }
            Grab::Clip(..) => None,
            Grab::None => topmost.filter(|top| *top == i).and_then(|_| {
                let (x, _) = pointer?;
                track::clip_grip_at(item.rect, item.ends, m, x as f32)
            }),
        };
        if grip.is_none() && item.label.is_none() {
            continue;
        }
        over.set_clip(item.clip);
        over.set_ink(item.ink);
        let th = item.theme.as_deref().unwrap_or(theme);
        if let Some((rect, side)) = grip {
            track::draw_clip_grip(&mut Draw::new(over, m, th), rect, side);
        }
        let Some(label) = item.label.as_deref() else {
            continue;
        };
        track::draw_clip_label(&mut Draw::new(over, m, th), item.rect, label);
    }
}

/// The **overlays**: what an element draws outside its own rect, into the
/// overlay mesh after everything else in it.
///
/// Last on purpose, and in a walk of its own rather than in the collect pass:
/// an overlay covers whatever it opened over — the widgets in the base mesh,
/// the heavy views the GPU pass paints between the two meshes, and the editor
/// chrome already in this one. Which elements have one is declared
/// ([`Element::overlay_rect`](super::super::widget::Element::overlay_rect)), so
/// this asks the tree instead of being told.
pub(super) fn draw_element_overlays(
    over: &mut Mesh,
    placed: &[layout::Placed],
    inputs: &FrameInputs,
    theme: &Theme,
) {
    let m = inputs.metrics;
    for p in placed {
        let (WidgetKind::Custom(el), Some(_)) = (&p.widget.kind, p.widget.kind.overlay_rect())
        else {
            continue;
        };
        over.set_clip(None);
        over.set_ink(super::ink_of(p));
        let th = p.widget.theme.as_deref().unwrap_or(theme);
        el.overlay(
            &mut Draw::new(over, m, th),
            &Ctx {
                world: &inputs.world,
                metrics: m,
                rect: p.rect,
                indent: p.indent,
                clip: p.clip,
                scale: p.scale,
                time: None,
                focused: p.widget.id.is_some() && p.widget.id == inputs.focused,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::waveform::DEFAULT_DOMAIN;

    /// The vertical strip follows the domain the geometry was built through.
    /// Full scale keeps the amplitude ladders — dBFS rungs walking outward from
    /// a silence line — and a domain of the element's own is ruled as the plain
    /// value axis, because a rung at -6 dB says nothing over `[20, 20000]`.
    #[test]
    fn a_named_domain_is_ruled_as_a_value_axis() {
        let m = Metrics::default();
        let amp = amp_or_value_ticks(DEFAULT_DOMAIN, RulerY::Db, 16, 300.0, (0.0, 1.0), &m);
        assert!(
            amp.iter().any(|t| t.label.as_deref() == Some("-INF")),
            "full scale keeps the dBFS ladder: {amp:?}"
        );
        let named = amp_or_value_ticks((20.0, 20_000.0), RulerY::Db, 16, 300.0, (0.0, 1.0), &m);
        assert!(
            !named.iter().any(|t| t.label.as_deref() == Some("-INF")),
            "a named domain is not an amplitude axis: {named:?}"
        );
        assert!(
            named.iter().any(|t| t
                .label
                .as_deref()
                .and_then(|l| l.replace(['k', 'K'], "").parse::<f64>().ok())
                .is_some_and(|v| v > 1.0)),
            "it labels its own values: {named:?}"
        );
    }

    /// The ticks of a named domain land on the values they name: a value's
    /// fraction of the visible slice is its fraction of the lane, margin
    /// included, so the strip and the geometry cannot disagree.
    #[test]
    fn a_named_domains_ticks_land_where_the_geometry_puts_the_value() {
        let m = Metrics::default();
        let domain = (0.0f32, 400.0f32);
        let window = (0.2, 0.5);
        let ticks = amp_or_value_ticks(domain, RulerY::Norm, 16, 400.0, window, &m);
        assert!(!ticks.is_empty());
        for t in &ticks {
            let Some(v) = t.label.as_deref().and_then(|l| l.parse::<f64>().ok()) else {
                continue;
            };
            // Where the geometry puts that value, as a fraction of the lane.
            let d = crate::waveform::value_to_display(v as f32, domain.0, domain.1);
            let geometry = (d - window.0) / window.1;
            assert!(
                (geometry - t.frac).abs() < 1e-6,
                "tick {v} at {}, geometry at {geometry}",
                t.frac
            );
        }
    }
}
