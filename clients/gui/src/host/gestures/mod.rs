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
use super::widget::WidgetKind;

/// What a gesture asks of the front: everything the machine cannot do itself
/// because it owns no transport, no window and no GPU.
#[derive(Debug, PartialEq)]
pub enum GestureEffect {
    /// Emit `/gui_event widget_id <args…>` to the script behind window `def_id`.
    Emit {
        def_id: i32,
        widget_id: i32,
        /// The stamp this edit went out with, so the owner's acknowledgement
        /// can name it (see [`crate::host::ack`]). Rides as the **second**
        /// argument of `/gui_event`, before the tag, so one rule reads every
        /// event whatever its payload.
        seq: i32,
        args: Vec<OscType>,
    },
    /// Repaint the window rooted at this def id (a gesture on one window may
    /// touch linked views in others, so this is not always the pressed window).
    Redraw(i32),
    /// Release the pointer grab a knob/number drag took on window `def_id`.
    ReleasePointer(i32),
    /// The keyboard focus **left this window's tree** — Tab stepped past the
    /// last stop on the ring, or there was no ring at all.
    ///
    /// A desktop front has nothing to do about it (nothing is focused, and the
    /// next Tab enters the ring again); a page **blurs its canvas**, so the
    /// browser's own tab order carries on past the mounted GuiDef instead of
    /// trapping the reader inside it.
    FocusOut(i32),
}

/// The per-call context the front supplies: which window, its framebuffer size
/// in device pixels, the modifier keys, and what the heavy views actually have
/// on the card (which only the front's GPU slots know, so it is snapshotted
/// here).
pub struct GestureCtx {
    pub def_id: i32,
    pub fb_w: u32,
    pub fb_h: u32,
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    /// The channel count the front found in each widget's GPU slot, by widget
    /// id — a waveform's channels, a spectrogram's analysis lanes: what is
    /// actually on the card, which is the only half of the answer the front
    /// has. How a widget *arranges* them is the widget's
    /// ([`WidgetKind::lanes`]), and a widget missing here counts as one lane.
    pub slot_channels: HashMap<i32, usize>,
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
            slot_channels: HashMap::new(),
            sample_rate: 0.0,
        }
    }

    /// The lane count a widget stacks on screen — the divisor for
    /// lane-relative y gestures. The two halves of one answer: what the front
    /// uploaded, and what the widget makes of it.
    fn lanes(&self, id: i32, kind: &WidgetKind) -> usize {
        kind.lanes(self.slot_channels.get(&id).copied().unwrap_or(1))
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
    /// delta — plus whether it asked for the axis under it to keep scrolling
    /// while the cursor is held past an edge, which is the group's to pan and
    /// not the element's ([`Take::edge_scroll`](super::widget::element::Take::edge_scroll)).
    Element {
        at: element::At,
        grab: bool,
        edge: bool,
    },
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
    /// the shared selection every linked view follows. An element that sweeps
    /// a *rectangle* over that span — a roll picking the notes inside it —
    /// takes the press itself and asks for the selection, so this stays the
    /// container's plain time sweep.
    Select {
        id: i32,
        body: Rect,
        nav_start: f64,
        nav_len: f64,
        anchor: f64,
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
    /// Dragging a lane header's level fader: the cursor's x over the fader's
    /// rectangle is the value, so the press itself already sets it.
    LaneLevel { id: i32, rect: Rect },
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

    /// What this drag is holding, in the terms the frame draws affordances by
    /// ([`crate::host::frame::Grab`]) — the clip and the edge of it, or that
    /// something else has the pointer.
    pub(crate) fn grab(&self) -> crate::host::frame::Grab {
        use crate::host::frame::Grab;
        use crate::host::graphics::track::ClipSide;
        match &self.drag {
            None => Grab::None,
            Some(Drag::Clip { id, part, .. }) => Grab::Clip(
                *id,
                match part {
                    interact::ClipPart::Start => Some(ClipSide::Start),
                    interact::ClipPart::End => Some(ClipSide::End),
                    interact::ClipPart::Body => None,
                },
            ),
            Some(_) => Grab::Other,
        }
    }

    /// Whether the active drag is a *locked* one — driven by relative deltas
    /// ([`Self::relative_motion`]), not by cursor positions. What an element
    /// asked for and the front granted.
    pub fn locked(&self) -> bool {
        matches!(self.drag, Some(Drag::Element { grab: true, .. }))
    }

    /// Whether a drag is currently held against the edge of the axis it is on,
    /// so the front must keep ticking ([`Self::tick`]) even though the pointer
    /// is standing still — a held cursor produces no events, and the view has
    /// to keep moving under it.
    pub fn edge_scrolling(&self, cx: f64) -> bool {
        self.edge_direction(cx) != 0.0
    }

    /// Which way the drag at `cx` pulls the view: `-1` past the left edge, `+1`
    /// past the right, `0` when the cursor is clear of both margins (or no
    /// scrolling drag is in flight). The margin reaches *outside* the body too,
    /// so a cursor pinned at the window's own edge keeps scrolling.
    ///
    /// The two drags that ask for it name their body differently — a clip's is
    /// the lane's, an element's is its own rect past the group's gutter — and
    /// the arithmetic after that is one.
    fn edge_direction(&self, cx: f64) -> f64 {
        let (body_x, body_w) = match self.drag {
            Some(Drag::Clip { body_x, body_w, .. }) => (body_x, body_w),
            Some(Drag::Element { at, edge: true, .. }) => (
                (at.rect.x + at.indent) as f64,
                (at.rect.w - at.indent).max(0.0) as f64,
            ),
            _ => return 0.0,
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
mod focus;
mod keys;
mod nav;
mod press;
mod wheel;

use nav::*;

// The rubber band is the patcher's alone (a marquee over its boxes).
#[cfg(feature = "patcher")]
pub(crate) use nav::corner_rect;

#[cfg(test)]
mod tests;
