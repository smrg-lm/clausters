//! Shared timeline navigation groups: the linked-views state model.
//!
//! Different views of the same sound must navigate as one — the classic editor
//! layout is a waveform lane and a spectrogram lane under one time axis with
//! one selection. This module extracts the per-widget navigation/selection/
//! playhead state (formerly a per-slot [`View`] in each front) into one
//! **group** model owned by the host core: a horizontal [`View`] window, the
//! selection and the playhead anchor, shared by every member widget. Both
//! widget kinds and both fronts drive the same component, so the navigation
//! logic lands once; per-widget state keeps only the vertical axis
//! (`y_start`/`y_len` on `EditorProps`).
//!
//! Grouping is **explicit**: a timeline widget with a `link` (int) prop joins
//! the group of that id; without one it gets a private group keyed by its own
//! widget id ([`GroupKey::Solo`]). Explicit rather than by shared source,
//! because an editor item may also want *unlinked* views of one file, and a
//! link may span sources (aligned takes). Membership is live: a `/gui_set
//! link` moves a widget between groups (a negative link unlinks it, keeping
//! its current view).
//!
//! The group state navigates in **timeline sample units**, not in any one
//! member's buffer: the group's length is the maximum of its members'
//! registered data extents (a shorter member simply ends earlier), and a group
//! may span windows. That shape is what the multitrack (DAW-style) view is
//! built on: a clip there is a member with a *placement* — a start offset on
//! the same shared timeline, set through [`Host::set_timeline_offset`] — so a
//! lane of clips and a pair of linked lanes are one model, not two.
//!
//! Selection and playhead live in the group and **only** there: every reader
//! — the frame renderer, the animation demand, the readouts — resolves the
//! member's key and reads the shared state, so there is nothing to keep in
//! step. A member's own `sel_*`/`playhead*` props are the *seed* a fresh
//! group takes its def-time values from, and are inert from then on.

use std::collections::HashMap;

use serde_json::Value;

use crate::viewport::View;

use super::layout::Rect;
use super::metrics::Metrics;
use super::widget::{EditorProps, RulerY, Widget, WidgetKind};
use super::{Host, HostEffect};

/// The navigation group a timeline widget belongs to.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum GroupKey {
    /// An explicit `link` group id, shared by every member declaring it.
    Link(i32),
    /// The private group of an unlinked widget, keyed by its own widget id.
    Solo(i32),
}

/// The group key of a timeline widget: its explicit `link`, or itself.
pub fn group_key(widget_id: i32, link: Option<i32>) -> GroupKey {
    match link {
        Some(group) => GroupKey::Link(group),
        None => GroupKey::Solo(widget_id),
    }
}

/// The shared state of one navigation group: the visible window, the
/// selection and the playhead anchor, all in timeline sample units.
///
/// Every member reads this one value — it is what a linked view *is* — so it
/// is `Copy`: a reader takes the state it draws with by value and the group
/// stays the only place it is written.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GroupState {
    /// The visible horizontal window.
    pub nav: View,
    /// Selection start (samples).
    pub sel_start: f64,
    /// Selection length (samples; `<= 0` = no selection).
    pub sel_len: f64,
    /// The engine sample-clock value mapping to timeline sample 0 (negative =
    /// no playhead) — the same convention as `EditorProps::playhead_at`.
    pub playhead_at: f64,
    /// The static cursor of a located, stopped transport (`< 0` = none).
    pub playhead: f64,
    /// The sweep's loop region (samples; `len <= 0` = the straight pass) — the
    /// same convention as `EditorProps::playhead_loop_start`/`_len`. Group-wide
    /// like the anchor: linked views must wrap the line at the same place, or
    /// one file's waveform and spectrogram would disagree about where it is.
    pub playhead_loop_start: f64,
    pub playhead_loop_len: f64,
}

impl GroupState {
    /// A fresh group seeded from a member's editor props — the def-time
    /// selection and playhead — over the window `nav`.
    pub(crate) fn seed(editor: &EditorProps, nav: View) -> GroupState {
        GroupState {
            nav,
            sel_start: editor.sel_start,
            sel_len: editor.sel_len,
            playhead_at: editor.playhead_at,
            playhead: editor.playhead,
            playhead_loop_start: editor.playhead_loop_start,
            playhead_loop_len: editor.playhead_loop_len,
        }
    }

    /// The selection as `(start, len)` in timeline samples, if one is active.
    pub fn selection(&self) -> Option<(f64, f64)> {
        (self.sel_len > 0.0).then_some((self.sel_start, self.sel_len))
    }

    /// Where the playhead stands, in timeline sample units, for an engine
    /// `sample_clock` — the one place the sweep is defined, so every member of
    /// the group agrees.
    ///
    /// A transport that is *playing* anchors `playhead_at` and the line is
    /// swept from Rust every frame with no message per frame; a *located,
    /// stopped* one parks on the static `playhead`. `playhead_loop_len > 0`
    /// wraps the sweep inside `[playhead_loop_start, +len)` — what a looping
    /// region (an editor's "play selection", a looping clip) actually does —
    /// and leaves the straight pass untouched otherwise.
    pub fn head_at(&self, sample_clock: f64) -> Option<f64> {
        self.swept_at(sample_clock)
            .or_else(|| (self.playhead >= 0.0).then_some(self.playhead))
    }

    /// Where the *swept* playhead stands — `Some` only while a transport is
    /// running (`playhead_at` anchored, the clock started), so a caller that
    /// must tell playing from stopped keeps that distinction; [`head_at`] adds
    /// the parked cursor on top.
    ///
    /// [`head_at`]: GroupState::head_at
    pub fn swept_at(&self, sample_clock: f64) -> Option<f64> {
        if self.playhead_at < 0.0 || sample_clock <= 0.0 {
            return None;
        }
        let swept = sample_clock - self.playhead_at;
        Some(match self.playhead_loop() {
            // `rem_euclid`, not `%`: a loop whose start sits past the anchor
            // makes the first pass negative, and a negative remainder would
            // park the line left of the region.
            Some((start, len)) => start + (swept - start).rem_euclid(len),
            None => swept,
        })
    }

    /// The playhead's loop region as `(start, len)` in samples, if one is set.
    pub fn playhead_loop(&self) -> Option<(f64, f64)> {
        (self.playhead_loop_len > 0.0)
            .then_some((self.playhead_loop_start.max(0.0), self.playhead_loop_len))
    }
}

/// What a timeline member would reserve **left of its body** for chrome of its
/// own: a lane's header, a piano-roll's keyboard gutter, a heavy view's value
/// ruler. A `timeruler` asks for nothing — it has no chrome, it only labels
/// whatever axis it follows.
///
/// This is the member's *wish*, not where its body actually begins: that is
/// [`group_indents`], the group's own. See it for why.
pub(crate) fn own_gutter(kind: &WidgetKind, metrics: &Metrics) -> f32 {
    match kind {
        WidgetKind::Track { header, .. } => header.width(metrics),
        WidgetKind::PianoRoll { .. } => super::pianoroll::KEYBOARD_W,
        WidgetKind::Signal(el) if el.editor.ruler_y != RulerY::Off => metrics.ruler_w,
        _ => 0.0,
    }
}

/// The x offset, inside a member's own rect, where the shared time axis begins
/// — and therefore where every member of the group draws its body.
///
/// It is the **widest** gutter any member of the group asks for, because the
/// alternative is what the catalog did until now: each widget indented by its
/// own idea of a gutter, so a lane, a roll and a ruler stacked on one axis
/// started that axis at three different x and the same sample sat at three
/// different pixels. The indent is a property of the axis, not of the widget
/// beside it. A member whose own chrome is narrower than the shared indent
/// simply draws it into the wider band.
///
/// A group with one member is its own gutter, which is why a solo view is
/// exactly where it always was.
/// The shared indent of every navigation group in one window's tree.
///
/// It is read from the **tree** rather than from the placements because it is a
/// fact about the *kinds* on an axis — which of them wants a header, a keyboard
/// or a value ruler — so it is known before a single rectangle is, which is
/// what lets the layout pass place a lane's clips with it. The layout stamps
/// the answer on every placement ([`super::layout::Placed::indent`]), and the
/// renderer and the hit-test read it from there, so a clip is dragged on the
/// pixels it was drawn on.
pub(crate) fn group_indents(tree: &Widget, metrics: &Metrics) -> HashMap<GroupKey, f32> {
    let mut out: HashMap<GroupKey, f32> = HashMap::new();
    for widget in tree.descendants() {
        if let (Some(id), Some(editor)) = (widget.id, widget.kind.editor()) {
            let slot = out.entry(group_key(id, editor.link)).or_insert(0.0f32);
            *slot = slot.max(own_gutter(&widget.kind, metrics));
        }
    }
    out
}

