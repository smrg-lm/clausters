//! `notes` — the editor-grade piano roll: a keyboard gutter, a note grid, a
//! velocity lane and an event lane, placed on a navigation group's shared time
//! axis.
//!
//! **The leaf that is placed on somebody else's axis and edits what is drawn on
//! it**, which is why it is the last but one of the port. Everything it draws
//! is mapped through [`Ctx::time`] — the group's window, its shared selection
//! and where its playhead stands — so a roll linked to a lane zooms, pans and
//! plays with it, and the same element fills a clip's [`Notes`](BodyRole::Notes)
//! body by being handed the clip's axis instead.
//!
//! Its drags are all **snapshotted**: the press records the note it grabbed and
//! that note's position, and every step is measured against the *current* axis
//! rather than a press-time copy of it. That is not a stylistic choice — a drag
//! held past the edge of a lane asks the machine to keep scrolling
//! ([`Take::edge_scroll`](crate::host::widget::element::Take::edge_scroll)), so
//! the window moves under the drag and a snapshot
//! of it would freeze the note where the axis used to be.
//!
//! **Two things it cannot do for itself**, and it asks for both the way `keys`
//! asks for a voice. The **time selection** a marquee sweeps is the navigation
//! group's — every linked view follows it — so the element names the span and
//! the machine writes it ([`Events::and_select`]). And **live MIDI** arrives
//! from a device only a front can open: the element declares [`Needs::midi`]
//! and paints what comes back ([`Element::midi`]), keeping the held keys and
//! the step cursor that used to live in two maps on the native front.

use clausters_core::osc::OscType;
use serde_json::{Map, Value};

use crate::host::graphics::pianoroll;
use crate::host::layout::Rect;
use crate::host::metrics::Metrics;
use crate::host::paint::Draw;
use crate::host::widget::element::{
    BodyRole, Claim, Ctx, Element, Events, Input, Key, KeyInput, MidiNote, Needs, TimeSpace,
};
use crate::host::widget::parse::{
    self, label, number, number_f64, parse_notes, parse_osc, set_f, set_label, truthy,
};
use crate::host::widget::{EditorProps, GestureMap, Ruler};
use crate::host::{font, ruler};
use crate::viewport::View;

/// The default pitch compass of a roll: the range of an 88-key piano.
const PITCH_MIN: f32 = 21.0;
const PITCH_MAX: f32 = 108.0;

/// The shortest note a resize may leave, in axis units.
const MIN_DUR: f64 = 1.0;

/// A piano roll. `selected`, `drag`, `held` and `step` are native view state —
/// the gestures and the MIDI leg build them and no `/gui_set` writes them.
#[derive(Debug, Clone)]
pub struct Notes {
    notes: Vec<pianoroll::Note>,
    osc: Vec<pianoroll::OscMark>,
    /// The multi-note selection (note indices). It clears when a script
    /// replaces `notes`, since the indices would dangle over the new list.
    selected: Vec<usize>,
    min: f32,
    max: f32,
    snap: f64,
    velocity_lane: bool,
    osc_lane: bool,
    midi_in: bool,
    label: Option<String>,
    editor: EditorProps,
    drag: Option<Drag>,
    /// The live-MIDI keys currently down: `(channel, pitch)` and the note each
    /// one is writing into.
    held: Vec<((i32, i32), usize)>,
    /// Where step entry writes the next note when the transport is stopped.
    step: f64,
}

/// What a held press on a roll is doing. Each carries the **press-time data**
/// it is measured from and no geometry: the rectangle and the axis arrive with
/// every step, because both may move under the drag.
#[derive(Debug, Clone)]
enum Drag {
    /// One note moving in time and pitch, or one of its edges resizing it.
    Note {
        index: usize,
        part: pianoroll::NotePart,
        press_time: f64,
        orig_start: f64,
        orig_dur: f64,
    },
    /// The whole selection moving rigidly: `orig` is `(index, start, pitch)`
    /// per selected note, the grabbed note leading (it is the snap anchor).
    Block {
        press_time: f64,
        press_pitch: f32,
        orig: Vec<(usize, f64, f32)>,
    },
    /// One velocity bar following the cursor's height.
    Velocity { index: usize },
    /// Every selected velocity nudged by one delta from a press snapshot.
    VelocityBlock {
        press_velocity: i32,
        orig: Vec<(usize, i32)>,
    },
    /// An event marker sliding along the time axis.
    OscMark { index: usize },
    /// The marquee: the shared time selection swept from `anchor`, restricted
    /// in pitch when the press was on the grid (`pitch` is `None` on the
    /// strips under it, which read time alone).
    Marquee { anchor: f64, pitch: Option<f32> },
}

/// Which region of a roll a press landed on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Region {
    Grid,
    Velocity,
    Osc,
    /// The time-ruler strip, or anything else on the axis: it reads a time and
    /// nothing else.
    Axis,
}

pub(super) fn build(
    props: &Map<String, Value>,
    _blobs: &[Vec<u8>],
) -> Result<Box<dyn Element>, String> {
    Ok(Box::new(from_props(props)))
}

/// The props a `notes` node carries, read once — shared by the constructor and
/// by the tests beside it.
fn from_props(props: &Map<String, Value>) -> Notes {
    let osc = parse_osc(props);
    Notes {
        notes: parse_notes(props),
        // The velocity lane is on by default; the event lane shows when there
        // are events or it is explicitly asked for (so an empty lane can still
        // be opened to author them).
        velocity_lane: props.get("velocity").and_then(truthy).unwrap_or(true),
        osc_lane: props
            .get("osc_lane")
            .and_then(truthy)
            .unwrap_or(!osc.is_empty()),
        osc,
        selected: Vec::new(),
        min: number(props, "min", PITCH_MIN),
        max: number(props, "max", PITCH_MAX),
        snap: number_f64(props, "snap", 0.0).max(0.0),
        midi_in: props.get("midi_in").and_then(truthy).unwrap_or(false),
        label: label(props),
        editor: EditorProps::parse(props, crate::host::widget::RulerY::Off),
        drag: None,
        held: Vec::new(),
        step: 0.0,
    }
}

/// A clip's **roll** body: the note events over a pitch window, with no chrome
/// of its own. `None` when the clip's props carry no notes.
///
/// The pitch window defaults to the roll's own compass rather than to an
/// amplitude range — a pitch axis of `[-1, 1]` would clamp every note to the
/// clip's top edge, silently, since nothing about the drawing would say why.
pub(crate) fn body(props: &Map<String, Value>) -> Option<Notes> {
    let notes = parse_notes(props);
    if notes.is_empty() {
        return None;
    }
    Some(Notes {
        notes,
        min: number(props, "min", PITCH_MIN),
        max: number(props, "max", PITCH_MAX),
        ..empty_body()
    })
}

