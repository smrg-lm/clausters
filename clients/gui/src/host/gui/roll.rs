//! Piano-roll editing gestures: the press dispatch (notes, velocity lane, OSC
//! lane, marquee), the keyboard block operations (delete, copy/cut/paste,
//! quantize) and live MIDI note painting. The note model itself — insert, move,
//! selection sets, quantize — is the shared [`pianoroll`] module; this file is
//! the winit-side dispatch that drives it.

use crate::host::interact;
use crate::host::pianoroll;
use crate::host::widget::WidgetKind;
use crate::viewport::View;

use super::app::App;
use super::input::Drag;

impl App {
    /// Handles a plain (non-Shift) press on a `pianoroll`: start a note
    /// move/resize (a **selected** note moves the whole selection), a velocity
    /// drag (over a selected note, the whole selection's) or an OSC-marker
    /// drag; Ctrl+click adds or removes a note/marker; Alt+click toggles a note
    /// in/out of the multi-note selection; a press on empty grid drags the
    /// marquee — the shared time selection restricted in pitch, which fills the
    /// selected set. The edit-back gestures, native-only (the browser keeps
    /// display + `/gui_set` parity).
    #[allow(clippy::too_many_arguments)] // one press: a hit, two modifiers, a cursor
    pub(super) fn pianoroll_press(
        &mut self,
        def_id: i32,
        id: i32,
        h: &interact::PianoRollHit,
        ctrl: bool,
        alt: bool,
        cx: f64,
        cy: f64,
    ) {
        let nav = View {
            start: h.nav.start,
            len: h.nav.len,
        };
        match h.region {
            interact::PrRegion::Grid => {
                // Alt+click toggles a note in/out of the multi-note selection
                // (a non-rectangular selection, one note at a time).
                if alt {
                    if let Some(nh) = h.note {
                        interact::pianoroll_state_edit(&mut self.host, def_id, id, |_, sel| {
                            pianoroll::toggle_selected(sel, nh.index);
                        });
                        self.redraw(def_id);
                    }
                    return;
                }
                if ctrl {
                    match h.note {
                        // Ctrl+click on a note removes it (the selection's
                        // indices shift down past it).
                        Some(nh) => {
                            interact::pianoroll_state_edit(
                                &mut self.host,
                                def_id,
                                id,
                                |notes, sel| {
                                    pianoroll::remove_note(notes, nh.index);
                                    *sel = pianoroll::selection_after_removal(sel, nh.index);
                                },
                            );
                        }
                        // Ctrl+click on empty grid adds a note there, then drags
                        // its end to set the length until release.
                        None => {
                            let time = interact::snap(
                                pianoroll::time_at(h.grid, &nav, 0.0, cx as f32),
                                h.snap,
                            )
                            .max(0.0);
                            let pitch = pianoroll::y_to_pitch(cy as f32, h.lo, h.hi, h.grid)
                                .round()
                                .clamp(h.lo, h.hi);
                            let dur = if h.snap > 0.0 {
                                h.snap
                            } else {
                                (h.nav.len * 0.05).max(1.0)
                            };
                            let index = interact::pianoroll_notes_edit(
                                &mut self.host,
                                def_id,
                                id,
                                |notes| {
                                    pianoroll::insert_note(
                                        notes,
                                        pianoroll::Note::new(time, dur, pitch),
                                    )
                                },
                            );
                            if let Some(index) = index {
                                self.set_drag(
                                    def_id,
                                    Drag::Note {
                                        id,
                                        index,
                                        part: pianoroll::NotePart::End,
                                        grid: h.grid,
                                        nav_start: h.nav.start,
                                        nav_len: h.nav.len,
                                        lo: h.lo,
                                        hi: h.hi,
                                        press_time: time,
                                        orig_start: time,
                                        orig_dur: dur,
                                        snap: h.snap,
                                    },
                                );
                            }
                        }
                    }
                    self.host.sync_track_totals();
                    self.emit_notes(def_id, id);
                    self.redraw(def_id);
                    return;
                }
                match h.note {
                    // Move (body) or resize (edge) the note under the cursor.
                    // Grabbing the body of a **selected** note moves the whole
                    // selection rigidly; grabbing an unselected one drops the
                    // selection first (the single-note gesture, as before).
                    Some(nh) => {
                        let press_time = pianoroll::time_at(h.grid, &nav, 0.0, cx as f32);
                        if nh.part == pianoroll::NotePart::Body {
                            let orig = interact::pianoroll_state_edit(
                                &mut self.host,
                                def_id,
                                id,
                                |notes, sel| {
                                    if !sel.contains(&nh.index) {
                                        sel.clear();
                                        return Vec::new();
                                    }
                                    // The grabbed note's snapshot leads (the
                                    // snap anchor).
                                    let mut idx = sel.clone();
                                    idx.retain(|&i| i != nh.index);
                                    idx.insert(0, nh.index);
                                    idx.iter()
                                        .filter_map(|&i| {
                                            notes.get(i).map(|n| (i, n.start, n.pitch))
                                        })
                                        .collect::<Vec<_>>()
                                },
                            )
                            .unwrap_or_default();
                            if !orig.is_empty() {
                                let press_pitch =
                                    pianoroll::y_to_pitch(cy as f32, h.lo, h.hi, h.grid);
                                self.set_drag(
                                    def_id,
                                    Drag::NoteBlock {
                                        id,
                                        grid: h.grid,
                                        nav_start: h.nav.start,
                                        nav_len: h.nav.len,
                                        lo: h.lo,
                                        hi: h.hi,
                                        press_time,
                                        press_pitch,
                                        snap: h.snap,
                                        orig,
                                    },
                                );
                                return;
                            }
                        }
                        let (orig_start, orig_dur) =
                            self.note_at(def_id, id, nh.index).unwrap_or((0.0, 0.0));
                        self.set_drag(
                            def_id,
                            Drag::Note {
                                id,
                                index: nh.index,
                                part: nh.part,
                                grid: h.grid,
                                nav_start: h.nav.start,
                                nav_len: h.nav.len,
                                lo: h.lo,
                                hi: h.hi,
                                press_time,
                                orig_start,
                                orig_dur,
                                snap: h.snap,
                            },
                        );
                    }
                    // Empty grid: plain drag selects (the heavy-view
                    // convention), and the marquee doubles as the note
                    // selection — the time span restricted in pitch.
                    None => {
                        if let Some((start, len, _)) = self.timeline_nav(id) {
                            let anchor = interact::sample_at(
                                start,
                                len,
                                h.grid.x as f64,
                                h.grid.w as f64,
                                cx,
                            );
                            self.set_selection(def_id, id, anchor, anchor);
                            let anchor_pitch = pianoroll::y_to_pitch(cy as f32, h.lo, h.hi, h.grid);
                            // The marquee restarts: the previous set drops.
                            interact::pianoroll_state_edit(&mut self.host, def_id, id, |_, sel| {
                                sel.clear()
                            });
                            self.set_drag(
                                def_id,
                                Drag::SelectNotes {
                                    id,
                                    grid: h.grid,
                                    nav_start: start,
                                    nav_len: len,
                                    lo: h.lo,
                                    hi: h.hi,
                                    anchor,
                                    anchor_pitch,
                                },
                            );
                            self.redraw(def_id);
                        }
                    }
                }
            }
            interact::PrRegion::Velocity => {
                if let Some(nh) = h.note {
                    // Over a **selected** note the whole selection's velocities
                    // nudge together (relative, from a press snapshot); over an
                    // unselected one the single bar follows the cursor.
                    let orig =
                        interact::pianoroll_state_edit(&mut self.host, def_id, id, |notes, sel| {
                            if !sel.contains(&nh.index) {
                                return Vec::new();
                            }
                            sel.iter()
                                .filter_map(|&i| notes.get(i).map(|n| (i, n.velocity)))
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    if !orig.is_empty() {
                        let lane = h.region_rect;
                        self.set_drag(
                            def_id,
                            Drag::VelocityBlock {
                                id,
                                lane,
                                press_velocity: pianoroll::velocity_at(lane, cy),
                                orig,
                            },
                        );
                        return;
                    }
                    self.set_drag(
                        def_id,
                        Drag::Velocity {
                            id,
                            index: nh.index,
                            lane: h.region_rect,
                        },
                    );
                }
            }
            interact::PrRegion::Osc => {
                if ctrl {
                    match h.osc_index {
                        Some(index) => {
                            interact::pianoroll_osc_edit(&mut self.host, def_id, id, |osc| {
                                if index < osc.len() {
                                    osc.remove(index);
                                }
                            });
                        }
                        None => {
                            let time = interact::snap(
                                pianoroll::time_at(h.grid, &nav, 0.0, cx as f32),
                                h.snap,
                            )
                            .max(0.0);
                            interact::pianoroll_osc_edit(&mut self.host, def_id, id, |osc| {
                                osc.push(pianoroll::OscMark { time, label: None });
                            });
                        }
                    }
                    self.host.sync_track_totals();
                    self.emit_osc(def_id, id);
                    self.redraw(def_id);
                } else if let Some(index) = h.osc_index {
                    self.set_drag(
                        def_id,
                        Drag::OscMark {
                            id,
                            index,
                            grid: h.grid,
                            nav_start: h.nav.start,
                            nav_len: h.nav.len,
                            snap: h.snap,
                        },
                    );
                }
            }
        }
    }

    /// A piano-roll note's current `(start, dur)` in the host tree.
    fn note_at(&self, def_id: i32, id: i32, index: usize) -> Option<(f64, f64)> {
        match &self.host.window_def(def_id)?.find(id)?.kind {
            WidgetKind::PianoRoll { notes, .. } => notes.get(index).map(|n| (n.start, n.dur)),
            _ => None,
        }
    }

    /// `q` over a piano-roll: quantize the selected notes' onsets (all of them
    /// when nothing is selected) to the widget's `snap` grid — the same grid a
    /// drag snaps to. Durations are kept; a roll with no grid is left alone.
    /// (The client-side counterpart, in beats over the model, is the Python
    /// `Timeline.quantize` — the standalone host cannot reach it, hence both.)
    pub(super) fn quantize_roll(&mut self, def_id: i32) {
        let Some((cx, cy)) = self.windows.get(&def_id).map(|w| w.cursor) else {
            return;
        };
        let Some((id, _rect, WidgetKind::PianoRoll { snap, .. })) = self.hit(def_id, cx, cy) else {
            return;
        };
        let moved = interact::pianoroll_state_edit(&mut self.host, def_id, id, |notes, sel| {
            pianoroll::quantize_notes(notes, sel, snap)
        })
        .unwrap_or(false);
        if moved {
            self.host.sync_track_totals();
            self.emit_notes(def_id, id);
            self.redraw(def_id);
        }
    }

    /// Ctrl+C / Ctrl+X over a piano-roll: copy the selected notes to the host
    /// clipboard, normalized to the block's first onset (a cut also removes
    /// them). The clipboard is host-wide, so a block travels between rolls and
    /// windows. A no-op when the cursor is elsewhere or nothing is selected.
    pub(super) fn copy_selected_notes(&mut self, def_id: i32, cut: bool) {
        let Some((cx, cy)) = self.windows.get(&def_id).map(|w| w.cursor) else {
            return;
        };
        let Some((id, _rect, WidgetKind::PianoRoll { .. })) = self.hit(def_id, cx, cy) else {
            return;
        };
        let copied = interact::pianoroll_state_edit(&mut self.host, def_id, id, |notes, sel| {
            let clip = pianoroll::copy_notes(notes, sel);
            if cut && !clip.is_empty() {
                pianoroll::remove_notes(notes, sel);
                sel.clear();
            }
            clip
        })
        .unwrap_or_default();
        if copied.is_empty() {
            return;
        }
        self.clipboard = copied;
        if cut {
            self.host.sync_track_totals();
            self.emit_notes(def_id, id);
            self.redraw(def_id);
        }
    }

    /// Ctrl+V over a piano-roll: paste the clipboard with its first onset at
    /// the cursor's time (snapped to the note grid), original pitches and
    /// spread kept. The pasted block becomes the new selection, ready to drag
    /// into place.
    pub(super) fn paste_notes_at_cursor(&mut self, def_id: i32) {
        if self.clipboard.is_empty() {
            return;
        }
        let Some((cx, cy)) = self.windows.get(&def_id).map(|w| w.cursor) else {
            return;
        };
        let (fb_w, fb_h) = self.fb(def_id);
        let Some(h) = interact::pianoroll_hit(&self.host, def_id, fb_w, fb_h, cx, cy) else {
            return;
        };
        let Some((id, _rect, WidgetKind::PianoRoll { .. })) = self.hit(def_id, cx, cy) else {
            return;
        };
        let nav = View {
            start: h.nav.start,
            len: h.nav.len,
        };
        let at = interact::snap(pianoroll::time_at(h.grid, &nav, 0.0, cx as f32), h.snap);
        let clip = self.clipboard.clone();
        interact::pianoroll_state_edit(&mut self.host, def_id, id, |notes, sel| {
            *sel = pianoroll::paste_notes(notes, &clip, at);
        });
        self.host.sync_track_totals();
        self.emit_notes(def_id, id);
        self.redraw(def_id);
    }

    /// Delete/Backspace: remove every selected note of the piano-roll under the
    /// cursor — the block delete (Ctrl+click removes one). A no-op when the
    /// cursor is elsewhere or nothing is selected.
    pub(super) fn delete_selected_notes(&mut self, def_id: i32) {
        let Some((cx, cy)) = self.windows.get(&def_id).map(|w| w.cursor) else {
            return;
        };
        let Some((id, _rect, WidgetKind::PianoRoll { .. })) = self.hit(def_id, cx, cy) else {
            return;
        };
        let removed = interact::pianoroll_state_edit(&mut self.host, def_id, id, |notes, sel| {
            if sel.is_empty() {
                return false;
            }
            pianoroll::remove_notes(notes, sel);
            sel.clear();
            true
        })
        .unwrap_or(false);
        if removed {
            self.host.sync_track_totals();
            self.emit_notes(def_id, id);
            self.redraw(def_id);
        }
    }
}

#[cfg(feature = "midi")]
impl App {
    /// Drain the virtual input port and paint each note event into every
    /// `midi_in` roll. A note-on inserts a **held** note — at the running
    /// playhead (live recording), or at the step cursor when the transport is
    /// stopped (step entry) — and the matching note-off closes it: the real
    /// held duration in playhead mode, or a grid step (advancing the cursor
    /// once all keys are up) in step mode.
    pub(super) fn drain_midi(&mut self, rolls: &[(i32, i32)]) {
        let mut events = Vec::new();
        if let Some(input) = &self.midi_in {
            while let Some(msg) = input.poll() {
                if let Some(ev) = clausters_midi::parse_note(&msg) {
                    events.push(ev);
                }
            }
        }
        if events.is_empty() {
            return;
        }
        for &(def_id, id) in rolls {
            for &ev in &events {
                self.paint_note(def_id, id, ev);
            }
            self.host.sync_track_totals();
            self.emit_notes(def_id, id);
            self.redraw(def_id);
        }
    }

    /// Paint one live note event into a roll (see [`App::drain_midi`]).
    fn paint_note(&mut self, def_id: i32, id: i32, ev: clausters_midi::NoteEvent) {
        use clausters_midi::NoteEvent;
        let playhead = self.playhead_sample(def_id, id);
        let snap = self.roll_snap(def_id, id);
        // The painted length: the note grid, else a visible sliver of the view
        // (the Ctrl+click default) — note-off then sets the real duration.
        let dur = if snap > 0.0 {
            snap
        } else {
            self.timeline_nav(id)
                .map_or(1.0, |(_, len, _)| (len * 0.05).max(1.0))
        };
        match ev {
            NoteEvent::On {
                channel,
                pitch,
                velocity,
            } => {
                let pos = match playhead {
                    Some(p) => interact::snap(p, snap).max(0.0),
                    None => *self.step.entry((def_id, id)).or_insert(0.0),
                };
                let index = interact::pianoroll_notes_edit(&mut self.host, def_id, id, |notes| {
                    pianoroll::insert_note(
                        notes,
                        pianoroll::Note {
                            start: pos,
                            dur,
                            pitch: pitch as f32,
                            velocity: velocity as i32,
                            channel: channel as i32,
                        },
                    )
                });
                if let Some(index) = index {
                    self.held.insert((def_id, id, channel, pitch), index);
                }
            }
            NoteEvent::Off { channel, pitch } => {
                let Some(index) = self.held.remove(&(def_id, id, channel, pitch)) else {
                    return;
                };
                if let Some(now) = playhead {
                    // Live recording: the key was held this long.
                    interact::pianoroll_notes_edit(&mut self.host, def_id, id, |notes| {
                        if let Some(n) = notes.get_mut(index) {
                            n.dur = (now - n.start).max(1.0);
                        }
                    });
                } else if !self
                    .held
                    .keys()
                    .any(|(d, w, _, _)| (*d, *w) == (def_id, id))
                {
                    // Step entry: the last key up advances the cursor a grid
                    // (a chord steps once).
                    *self.step.entry((def_id, id)).or_insert(0.0) += dur;
                }
            }
        }
    }
}