/// The chrome band of a member: everything left of the shared body, full
/// height. A lane draws its header here, a roll its keys, a heavy view its
/// value ruler — each into the whole band, so the band's right edge (which is
/// the axis' left edge) is the one they agree on.
pub(crate) fn gutter_band(rect: Rect, indent: f32) -> Rect {
    Rect::new(rect.x, rect.y, indent.min(rect.w), rect.h)
}

/// The host-owned store of every navigation group, plus the per-widget data
/// extents the group lengths aggregate over.
#[derive(Default)]
pub struct TimelineGroups {
    states: HashMap<GroupKey, GroupState>,
    /// Per-widget data extent in samples, registered by the fronts when a
    /// view's data loads; a group's timeline length is the max over members.
    totals: HashMap<i32, usize>,
}

impl TimelineGroups {
    /// The state of group `key`, if it exists.
    pub fn state(&self, key: GroupKey) -> Option<&GroupState> {
        self.states.get(&key)
    }

    /// The navigation window of group `key`, if it exists.
    pub fn nav(&self, key: GroupKey) -> Option<View> {
        self.states.get(&key).map(|s| s.nav)
    }

    /// The registered data extent of widget `id` (0 when none loaded yet).
    pub fn total_of(&self, id: i32) -> usize {
        self.totals.get(&id).copied().unwrap_or(0)
    }
}

/// One timeline widget in some window: where it lives, which group it is in,
/// and its placement (start offset in timeline samples) on that group.
#[derive(Clone, Copy)]
struct Member {
    root: i32,
    id: i32,
    key: GroupKey,
    offset: f64,
}

impl Host {
    /// Read access to the navigation groups (the frame renderer reads the nav
    /// window per timeline view from here).
    pub fn timelines(&self) -> &TimelineGroups {
        &self.timelines
    }

    /// Every timeline widget in every window def, with its group key.
    fn timeline_members(&self) -> Vec<Member> {
        let mut out = Vec::new();
        for (root, tree) in &self.window_defs {
            out.extend(tree.descendants().filter_map(|w| {
                let editor = w.kind.editor()?;
                Some(Member {
                    root: *root,
                    id: w.id?,
                    key: group_key(w.id?, editor.link),
                    offset: editor.offset,
                })
            }));
        }
        out
    }

    /// The group key of timeline widget `id`, if it exists in some window.
    pub fn timeline_key(&self, id: i32) -> Option<GroupKey> {
        self.timeline_members()
            .iter()
            .find(|m| m.id == id)
            .map(|m| m.key)
    }

    /// The timeline length of group `key`: the max of its members' **placed**
    /// data extents — each member's data occupies `[offset, offset + extent]`
    /// on the shared timeline, so a clip placed late lengthens the group. A
    /// group with nothing in it yet falls back to the axis it navigates empty
    /// (see [`timeline_empty_span`]), so it is never zero.
    ///
    /// [`timeline_empty_span`]: Host::timeline_empty_span
    pub fn timeline_total(&self, key: GroupKey) -> usize {
        let content = self
            .timeline_members()
            .iter()
            .filter(|m| m.key == key)
            .map(|m| m.offset.max(0.0).ceil() as usize + self.timelines.total_of(m.id))
            .max()
            .unwrap_or(0);
        // Content is the axis once there *is* content; the empty span only
        // stands in for a group that has none yet (>= 1, so it still navigates).
        if content == 0 {
            self.timeline_empty_span(key)
        } else {
            content
        }
    }

    /// How far past its content an **authoring** group may be navigated: a lane
    /// or a roll must be zoomable *out* into empty time, or there is nowhere to
    /// drag a clip to and nowhere to record into — the composition would only
    /// ever grow by dropping something beyond the visible edge. The heavy views
    /// keep their content-bound axis (there is no signal out there to look at),
    /// so the headroom applies only to a group that holds an authoring surface.
    const NAV_HEADROOM: usize = 4;

    /// The empty time a `pianoroll` group navigates over, in beats — the axis a
    /// roll has *before* its content does. A roll is written into (drawn, or
    /// recorded from MIDI), so its timeline is not its notes: it is a grid that
    /// exists first and scrolls, like a DAW's lane while it records. Without it
    /// an empty roll navigates the single sample [`View::full`] floors to, and
    /// every note painted into it lands outside the window — the roll stays
    /// blank however much is written. Four bars of 4/4, read off its own grid.
    ///
    /// It is a **floor on the axis, not on the content**: `timeline_total` stays
    /// the honest extent of what is there, so nothing that measures the material
    /// (a lane's clips, a hit-test) sees this number.
    const EMPTY_BEATS: f64 = 16.0;

    /// The sample rate assumed for a roll that declares none. The timeline's
    /// unit is samples either way; this only scales the empty axis it starts on,
    /// before there is anything drawn against it.
    const ASSUMED_RATE: f64 = 48_000.0;

    /// The length the navigation window is clamped against — the group's content
    /// plus the authoring headroom, and never less than a roll's empty grid (so
    /// the window a roll opened on survives its first note arriving).
    fn timeline_span(&self, key: GroupKey) -> usize {
        let total = self.timeline_total(key);
        if self.group_has_lane(key) || self.roll_grid(key).is_some() {
            total
                .saturating_mul(Self::NAV_HEADROOM)
                .max(self.timeline_empty_span(key))
        } else {
            total
        }
    }

    /// The empty axis group `key` navigates before it holds anything:
    /// [`EMPTY_BEATS`] of a roll's own grid, or one sample for a group with no
    /// roll (a view of a given signal has nothing to show out there, and a lane
    /// spans its clips).
    ///
    /// [`EMPTY_BEATS`]: Host::EMPTY_BEATS
    fn timeline_empty_span(&self, key: GroupKey) -> usize {
        let Some((rate, tempo)) = self.roll_grid(key) else {
            return 1;
        };
        let rate = if rate > 0.0 { rate } else { Self::ASSUMED_RATE };
        let tempo = if tempo > 0.0 { tempo } else { 1.0 };
        (rate * Self::EMPTY_BEATS / tempo).ceil() as usize
    }

    /// The `(sample_rate, tempo)` grid of the first `pianoroll` in group `key`,
    /// if it holds one — the surface whose axis exists before its content.
    fn roll_grid(&self, key: GroupKey) -> Option<(f64, f64)> {
        self.window_defs.values().find_map(|tree| {
            tree.descendants().find_map(|w| {
                let editor = w.kind.editor()?;
                (matches!(w.kind, super::widget::WidgetKind::PianoRoll { .. })
                    && group_key(w.id?, editor.link) == key)
                    .then_some((editor.sample_rate, editor.tempo))
            })
        })
    }

    /// Whether group `key` holds a `track` lane (a multitrack axis).
    fn group_has_lane(&self, key: GroupKey) -> bool {
        self.window_defs.values().any(|tree| {
            tree.descendants().any(|w| {
                matches!(w.kind, super::widget::WidgetKind::Track { .. })
                    && w.id
                        .zip(w.kind.editor())
                        .is_some_and(|(id, editor)| group_key(id, editor.link) == key)
            })
        })
    }

    /// The navigation window and timeline length of widget `id`'s group.
    pub fn timeline_nav(&self, id: i32) -> Option<(View, usize)> {
        let key = self.timeline_key(id)?;
        let nav = self.timelines.nav(key)?;
        Some((nav, self.timeline_total(key)))
    }

    /// The distinct window roots showing any member of group `key` — the
    /// windows a group mutation must repaint.
    fn timeline_roots(&self, key: GroupKey) -> Vec<i32> {
        let mut roots = Vec::new();
        for m in self.timeline_members() {
            if m.key == key && !roots.contains(&m.root) {
                roots.push(m.root);
            }
        }
        roots
    }

