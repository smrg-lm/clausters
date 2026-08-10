//! The shared pointer-gesture state machine — one press → drag → release
//! interpreter over the widget tree for **both fronts**.
//!
//! The machine owns the in-progress [`Drag`] and turns pointer/wheel/keyboard
//! input into tree mutations (through the [`interact`] doors and the widget
//! model modules) plus a list of [`GestureEffect`]s for whatever only the front
//! can do: emitting `/gui_event` over its transport, requesting repaints,
//! releasing a pointer grab. The front supplies the per-call [`GestureCtx`]
//! (framebuffer size, modifier keys, the heavy views' lane counts — the one
//! datum that lives in front-side GPU slots) and, at press time, a pointer-grab
//! callback (native pointer lock; a front without one returns `false`).
//!
//! The module is platform-agnostic (no winit, no web-sys): the native windowed
//! front ([`super::gui`]) and the browser front (`super::web`) both drive it,
//! so a selection, a clip drag or a BPF edit behaves identically on either
//! platform by construction.
//!
//! **Module layout.** This file is the machine's *state*: the [`Drag`] enum
//! (every gesture in flight), the [`Gestures`] struct and the small readers the
//! fronts ask it for between calls (what is being dragged, whether a menu is
//! open, whether the pointer sits in a lane's edge-scroll band).
//!
//! The **phases are one child module each**, because that is how a gesture is
//! read and changed — a variant's press, its drag and its release are three
//! places in three matches, and having them in three files instead of one
//! thousand-line one is what keeps the three in view of each other:
//! [`press`] (the containers' plans, then the element under the cursor — where
//! every `Drag` is opened), [`drag`] (what a held drag moves, what its release
//! delivers, and the edge-scroll frame step that continues one), and
//! [`wheel`] (the phase that opens no drag at all).
//!
//! Three more children are what the phases lean on, so the machine reads as
//! press → drag → release without the plumbing in between: [`effects`] is what
//! a gesture *delivers* (the one place the bound-vs-event decision is made),
//! [`nav`] is what it *reads and moves* (hit-testing, a scroll plane, a
//! timeline group's pan/zoom/selection), and [`keys`] is the keyboard half —
//! text editing and the block operations, which share nothing with the pointer
//! but the `Gestures` state they hang off.

use std::collections::HashMap;

use clausters_core::osc::OscType;

use super::interact::{self};
use super::layout::Rect;
use super::signal::Presentation;
use super::widget::WidgetKind;
use super::{piano, pianoroll};

/// What a gesture asks of the front: everything the machine cannot do itself
/// because it owns no transport, no window and no GPU.
#[derive(Debug, PartialEq)]
pub enum GestureEffect {
    /// Emit `/gui_event widget_id <args…>` to the script behind window `def_id`.
    Emit {
        def_id: i32,
        widget_id: i32,
        args: Vec<OscType>,
    },
    /// Repaint the window rooted at this def id (a gesture on one window may
    /// touch linked views in others, so this is not always the pressed window).
    Redraw(i32),
    /// Release the pointer grab a knob/number drag took on window `def_id`.
    ReleasePointer(i32),
}

/// The per-call context the front supplies: which window, its framebuffer size
/// in device pixels, the modifier keys, and the heavy views' lane counts (the
/// channel/lane split lives in the front's GPU slots, so the front snapshots it
/// here; a widget missing from the maps counts as one lane).
pub struct GestureCtx {
    pub def_id: i32,
    pub fb_w: u32,
    pub fb_h: u32,
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    /// Channel count per `waveform` widget id (stacked lanes; `overlay` still
    /// draws one lane and is resolved here, not in the map).
    pub wave_lanes: HashMap<i32, usize>,
    /// Lane (channel STFT) count per `spectrogram` widget id.
    pub spect_lanes: HashMap<i32, usize>,
    /// The server's sample rate (`0.0` when this front does not know it) — the
    /// same one the frame draws with. A gesture over a *measured* axis needs
    /// it: a frequency axis has a resolution, and the zoom is not allowed past
    /// it.
    pub sample_rate: f64,
}