/// An **empty** body, for a clip growing a roll it was not built with.
pub(crate) fn empty_body() -> Notes {
    Notes {
        notes: Vec::new(),
        osc: Vec::new(),
        selected: Vec::new(),
        min: PITCH_MIN,
        max: PITCH_MAX,
        snap: 0.0,
        velocity_lane: false,
        osc_lane: false,
        midi_in: false,
        label: None,
        editor: EditorProps::body(),
        drag: None,
        held: Vec::new(),
        step: 0.0,
    }
}

impl Notes {
    /// The regions this placement is split into — the same call the drawing and
    /// the hit-test both make, so a note is grabbed by the pixels it is
    /// painted on.
    fn regions(&self, rect: Rect, indent: f32, m: &Metrics) -> pianoroll::Regions {
        pianoroll::regions(
            rect,
            self.editor.ruler != Ruler::Off,
            self.osc_lane,
            self.velocity_lane,
            indent,
            m,
        )
    }

    /// The visible MIDI pitch window `[lo, hi]`: the `[min, max]` compass sliced
    /// by the vertical display window, so a pitch zoom holds the way the heavy
    /// views' amplitude and frequency windows do.
    fn pitch_window(&self) -> (f32, f32) {
        let (y0, yl) = self.editor.y_view();
        let span = (self.max - self.min) as f64;
        let lo = self.min as f64 + y0 * span;
        (lo as f32, (lo + yl * span) as f32)
    }

    /// The axis this roll is drawn against: the container's when it was placed
    /// on one, else its own content spanned over the body — the fallback a view
    /// that has not joined a group yet still draws through.
    fn view(&self, time: Option<TimeSpace>) -> View {
        match time {
            Some(t) => t.view,
            None => View::full(self.span().ceil().max(1.0) as usize),
        }
    }

    /// How far this roll's own content reaches on the axis: the end of its last
    /// note and of its last event.
    fn span(&self) -> f64 {
        let notes = self.notes.iter().map(|n| n.start + n.dur.max(0.0));
        let events = self.osc.iter().map(|m| m.time);
        notes.chain(events).fold(0.0f64, f64::max)
    }

    /// The `"notes"` edit-back payload: the tag plus the flat `start dur pitch
    /// velocity channel` quintuple list — the wire form the roll and the clip
    /// share, in the owner's own units.
    fn notes_event(&self) -> Events {
        let mut args = vec![OscType::String("notes".into())];
        for n in &self.notes {
            args.push(OscType::Float(n.start as f32));
            args.push(OscType::Float(n.dur as f32));
            args.push(OscType::Float(n.pitch));
            args.push(OscType::Int(n.velocity));
            args.push(OscType::Int(n.channel));
        }
        Events::message(args)
    }

    /// The `"osc"` edit-back payload: the tag plus the flat `time label` pairs
    /// (an empty string when a marker has no label).
    fn osc_event(&self) -> Events {
        let mut args = vec![OscType::String("osc".into())];
        for m in &self.osc {
            args.push(OscType::Float(m.time as f32));
            args.push(OscType::String(m.label.clone().unwrap_or_default()));
        }
        Events::message(args)
    }

    /// The axis position a cursor x maps back to, through the grid.
    fn time_at(&self, grid: Rect, nav: &View, x: f64) -> f64 {
        pianoroll::time_at(grid, nav, 0.0, x as f32)
    }

    /// Where a press landed, resolved against the placement it was drawn at.
    fn hit(&self, at: (f64, f64), input: &Input) -> Hit {
        let r = self.regions(input.rect, input.indent, input.metrics);
        let nav = self.view(input.time);
        let (lo, hi) = self.pitch_window();
        let (fx, fy) = (at.0 as f32, at.1 as f32);
        if self.osc_lane && r.osc.contains(at.0, at.1) {
            let osc = nearest(r.osc, &nav, self.osc.iter().map(|m| m.time), fx);
            return Hit {
                region: Region::Osc,
                rect: r.osc,
                grid: r.grid,
                nav,
                lo,
                hi,
                note: None,
                osc,
            };
        }
        if self.velocity_lane && r.velocity.contains(at.0, at.1) {
            // A velocity press picks the note whose bar it is nearest; it rides
            // as a body hit so one arm reads the index either way.
            let note =
                nearest(r.velocity, &nav, self.notes.iter().map(|n| n.start), fx).map(|index| {
                    pianoroll::NoteHit {
                        index,
                        part: pianoroll::NotePart::Body,
                    }
                });
            return Hit {
                region: Region::Velocity,
                rect: r.velocity,
                grid: r.grid,
                nav,
                lo,
                hi,
                note,
                osc: None,
            };
        }
        let region = if r.grid.contains(at.0, at.1) {
            Region::Grid
        } else {
            Region::Axis
        };
        Hit {
            region,
            rect: r.grid,
            grid: r.grid,
            nav,
            lo,
            hi,
            note: (region == Region::Grid)
                .then(|| pianoroll::note_hit(r.grid, &nav, 0.0, &self.notes, lo, hi, fx, fy))
                .flatten(),
            osc: None,
        }
    }

    /// Starts a marquee at `time`, dropping whatever the last sweep selected:
    /// the shared selection collapses to the press and the drag sweeps from
    /// there.
    fn start_marquee(&mut self, time: f64, pitch: Option<f32>) -> Claim {
        self.selected.clear();
        self.drag = Some(Drag::Marquee {
            anchor: time,
            pitch,
        });
        Claim::events(Events::none().and_select(time, time))
    }

    /// Inserts a note at `start`/`pitch` for the live-MIDI leg and the Ctrl+add
    /// gesture, returning its index.
    fn insert(&mut self, note: pianoroll::Note) -> usize {
        pianoroll::insert_note(&mut self.notes, note)
    }

    /// The length a note is painted with when nothing said otherwise: the note
    /// grid, else a visible sliver of the window.
    fn default_dur(&self, nav: &View) -> f64 {
        if self.snap > 0.0 {
            self.snap
        } else {
            (nav.len * 0.05).max(MIN_DUR)
        }
    }
}

/// Where a press landed and what it landed on.
struct Hit {
    region: Region,
    /// The region's own rectangle — what a velocity drag maps the cursor's
    /// height through.
    rect: Rect,
    grid: Rect,
    nav: View,
    lo: f32,
    hi: f32,
    note: Option<pianoroll::NoteHit>,
    osc: Option<usize>,
}

