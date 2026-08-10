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
pub(super) fn draw_time_ruler(
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

/// The visible MIDI pitch window `[lo, hi]` of a piano-roll: the widget's
/// `[min, max]` axis sliced by the vertical display window (`y_start`/`y_len`,
/// `0` = the low pitch at the bottom), so pitch zoom/pan holds the same way the
/// heavy views' amplitude/frequency windows do.
pub(super) fn pitch_window(item: &PianoRollItem) -> (f32, f32) {
    let (y0, yl) = item.editor.y_view();
    let span = (item.max - item.min) as f64;
    let lo = item.min as f64 + y0 * span;
    let hi = item.min as f64 + (y0 + yl) * span;
    (lo as f32, hi as f32)
}

/// Draws a `pianoroll`: keyboard gutter, note grid, the velocity/OSC lanes and
/// the time ruler into `mesh`; the selection band and the playhead into `over`.
/// Everything rides `chrome`, its navigation group's shared window, selection
/// and playhead, so it zooms/pans/plays in lockstep with linked sibling views.
#[allow(clippy::too_many_arguments)] // one view's flat draw inputs
pub(super) fn draw_pianoroll_item(
    d: &mut Draw,
    over: &mut Mesh,
    item: &PianoRollItem,
    chrome: &GroupState,
    rate: f64,
    sample_clock: f64,
    cursor: Option<(f64, f64)>,
    indent: f32,
) {
    let (m, theme) = (d.m, d.theme);
    let nav = &chrome.nav;
    let ruler_on = item.editor.ruler != Ruler::Off;
    let r = pianoroll::regions(
        item.rect,
        ruler_on,
        item.osc_lane,
        item.velocity_lane,
        indent,
        m,
    );
    let (lo, hi) = pitch_window(item);
    pianoroll::draw_grid_background(d, r.grid, lo, hi);
    pianoroll::draw_notes(
        d,
        r.grid,
        r.grid,
        nav,
        0.0,
        &item.notes,
        lo,
        hi,
        true,
        &item.selected,
    );
    pianoroll::draw_keyboard(d, r.keyboard, lo, hi);
    if item.osc_lane {
        pianoroll::draw_osc_lane(d, r.osc, nav, 0.0, &item.osc);
    }
    if item.velocity_lane {
        pianoroll::draw_velocity_lane(d, r.velocity, nav, 0.0, &item.notes);
    }
    if let Some(t) = &item.label {
        font::text(
            d.mesh,
            t,
            r.grid.x + m.pad,
            r.grid.y + 2.0,
            m.caption_scale,
            theme.ruler_text,
        );
    }
    if ruler_on {
        // The ruler strip sits under the grid, aligned to the grid's x-range —
        // build the "body" `draw_time_ruler` derives the strip from.
        let ruler_body = Rect::new(r.grid.x, item.rect.y, r.grid.w, r.ruler.y - item.rect.y);
        draw_time_ruler(d, item.rect, ruler_body, nav, rate, &item.editor);
    }
    // Selection band over the grid.
    if let Some((start, len)) = chrome.selection() {
        let x0 = sample_to_x(start, nav, r.grid).clamp(r.grid.x, r.grid.x + r.grid.w);
        let x1 = sample_to_x(start + len, nav, r.grid).clamp(r.grid.x, r.grid.x + r.grid.w);
        if x1 > x0 {
            over.rect(
                Rect::new(x0, r.grid.y, x1 - x0, r.grid.h),
                with_alpha(theme.selection, 0.18),
            );
            over.rect(
                Rect::new(x0, r.grid.y, 1.0, r.grid.h),
                with_alpha(theme.selection, 0.75),
            );
            over.rect(
                Rect::new(x1 - 1.0, r.grid.y, 1.0, r.grid.h),
                with_alpha(theme.selection, 0.75),
            );
        }
    }
    // Playhead: swept by the engine clock while playing, else the static cursor.
    if let Some(pos) = chrome.head_at(sample_clock)
        && pos >= nav.start
        && pos <= nav.start + nav.len
    {
        let x = sample_to_x(pos, nav, r.grid);
        over.rect(Rect::new(x, r.grid.y, m.trace_w, r.grid.h), theme.playhead);
    }
    // Cursor readout: the note name (the pitch under the cursor, via the core's
    // MIDI-note spelling) and the time (per the ruler mode), in the grid's
    // bottom-right corner — pure math over the view mapping.
    if let Some((cx, cy)) = cursor
        && r.grid.contains(cx, cy)
    {
        let pitch = pianoroll::y_to_pitch(cy as f32, lo, hi, r.grid).round() as i32;
        let s = nav.start + nav.len * ((cx - r.grid.x as f64) / r.grid.w.max(1.0) as f64);
        let time = match item.editor.ruler {
            Ruler::Samples => ruler::readout_samples(s),
            Ruler::Beats => ruler::readout_beats(
                s,
                rate,
                item.editor.tempo,
                item.editor.beat_at,
                item.editor.quant,
                nav.len / rate * item.editor.tempo / r.grid.w.max(1.0) as f64,
            ),
            _ => ruler::readout_time(s, rate, nav.len / rate / r.grid.w.max(1.0) as f64),
        };
        let text = format!("{}  {time}", clausters_core::scale::note_name(pitch));
        let w = font::width(&text, m.caption_scale);
        font::text(
            over,
            &text,
            r.grid.x + r.grid.w - w - m.pad,
            r.grid.y + r.grid.h - font::height(m.caption_scale) - 2.0,
            m.caption_scale,
            theme.ruler_text,
        );
    }
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
    nyquist_scale: Option<(f64, FreqScale, f64)>,
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
        let text = match nyquist_scale {
            // Spectrogram: invert the shader's display→bin mapping at the
            // cursor's height for the frequency under it.
            Some((nyquist, scale, f_lo)) => {
                let f = ruler::display_to_hz(display, nyquist, scale, f_lo);
                format!("{time}  {} HZ", f.round() as i64)
            }
            // Waveform: the amplitude at the cursor's height within its lane,
            // in the vertical ruler's unit.
            None => {
                let amp = (2.0 * display - 1.0) / crate::waveform::AMP_MARGIN as f64;
                let amp = amp.clamp(-1.0, 1.0);
                let amp_per_px =
                    2.0 * y_len / crate::waveform::AMP_MARGIN as f64 / lane.h.max(1.0) as f64;
                let value = ruler::readout_amp(amp, editor.ruler_y, editor.bit_depth, amp_per_px);
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

/// Draws the live widgets (meters, control- and audio-rate scopes, phasescopes
/// and spectra) into the base mesh: each reads its source (shared-memory bus,
/// the tick's rolling history, the stored tap window, the analysis states) and
/// draws the current value — the tick advances the state, a repaint only draws.
pub(super) fn draw_live_meshes(
    mesh: &mut Mesh,
    collected: &Collected,
    scopes: &HashMap<i32, VecDeque<f32>>,
    tap_windows: &HashMap<i32, live::TapWindow>,
    spectra: &HashMap<i32, Vec<SpectrumState>>,
    inputs: &FrameInputs,
    theme: &Theme,
) {
    let m = inputs.metrics;
    // Meters and scopes read their control bus straight from shared memory each
    // frame (zero messages); the scope keeps a per-widget rolling history in this
    // window's state.
    // The history is advanced on the frame tick (`advance_scopes`), not here, so a
    // repaint only ever *draws* the current samples — never adds one.
    for item in &collected.scope_rects {
        mesh.set_clip(item.clip);
        let th = item.theme.as_deref().unwrap_or(theme);
        let samples: Vec<f32> = scopes
            .get(&item.id)
            .map(|h| h.iter().copied().collect())
            .unwrap_or_default();
        meters::draw_scope(
            &mut Draw::new(mesh, m, th),
            item.rect,
            &samples,
            item.min,
            item.max,
            item.label.as_deref(),
        );
    }
    // Audio-rate scopes likewise draw the triggered multichannel window stored
    // on the tick (`live::update_tap_windows`); an empty one draws just the
    // framed field.
    let empty_window = live::TapWindow::default();
    for item in &collected.wave_rects {
        mesh.set_clip(item.clip);
        let th = item.theme.as_deref().unwrap_or(theme);
        let window = tap_windows.get(&item.id).unwrap_or(&empty_window);
        meters::draw_wave(
            &mut Draw::new(mesh, m, th),
            item.rect,
            &meters::WaveParams {
                window,
                min: item.min,
                max: item.max,
                window_ms: item.window_ms,
                trigger: item.trigger,
                overlay: item.overlay,
                ruler: item.ruler,
                ruler_y: item.ruler_y,
                label: item.label.as_deref(),
            },
        );
    }
    // Phasescopes draw the interleaved L/R window the tick stored (the same
    // `tap_windows` map, keyed by their own ids); spectra draw the per-bin
    // curves the tick folded into their per-channel analysis states.
    for item in &collected.phase_rects {
        mesh.set_clip(item.clip);
        let th = item.theme.as_deref().unwrap_or(theme);
        let inter = tap_windows
            .get(&item.id)
            .map(|w| w.samples.as_slice())
            .unwrap_or(&[]);
        phasescope::draw_phasescope(
            &mut Draw::new(mesh, m, th),
            item.rect,
            inter,
            item.label.as_deref(),
        );
    }
    for item in &collected.spectrum_rects {
        mesh.set_clip(item.clip);
        let th = item.theme.as_deref().unwrap_or(theme);
        if let Some(states) = spectra.get(&item.id) {
            spectrum::draw_spectrum(
                &mut Draw::new(mesh, m, th),
                item.rect,
                states,
                &spectrum::SpectrumParams {
                    sample_rate: inputs.world.sample_rate,
                    fft_size: item.fft_size,
                    db_floor: item.db_floor,
                    db_ceil: item.db_ceil,
                    freq_scale: item.freq_scale,
                    peak_hold: item.peak_hold,
                    ruler: item.ruler,
                    ruler_y: item.ruler_y,
                    x_view: item.x_view,
                    label: item.label.as_deref(),
                },
            );
        }
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
        let th = item.theme.as_deref().unwrap_or(theme);
        let body = timeline_body(item.rect, &item.editor, item.indent, m);
        mesh.rect(body, th.view_field);
        match &item.kind {
            TimelineKind::Waveform { overlay: overlaid } => {
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
                    let (y0, y_len) = item.editor.y_view();
                    for ch in 0..draw_lanes {
                        let lane = lane_rect(body, draw_lanes, ch);
                        let ticks = ruler::amp_ticks(
                            item.editor.ruler_y,
                            lane.h as f64,
                            item.editor.bit_depth,
                            y0,
                            y_len,
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
                    &mut *over, item, body, &chrome, rate, draw_lanes, inputs, None, th,
                );
            }
            TimelineKind::Spectrogram { freq_scale, .. } => {
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
                            *freq_scale,
                            f_lo,
                            lane.h as f64,
                            item.editor.y_view().0,
                            item.editor.y_view().1,
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
                meters::value_text(
                    &mut Draw::new(over, m, th),
                    ruler::scale_tag(*freq_scale),
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
                    Some((nyquist, *freq_scale, f_lo)),
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
    for item in &collected.plot_rects {
        mesh.set_clip(item.clip);
        over.set_clip(item.clip);
        let th = item.theme.as_deref().unwrap_or(theme);
        let params = item.params();
        plot::draw(&mut Draw::new(mesh, m, th), item.rect, &params);
        // The hover readout (hairline + the value under the cursor) rides the
        // overlay mesh, like the editor views' chrome.
        if let Some(cursor) = inputs.world.cursor {
            plot::draw_readout(&mut Draw::new(over, m, th), item.rect, &params, cursor);
        }
    }
    // Envelope editors are pure mesh work: the curve evaluated per pixel
    // column through the shared shape math, discs for the breakpoints.
    for item in &collected.bpf_rects {
        mesh.set_clip(item.clip);
        let th = item.theme.as_deref().unwrap_or(theme);
        bpf::draw(
            &mut Draw::new(mesh, m, th),
            item.rect,
            &item.points,
            item.min,
            item.max,
            item.duration,
            item.exp,
            item.label.as_deref(),
        );
    }
    // Multitrack lanes: the window's tracks share one time axis (aligned
    // lanes), spanning the longest clip end; each lane's clips are placed on it.
    // The hit-test (`interact::clip_hit`) reads the same `window_nav`, so a clip
    // maps to the same pixels for drawing and dragging.
    // The free-standing time rulers: each labels its group's window, in a strip
    // of its own that no lane pays for. Drawn before the lanes so a lane's own
    // chrome still reads over it where both exist.
    for item in &collected.ruler_items {
        mesh.set_clip(item.clip);
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
        let th = item.theme.as_deref().unwrap_or(theme);
        track::draw_clip(&mut Draw::new(mesh, m, th), item.rect);
    }
    for item in &collected.clip_bodies {
        mesh.set_clip(item.clip);
        let th = item.theme.as_deref().unwrap_or(theme);
        track::draw_body_widget(
            &mut Draw::new(mesh, m, th),
            &item.kind,
            item.rect,
            &item.local,
            item.dur,
        );
    }
    // A clip's **name**, last and into the overlay: a body drawn over it would
    // bury it (the take's trace does, and the time-frequency texture — a GPU
    // pass after every mesh — hides it outright), and a clip nobody can read is
    // a rectangle. Same reason the playhead and the selection live here.
    for item in &collected.clip_items {
        let Some(label) = item.label.as_deref() else {
            continue;
        };
        over.set_clip(item.clip);
        let th = item.theme.as_deref().unwrap_or(theme);
        track::draw_clip_label(&mut Draw::new(over, m, th), item.rect, label);
    }
    // Piano-roll views: flat geometry (keyboard/grid/lanes/ruler) into the base
    // mesh, selection/playhead into the overlay. Each draws through its
    // navigation group's shared window (a linked pianoroll zooms/pans with its
    // siblings), falling back to its own content extent when in no group.
    for item in &collected.pianoroll_items {
        mesh.set_clip(item.clip);
        over.set_clip(item.clip);
        let th = item.theme.as_deref().unwrap_or(theme);
        let chrome = chrome_for(inputs, item.id, &item.editor, || {
            let mut span = 0.0f64;
            for n in &item.notes {
                span = span.max(n.start + n.dur);
            }
            for m in &item.osc {
                span = span.max(m.time);
            }
            View::full(span.ceil().max(1.0) as usize)
        });
        let rate = if item.editor.sample_rate > 0.0 {
            item.editor.sample_rate
        } else {
            inputs.world.sample_rate
        };
        draw_pianoroll_item(
            &mut Draw::new(mesh, m, th),
            over,
            item,
            &chrome,
            rate,
            inputs.world.sample_clock,
            inputs.world.cursor,
            item.indent,
        );
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
        let th = p.widget.theme.as_deref().unwrap_or(theme);
        el.overlay(
            &mut Draw::new(over, m, th),
            &Ctx {
                world: &inputs.world,
                metrics: m,
                rect: p.rect,
                scale: p.scale,
                time: None,
            },
        );
    }
}