impl GestureCtx {
    /// A bare context (no modifiers, no lane info) — enough for the control
    /// widgets and for tests.
    pub fn new(def_id: i32, fb_w: u32, fb_h: u32) -> Self {
        Self {
            def_id,
            fb_w,
            fb_h,
            shift: false,
            ctrl: false,
            alt: false,
            wave_lanes: HashMap::new(),
            spect_lanes: HashMap::new(),
            sample_rate: 0.0,
        }
    }

    /// The lane count a timeline view stacks on screen (overlaid waveform
    /// traces share one lane) — the divisor for lane-relative y gestures.
    fn lanes(&self, id: i32, kind: &WidgetKind) -> usize {
        match kind.signal() {
            // Overlaid traces share one lane, however many channels there are.
            Some(el) if el.display.overlay => 1,
            Some(el) if el.presentation == Presentation::TimeFrequency => {
                self.spect_lanes.get(&id).copied().unwrap_or(1).max(1)
            }
            Some(_) => self.wave_lanes.get(&id).copied().unwrap_or(1).max(1),
            None => 1,
        }
    }
}

/// An in-progress pointer drag, by what it is driving.
///
/// A drag is either a **container's navigation plan** — panning, selecting,
/// scrolling a coordinate system, which is a property of that system and not of
/// anything drawn in it — or [`Drag::Element`], which carries no geometry at
/// all: what the drag *means* lives in the element, where its state belongs,
/// and what the machine keeps is the sequence and the pointer grab.
#[derive(Clone)]
enum Drag {
    /// An element is holding the press. The machine remembers only what it
    /// alone can answer for: **where** it is ([`element::At`] — which widget or
    /// which body of which container, the placement the press was measured
    /// against, the axis it was placed on) and whether the front granted a
    /// pointer grab, which decides whether motion arrives as a position or as a
    /// delta.
    Element { at: element::At, grab: bool },
    /// Panning a timeline view's (waveform/spectrogram) window from a snapshot
    /// (Shift+drag).
    Pan {
        id: i32,
        origin_x: f64,
        start: f64,
        body_w: f64,
    },
    /// Sweeping a selection on a timeline container: `anchor` is the sample
    /// under the press, and the selection spans from it to the cursor's sample.
    /// On an axis that measures a **value** as well (a piano-roll's pitch,
    /// `value` carrying its window and the value under the press) the sweep is
    /// a rectangle: the time span still drives the shared selection every
    /// linked view follows, and the elements inside the rectangle become the
    /// container's own selection.
    Select {
        id: i32,
        body: Rect,
        nav_start: f64,
        nav_len: f64,
        anchor: f64,
        value: Option<(f64, f64, f64)>,
    },
    /// Panning a timeline view's **vertical** display window from a drag on
    /// its y-ruler strip: `y_start` is the window snapshot at the press,
    /// `lane_h` the lane height in device pixels (absolute panning, so a
    /// clamped edge never drifts).
    PanY {
        id: i32,
        origin_y: f64,
        y_start: f64,
        lane_h: f64,
    },
    /// Panning a spectrum's **frequency** window from a drag anywhere on its
    /// axis: `x_start` is the window snapshot at the press, `body_w` the pixels
    /// one window's worth spans. Absolute from the snapshot, exactly like
    /// [`Drag::PanY`], and per-element for the same reason — a frequency axis
    /// is in no navigation group.
    PanX {
        id: i32,
        origin_x: f64,
        x_start: f64,
        body_w: f64,
    },
    /// Dragging a multitrack `clip`: the body moves its `offset`, an edge
    /// resizes its `dur`. The cursor maps to a timeline sample through the
    /// lane's `body_x`/`body_w` and the shared `nav_start`/`nav_len`; the
    /// placement follows from a press-time snapshot (`press_sample`,
    /// `orig_offset`, `orig_dur`) so a clamped edge never drifts, snapped to
    /// `grid`.
    Clip {
        id: i32,
        /// The lane the clip sits on — the navigation-group member, which the
        /// clip itself is not; the cursor mapping and the edge scroll reach the
        /// shared axis through it.
        lane: i32,
        part: interact::ClipPart,
        body_x: f64,
        body_w: f64,
        nav_start: f64,
        nav_len: f64,
        press_sample: f64,
        orig_offset: f64,
        orig_dur: f64,
        grid: f64,
    },
    /// A cord being pulled from a patch's port: the widget, the grabbed
    /// port `(box, side, index)` and the widget's area — released over a
    /// compatible port (an outlet↔inlet of matching rate) to draw a cord, over
    /// anything else to cancel. `scale` is the workspace zoom the patch is seen
    /// through, so the pin geometry matches the drawing.
    Wire {
        id: i32,
        port: (usize, super::patch::Side, usize),
        area: Rect,
        scale: f32,
    },
    /// A `patch` box (or the whole selection) being moved on the patch
    /// canvas: the grabbed boxes with their positions at press time (canvas
    /// units), moved together by the cursor delta and emitted as one
    /// `"move"` event per box on release.
    Box {
        id: i32,
        scale: f32,
        origin: (f64, f64),
        grabbed: Vec<(usize, f32, f32)>,
        moved: bool,
    },
    /// The selection marquee on a patch's empty canvas: the selected
    /// set follows the rectangle live; the rectangle itself draws through
    /// [`Gestures::marquee`].
    Marquee {
        id: i32,
        area: Rect,
        scale: f32,
        origin: (f64, f64),
        cursor: (f64, f64),
    },
    /// Dragging a lane header's level fader: the cursor's x over the fader's
    /// rectangle is the value, so the press itself already sets it.
    LaneLevel { id: i32, rect: Rect },
    /// Dragging a piano-roll note: the body moves it in time and pitch, an edge
    /// resizes its duration. The cursor maps to a region-relative time through
    /// the grid and the shared `nav`, and to a pitch through the visible window
    /// `[lo, hi]`; a press-time snapshot (`press_time`, `orig_*`) keeps a clamped
    /// edge from drifting, snapped to `grid`.
    Note {
        id: i32,
        index: usize,
        part: pianoroll::NotePart,
        grid: Rect,
        nav_start: f64,
        nav_len: f64,
        lo: f32,
        hi: f32,
        press_time: f64,
        orig_start: f64,
        orig_dur: f64,
        snap: f64,
    },
    /// Dragging a note's velocity bar in the velocity lane: the velocity follows
    /// the cursor's height within `lane`.
    Velocity { id: i32, index: usize, lane: Rect },
    /// Dragging an OSC-event marker along the time axis (its `time` follows the
    /// cursor through the grid's shared `nav`, snapped to `grid`).
    OscMark {
        id: i32,
        index: usize,
        grid: Rect,
        nav_start: f64,
        nav_len: f64,
        snap: f64,
    },
    /// Dragging a **selected** note moves the whole selection rigidly in time
    /// and pitch. `orig` is the `(index, start, pitch)` snapshot at press time
    /// — the grabbed note's entry leads it (the snap anchor) — so a clamped
    /// block never drifts.
    NoteBlock {
        id: i32,
        grid: Rect,
        nav_start: f64,
        nav_len: f64,
        lo: f32,
        hi: f32,
        press_time: f64,
        press_pitch: f32,
        snap: f64,
        orig: Vec<(usize, f64, f32)>,
    },
    /// Dragging the velocity lane over a **selected** note nudges every selected
    /// velocity by the same delta (relative, from the `(index, velocity)` press
    /// snapshot — each note saturates on its own).
    VelocityBlock {
        id: i32,
        lane: Rect,
        /// The lane velocity under the press (the delta's zero).
        press_velocity: i32,
        orig: Vec<(usize, i32)>,
    },
    /// A held `piano` key: crossing into another key glissandos (note-off of
    /// the old, note-on of the new); release sends the note-off. The layout is
    /// snapshotted at press time (the range cannot change under a key drag).
    PianoKey {
        id: i32,
        layout: piano::Layout,
        pitch: i32,
        /// The widget's fixed velocity, or `None` for the press-height map.
        fixed_vel: Option<i32>,
        channel: i32,
    },
    /// Dragging the `piano`'s overview strip pans the visible range relative to
    /// the press snapshot (`min0`/`max0`, the key under the press as `anchor`).
    PianoView {
        id: i32,
        strip: Rect,
        min0: i32,
        max0: i32,
        anchor: i32,
    },
    /// Panning a `scroll` workspace from a press on its empty area: the view
    /// follows the cursor absolutely from the press snapshot (`x0`/`y0`), so a
    /// clamped edge never drifts. `area` is the container's laid-out rect at
    /// press time (the clamp geometry).
    ScrollPan {
        id: i32,
        area: Rect,
        origin_x: f64,
        origin_y: f64,
        x0: f64,
        y0: f64,
    },
    /// Dragging an engraved element up or down a `score`: the vertical
    /// displacement quantizes to whole **diatonic steps** (the page's `step`),
    /// which the widget draws as it moves and the release emits as a
    /// `"transpose"` edit-back. `rect` is the page's laid-out area at press time
    /// (the page→screen fit, which cannot change under a drag) and `origin_y`
    /// the press, so the count is absolute from the snapshot rather than
    /// accumulated.
    ScoreStep {
        id: i32,
        element: String,
        rect: Rect,
        origin_y: f64,
        steps: i32,
    },
    /// Selecting text in an editable `text` field: `anchor` is the caret byte
    /// offset the press landed on; dragging extends the selection from it to the
    /// caret under the cursor. `rect`/`scale` reconstruct the field's layout so
    /// the cursor maps to a caret exactly as the renderer drew it.
    TextSelect {
        id: i32,
        rect: Rect,
        scale: f32,
        anchor: usize,
    },
}