    /// Registers widget `id`'s data extent (called by the fronts when a view's
    /// data loads). A group that was showing its full timeline keeps showing
    /// all of it as the timeline grows; a zoomed one just re-clamps.
    pub fn set_timeline_total(&mut self, id: i32, total: usize) {
        self.set_timeline_total_inner(id, total, true);
    }

    /// Registers an extent **without refitting** a window that happens to be
    /// showing the whole timeline — the variant a *gesture* uses.
    ///
    /// The refit below ("it was showing it all, keep showing it all") is right
    /// when the content changes under a still view: a def arriving, a `/gui_set`
    /// moving a clip. Under a drag it is wrong, and visibly so — every step
    /// grows the content, the window grows with it, and dragging a clip rightward
    /// *zooms the axis out* from under the cursor instead of scrolling. A DAW
    /// scrolls at constant zoom, so a dragged extent keeps the window's length.
    pub fn set_timeline_total_keeping_view(&mut self, id: i32, total: usize) {
        self.set_timeline_total_inner(id, total, false);
    }

    fn set_timeline_total_inner(&mut self, id: i32, total: usize, refit: bool) {
        let Some(key) = self.timeline_key(id) else {
            self.timelines.totals.insert(id, total);
            return;
        };
        let old_total = self.timeline_total(key);
        self.timelines.totals.insert(id, total);
        let new_total = self.timeline_total(key);
        let span = self.timeline_span(key);
        if let Some(state) = self.timelines.states.get_mut(&key) {
            if refit && (state.nav.len - old_total as f64).abs() < 1.0 {
                // It was showing exactly the whole timeline: keep showing it all.
                state.nav = View::full(new_total);
            } else {
                // Zoomed in *or* zoomed out into the empty headroom: keep the
                // window, only re-clamp it (growing the content must not yank a
                // deliberately zoomed-out view back onto the content).
                state.nav.set_start(state.nav.start, span);
            }
        }
    }

    /// Registers every `track` lane's extent — the end of its last clip — with
    /// its navigation group, so the shared axis spans the composition. A lane's
    /// "data" is its clips, so this is the lane's answer to the data extent the
    /// fronts register for a loaded waveform, and it must be re-run whenever a
    /// clip moves or resizes (a `/gui_def`, a `/gui_set`, or a drag).
    pub(super) fn sync_track_totals(&mut self) {
        self.sync_track_totals_inner(true);
    }

    /// The same registration, keeping the window's length — what a **drag**
    /// calls, so extending the content scrolls the axis instead of zooming it
    /// out from under the cursor (see [`set_timeline_total_keeping_view`]).
    ///
    /// [`set_timeline_total_keeping_view`]: Host::set_timeline_total_keeping_view
    pub(super) fn sync_track_totals_keeping_view(&mut self) {
        self.sync_track_totals_inner(false);
    }

    fn sync_track_totals_inner(&mut self, refit: bool) {
        /// A surface's own extent in samples: a lane spans its clips, a roll
        /// spans its material (the end of its last note and its last event).
        fn extent(widget: &Widget) -> Option<(i32, usize)> {
            let span = match (&widget.id?, &widget.kind) {
                (_, super::widget::WidgetKind::Track { .. }) => super::track::clips_span(widget),
                (_, super::widget::WidgetKind::PianoRoll { notes, osc, .. }) => {
                    let notes = notes.iter().map(|n| n.start + n.dur);
                    let events = osc.iter().map(|m| m.time);
                    notes.chain(events).fold(0.0f64, f64::max)
                }
                _ => return None,
            };
            Some((widget.id?, span.ceil().max(0.0) as usize))
        }
        let mut lanes = Vec::new();
        for tree in self.window_defs.values() {
            lanes.extend(tree.descendants().filter_map(extent));
        }
        for (id, span) in lanes {
            if refit {
                self.set_timeline_total(id, span);
            } else {
                self.set_timeline_total_keeping_view(id, span);
            }
        }
    }

    /// Anchor-preserving zoom of widget `id`'s group. Returns the roots to
    /// repaint (empty when the widget is in no group).
    pub fn zoom_timeline(&mut self, id: i32, factor: f64, anchor: f64) -> Vec<i32> {
        let Some(key) = self.timeline_key(id) else {
            return Vec::new();
        };
        let total = self.timeline_span(key);
        let Some(state) = self.timelines.states.get_mut(&key) else {
            return Vec::new();
        };
        state.nav.zoom(factor, anchor, total);
        self.timeline_roots(key)
    }

    /// Absolute pan of widget `id`'s group to window start `start` (clamped).
    /// Returns the roots to repaint.
    pub fn pan_timeline(&mut self, id: i32, start: f64) -> Vec<i32> {
        let Some(key) = self.timeline_key(id) else {
            return Vec::new();
        };
        let total = self.timeline_span(key);
        let Some(state) = self.timelines.states.get_mut(&key) else {
            return Vec::new();
        };
        state.nav.set_start(start, total);
        self.timeline_roots(key)
    }

    /// Sets widget `id`'s group window from the `/gui_set` `view_start`/
    /// `view_len` keys (either alone keeps the other; a non-positive length
    /// resets to the full timeline). Returns the roots to repaint.
    pub fn set_timeline_view(&mut self, id: i32, start: Option<f64>, len: Option<f64>) -> Vec<i32> {
        let Some(key) = self.timeline_key(id) else {
            return Vec::new();
        };
        let span = self.timeline_span(key);
        let total = self.timeline_total(key);
        let Some(state) = self.timelines.states.get_mut(&key) else {
            return Vec::new();
        };
        match len {
            Some(len) if len <= 0.0 => {
                // A reset shows the *content*, not the headroom.
                state.nav = View::full(total);
                return self.timeline_roots(key);
            }
            Some(len) => state.nav.len = len.max(1.0),
            None => {}
        }
        let start = start.unwrap_or(state.nav.start);
        state.nav.set_start(start, span);
        self.timeline_roots(key)
    }

    /// Resets widget `id`'s group to the full timeline. Returns the roots to
    /// repaint.
    pub fn reset_timeline(&mut self, id: i32) -> Vec<i32> {
        self.set_timeline_view(id, None, Some(0.0))
    }

    /// Writes the selection spanning timeline samples `a..b` (any order,
    /// clamped) into widget `id`'s group and mirrors it into every member.
    /// Returns `(start, len, roots to repaint)` — the gesture path.
    pub fn select_timeline(&mut self, id: i32, a: f64, b: f64) -> Option<(f64, f64, Vec<i32>)> {
        let key = self.timeline_key(id)?;
        let total = self.timeline_total(key) as f64;
        let (a, b) = (a.clamp(0.0, total), b.clamp(0.0, total));
        let (start, len) = (a.min(b), (a - b).abs());
        let roots = self.set_timeline_selection(id, Some(start), Some(len));
        Some((start, len, roots))
    }

    /// Sets widget `id`'s group selection from the `/gui_set` `sel_start`/
    /// `sel_len` keys (either alone keeps the other). Returns the roots to
    /// repaint — every member draws the group's selection, so they all do.
    pub fn set_timeline_selection(
        &mut self,
        id: i32,
        start: Option<f64>,
        len: Option<f64>,
    ) -> Vec<i32> {
        let Some(key) = self.timeline_key(id) else {
            return Vec::new();
        };
        let Some(state) = self.timelines.states.get_mut(&key) else {
            return Vec::new();
        };
        if let Some(start) = start {
            state.sel_start = start;
        }
        if let Some(len) = len {
            state.sel_len = len;
        }
        self.timeline_roots(key)
    }

    /// Sets widget `id`'s group playhead anchor. Returns the roots to repaint
    /// (the anchor is group-wide: every member sweeps from it).
    pub fn set_timeline_playhead(&mut self, id: i32, at: f64) -> Vec<i32> {
        let Some(key) = self.timeline_key(id) else {
            return Vec::new();
        };
        let Some(state) = self.timelines.states.get_mut(&key) else {
            return Vec::new();
        };
        state.playhead_at = at;
        self.timeline_roots(key)
    }

    /// Sets the group's **static cursor** — where a located, stopped transport
    /// sits. Group-wide, so all the lanes show one cursor.
    pub fn set_timeline_cursor(&mut self, id: i32, pos: f64) -> Vec<i32> {
        let Some(key) = self.timeline_key(id) else {
            return Vec::new();
        };
        let Some(state) = self.timelines.states.get_mut(&key) else {
            return Vec::new();
        };
        state.playhead = pos;
        self.timeline_roots(key)
    }

