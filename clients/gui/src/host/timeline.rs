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
//! may span windows. This is deliberately the shape a future multitrack
//! (DAW-style) view needs — a clip item there is a member with a *placement*
//! (a start offset) on the same shared timeline. G20d ships every member at
//! offset 0; the per-member placement is the one designed extension point.
//!
//! Selection and playhead are **mirrored** from the group into every member's
//! `EditorProps` on each change, so the frame renderer, `has_playhead` and
//! the readouts keep reading the widget tree they always read — the group is
//! the single writer, the mirrors can never drift.

use std::collections::HashMap;

use serde_json::Value;

use crate::viewport::View;

use super::widget::{EditorProps, Widget};
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
#[derive(Clone, Debug)]
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
}

impl GroupState {
    /// A fresh group seeded from its first member's editor props (the def-time
    /// selection/playhead), spanning `total` timeline samples.
    fn seed(editor: &EditorProps, total: usize) -> GroupState {
        GroupState {
            nav: View::full(total),
            sel_start: editor.sel_start,
            sel_len: editor.sel_len,
            playhead_at: editor.playhead_at,
            playhead: editor.playhead,
        }
    }
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
        fn walk(widget: &Widget, root: i32, out: &mut Vec<Member>) {
            if let (Some(id), Some(editor)) = (widget.id, widget.kind.editor()) {
                out.push(Member {
                    root,
                    id,
                    key: group_key(id, editor.link),
                    offset: editor.offset,
                });
            }
            for child in &widget.children {
                walk(child, root, out);
            }
        }
        let mut out = Vec::new();
        for (root, tree) in &self.window_defs {
            walk(tree, *root, &mut out);
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
    /// on the shared timeline, so a clip placed late lengthens the group (>= 1,
    /// so an empty group still navigates sanely).
    pub fn timeline_total(&self, key: GroupKey) -> usize {
        self.timeline_members()
            .iter()
            .filter(|m| m.key == key)
            .map(|m| m.offset.max(0.0).ceil() as usize + self.timelines.total_of(m.id))
            .max()
            .unwrap_or(0)
            .max(1)
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
        let Some(key) = self.timeline_key(id) else {
            self.timelines.totals.insert(id, total);
            return;
        };
        let old_total = self.timeline_total(key);
        self.timelines.totals.insert(id, total);
        let new_total = self.timeline_total(key);
        if let Some(state) = self.timelines.states.get_mut(&key) {
            if state.nav.len >= old_total as f64 {
                state.nav = View::full(new_total);
            } else {
                state.nav.set_start(state.nav.start, new_total);
            }
        }
    }

    /// Registers every `track` lane's extent — the end of its last clip — with
    /// its navigation group, so the shared axis spans the composition. A lane's
    /// "data" is its clips, so this is the lane's answer to the data extent the
    /// fronts register for a loaded waveform, and it must be re-run whenever a
    /// clip moves or resizes (a `/gui_def`, a `/gui_set`, or a drag).
    pub(super) fn sync_track_totals(&mut self) {
        fn walk(widget: &Widget, out: &mut Vec<(i32, usize)>) {
            if let (Some(id), super::widget::WidgetKind::Track { .. }) = (widget.id, &widget.kind) {
                let span = super::track::clips_span(widget);
                out.push((id, span.ceil().max(0.0) as usize));
            }
            for child in &widget.children {
                walk(child, out);
            }
        }
        let mut lanes = Vec::new();
        for tree in self.window_defs.values() {
            walk(tree, &mut lanes);
        }
        for (id, span) in lanes {
            self.set_timeline_total(id, span);
        }
    }

    /// Anchor-preserving zoom of widget `id`'s group. Returns the roots to
    /// repaint (empty when the widget is in no group).
    pub fn zoom_timeline(&mut self, id: i32, factor: f64, anchor: f64) -> Vec<i32> {
        let Some(key) = self.timeline_key(id) else {
            return Vec::new();
        };
        let total = self.timeline_total(key);
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
        let total = self.timeline_total(key);
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
        let total = self.timeline_total(key);
        let Some(state) = self.timelines.states.get_mut(&key) else {
            return Vec::new();
        };
        match len {
            Some(len) if len <= 0.0 => {
                state.nav = View::full(total);
                return self.timeline_roots(key);
            }
            Some(len) => state.nav.len = len.max(1.0),
            None => {}
        }
        let start = start.unwrap_or(state.nav.start);
        state.nav.set_start(start, total);
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
    /// `sel_len` keys (either alone keeps the other) and mirrors it into every
    /// member's editor props. Returns the roots to repaint.
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
        self.mirror_timeline_group(key);
        self.timeline_roots(key)
    }

    /// Sets widget `id`'s group playhead anchor and mirrors it into every
    /// member's editor props. Returns the roots to repaint.
    pub fn set_timeline_playhead(&mut self, id: i32, at: f64) -> Vec<i32> {
        let Some(key) = self.timeline_key(id) else {
            return Vec::new();
        };
        let Some(state) = self.timelines.states.get_mut(&key) else {
            return Vec::new();
        };
        state.playhead_at = at;
        self.mirror_timeline_group(key);
        self.timeline_roots(key)
    }

    /// Sets the group's **static cursor** — where a located, stopped transport
    /// sits. Mirrored into every member, so all the lanes show one cursor.
    pub fn set_timeline_cursor(&mut self, id: i32, pos: f64) -> Vec<i32> {
        let Some(key) = self.timeline_key(id) else {
            return Vec::new();
        };
        let Some(state) = self.timelines.states.get_mut(&key) else {
            return Vec::new();
        };
        state.playhead = pos;
        self.mirror_timeline_group(key);
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
            })
        });
        // Re-clamp the (carried or adopted) window against the new membership
        // and align every member with the group.
        let total = self.timeline_total(new_key);
        if let Some(state) = self.timelines.states.get_mut(&new_key) {
            let start = state.nav.start;
            state.nav.set_start(start, total);
        }
        self.mirror_timeline_group(new_key);
        self.prune_timeline_groups();
        for root in self.timeline_roots(new_key) {
            if !roots.contains(&root) {
                roots.push(root);
            }
        }
        roots
    }

    /// Copies group `key`'s selection/playhead into every member's editor
    /// props — the mirror that keeps the widget tree (what the renderer and
    /// `has_playhead` read) in lockstep with the group (the single writer).
    fn mirror_timeline_group(&mut self, key: GroupKey) {
        let Some(state) = self.timelines.states.get(&key).cloned() else {
            return;
        };
        let members: Vec<Member> = self
            .timeline_members()
            .into_iter()
            .filter(|m| m.key == key)
            .collect();
        for m in members {
            if let Some(editor) = self
                .window_defs
                .get_mut(&m.root)
                .and_then(|t| t.find_mut(m.id))
                .and_then(|w| w.kind.editor_mut())
            {
                editor.sel_start = state.sel_start;
                editor.sel_len = state.sel_len;
                editor.playhead_at = state.playhead_at;
                editor.playhead = state.playhead;
            }
        }
    }

    /// Ensures every timeline widget's group exists and every member agrees
    /// with it. Called after a `/gui_def` builds (or rebuilds) a window tree:
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
        // editor props; every member then mirrors the group.
        let mut keys: Vec<GroupKey> = Vec::new();
        for m in &members {
            if !keys.contains(&m.key) {
                keys.push(m.key);
            }
            if !self.timelines.states.contains_key(&m.key) {
                let editor = self
                    .window_defs
                    .get(&m.root)
                    .and_then(|t| t.find(m.id))
                    .and_then(|w| w.kind.editor())
                    .cloned();
                if let Some(editor) = editor {
                    let total = self.timeline_total(m.key);
                    self.timelines
                        .states
                        .insert(m.key, GroupState::seed(&editor, total));
                }
            }
        }
        for key in keys {
            self.mirror_timeline_group(key);
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
    /// `sel_len`, `playhead_at` and `link` (a negative link unlinks) apply
    /// group-wide; every other key is applied to the widget itself by the
    /// caller. Pushes a redraw effect per affected window.
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
        for (k, v) in props {
            match k.as_str() {
                "view_start" => view.0 = v.as_f64(),
                "view_len" => view.1 = v.as_f64(),
                "sel_start" => sel.0 = v.as_f64(),
                "sel_len" => sel.1 = v.as_f64(),
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
        roots.dedup();
        let mut seen: Vec<i32> = Vec::new();
        for root in roots {
            if !seen.contains(&root) {
                seen.push(root);
                effects.push(HostEffect::Redraw(root));
            }
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
        {"id":10,"type":"waveform","data":[0.0,0.5,-0.5,1.0],"link":1,
         "sel_start":1.0,"sel_len":2.0},
        {"id":11,"type":"spectrogram","data":[0.0,0.5,-0.5,1.0],"link":1},
        {"id":12,"type":"waveform","data":[0.0,0.5,-0.5,1.0]}
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

    #[test]
    fn def_seeds_the_group_from_its_first_member_and_mirrors_the_rest() {
        let host = linked_host();
        // Member 10 declared the selection; member 11 adopted it on def.
        assert_eq!(editor_of(&host, 11).selection(), Some((1.0, 2.0)));
        // The unlinked widget keeps its own (empty) selection.
        assert_eq!(editor_of(&host, 12).selection(), None);
        // Linked members share one key; the solo widget has its own.
        assert_eq!(host.timeline_key(10), Some(GroupKey::Link(1)));
        assert_eq!(host.timeline_key(10), host.timeline_key(11));
        assert_eq!(host.timeline_key(12), Some(GroupKey::Solo(12)));
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
        // Both members mirror the group; the unlinked one is untouched.
        assert_eq!(editor_of(&host, 10).selection(), Some((0.0, 3.0)));
        assert_eq!(editor_of(&host, 11).selection(), Some((0.0, 3.0)));
        assert_eq!(editor_of(&host, 12).selection(), None);
        // The playhead applies group-wide too.
        host.handle_packet(
            set_msg(10, &[("playhead_at", OscType::Float(100.0))]),
            from(),
        );
        assert_eq!(editor_of(&host, 11).playhead_at, 100.0);
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
        assert_eq!(editor_of(&host, 10).selection(), Some((0.5, 3.0)));
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
        assert_eq!(editor_of(&host, 12).selection(), Some((0.0, 1.0)));
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
                    {"id":20,"type":"waveform","data":[0.0,0.5],"link":1}
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
        {"id":100,"type":"track","label":"drums","children":[
            {"id":110,"type":"clip","offset":0.0,"dur":100.0},
            {"id":111,"type":"clip","offset":200.0,"dur":100.0}
        ]},
        {"id":200,"type":"track","label":"lead","children":[
            {"id":210,"type":"clip","offset":100.0,"dur":400.0}
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
    fn a_playhead_set_on_a_lane_reaches_the_tree_the_renderer_reads() {
        let mut host = lanes_host();
        host.handle_packet(
            set_msg(100, &[("playhead_at", OscType::Float(12288.0))]),
            from(),
        );
        // The renderer reads the *typed tree*, not the registry: a lane is a
        // group member, so the value must land there through the group mirror.
        for lane in [100, 200] {
            let editor = host
                .window_def(1)
                .and_then(|t| t.find(lane))
                .and_then(|w| w.kind.editor())
                .unwrap();
            assert_eq!(
                editor.playhead_at, 12288.0,
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
            let editor = host
                .window_def(1)
                .and_then(|t| t.find(lane))
                .and_then(|w| w.kind.editor())
                .unwrap();
            assert_eq!(editor.playhead, 300.0, "every lane shows the one cursor");
            // The clock anchor is untouched: the cursor is what a *stopped*
            // transport shows, and the two are different things.
            assert!(editor.playhead_at < 0.0);
        }
    }
}