/// A platform-neutral key for editing a focused `text` field — the fronts
/// (winit native, winit-on-wasm web) translate their key events into this so
/// the editing behavior lives once in the machine. Modifier state rides in the
/// [`GestureCtx`] (`shift` extends a selection, `ctrl` word-jumps and drives
/// cut/copy/paste/select-all on the letter keys).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextKey {
    /// A printable character to insert (already resolved from the layout).
    Char(char),
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    /// Enter: a newline in a multiline field, ignored in a single-line one.
    Enter,
}

/// One window's gesture state: the in-progress drag, if any. The front holds
/// one per window (the browser's single canvas holds one).
#[derive(Default)]
pub struct Gestures {
    drag: Option<Drag>,
}

impl Gestures {
    /// Whether a drag is in progress (the front routes cursor motion here).
    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// The selection marquee in flight, if any: the `patch` widget and the
    /// rectangle between the press and the cursor (device pixels), for the
    /// renderer to draw over the patch.
    pub fn marquee(&self) -> Option<(i32, Rect)> {
        match &self.drag {
            Some(Drag::Marquee {
                id, origin, cursor, ..
            }) => Some((*id, corner_rect(*origin, *cursor))),
            _ => None,
        }
    }

    /// The cord drag in flight, if any: the `patch` widget and the grabbed port
    /// `(box, side, index)` (the renderer draws the cord to the pointer).
    pub fn wiring(&self) -> Option<(i32, (usize, super::patch::Side, usize))> {
        match &self.drag {
            Some(Drag::Wire { id, port, .. }) => Some((*id, *port)),
            _ => None,
        }
    }