    /// Sets the group's **playhead loop region** — where the swept line wraps
    /// (samples; a non-positive length restores the straight pass). Group-wide,
    /// so linked views wrap at one place.
    pub fn set_timeline_playhead_loop(
        &mut self,
        id: i32,
        start: Option<f64>,
        len: Option<f64>,
    ) -> Vec<i32> {
        let Some(key) = self.timeline_key(id) else {
            return Vec::new();
        };
        let Some(state) = self.timelines.states.get_mut(&key) else {
            return Vec::new();
        };
        if let Some(start) = start {
            state.playhead_loop_start = start;
        }
        if let Some(len) = len {
            state.playhead_loop_len = len;
        }
        self.timeline_roots(key)
    }

    /// Sets widget `id`'s **placement** (start offset in timeline samples) on
    /// its group. Unlike the group-wide keys this is written to the one
    /// widget's own editor props (each clip carries its own offset), but it
    /// changes the group's timeline length, so the group window is re-clamped
    /// and every member repaints. Returns the roots to repaint.
    pub fn set_timeline_offset(&mut self, id: i32, offset: f64) -> Vec<i32> {
        let Some(key) = self.timeline_key(id) else {
            return Vec::new();
        };
        let offset = offset.max(0.0);
        let old_total = self.timeline_total(key);
        for root in self.window_defs.values_mut() {
            if let Some(editor) = root.find_mut(id).and_then(|w| w.kind.editor_mut()) {
                editor.offset = offset;
            }
        }
        // The placement changed the group length: a group showing its whole
        // timeline keeps showing all of it (follows the growth/shrink); a
        // zoomed one just re-clamps — the same rule as `set_timeline_total`.
        let new_total = self.timeline_total(key);
        if let Some(state) = self.timelines.states.get_mut(&key) {
            if state.nav.len >= old_total as f64 {
                state.nav = View::full(new_total);
            } else {
                state.nav.set_start(state.nav.start, new_total);
            }
        }
        self.timeline_roots(key)
    }

    /// Moves widget `id` to the group of `link` (`None` unlinks it into its
    /// private group). The widget *carries its current group state along* when
    /// the target group does not exist yet — unlinking keeps the view it had,
    /// diverging from there — and *adopts* the target group's state when it
    /// does. Returns the roots to repaint (the old and the new group's).
    pub fn set_timeline_link(&mut self, id: i32, link: Option<i32>) -> Vec<i32> {
        let Some(old_key) = self.timeline_key(id) else {
            return Vec::new();
        };
        let mut roots = self.timeline_roots(old_key);
        let carried = self.timelines.states.get(&old_key).cloned();
        // Write the new membership into the widget's editor props.
        for root in self.window_defs.values_mut() {
            if let Some(editor) = root.find_mut(id).and_then(|w| w.kind.editor_mut()) {
                editor.link = link;
            }
        }
        let new_key = group_key(id, link);
        self.timelines.states.entry(new_key).or_insert_with(|| {
            carried.unwrap_or(GroupState {
                nav: View::full(1),
                sel_start: 0.0,
                sel_len: 0.0,
                playhead_at: -1.0,
                playhead: -1.0,
                playhead_loop_start: 0.0,
                playhead_loop_len: 0.0,
            })
        });
        // Re-clamp the (carried or adopted) window against the new membership
        // and align every member with the group.
        let total = self.timeline_total(new_key);
        if let Some(state) = self.timelines.states.get_mut(&new_key) {
            let start = state.nav.start;
            state.nav.set_start(start, total);
        }
        self.prune_timeline_groups();
        for root in self.timeline_roots(new_key) {
            if !roots.contains(&root) {
                roots.push(root);
            }
        }
        roots
    }

    /// Ensures every timeline widget's group exists. Called after a `/gui_def`
    /// builds (or rebuilds) a window tree:
    /// `redefined` names that def, whose widgets get rebuild semantics — any
    /// group confined to that def is reseeded fresh; a group spanning other
    /// windows survives and the redefined members adopt it. Data extents are
    /// kept (the fronts re-register them as the new data loads, and the prune
    /// below drops the ones whose widgets are gone).
    pub(super) fn sync_timeline_groups(&mut self, redefined: Option<i32>) {
        // A lane's extent is its clips (no data loads for it), so register it
        // here, before the groups seed their windows from the totals.
        self.sync_track_totals();
        let members = self.timeline_members();
        if let Some(def) = redefined {
            let spans_other: Vec<GroupKey> = members
                .iter()
                .filter(|m| m.root != def)
                .map(|m| m.key)
                .collect();
            for m in members.iter().filter(|m| m.root == def) {
                if !spans_other.contains(&m.key) {
                    self.timelines.states.remove(&m.key);
                }
            }
        }
        // The first member of a fresh group seeds it from its own def-time
        // editor props; the rest of the members read what it seeded.
        for m in &members {
            if !self.timelines.states.contains_key(&m.key) {
                let editor = self
                    .window_defs
                    .get(&m.root)
                    .and_then(|t| t.find(m.id))
                    .and_then(|w| w.kind.editor())
                    .cloned();
                if let Some(editor) = editor {
                    let nav = View::full(self.timeline_total(m.key));
                    self.timelines
                        .states
                        .insert(m.key, GroupState::seed(&editor, nav));
                }
            }
        }
        self.prune_timeline_groups();
    }

    /// Drops group states and data extents whose widgets no longer exist (after
    /// a `/gui_free` or a redefining `/gui_def`) — the timeline sibling of
    /// `prune_bindings`.
    pub(super) fn prune_timeline_groups(&mut self) {
        let members = self.timeline_members();
        self.timelines
            .states
            .retain(|key, _| members.iter().any(|m| m.key == *key));
        self.timelines
            .totals
            .retain(|id, _| members.iter().any(|m| m.id == *id));
    }

    /// Routes the shared timeline keys of one `/gui_set` on timeline widget
    /// `id` through the group model — `view_start`/`view_len`, `sel_start`/
    /// `sel_len`, `playhead_at`, the `playhead_loop_*` pair and `link` (a
    /// negative link unlinks) apply group-wide; every other key is applied to
    /// the widget itself by the caller. Pushes a redraw effect per affected window.
    pub(super) fn set_timeline_props(
        &mut self,
        id: i32,
        props: &[(String, Value)],
        effects: &mut Vec<HostEffect>,
    ) {
        // A start/len pair gathered from one message, so setting both keys in
        // one `/gui_set` is order-independent.
        type Span = (Option<f64>, Option<f64>);
        let mut roots: Vec<i32> = Vec::new();
        let (mut view, mut sel): (Span, Span) = ((None, None), (None, None));
        let mut loop_span: Span = (None, None);
        for (k, v) in props {
            match k.as_str() {
                "view_start" => view.0 = v.as_f64(),
                "view_len" => view.1 = v.as_f64(),
                "sel_start" => sel.0 = v.as_f64(),
                "sel_len" => sel.1 = v.as_f64(),
                "playhead_loop_start" => loop_span.0 = v.as_f64(),
                "playhead_loop_len" => loop_span.1 = v.as_f64(),
                "playhead_at" => {
                    if let Some(at) = v.as_f64() {
                        roots.extend(self.set_timeline_playhead(id, at));
                    }
                }
                "playhead" => {
                    if let Some(pos) = v.as_f64() {
                        roots.extend(self.set_timeline_cursor(id, pos));
                    }
                }
                "link" => {
                    if let Some(n) = v.as_i64() {
                        let link = (n >= 0).then_some(n as i32);
                        roots.extend(self.set_timeline_link(id, link));
                    }
                }
                "offset" => {
                    if let Some(offset) = v.as_f64() {
                        roots.extend(self.set_timeline_offset(id, offset));
                    }
                }
                _ => {}
            }
        }
        if view != (None, None) {
            roots.extend(self.set_timeline_view(id, view.0, view.1));
        }
        if sel != (None, None) {
            roots.extend(self.set_timeline_selection(id, sel.0, sel.1));
        }
        if loop_span != (None, None) {
            roots.extend(self.set_timeline_playhead_loop(id, loop_span.0, loop_span.1));
        }
        roots.dedup();
        let mut seen: Vec<i32> = Vec::new();
        for root in roots {
            if !seen.contains(&root) {
                seen.push(root);
                effects.push(HostEffect::Redraw(root));
            }
        }
    }