/// The index of the element whose time is nearest the cursor x, within a small
/// pixel tolerance — the picker both strips under the grid use.
fn nearest(lane: Rect, nav: &View, times: impl Iterator<Item = f64>, x: f32) -> Option<usize> {
    let to_x = |s: f64| lane.x + ((s - nav.start) / nav.len.max(1.0) * lane.w as f64) as f32;
    times
        .enumerate()
        .map(|(i, s)| (i, (to_x(s) - x).abs()))
        .filter(|(_, d)| *d <= 6.0)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

impl Element for Notes {
    fn set(&mut self, key: &str, v: &Value) -> bool {
        match key {
            // Arrays ride a `/gui_set` as their JSON — the scalar carrier a set
            // of a non-scalar always uses.
            "notes" => {
                self.notes = parse_notes(&parse::as_array_props("notes", v));
                // The indices would dangle over the new list.
                self.selected.clear();
                self.held.clear();
                true
            }
            "osc" => {
                self.osc = parse_osc(&parse::as_array_props("osc", v));
                true
            }
            "min" => set_f(&mut self.min, v),
            "max" => set_f(&mut self.max, v),
            "snap" => v.as_f64().map(|x| self.snap = x.max(0.0)).is_some(),
            "velocity" => truthy(v).map(|b| self.velocity_lane = b).is_some(),
            "osc_lane" => truthy(v).map(|b| self.osc_lane = b).is_some(),
            "midi_in" => truthy(v).map(|b| self.midi_in = b).is_some(),
            "label" => set_label(&mut self.label, v),
            _ => self.editor.apply(key, v),
        }
    }

    /// The whole picture, into the window's one mesh: the grid and its notes,
    /// the keyboard, the strips, and — for a roll standing on its own axis —
    /// the chrome of that axis over them.
    ///
    /// The chrome is drawn here rather than by the frame because a roll's is
    /// not the heavy views': its ruler sits under a grid with two strips
    /// between them, its selection band covers the grid alone, and its readout
    /// names a pitch. What it needs from outside is the axis' own three facts,
    /// and those arrive on [`Ctx::time`].
    fn draw(&self, d: &mut Draw, ctx: &Ctx) {
        let r = self.regions(ctx.rect, ctx.indent, ctx.metrics);
        let nav = self.view(ctx.time);
        let (lo, hi) = self.pitch_window();
        pianoroll::draw_grid_background(d, r.grid, lo, hi);
        pianoroll::draw_notes(
            d,
            r.grid,
            r.grid,
            &nav,
            0.0,
            &self.notes,
            lo,
            hi,
            true,
            &self.selected,
        );
        pianoroll::draw_keyboard(d, r.keyboard, lo, hi);
        if self.osc_lane {
            pianoroll::draw_osc_lane(d, r.osc, &nav, 0.0, &self.osc);
        }
        if self.velocity_lane {
            pianoroll::draw_velocity_lane(d, r.velocity, &nav, 0.0, &self.notes);
        }
        if let Some(text) = &self.label {
            let (mesh, m, theme) = d.parts();
            font::text(
                mesh,
                text,
                r.grid.x + m.pad,
                r.grid.y + 2.0,
                m.caption_scale,
                theme.ruler_text,
            );
        }
        let rate = self.rate(ctx.world.sample_rate);
        if self.editor.ruler != Ruler::Off {
            // The strip sits under the grid, aligned to the grid's x range —
            // the "body" the tick math derives it from.
            let body = Rect::new(r.grid.x, ctx.rect.y, r.grid.w, r.ruler.y - ctx.rect.y);
            crate::host::frame::draw_time_ruler(d, ctx.rect, body, &nav, rate, &self.editor);
        }
        self.draw_axis_chrome(d, ctx, r.grid, &nav, rate);
    }

    fn info(&self) -> Vec<(String, Value)> {
        // Each list as the JSON string its own `/gui_set` accepts: a query
        // gives back exactly what a set would take.
        vec![
            (
                "notes".into(),
                Value::from(pianoroll::notes_json(&self.notes).to_string()),
            ),
            (
                "osc".into(),
                Value::from(pianoroll::osc_json(&self.osc).to_string()),
            ),
        ]
    }

    fn needs(&self) -> Needs {
        Needs {
            // A roll follows the transport, so the window has to keep repainting
            // while one is running.
            clock: true,
            midi: self.midi_in,
            ..Needs::default()
        }
    }

    fn navigates_time(&self) -> bool {
        true
    }

    fn hover_readout(&self) -> bool {
        true
    }

    fn editor(&self) -> Option<&EditorProps> {
        Some(&self.editor)
    }

    fn editor_mut(&mut self) -> Option<&mut EditorProps> {
        Some(&mut self.editor)
    }

    fn body_role(&self) -> Option<BodyRole> {
        Some(BodyRole::Notes)
    }

    /// The keyboard gutter, which is the roll's own structural geometry. What
    /// it actually gets is its group's shared indent — this when it is alone on
    /// its axis, wider when it shares one with a lane.
    fn gutter(&self, _m: &Metrics) -> f32 {
        pianoroll::KEYBOARD_W
    }

    /// The grid is the body a sample maps into — not the rect minus its chrome,
    /// because the velocity and event strips are stacked *under* the grid and
    /// read the same axis. The keyboard gutter is always a vertical surface, so
    /// a wheel over it navigates the pitch window whatever `ruler_y` says.
    fn axis_body(&self, rect: Rect, indent: f32, m: &Metrics) -> Option<(Rect, bool)> {
        Some((self.regions(rect, indent, m).grid, true))
    }

    fn content_span(&self) -> Option<f64> {
        Some(self.span())
    }

    /// A clip's body: the same notes over the clip's own axis, with no keyboard,
    /// no strips and no chrome.
    fn draw_body(&self, d: &mut Draw, rect: Rect, local: &View, _dur: f64) {
        let (lo, hi) = (self.min, self.max);
        pianoroll::draw_notes(d, rect, rect, local, 0.0, &self.notes, lo, hi, false, &[]);
        pianoroll::draw_pitch_labels(d, rect, lo, hi);
    }

    /// The roll takes every press on its own axis (its notes, its strips, its
    /// marquee) and leaves the modifier that is the *container's* — Shift pans
    /// the window, which is the axis' gesture and not the picture's.
    fn gesture_map(&self) -> Option<GestureMap> {
        use crate::host::widget::GestureStep::{Element as El, Pan};
        Some(GestureMap::of_plans(&[El], &[Pan], &[El], &[El]))
    }

    fn press(&mut self, at: (f64, f64), input: &Input) -> Claim {
        let h = self.hit(at, input);
        // **A body claims its own parts and declines everywhere else.** Inside a
        // clip the rest of the rectangle means the clip's own drag (move it,
        // resize it), so a body grabs a note and hands back anything else.
        let is_body = input.time.is_some() && !self.navigable_placement(input);
        match h.region {
            Region::Grid => self.press_grid(&h, at, input, is_body),
            Region::Velocity => self.press_velocity(&h, at, is_body),
            Region::Osc => self.press_osc(&h, at, input, is_body),
            // The ruler strip and the slack beside the body: a time and nothing
            // else, so the sweep is time-only.
            Region::Axis if !is_body => {
                let t = self.time_at(h.grid, &h.nav, at.0);
                self.start_marquee(t, None)
            }
            Region::Axis => Claim::Decline,
        }
    }

    fn drag(&mut self, at: (f64, f64), input: &Input) -> Events {
        let r = self.regions(input.rect, input.indent, input.metrics);
        let nav = self.view(input.time);
        let (lo, hi) = self.pitch_window();
        let time = self.time_at(r.grid, &nav, at.0);
        match self.drag.clone() {
            Some(Drag::Note {
                index,
                part,
                press_time,
                orig_start,
                orig_dur,
            }) => {
                match part {
                    pianoroll::NotePart::Body => {
                        let start = snap_to(orig_start + (time - press_time), self.snap);
                        let pitch = pianoroll::y_to_pitch(at.1 as f32, lo, hi, r.grid);
                        pianoroll::move_note(&mut self.notes, index, start, pitch, lo, hi);
                        // `move_note` keeps the duration; re-assert it in case a
                        // set changed it under a running drag.
                        if let Some(n) = self.notes.get_mut(index) {
                            n.dur = orig_dur;
                        }
                    }
                    other => pianoroll::resize_note(
                        &mut self.notes,
                        index,
                        other,
                        snap_to(time, self.snap),
                        MIN_DUR,
                    ),
                }
                self.notes_event()
            }
            Some(Drag::Block {
                press_time,
                press_pitch,
                orig,
            }) => {
                // The grabbed note (the leading snapshot entry) snaps to the
                // grid and the whole selection moves rigidly by that delta —
                // the core clamps it as one.
                let dt = match orig.first() {
                    Some((_, s0, _)) => snap_to(s0 + (time - press_time), self.snap) - s0,
                    None => 0.0,
                };
                let dp = pianoroll::y_to_pitch(at.1 as f32, lo, hi, r.grid) - press_pitch;
                pianoroll::move_notes_from(&mut self.notes, &orig, dt, dp, lo, hi);
                self.notes_event()
            }
            Some(Drag::Velocity { index }) => {
                pianoroll::set_velocity(
                    &mut self.notes,
                    index,
                    pianoroll::velocity_at(r.velocity, at.1),
                );
                self.notes_event()
            }
            Some(Drag::VelocityBlock {
                press_velocity,
                orig,
            }) => {
                let dv = pianoroll::velocity_at(r.velocity, at.1) - press_velocity;
                pianoroll::nudge_velocities_from(&mut self.notes, &orig, dv);
                self.notes_event()
            }
            Some(Drag::OscMark { index }) => {
                if let Some(m) = self.osc.get_mut(index) {
                    m.time = snap_to(time, self.snap).max(0.0);
                }
                self.osc_event()
            }
            Some(Drag::Marquee { anchor, pitch }) => {
                // The time span drives the **group's** selection, which the
                // element asks for; the rectangle over it picks this roll's own
                // notes, which is state the element keeps.
                if let Some(p0) = pitch {
                    let p = pianoroll::y_to_pitch(at.1 as f32, lo, hi, r.grid);
                    self.selected = pianoroll::notes_in_rect(&self.notes, anchor, time, p0, p);
                }
                Events::none().and_select(anchor, time)
            }
            None => Events::none(),
        }
    }

    fn release(&mut self, _at: (f64, f64), _input: &Input) -> Events {
        self.drag = None;
        Events::none()
    }

    /// The block operations, addressed to whatever the pointer is over: `q`
    /// quantizes, Delete removes the selection, Ctrl+C/X/V move a block through
    /// the host-wide clipboard.
    ///
    /// They are keys rather than gestures because they act on the *selection*,
    /// which is already where the pointer has been. A key this element has no
    /// arm for falls through to the front's own shortcuts.
    fn key(&mut self, key: &Key, input: &mut KeyInput) -> Option<Events> {
        match key {
            // Quantize the selected onsets (all of them when nothing is
            // selected) to the note grid — the same grid a drag snaps to.
            Key::Char('q') | Key::Char('Q') if !input.mods.ctrl => {
                pianoroll::quantize_notes(&mut self.notes, &self.selected, self.snap)
                    .then(|| self.notes_event())
            }
            Key::Delete | Key::Backspace if !self.selected.is_empty() => {
                pianoroll::remove_notes(&mut self.notes, &self.selected);
                self.selected.clear();
                Some(self.notes_event())
            }
            // The clipboard is the host's one string, so a block travels
            // between rolls and windows — and rides it in the same JSON form a
            // `/gui_set notes` accepts, which is the carrier every non-scalar
            // already uses.
            Key::Char('c') | Key::Char('C') | Key::Char('x') | Key::Char('X')
                if input.mods.ctrl =>
            {
                let block = pianoroll::copy_notes(&self.notes, &self.selected);
                if block.is_empty() {
                    return None;
                }
                *input.clipboard = pianoroll::notes_json(&block).to_string();
                let cut = matches!(key, Key::Char('x') | Key::Char('X'));
                if !cut {
                    // A copy changed nothing, so it reports nothing — but it
                    // consumed the key.
                    return Some(Events::none());
                }
                pianoroll::remove_notes(&mut self.notes, &self.selected);
                self.selected.clear();
                Some(self.notes_event())
            }
            Key::Char('v') | Key::Char('V') if input.mods.ctrl => {
                let block = clipboard_notes(input.clipboard)?;
                // At the step cursor: a paste has no pointer, and the cursor is
                // where the roll is being written.
                let at = snap_to(self.step, self.snap).max(0.0);
                self.selected = pianoroll::paste_notes(&mut self.notes, &block, at);
                Some(self.notes_event())
            }
            _ => None,
        }
    }

    /// A live note: painted at the running playhead (recording), or on the step
    /// cursor when the transport is stopped (step entry — a chord shares one
    /// step, and the last key up advances it).
    fn midi(&mut self, note: MidiNote, playhead: Option<f64>) -> Option<Events> {
        let key = (note.channel, note.pitch);
        if note.on {
            let dur = if self.snap > 0.0 { self.snap } else { MIN_DUR };
            let start = match playhead {
                Some(p) => snap_to(p, self.snap).max(0.0),
                None => self.step,
            };
            let index = self.insert(pianoroll::Note {
                start,
                dur,
                pitch: note.pitch as f32,
                velocity: note.velocity,
                channel: note.channel,
            });
            self.held.push((key, index));
            return Some(self.notes_event());
        }
        let pos = self.held.iter().position(|(k, _)| *k == key)?;
        let (_, index) = self.held.remove(pos);
        match playhead {
            // Recording: the key was held this long.
            Some(now) => {
                if let Some(n) = self.notes.get_mut(index) {
                    n.dur = (now - n.start).max(MIN_DUR);
                }
            }
            // Step entry: the last key up advances the cursor one grid.
            None if self.held.is_empty() => {
                self.step += if self.snap > 0.0 { self.snap } else { MIN_DUR };
            }
            None => {}
        }
        Some(self.notes_event())
    }

    fn clone_box(&self) -> Box<dyn Element> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}

impl Notes {
    #[cfg(test)]
    /// The multi-note selection, for the crate's own gesture suite — which
    /// drives a real host and has no other way to see it (it is view state, so
    /// no `/gui_query` reports it).
    pub(crate) fn selected(&self) -> &[usize] {
        &self.selected
    }

    /// Whether this placement is the roll's **own** view rather than a clip's
    /// body: a body is handed a container's axis and draws none of the chrome
    /// that would make it navigable.
    fn navigable_placement(&self, input: &Input) -> bool {
        input.indent > 0.0
    }

    /// The rate the ruler and the readout are placed on: the widget's own when
    /// it names one, else the server's.
    fn rate(&self, world_rate: f64) -> f64 {
        if self.editor.sample_rate > 0.0 {
            self.editor.sample_rate
        } else {
            world_rate
        }
    }

    /// The axis' chrome over the grid: the shared selection band, the playhead,
    /// and the cursor readout naming the pitch and the time under the pointer.
    fn draw_axis_chrome(&self, d: &mut Draw, ctx: &Ctx, grid: Rect, nav: &View, rate: f64) {
        let Some(time) = ctx.time else {
            return;
        };
        let (mesh, m, theme) = d.parts();
        let to_x =
            |s: f64| (grid.x as f64 + (s - nav.start) / nav.len.max(1.0) * grid.w as f64) as f32;
        if let Some((start, len)) = time.sel {
            let x0 = to_x(start).clamp(grid.x, grid.x + grid.w);
            let x1 = to_x(start + len).clamp(grid.x, grid.x + grid.w);
            if x1 > x0 {
                let band = crate::host::theme::with_alpha(theme.selection, 0.18);
                let edge = crate::host::theme::with_alpha(theme.selection, 0.75);
                mesh.rect(Rect::new(x0, grid.y, x1 - x0, grid.h), band);
                mesh.rect(Rect::new(x0, grid.y, m.divider_w, grid.h), edge);
                mesh.rect(
                    Rect::new(x1 - m.divider_w, grid.y, m.divider_w, grid.h),
                    edge,
                );
            }
        }
        if let Some(pos) = time.head
            && pos >= nav.start
            && pos <= nav.start + nav.len
        {
            mesh.rect(
                Rect::new(to_x(pos), grid.y, m.trace_w, grid.h),
                theme.playhead,
            );
        }
        // The readout: the note name under the cursor and the time, in the
        // grid's bottom-right corner.
        let Some((cx, cy)) = ctx.world.cursor.filter(|(x, y)| grid.contains(*x, *y)) else {
            return;
        };
        let (lo, hi) = self.pitch_window();
        let pitch = pianoroll::y_to_pitch(cy as f32, lo, hi, grid).round() as i32;
        let s = nav.start + nav.len * ((cx - grid.x as f64) / grid.w.max(1.0) as f64);
        let time = match self.editor.ruler {
            Ruler::Samples => ruler::readout_samples(s),
            Ruler::Beats => ruler::readout_beats(
                s,
                rate,
                self.editor.tempo,
                self.editor.beat_at,
                self.editor.quant,
                nav.len / rate * self.editor.tempo / grid.w.max(1.0) as f64,
            ),
            _ => ruler::readout_time(s, rate, nav.len / rate / grid.w.max(1.0) as f64),
        };
        let text = format!("{}  {time}", clausters_core::scale::note_name(pitch));
        let w = font::width(&text, m.caption_scale);
        font::text(
            mesh,
            &text,
            grid.x + grid.w - w - m.pad,
            grid.y + grid.h - font::height(m.caption_scale) - 2.0,
            m.caption_scale,
            theme.ruler_text,
        );
    }

    /// A press on the note grid: Alt toggles a note in or out of the selection,
    /// Ctrl adds or removes one, a note moves or resizes (a **selected** note
    /// moves the whole selection), and empty grid sweeps the marquee.
    fn press_grid(&mut self, h: &Hit, at: (f64, f64), input: &Input, is_body: bool) -> Claim {
        if input.mods.alt {
            let Some(nh) = h.note else {
                return Claim::Decline;
            };
            pianoroll::toggle_selected(&mut self.selected, nh.index);
            return Claim::take();
        }
        if input.mods.ctrl {
            match h.note {
                // Ctrl on a note removes it; the selection's indices shift down
                // past it.
                Some(nh) => {
                    pianoroll::remove_note(&mut self.notes, nh.index);
                    self.selected = pianoroll::selection_after_removal(&self.selected, nh.index);
                }
                // Ctrl on empty grid adds one there, then drags its end to set
                // the length until release.
                None if !is_body => {
                    let time = snap_to(self.time_at(h.grid, &h.nav, at.0), self.snap).max(0.0);
                    let pitch = pianoroll::y_to_pitch(at.1 as f32, h.lo, h.hi, h.grid)
                        .round()
                        .clamp(h.lo, h.hi);
                    let dur = self.default_dur(&h.nav);
                    let index = self.insert(pianoroll::Note::new(time, dur, pitch));
                    self.drag = Some(Drag::Note {
                        index,
                        part: pianoroll::NotePart::End,
                        press_time: time,
                        orig_start: time,
                        orig_dur: dur,
                    });
                }
                None => return Claim::Decline,
            }
            return Claim::events(self.notes_event()).edge_scrolling();
        }
        let Some(nh) = h.note else {
            // Nothing of this element's, inside a clip: the press goes back to
            // the container, whose own drag is what the rest of the rectangle
            // means. Standing alone the whole grid is the roll's, and the empty
            // part of it sweeps.
            if is_body {
                return Claim::Decline;
            }
            let t = self.time_at(h.grid, &h.nav, at.0);
            let p = pianoroll::y_to_pitch(at.1 as f32, h.lo, h.hi, h.grid);
            return self.start_marquee(t, Some(p));
        };
        let press_time = self.time_at(h.grid, &h.nav, at.0);
        if nh.part == pianoroll::NotePart::Body {
            // Grabbing a **selected** note moves the whole selection; grabbing
            // an unselected one drops the selection and moves singly.
            if self.selected.contains(&nh.index) {
                let mut idx = self.selected.clone();
                idx.retain(|&i| i != nh.index);
                // The grabbed note leads: it is the snap anchor.
                idx.insert(0, nh.index);
                let orig: Vec<_> = idx
                    .iter()
                    .filter_map(|&i| self.notes.get(i).map(|n| (i, n.start, n.pitch)))
                    .collect();
                if !orig.is_empty() {
                    self.drag = Some(Drag::Block {
                        press_time,
                        press_pitch: pianoroll::y_to_pitch(at.1 as f32, h.lo, h.hi, h.grid),
                        orig,
                    });
                    return Claim::take().edge_scrolling();
                }
            }
            self.selected.clear();
        }
        let (orig_start, orig_dur) = self
            .notes
            .get(nh.index)
            .map_or((0.0, 0.0), |n| (n.start, n.dur));
        self.drag = Some(Drag::Note {
            index: nh.index,
            part: nh.part,
            press_time,
            orig_start,
            orig_dur,
        });
        Claim::take().edge_scrolling()
    }

    /// A press on the velocity lane: over a **selected** note the whole
    /// selection nudges together (relative, from a snapshot); over an
    /// unselected one the single bar follows the cursor.
    fn press_velocity(&mut self, h: &Hit, at: (f64, f64), is_body: bool) -> Claim {
        let Some(nh) = h.note else {
            if is_body {
                return Claim::Decline;
            }
            let t = self.time_at(h.grid, &h.nav, at.0);
            return self.start_marquee(t, None);
        };
        if self.selected.contains(&nh.index) {
            let orig: Vec<_> = self
                .selected
                .iter()
                .filter_map(|&i| self.notes.get(i).map(|n| (i, n.velocity)))
                .collect();
            if !orig.is_empty() {
                self.drag = Some(Drag::VelocityBlock {
                    press_velocity: pianoroll::velocity_at(h.rect, at.1),
                    orig,
                });
                return Claim::take();
            }
        }
        self.drag = Some(Drag::Velocity { index: nh.index });
        Claim::take()
    }

    /// A press on the event lane: Ctrl adds or removes a marker, a press on one
    /// slides it.
    fn press_osc(&mut self, h: &Hit, at: (f64, f64), input: &Input, is_body: bool) -> Claim {
        if input.mods.ctrl {
            match h.osc {
                Some(index) if index < self.osc.len() => {
                    self.osc.remove(index);
                }
                Some(_) => return Claim::Decline,
                None => {
                    let time = snap_to(self.time_at(h.grid, &h.nav, at.0), self.snap).max(0.0);
                    self.osc.push(pianoroll::OscMark { time, label: None });
                }
            }
            return Claim::events(self.osc_event());
        }
        match h.osc {
            Some(index) => {
                self.drag = Some(Drag::OscMark { index });
                Claim::take().edge_scrolling()
            }
            None if is_body => Claim::Decline,
            None => {
                let t = self.time_at(h.grid, &h.nav, at.0);
                self.start_marquee(t, None)
            }
        }
    }
}

/// Snaps `t` to the `grid` (0 = whole units), the one rounding every note edit
/// shares.
fn snap_to(t: f64, grid: f64) -> f64 {
    if grid > 0.0 {
        (t / grid).round() * grid
    } else {
        t
    }
}

/// The notes on the host-wide clipboard, when what is on it is a note block —
/// the same flat quintuple JSON a `/gui_set notes` takes, so a block copied out
/// of one roll pastes into another and a field's text pastes into neither.
fn clipboard_notes(text: &str) -> Option<Vec<pianoroll::Note>> {
    let value: Value = serde_json::from_str(text).ok()?;
    let notes = parse_notes(&parse::as_array_props("notes", &value));
    (!notes.is_empty()).then_some(notes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::widget::element::Mods;

    fn props(json: &str) -> Map<String, Value> {
        serde_json::from_str(json).unwrap()
    }

    fn roll(json: &str) -> Notes {
        from_props(&props(json))
    }

    /// A roll placed on a navigation group: the indent is its keyboard gutter,
    /// the axis is the group's window over its content.
    fn input<'a>(m: &'a Metrics, rect: Rect, time: Option<TimeSpace>) -> Input<'a> {
        Input {
            metrics: m,
            rect,
            indent: pianoroll::KEYBOARD_W,
            scale: 1.0,
            mods: Mods::default(),
            viewport: (rect.w, rect.h),
            time,
        }
    }

    fn axis(len: f64) -> Option<TimeSpace> {
        Some(TimeSpace::of(View { start: 0.0, len }, len))
    }

    fn rect() -> Rect {
        Rect::new(0.0, 0.0, 500.0, 400.0)
    }

    /// The x pixel a time falls on in the grid — what the drawing maps and what
    /// a press has to invert.
    fn x_of(r: &Notes, m: &Metrics, t: f64, len: f64) -> f64 {
        let grid = r.regions(rect(), pianoroll::KEYBOARD_W, m).grid;
        grid.x as f64 + t / len * grid.w as f64
    }

    fn y_of(r: &Notes, m: &Metrics, pitch: f32) -> f64 {
        let grid = r.regions(rect(), pianoroll::KEYBOARD_W, m).grid;
        let (lo, hi) = r.pitch_window();
        pianoroll::pitch_to_y(pitch, lo, hi, grid) as f64
    }

    #[test]
    fn parses_defaults_and_the_wire_lists() {
        let r = roll("{}");
        assert_eq!((r.min, r.max), (PITCH_MIN, PITCH_MAX));
        assert!(r.velocity_lane && !r.osc_lane && !r.midi_in);
        assert_eq!(r.snap, 0.0);
        assert!(r.notes.is_empty() && r.osc.is_empty());

        // The canonical quintuple form, and the event lane opening because
        // there are events.
        let r = roll(
            r#"{"notes":[0.0,100.0,60.0,90,2],"osc":[50.0,"hit"],
                "min":48,"max":72,"snap":25.0,"midi_in":true}"#,
        );
        assert_eq!(r.notes.len(), 1);
        assert_eq!((r.notes[0].velocity, r.notes[0].channel), (90, 2));
        assert_eq!(r.osc.len(), 1);
        assert!(r.osc_lane, "events open their lane");
        assert!(r.midi_in);
        assert_eq!(r.snap, 25.0);
    }

    /// A `/gui_set` of a list rides as its JSON string and drops the selection,
    /// whose indices would dangle over the new list.
    #[test]
    fn apply_replaces_the_lists_and_drops_the_selection() {
        let mut r = roll(r#"{"notes":[0.0,10.0,60.0,100,0]}"#);
        r.selected = vec![0];
        assert!(r.set(
            "notes",
            &Value::from("[0.0,10.0,64.0,100,0,20.0,10.0,67.0,100,0]")
        ));
        assert_eq!(r.notes.len(), 2);
        assert!(r.selected.is_empty());
        assert!(r.set("osc", &Value::from("[5.0,\"a\"]")));
        assert_eq!(r.osc.len(), 1);
        assert!(r.set("snap", &Value::from(50.0)));
        assert!(r.set("velocity", &Value::from(0)));
        assert!(!r.velocity_lane);
        // The editor chrome is the element's too, so its keys apply here.
        assert!(r.set("ruler", &Value::from("beats")));
        assert!(!r.set("nonesuch", &Value::from(1)));
    }

    /// The extent a group's axis reads off a roll is its content's, which is
    /// what lets a roll being written into lengthen the timeline.
    #[test]
    fn the_content_span_is_the_end_of_the_last_thing_on_it() {
        let r = roll(r#"{"notes":[0.0,100.0,60.0,100,0],"osc":[400.0,""]}"#);
        assert_eq!(r.content_span(), Some(400.0));
        let r = roll(r#"{"notes":[100.0,300.0,60.0,100,0]}"#);
        assert_eq!(r.content_span(), Some(400.0));
        assert_eq!(roll("{}").content_span(), Some(0.0));
    }

    /// A note is grabbed by the pixels it is drawn on, and the drag reports the
    /// whole list in the owner's own units.
    #[test]
    fn a_note_drag_moves_it_in_time_and_pitch_and_reports_the_list() {
        let m = Metrics::default();
        let mut r = roll(r#"{"notes":[0.0,100.0,60.0,100,0],"min":48,"max":72}"#);
        let at = (x_of(&r, &m, 50.0, 1000.0), y_of(&r, &m, 60.0));
        assert!(matches!(
            r.press(at, &input(&m, rect(), axis(1000.0))),
            Claim::Take(_)
        ));
        assert!(matches!(r.drag, Some(Drag::Note { index: 0, .. })));

        let to = (x_of(&r, &m, 250.0, 1000.0), y_of(&r, &m, 64.0));
        let events = r.drag(to, &input(&m, rect(), axis(1000.0)));
        assert!((r.notes[0].start - 200.0).abs() < 1.0, "{:?}", r.notes[0]);
        assert_eq!(r.notes[0].pitch, 64.0);
        assert_eq!(r.notes[0].dur, 100.0, "a move keeps the duration");
        let msgs = events.into_messages();
        assert_eq!(msgs[0][0], OscType::String("notes".into()));
        assert_eq!(msgs[0].len(), 1 + 5, "the tag plus a quintuple per note");

        assert!(r.release(to, &input(&m, rect(), axis(1000.0))).is_empty());
        assert!(r.drag.is_none());
    }

    /// **A note drag is measured against the axis it is handed each step**, not
    /// against a press-time copy of it — which is what lets the machine scroll
    /// the axis under a drag held past a lane's edge.
    #[test]
    fn a_drag_follows_an_axis_that_moves_under_it() {
        let m = Metrics::default();
        let mut r = roll(r#"{"notes":[0.0,100.0,60.0,100,0],"min":48,"max":72}"#);
        let at = (x_of(&r, &m, 50.0, 1000.0), y_of(&r, &m, 60.0));
        r.press(at, &input(&m, rect(), axis(1000.0)));
        // The same cursor, against a window that has panned 500 forward: the
        // note lands 500 later, because the pixel now names a later sample.
        let panned = Some(TimeSpace::of(
            View {
                start: 500.0,
                len: 1000.0,
            },
            2000.0,
        ));
        r.drag(at, &input(&m, rect(), panned));
        assert!((r.notes[0].start - 500.0).abs() < 1.0, "{:?}", r.notes[0]);
    }

    /// The marquee is two things at once, and the split is the port's point:
    /// the **time span** is asked of the machine (it is the group's selection,
    /// which every linked view follows) and the **notes inside the rectangle**
    /// are the element's own state.
    #[test]
    fn the_marquee_asks_for_the_selection_and_keeps_the_notes() {
        let m = Metrics::default();
        let mut r = roll(
            r#"{"notes":[0.0,100.0,60.0,100,0,200.0,100.0,64.0,100,0,
                         500.0,100.0,80.0,100,0],"min":48,"max":84}"#,
        );
        // A press on empty grid, low in the pitch window.
        let at = (x_of(&r, &m, 0.0, 1000.0), y_of(&r, &m, 58.0));
        let Claim::Take(take) = r.press(at, &input(&m, rect(), axis(1000.0))) else {
            panic!("empty grid sweeps")
        };
        assert_eq!(take.events.selection(), Some((0.0, 0.0)));

        let to = (x_of(&r, &m, 400.0, 1000.0), y_of(&r, &m, 66.0));
        let events = r.drag(to, &input(&m, rect(), axis(1000.0)));
        let (a, b) = events.selection().expect("the sweep asks every step");
        assert!((a - 0.0).abs() < 1.0 && (b - 400.0).abs() < 2.0, "{a} {b}");
        assert_eq!(r.selected, vec![0, 1], "the third note is out of the band");
        assert!(
            events.into_messages().is_empty(),
            "a sweep edits nothing, so it reports nothing"
        );
    }

    /// Grabbing a **selected** note moves the whole selection rigidly; grabbing
    /// an unselected one drops the selection and moves singly.
    #[test]
    fn a_block_moves_together_and_an_unselected_note_alone() {
        let m = Metrics::default();
        let mut r =
            roll(r#"{"notes":[0.0,100.0,60.0,100,0,200.0,100.0,64.0,100,0],"min":48,"max":72}"#);
        r.selected = vec![0, 1];
        let at = (x_of(&r, &m, 50.0, 1000.0), y_of(&r, &m, 60.0));
        r.press(at, &input(&m, rect(), axis(1000.0)));
        assert!(matches!(r.drag, Some(Drag::Block { .. })));
        let to = (x_of(&r, &m, 150.0, 1000.0), y_of(&r, &m, 60.0));
        r.drag(to, &input(&m, rect(), axis(1000.0)));
        assert!((r.notes[0].start - 100.0).abs() < 1.0);
        assert!((r.notes[1].start - 300.0).abs() < 1.0, "rigid");

        // The other half: an unselected note drops the set.
        let mut r =
            roll(r#"{"notes":[0.0,100.0,60.0,100,0,200.0,100.0,64.0,100,0],"min":48,"max":72}"#);
        r.selected = vec![1];
        r.press(at, &input(&m, rect(), axis(1000.0)));
        assert!(matches!(r.drag, Some(Drag::Note { index: 0, .. })));
        assert!(r.selected.is_empty());
    }

    /// Ctrl adds a note where there is none and removes the one under the
    /// cursor; both report the list, and the add drags its end.
    #[test]
    fn ctrl_adds_and_removes_a_note() {
        let m = Metrics::default();
        let mut r = roll(r#"{"min":48,"max":72,"snap":100.0}"#);
        let mut ctrl = input(&m, rect(), axis(1000.0));
        ctrl.mods = Mods {
            ctrl: true,
            ..Mods::default()
        };
        let at = (x_of(&r, &m, 250.0, 1000.0), y_of(&r, &m, 60.0));
        assert!(matches!(r.press(at, &ctrl), Claim::Take(_)));
        assert_eq!(r.notes.len(), 1);
        assert_eq!(r.notes[0].start, 300.0, "snapped to the note grid");
        assert!(matches!(
            r.drag,
            Some(Drag::Note {
                part: pianoroll::NotePart::End,
                ..
            })
        ));

        r.drag = None;
        // The note it just added spans 300..400 on the grid.
        let on_note = (x_of(&r, &m, 350.0, 1000.0), y_of(&r, &m, 60.0));
        assert!(matches!(r.press(on_note, &ctrl), Claim::Take(_)));
        assert!(r.notes.is_empty());
    }

    /// A press inside a **clip** claims a note and declines everywhere else:
    /// the rest of the rectangle is the clip's own drag.
    #[test]
    fn a_body_claims_its_notes_and_declines_the_rest() {
        let m = Metrics::default();
        let mut b = body(&props(r#"{"notes":[0.0,100.0,60.0,100,0]}"#)).expect("notes");
        let mut i = input(&m, rect(), axis(1000.0));
        // A body is placed on its clip's rectangle, with no gutter of its own.
        i.indent = 0.0;
        let grid = b.regions(rect(), 0.0, &m).grid;
        let (lo, hi) = b.pitch_window();
        let on = (
            grid.x as f64 + 50.0 / 1000.0 * grid.w as f64,
            pianoroll::pitch_to_y(60.0, lo, hi, grid) as f64,
        );
        assert!(matches!(b.press(on, &i), Claim::Take(_)));
        b.drag = None;
        let off = (grid.x as f64 + 0.9 * grid.w as f64, on.1);
        assert_eq!(b.press(off, &i), Claim::Decline, "the clip's own drag");
        assert_eq!(b.body_role(), Some(BodyRole::Notes));
        assert_eq!(empty_body().body_role(), Some(BodyRole::Notes));
    }

    /// The block keys: `q` quantizes the selection, Delete removes it, and
    /// cut/paste travel through the host-wide clipboard in the same JSON a
    /// `/gui_set notes` takes.
    #[test]
    fn the_block_keys_quantize_delete_and_travel_through_the_clipboard() {
        let mut clipboard = String::new();
        let mut r = roll(r#"{"notes":[90.0,50.0,60.0,100,0,260.0,50.0,64.0,100,0],"snap":100.0}"#);
        r.selected = vec![0];
        fn ki(clip: &mut String, ctrl: bool) -> KeyInput<'_> {
            KeyInput {
                mods: Mods {
                    ctrl,
                    ..Mods::default()
                },
                clipboard: clip,
            }
        }
        assert!(
            r.key(&Key::Char('q'), &mut ki(&mut clipboard, false))
                .is_some()
        );
        assert_eq!(r.notes[0].start, 100.0);
        assert_eq!(r.notes[1].start, 260.0, "unselected, untouched");

        // Cut: the block lands on the clipboard and leaves the roll.
        r.selected = vec![0];
        assert!(
            r.key(&Key::Char('x'), &mut ki(&mut clipboard, true))
                .is_some()
        );
        assert_eq!(r.notes.len(), 1);
        assert!(clipboard.starts_with('['), "{clipboard}");

        // ...and pastes back at the step cursor, keeping its pitch.
        assert!(
            r.key(&Key::Char('v'), &mut ki(&mut clipboard, true))
                .is_some()
        );
        assert_eq!(r.notes.len(), 2);
        assert_eq!(r.notes[1].pitch, 60.0);
        assert_eq!(r.selected, vec![1], "the pasted block is selected");

        // Delete takes the selection away; a key it has no arm for falls
        // through to the front's own shortcuts.
        assert!(
            r.key(&Key::Delete, &mut ki(&mut clipboard, false))
                .is_some()
        );
        assert_eq!(r.notes.len(), 1);
        assert!(
            r.key(&Key::Char('z'), &mut ki(&mut clipboard, false))
                .is_none()
        );
        // Text on the clipboard is not a note block, so a paste declines it.
        let mut text = "hola".to_string();
        assert!(r.key(&Key::Char('v'), &mut ki(&mut text, true)).is_none());
    }

    /// Live MIDI: a note-on paints a held note, the matching note-off closes it
    /// — at the running playhead when recording, on the step cursor when the
    /// transport is stopped (and the last key up advances it).
    #[test]
    fn live_midi_records_at_the_playhead_and_steps_when_stopped() {
        let mut r = roll(r#"{"midi_in":true,"snap":100.0}"#);
        assert!(r.needs().midi);
        assert!(!roll("{}").needs().midi);

        // Recording: the key is held from 200 to 350.
        let on = MidiNote {
            on: true,
            channel: 0,
            pitch: 60,
            velocity: 90,
        };
        assert!(r.midi(on, Some(200.0)).is_some());
        assert_eq!(r.notes[0].start, 200.0);
        assert!(
            r.midi(
                MidiNote {
                    on: false,
                    velocity: 0,
                    ..on
                },
                Some(350.0)
            )
            .is_some()
        );
        assert_eq!(r.notes[0].dur, 150.0, "the key was held this long");

        // Stopped: a chord lands on the step cursor and advances it once.
        let mut r = roll(r#"{"midi_in":true,"snap":100.0}"#);
        for pitch in [60, 64] {
            r.midi(
                MidiNote {
                    on: true,
                    channel: 0,
                    pitch,
                    velocity: 90,
                },
                None,
            );
        }
        assert_eq!((r.notes[0].start, r.notes[1].start), (0.0, 0.0));
        for pitch in [60, 64] {
            r.midi(
                MidiNote {
                    on: false,
                    channel: 0,
                    pitch,
                    velocity: 0,
                },
                None,
            );
        }
        assert_eq!(r.step, 100.0, "one step for the whole chord");
        // A note-off nobody is holding is tolerated, not a panic.
        assert!(
            r.midi(
                MidiNote {
                    on: false,
                    channel: 9,
                    pitch: 1,
                    velocity: 0
                },
                None
            )
            .is_none()
        );
    }
}