    /// Whether the active drag is a *locked* one — driven by relative deltas
    /// ([`Self::relative_motion`]), not by cursor positions. What an element
    /// asked for and the front granted.
    pub fn locked(&self) -> bool {
        matches!(self.drag, Some(Drag::Element { grab: true, .. }))
    }

    /// Whether a clip drag is currently held against a lane's edge, so the
    /// front must keep ticking ([`Self::tick`]) even though the pointer is
    /// standing still — a held cursor produces no events, and the view has to
    /// keep moving under it.
    pub fn edge_scrolling(&self, cx: f64) -> bool {
        self.edge_direction(cx) != 0.0
    }

    /// Which way a clip drag at `cx` pulls the view: `-1` past the left edge,
    /// `+1` past the right, `0` when the cursor is clear of both margins (or no
    /// clip drag is in flight). The margin reaches *outside* the body too, so a
    /// cursor pinned at the window's own edge keeps scrolling.
    fn edge_direction(&self, cx: f64) -> f64 {
        let Some(Drag::Clip { body_x, body_w, .. }) = self.drag else {
            return 0.0;
        };
        if body_w <= 2.0 * EDGE_MARGIN {
            return 0.0;
        }
        if cx < body_x + EDGE_MARGIN {
            -1.0
        } else if cx > body_x + body_w - EDGE_MARGIN {
            1.0
        } else {
            0.0
        }
    }
}

mod drag;
mod effects;
mod element;
mod keys;
mod nav;
mod press;
mod wheel;

use nav::*;

pub(crate) use nav::corner_rect;

#[cfg(test)]
mod tests;