    /// Page widget `id`'s group forward until the end of its content is inside
    /// the window again — the other half of writing into a still axis. Keeping
    /// the window (`set_timeline_total_keeping_view`) is what stops a growing
    /// take from zooming the axis out from under the notes; this is what stops
    /// it from being written off the right edge. Whole windows at a time, at
    /// constant zoom, so what is on screen holds still while it fills and the
    /// take continues at the left — a DAW's page scroll while recording, not a
    /// re-fit. A window still showing everything never moves.
    ///
    /// The same principle as the edge auto-scroll a held clip drag runs
    /// (`Gestures::tick`): *at constant zoom, the window goes where the writing
    /// is*. They differ only in the trigger — content arriving here, a standing
    /// cursor there — and are two copies of one rule for now; unifying them is
    /// recorded as open in `clients/gui/PLAN.md` (G32d).
    pub(super) fn follow_timeline_end(&mut self, id: i32, effects: &mut Vec<HostEffect>) {
        let Some(key) = self.timeline_key(id) else {
            return;
        };
        let total = self.timeline_total(key) as f64;
        let span = self.timeline_span(key);
        let Some(state) = self.timelines.states.get_mut(&key) else {
            return;
        };
        let (start, len) = (state.nav.start, state.nav.len);
        if len <= 0.0 || total <= start + len {
            return;
        }
        let pages = ((total - start - len) / len).ceil();
        state.nav.set_start(start + pages * len, span);
        for root in self.timeline_roots(key) {
            effects.push(HostEffect::Redraw(root));
        }
    }
}

/// Whether a `/gui_set` key is one of the shared timeline keys the group model
/// owns (routed through [`Host::set_timeline_props`] instead of the widget).
pub(super) fn is_timeline_key(key: &str) -> bool {
    matches!(
        key,
        "view_start"
            | "view_len"
            | "sel_start"
            | "sel_len"
            | "playhead_at"
            | "playhead"
            | "playhead_loop_start"
            | "playhead_loop_len"
            | "link"
            | "offset"
    )
}

#[cfg(test)]
mod tests {
    use clausters_core::osc::{OscMessage, OscPacket, OscType};

    use super::super::{ClientId, GUI_DEF, GUI_FREE, GUI_SET, HostEffect};
    use super::*;

    fn from() -> ClientId {
        ClientId::Udp(std::net::SocketAddr::from((
            std::net::Ipv4Addr::LOCALHOST,
            9000,
        )))
    }

    fn def_msg(id: i32, json: &str) -> OscPacket {
        OscPacket::Message(OscMessage {
            addr: GUI_DEF.into(),
            args: vec![OscType::Int(id), OscType::String(json.into())],
        })
    }

    fn set_msg(id: i32, pairs: &[(&str, OscType)]) -> OscPacket {
        let mut args = vec![OscType::Int(id)];
        for (k, v) in pairs {
            args.push(OscType::String((*k).into()));
            args.push(v.clone());
        }
        OscPacket::Message(OscMessage {
            addr: GUI_SET.into(),
            args,
        })
    }

    /// A window with a waveform (id 10) and a spectrogram (id 11) linked as
    /// group 1, plus an unlinked waveform (id 12) of the same data.
    const LINKED: &str = r#"{"type":"window","children":[
        {"id":10,"type":"signal","view":"trace","data":[0.0,0.5,-0.5,1.0],"link":1,
         "sel_start":1.0,"sel_len":2.0},
        {"id":11,"type":"signal","view":"spectrogram","data":[0.0,0.5,-0.5,1.0],"link":1},
        {"id":12,"type":"signal","view":"trace","data":[0.0,0.5,-0.5,1.0]}
    ]}"#;

    fn linked_host() -> Host {
        let mut host = Host::new();
        host.handle_packet(def_msg(1, LINKED), from());
        // Stand in for the fronts loading the data.
        host.set_timeline_total(10, 4);
        host.set_timeline_total(11, 4);
        host.set_timeline_total(12, 4);
        host
    }

    fn editor_of(host: &Host, id: i32) -> EditorProps {
        host.window_def(1)
            .and_then(|t| t.find(id))
            .and_then(|w| w.kind.editor())
            .cloned()
            .unwrap()
    }

    /// What widget `id` draws with: the state of the group it resolves to —
    /// the same lookup the frame path and the animation demand do, and the
    /// only place the selection and the playhead exist.
    fn chrome_of(host: &Host, id: i32) -> GroupState {
        *host
            .timelines()
            .state(host.timeline_key(id).unwrap())
            .unwrap()
    }

    /// A bare group state carrying just a playhead: the sweep math is all
    /// these read.
    fn head_state(at: f64, parked: f64, loop_start: f64, loop_len: f64) -> GroupState {
        GroupState {
            nav: View::full(1),
            sel_start: 0.0,
            sel_len: 0.0,
            playhead_at: at,
            playhead: parked,
            playhead_loop_start: loop_start,
            playhead_loop_len: loop_len,
        }
    }

    #[test]
    fn the_playhead_sweeps_straight_without_a_loop() {
        let e = head_state(1000.0, -1.0, 0.0, 0.0);
        assert_eq!(e.playhead_loop(), None, "no loop by default");
        assert_eq!(e.head_at(1500.0), Some(500.0));
        // Past the end it keeps running: a straight pass is unbounded.
        assert_eq!(e.head_at(9000.0), Some(8000.0));
        // No clock yet, and no static cursor: nothing to draw.
        assert_eq!(e.head_at(0.0), None);
    }

    #[test]
    fn a_stopped_transport_parks_on_the_static_playhead() {
        let e = head_state(-1.0, 320.0, 0.0, 0.0);
        // The static cursor wins when no anchor is set, whatever the clock.
        assert_eq!(e.head_at(0.0), Some(320.0));
        assert_eq!(e.head_at(99_000.0), Some(320.0));
    }

    #[test]
    fn the_playhead_wraps_inside_its_loop_region() {
        let e = head_state(1000.0, -1.0, 400.0, 100.0);
        assert_eq!(e.playhead_loop(), Some((400.0, 100.0)));
        // Inside the region the sweep is untouched.
        assert_eq!(e.head_at(1450.0), Some(450.0));
        // Past its end it wraps to the start, and keeps wrapping.
        assert_eq!(e.head_at(1500.0), Some(400.0));
        assert_eq!(e.head_at(1530.0), Some(430.0));
        assert_eq!(e.head_at(1700.0), Some(400.0));
        assert_eq!(e.head_at(1725.0), Some(425.0));
        // Before the region — the anchor precedes the loop start, so the first
        // pass runs up to it — the line still lands inside, never left of it.
        for clock in [1001.0, 1100.0, 1399.0] {
            let pos = e.head_at(clock).unwrap();
            assert!(
                (400.0..500.0).contains(&pos),
                "clock {clock} put the line at {pos}, outside the region"
            );
        }
    }

    #[test]
    fn a_non_positive_loop_length_is_the_straight_pass() {
        let e = head_state(0.0, -1.0, 400.0, 0.0);
        assert_eq!(e.playhead_loop(), None);
        assert_eq!(e.head_at(900.0), Some(900.0));
    }

    #[test]
    fn def_seeds_the_group_from_its_first_member_for_all_of_them() {
        let host = linked_host();
        // Member 10 declared the selection; member 11 reads the same group.
        assert_eq!(chrome_of(&host, 11).selection(), Some((1.0, 2.0)));
        // The unlinked widget keeps its own (empty) selection.
        assert_eq!(chrome_of(&host, 12).selection(), None);
        // Linked members share one key; the solo widget has its own.
        assert_eq!(host.timeline_key(10), Some(GroupKey::Link(1)));
        assert_eq!(host.timeline_key(10), host.timeline_key(11));
        assert_eq!(host.timeline_key(12), Some(GroupKey::Solo(12)));
    }

    /// Every reader resolves the group, so a playhead set on one member is the
    /// one that decides the *window* animates — including for a member whose
    /// own props never carried an anchor. Nothing is copied into the tree, so
    /// this is the check that the resolution actually happens.
    #[test]
    fn the_animation_demand_follows_the_group_not_the_seed() {
        let mut host = linked_host();
        let still = host.window_def(1).unwrap();
        assert!(!super::super::live::tree_has_playhead(
            still,
            host.timelines()
        ));
        // Anchored through member 11, whose own `playhead_at` prop is unset.
        host.handle_packet(set_msg(11, &[("playhead_at", OscType::Float(0.0))]), from());
        assert_eq!(editor_of(&host, 11).playhead_at, -1.0, "the seed stands");
        let running = host.window_def(1).unwrap();
        assert!(super::super::live::tree_has_playhead(
            running,
            host.timelines()
        ));
    }

    #[test]
    fn set_on_any_member_applies_group_wide_and_redraws() {
        let mut host = linked_host();
        let effects = host.handle_packet(
            set_msg(
                11,
                &[
                    ("sel_start", OscType::Float(0.0)),
                    ("sel_len", OscType::Float(3.0)),
                ],
            ),
            from(),
        );
        assert!(
            effects.iter().any(|e| matches!(e, HostEffect::Redraw(1))),
            "the members' window repaints"
        );
        // Both members read the group; the unlinked one is untouched.
        assert_eq!(chrome_of(&host, 10).selection(), Some((0.0, 3.0)));
        assert_eq!(chrome_of(&host, 11).selection(), Some((0.0, 3.0)));
        assert_eq!(chrome_of(&host, 12).selection(), None);
        // The playhead applies group-wide too.
        host.handle_packet(
            set_msg(10, &[("playhead_at", OscType::Float(100.0))]),
            from(),
        );
        assert_eq!(chrome_of(&host, 11).playhead_at, 100.0);
    }

    /// The loop region is group-wide like the anchor: a linked waveform and
    /// spectrogram must wrap the swept line at the same place, or the two
    /// views of one file would draw it in different spots.
    #[test]
    fn the_playhead_loop_applies_group_wide() {
        let mut host = linked_host();
        let effects = host.handle_packet(
            set_msg(
                11,
                &[
                    ("playhead_at", OscType::Float(0.0)),
                    ("playhead_loop_start", OscType::Float(2.0)),
                    ("playhead_loop_len", OscType::Float(4.0)),
                ],
            ),
            from(),
        );
        assert!(effects.iter().any(|e| matches!(e, HostEffect::Redraw(1))));
        for id in [10, 11] {
            let e = chrome_of(&host, id);
            assert_eq!(e.playhead_loop(), Some((2.0, 4.0)), "member {id}");
            // And the wrap is live for both: a clock of 9 sweeps to 9, which
            // folds into [2, 6) at 5.
            assert_eq!(e.head_at(9.0), Some(5.0), "member {id} wraps");
        }
        // The unlinked member keeps the straight pass.
        assert_eq!(chrome_of(&host, 12).playhead_loop(), None);
        // A non-positive length restores it group-wide.
        host.handle_packet(
            set_msg(10, &[("playhead_loop_len", OscType::Float(0.0))]),
            from(),
        );
        assert_eq!(chrome_of(&host, 11).playhead_loop(), None);
        assert_eq!(chrome_of(&host, 11).head_at(9.0), Some(9.0));
    }

    #[test]
    fn view_keys_drive_the_shared_window() {
        let mut host = linked_host();
        host.handle_packet(
            set_msg(
                10,
                &[
                    ("view_start", OscType::Float(1.0)),
                    ("view_len", OscType::Float(2.0)),
                ],
            ),
            from(),
        );
        let (nav, total) = host.timeline_nav(11).expect("the linked member sees it");
        assert_eq!((nav.start, nav.len, total), (1.0, 2.0, 4));
        // The solo widget's window is unaffected.
        let (nav, _) = host.timeline_nav(12).unwrap();
        assert_eq!((nav.start, nav.len), (0.0, 4.0));
        // A non-positive length resets to the full timeline.
        host.handle_packet(set_msg(11, &[("view_len", OscType::Float(0.0))]), from());
        let (nav, _) = host.timeline_nav(10).unwrap();
        assert_eq!((nav.start, nav.len), (0.0, 4.0));
    }

    #[test]
    fn gestures_mutate_the_group_and_report_the_members_roots() {
        let mut host = linked_host();
        let roots = host.zoom_timeline(10, 0.5, 0.5);
        assert_eq!(roots, vec![1]);
        let (nav, _) = host.timeline_nav(11).unwrap();
        assert_eq!(nav.len, 2.0, "the linked member zoomed too");
        let (start, len, _) = host.select_timeline(11, 3.5, 0.5).unwrap();
        assert_eq!((start, len), (0.5, 3.0), "sorted and clamped");
        assert_eq!(chrome_of(&host, 10).selection(), Some((0.5, 3.0)));
    }

    #[test]
    fn the_group_timeline_spans_the_longest_member() {
        let mut host = linked_host();
        // A longer take joins the group: the full view follows the growth.
        host.set_timeline_total(11, 10);
        let (nav, total) = host.timeline_nav(10).unwrap();
        assert_eq!((nav.len, total), (10.0, 10));
        // A zoomed window is kept (re-clamped), not reset, by later growth.
        host.zoom_timeline(10, 0.5, 0.0);
        host.set_timeline_total(10, 20);
        let (nav, total) = host.timeline_nav(10).unwrap();
        assert_eq!((nav.len, total), (5.0, 20));
    }

    #[test]
    fn live_link_moves_between_groups_and_unlink_keeps_the_view() {
        let mut host = linked_host();
        // Zoom group 1, then pull member 11 out (negative link = unlink): it
        // carries the window it had and diverges from there.
        host.zoom_timeline(10, 0.5, 0.0);
        host.handle_packet(set_msg(11, &[("link", OscType::Int(-1))]), from());
        assert_eq!(host.timeline_key(11), Some(GroupKey::Solo(11)));
        let (nav, _) = host.timeline_nav(11).unwrap();
        assert_eq!(nav.len, 2.0, "the view came along");
        host.zoom_timeline(10, 0.5, 0.0);
        let (nav, _) = host.timeline_nav(11).unwrap();
        assert_eq!(nav.len, 2.0, "…and no longer follows the old group");
        // Joining an existing group adopts its state instead.
        host.handle_packet(
            set_msg(
                10,
                &[
                    ("sel_start", OscType::Float(0.0)),
                    ("sel_len", OscType::Float(1.0)),
                ],
            ),
            from(),
        );
        host.handle_packet(set_msg(12, &[("link", OscType::Int(1))]), from());
        assert_eq!(host.timeline_key(12), Some(GroupKey::Link(1)));
        assert_eq!(chrome_of(&host, 12).selection(), Some((0.0, 1.0)));
    }

    #[test]
    fn placement_offset_lengthens_the_group_and_reclamps() {
        let mut host = linked_host();
        // Members 10/11 are 4 samples long, linked as group 1. Place member 11
        // at timeline sample 6: the group timeline now spans 6 + 4 = 10.
        host.handle_packet(set_msg(11, &[("offset", OscType::Float(6.0))]), from());
        let (nav, total) = host.timeline_nav(10).expect("the linked member sees it");
        assert_eq!(total, 10, "the placed member lengthened the group");
        // A group showing its full timeline grows with the placement.
        assert_eq!((nav.start, nav.len), (0.0, 10.0));
        // The offset is per-member: member 10 is untouched, member 11 carries it.
        assert_eq!(editor_of(&host, 10).offset, 0.0);
        assert_eq!(editor_of(&host, 11).offset, 6.0);
        // A zoomed window is re-clamped, not reset, when a placement shrinks the
        // timeline back.
        host.zoom_timeline(10, 0.5, 1.0); // len 5, pinned to the right edge
        host.handle_packet(set_msg(11, &[("offset", OscType::Float(0.0))]), from());
        let (nav, total) = host.timeline_nav(10).unwrap();
        assert_eq!(total, 4, "back to the longest unplaced extent");
        assert!(nav.start + nav.len <= 4.0, "the window stays inside");
    }

    #[test]
    fn free_prunes_the_group_state() {
        let mut host = linked_host();
        host.handle_packet(
            OscPacket::Message(OscMessage {
                addr: GUI_FREE.into(),
                args: vec![OscType::Int(1)],
            }),
            from(),
        );
        assert!(host.timelines().state(GroupKey::Link(1)).is_none());
        assert!(host.timelines().state(GroupKey::Solo(12)).is_none());
        assert_eq!(host.timelines().total_of(10), 0);
    }

    #[test]
    fn redefine_reseeds_confined_groups_but_keeps_cross_window_ones() {
        let mut host = linked_host();
        host.zoom_timeline(12, 0.5, 0.0); // solo group, confined to def 1
        // A second window joins group 1, then def 1 is re-sent: group 1 spans
        // the other window, so it survives; the solo group reseeds fresh.
        host.handle_packet(
            def_msg(
                2,
                r#"{"type":"window","children":[
                    {"id":20,"type":"signal","view":"trace","data":[0.0,0.5],"link":1}
                ]}"#,
            ),
            from(),
        );
        host.zoom_timeline(10, 0.5, 0.0);
        host.handle_packet(def_msg(1, LINKED), from());
        host.set_timeline_total(10, 4);
        host.set_timeline_total(12, 4);
        let (nav, _) = host.timeline_nav(10).unwrap();
        assert!(nav.len < 4.0, "the cross-window group survived the re-def");
        let (nav, total) = host.timeline_nav(12).unwrap();
        assert_eq!((nav.len, total), (4.0, 4), "the confined group reset");
    }

    /// Two lanes of one window (the window root is id 1, so the lanes take ids
    /// clear of it); the drums' clips end at 300, the lead's at 500.
    const LANES: &str = r#"{"type":"window","children":[
        {"id":100,"type":"field","label":"drums","children":[
            {"id":110,"type":"field","offset":0.0,"dur":100.0},
            {"id":111,"type":"field","offset":200.0,"dur":100.0}
        ]},
        {"id":200,"type":"field","label":"lead","children":[
            {"id":210,"type":"field","offset":100.0,"dur":400.0}
        ]}
    ]}"#;

    fn lanes_host() -> Host {
        let mut host = Host::new();
        host.handle_packet(def_msg(1, LANES), from());
        host
    }

    #[test]
    fn the_lanes_of_a_window_share_one_axis_spanning_the_composition() {
        let host = lanes_host();
        // Linked by default (keyed by the window root), so one group...
        assert_eq!(host.timeline_key(100), host.timeline_key(200));
        // ...whose length is the longest clip end over every lane, not one lane's.
        let (nav, total) = host.timeline_nav(100).unwrap();
        assert_eq!(total, 500);
        assert_eq!(
            (nav.start, nav.len),
            (0.0, 500.0),
            "it starts showing it all"
        );
    }

    #[test]
    fn zooming_one_lane_moves_the_axis_of_all_of_them() {
        let mut host = lanes_host();
        host.zoom_timeline(200, 0.5, 0.0); // wheel over the lead lane, at its left
        let (a, _) = host.timeline_nav(100).unwrap();
        let (b, _) = host.timeline_nav(200).unwrap();
        assert_eq!(
            (a.start, a.len),
            (b.start, b.len),
            "aligned lanes stay aligned"
        );
        assert_eq!(a.len, 250.0, "zoomed in by half");

        // Panning is shared too (the gesture lands on whichever lane is under
        // the pointer, the group moves).
        host.pan_timeline(100, 100.0);
        let (a, _) = host.timeline_nav(100).unwrap();
        let (b, _) = host.timeline_nav(200).unwrap();
        assert_eq!((a.start, b.start), (100.0, 100.0));
    }

    #[test]
    fn a_clip_moved_past_the_end_lengthens_the_shared_axis() {
        let mut host = lanes_host();
        host.handle_packet(set_msg(111, &[("offset", OscType::Float(900.0))]), from());
        let (nav, total) = host.timeline_nav(100).unwrap();
        assert_eq!(total, 1000, "the lane's extent followed its clip");
        assert_eq!(
            nav.len, 1000.0,
            "a fully-zoomed-out axis keeps showing it all"
        );
    }

    #[test]
    fn a_playhead_set_on_a_lane_reaches_every_lane_of_its_group() {
        let mut host = lanes_host();
        host.handle_packet(
            set_msg(100, &[("playhead_at", OscType::Float(12288.0))]),
            from(),
        );
        // The renderer resolves each lane to its group and draws that: a set on
        // one lane is what every lane of the group sweeps from.
        for lane in [100, 200] {
            assert_eq!(
                chrome_of(&host, lane).playhead_at,
                12288.0,
                "lane {lane} draws no playhead without it"
            );
        }
    }

    #[test]
    fn a_located_cursor_is_group_wide_and_stands_still() {
        let mut host = lanes_host();
        // The transport is located at 300 (a click on the ruler, or a script).
        host.handle_packet(set_msg(100, &[("playhead", OscType::Float(300.0))]), from());
        for lane in [100, 200] {
            let chrome = chrome_of(&host, lane);
            assert_eq!(chrome.playhead, 300.0, "every lane shows the one cursor");
            // The clock anchor is untouched: the cursor is what a *stopped*
            // transport shows, and the two are different things.
            assert!(chrome.playhead_at < 0.0);
        }
    }

    #[test]
    fn a_lane_zooms_out_past_its_content_into_empty_time() {
        let mut host = lanes_host();
        let (_nav, total) = host.timeline_nav(100).unwrap();
        assert_eq!(total, 500);

        // Wheel out: the window may show empty time past the last clip, or there
        // is nowhere to drag a clip *to*.
        for _ in 0..6 {
            host.zoom_timeline(100, 1.6, 0.0);
        }
        let (nav, _) = host.timeline_nav(100).unwrap();
        assert!(nav.len > 500.0, "the axis shows past the composition");
        assert!(nav.len <= 4.0 * 500.0, "but the headroom is bounded");

        // Growing the content (a clip dragged out) must not yank the zoomed-out
        // window back onto the content.
        let zoomed = nav.len;
        host.handle_packet(set_msg(111, &[("offset", OscType::Float(700.0))]), from());
        assert_eq!(host.timeline_nav(100).unwrap().0.len, zoomed);

        // A reset shows the content, not the headroom.
        host.reset_timeline(100);
        let (nav, total) = host.timeline_nav(100).unwrap();
        assert_eq!((nav.start, nav.len), (0.0, total as f64));
    }

    /// A content change under a *still* view keeps the refit: a window showing
    /// the whole timeline keeps showing all of it when a clip is placed by
    /// `/gui_set` rather than dragged. (A drag takes the other path -- see the
    /// gesture tests -- so extending the content scrolls instead of zooming
    /// the axis out from under the cursor.)
    #[test]
    fn a_scripted_move_refits_the_full_view_but_a_dragged_one_does_not() {
        let mut host = lanes_host();
        let (before, total) = host.timeline_nav(100).unwrap();
        assert_eq!(before.len, total as f64, "it starts showing it all");

        // Scripted: the window follows the piece as it grows.
        host.handle_packet(set_msg(111, &[("offset", OscType::Float(900.0))]), from());
        let (after, grown) = host.timeline_nav(100).unwrap();
        assert!(grown > total);
        assert_eq!(after.len, grown as f64, "it still shows the whole piece");

        // The gesture variant keeps the window's length instead.
        let held = host.timeline_nav(100).unwrap().0.len;
        host.set_timeline_total_keeping_view(100, grown * 2);
        let (kept, _) = host.timeline_nav(100).unwrap();
        assert_eq!(kept.len, held, "a drag holds the zoom");
    }

    /// A roll's axis exists *before* its content: it is a surface to write into
    /// (drawn, or recorded from MIDI), so it opens on an empty grid instead of
    /// the one sample an empty extent would give it — and the notes painted in
    /// afterwards scroll under that window rather than collapsing it onto the
    /// first of them. Without this a roll opened empty never shows anything
    /// written into it, which is what the client-side MIDI painting does.
    #[test]
    fn a_roll_has_an_axis_before_it_has_notes() {
        let mut host = Host::new();
        host.handle_packet(
            def_msg(
                1,
                r#"{"type":"window","margin":0,"children":[
                {"id":100,"type":"notes","notes":[],"min":48,"max":84,
                 "sample_rate":48000.0,"tempo":2.0}
            ]}"#,
            ),
            from(),
        );
        // Sixteen beats at two beats a second, 48 kHz: the grid it opens on.
        let (nav, total) = host.timeline_nav(100).unwrap();
        assert_eq!(total, 384_000, "an empty roll still spans its grid");
        assert_eq!((nav.start, nav.len), (0.0, 384_000.0));

        // A note painted in registers as content, and the window survives it.
        host.handle_packet(
            set_msg(
                100,
                &[(
                    "notes",
                    OscType::String("[1000.0, 500.0, 60.0, 100, 0]".into()),
                )],
            ),
            from(),
        );
        let (nav, total) = host.timeline_nav(100).unwrap();
        assert_eq!(total, 1500, "the extent is the end of the last note");
        assert_eq!(nav.len, 384_000.0, "and the grid stays under the take");

        // Past the grid, the take is the axis.
        host.handle_packet(
            set_msg(
                100,
                &[(
                    "notes",
                    OscType::String("[0.0, 500000.0, 60.0, 100, 0]".into()),
                )],
            ),
            from(),
        );
        assert_eq!(host.timeline_nav(100).unwrap().1, 500_000);
    }

    /// ...and a take written past the right edge pages the axis forward, at
    /// constant zoom: what is on screen holds still while it fills, and the
    /// writing continues at the left of the next window. Recording off the edge
    /// of a still axis is as blank as recording into a one-sample one.
    #[test]
    fn a_take_written_past_the_edge_pages_the_axis_forward() {
        let mut host = Host::new();
        host.handle_packet(
            def_msg(
                1,
                r#"{"type":"window","margin":0,"children":[
                {"id":100,"type":"notes","notes":[],"min":48,"max":84,
                 "sample_rate":48000.0,"tempo":2.0}
            ]}"#,
            ),
            from(),
        );
        let (nav, _) = host.timeline_nav(100).unwrap();
        assert_eq!((nav.start, nav.len), (0.0, 384_000.0), "the first window");

        // A note inside the window leaves it where it is.
        host.handle_packet(
            set_msg(
                100,
                &[(
                    "notes",
                    OscType::String("[1000.0, 500.0, 60.0, 100, 0]".into()),
                )],
            ),
            from(),
        );
        assert_eq!(host.timeline_nav(100).unwrap().0.start, 0.0);

        // One ending past it pages forward, keeping the zoom.
        host.handle_packet(
            set_msg(
                100,
                &[(
                    "notes",
                    OscType::String("[400000.0, 24000.0, 62.0, 100, 0]".into()),
                )],
            ),
            from(),
        );
        let (nav, _) = host.timeline_nav(100).unwrap();
        assert_eq!(
            (nav.start, nav.len),
            (384_000.0, 384_000.0),
            "one window forward, same length"
        );
    }

    /// A free-standing `timeruler` joins the lanes' group and reads their
    /// window, so it labels what they show — and it costs no lane any height,
    /// which is the reason it exists.
    #[test]
    fn a_free_standing_ruler_reads_the_lanes_axis() {
        let mut host = Host::new();
        host.handle_packet(
            def_msg(
                1,
                r#"{"type":"window","margin":0,"children":[
                {"id":90,"type":"field","link":7,"h":20.0},
                {"id":100,"type":"field","link":7,"children":[
                    {"id":110,"type":"field","offset":0.0,"dur":400.0}
                ]}
            ]}"#,
            ),
            from(),
        );
        host.sync_track_totals();
        // One group: the ruler and the lane resolve to the same key...
        assert_eq!(host.timeline_key(90), host.timeline_key(100));
        // ...so the ruler sees the lane's window, and follows it when it moves.
        let (lane, _) = host.timeline_nav(100).unwrap();
        let (ruler, _) = host.timeline_nav(90).unwrap();
        assert_eq!((ruler.start, ruler.len), (lane.start, lane.len));
        host.zoom_timeline(100, 0.5, 0.0);
        let (lane, _) = host.timeline_nav(100).unwrap();
        let (ruler, _) = host.timeline_nav(90).unwrap();
        assert_eq!((ruler.start, ruler.len), (lane.start, lane.len));
        // And it contributes no extent of its own: the axis is still the clips'.
        assert_eq!(host.timeline_nav(90).unwrap().1, 400);
    }

    /// A heavy view's axis stays bound to its data: there is no signal out past
    /// the end of a file to look at.
    #[test]
    fn a_waveform_group_gets_no_headroom() {
        let mut host = linked_host();
        for _ in 0..6 {
            host.zoom_timeline(10, 1.6, 0.0);
        }
        let (nav, total) = host.timeline_nav(10).unwrap();
        assert_eq!(nav.len, total as f64);
    }
}

#[cfg(test)]
mod indent_tests {
    use super::*;
    use crate::host::guidef::GuiNode;
    use crate::host::layout::{self, Rect};

    fn tree(json: &str) -> Widget {
        Widget::from_node(1, &GuiNode::parse(json.as_bytes()).unwrap(), &[]).unwrap()
    }

    /// The bug this rule exists for: a lane, a piano-roll and a free-standing
    /// ruler stacked on **one** navigation group used to start their bodies at
    /// three different x — a lane's header, the roll's keyboard, the ruler's
    /// copy of the lane's header — so the same sample sat at three places.
    /// They agree now, on the widest gutter any of them asks for.
    #[test]
    fn one_group_starts_its_body_at_one_x() {
        let root = tree(
            r#"{"type":"window","children":[
                {"id":1,"type":"field","link":7},
                {"id":2,"type":"field","link":7},
                {"id":3,"type":"notes","link":7},
                {"id":4,"type":"signal","view":"trace","data":[0.0,1.0],"link":7}
            ]}"#,
        );
        let m = Metrics::default();
        let placed = layout::layout(Rect::new(0.0, 0.0, 800.0, 400.0), &root, &m);
        // The widest wish wins, and the layout stamps it on every member.
        let widest = m.header_w.max(super::super::pianoroll::KEYBOARD_W);
        assert_eq!(group_indents(&root, &m)[&GroupKey::Link(7)], widest);
        for p in &placed {
            if p.widget.kind.editor().is_some() {
                assert_eq!(p.indent, widest, "member {:?}", p.widget.id);
            }
        }
    }

    /// The point of a *sizeable* header: what one lane reserves is a fact about
    /// the axis, so widening one lane's gutter moves the roll and the ruler
    /// stacked with it. Nothing else in the group has to be told.
    #[test]
    fn one_wide_lane_header_moves_the_whole_axis() {
        let root = tree(
            r#"{"type":"window","children":[
                {"id":1,"type":"field","link":7},
                {"id":2,"type":"field","link":7,"header_w":240},
                {"id":3,"type":"notes","link":7}
            ]}"#,
        );
        let m = Metrics::default();
        let placed = layout::layout(Rect::new(0.0, 0.0, 800.0, 400.0), &root, &m);
        assert_eq!(group_indents(&root, &m)[&GroupKey::Link(7)], 240.0);
        for p in &placed {
            if p.widget.kind.editor().is_some() {
                assert_eq!(p.indent, 240.0, "member {:?}", p.widget.id);
            }
        }
    }

    /// A free-standing ruler dropped among lanes joins **their** axis without
    /// being told: it exists to rule them, so it starts its ticks where they
    /// start their bodies and moves when they move.
    #[test]
    fn an_unlinked_timeruler_joins_the_windows_lanes() {
        let root = tree(
            r#"{"type":"window","children":[
                {"id":1,"type":"field","header_w":180},
                {"id":2,"type":"field"}
            ]}"#,
        );
        let m = Metrics::default();
        let placed = layout::layout(Rect::new(0.0, 0.0, 800.0, 400.0), &root, &m);
        for p in &placed {
            if p.widget.kind.editor().is_some() {
                assert_eq!(p.indent, 180.0, "member {:?}", p.widget.id);
            }
        }
    }

    /// A group of one is its own gutter — which is why every view that was
    /// alone on its axis is exactly where it always was.
    #[test]
    fn a_solo_member_keeps_its_own_gutter() {
        let root = tree(
            r#"{"type":"window","children":[
                {"id":1,"type":"field"},
                {"id":2,"type":"signal","view":"trace","data":[0.0,1.0]},
                {"id":3,"type":"signal","view":"trace","data":[0.0,1.0],"ruler_y":"off"}
            ]}"#,
        );
        let m = Metrics::default();
        let indents = group_indents(&root, &m);
        // A lane auto-links into its window's group (`link_lanes`), so its
        // key is the root's, not its own.
        assert_eq!(indents[&GroupKey::Link(1)], m.header_w);
        assert_eq!(indents[&GroupKey::Solo(2)], m.ruler_w);
        // No value ruler, no gutter: the trace fills the widget.
        assert_eq!(indents[&GroupKey::Solo(3)], 0.0);
    }
}
