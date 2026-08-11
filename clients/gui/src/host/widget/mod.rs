//! The typed widget schema: a renderer's interpretation of a GuiDef tree.
//!
//! `host::guidef::GuiNode` is the **generic** wire form (any `{id, type, props,
//! children}`), kept deliberately open so the protocol never changes when a
//! widget type is added. This module is the other half of that principle: the
//! *renderer* turns a `GuiNode` into a **typed** [`Widget`] it knows how to lay
//! out and draw. Adding a widget type is a new [`WidgetKind`] variant plus a
//! handler here and in the renderer — not a protocol change. An unrecognized
//! type is not an error: it becomes [`WidgetKind::Unknown`], laid out (it
//! reserves its space) but not painted, so a host built today renders the parts
//! of a newer GuiDef it understands and ignores the rest.
//!
//! The standardized widgets at this milestone are `window` + `panel`/layout
//! (`row`/`col`/`grid`/`free`) + `label`, plus the heavy `waveform` view, fed
//! its samples either inline (`"data": [f32…]`) or — for bulk — from an OSC blob
//! carried alongside the JSON in the same `/gui_def` message (`"blob": <index>`).
//! Both keep the int/float distinction and the "flat primitives at the boundary"
//! rule; a server buffer reference (`"buffer"`) is recognized but deferred to the
//! milestone where the host attaches to the audio server.
//!
//! **Module layout.** This file is the *schema* and nothing else: the
//! [`WidgetKind`] enum (the closed sum type the whole renderer matches on), the
//! [`Widget`] node around it and the tree walk over both. That is where a
//! reader looking for the model should land, so everything that is *about* the
//! model rather than *part of* it is a child module.
//!
//! Four of them are one arm per widget type, which is the shape this schema
//! keeps producing: [`build`] turns a `GuiNode` into a `WidgetKind`
//! (construction), [`apply`] applies a `/gui_set` key to a live one (mutation),
//! [`size`] says how big a kind wants to be ([`WidgetKind::natural_size`]) —
//! the same pass in the other direction — and [`query`] answers what a widget
//! currently *is*: its event value, the bus it reads, the editor chrome it
//! carries, and which body of a clip owns the data a caller asked the clip for.
//! [`props`] is the fifth child and the only one that is not a pass: the prop
//! bundles several kinds share ([`EditorProps`], [`GestureMap`], [`ScrollView`],
//! [`Place`], [`Flow`], [`Range`]) with the small vocabularies they are built
//! from, which are a detail of a variant's payload rather than part of the
//! model's shape. All of them are descendants, so they share the private
//! prop-reading helpers ([`parse`]) without exposing them.
//!
//! Per-widget *behavior* (drawing, hit-testing, editing) is not here at all —
//! it lives in each widget's own module (`bpf`, `pianoroll`, `track`, `patch`,
//! `textedit`, …); this module owns only the typed data and its wire mapping.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;

use super::guidef::GuiNode;
// Sibling widget modules the wire matches reach via `super::` — re-imported here
// so the `build`/`apply` child modules resolve the same paths (a descendant sees
// the parent's private `use` items).
use super::signal;

mod apply;
mod axes;
mod build;
pub(crate) use build::signal_element;
pub mod element;
pub(crate) mod parse;
mod props;
mod query;
pub(crate) mod size;

#[cfg(test)]
mod tests;

pub(super) use axes::{AXES, flatten as flatten_axes, flatten_tree as flatten_tree_axes};
pub use element::{Claim, Element, Needs};
pub use props::{
    Align, Axis, EditorProps, Flow, GestureMap, GesturePlan, GestureStep, Layout, Place, Range,
    Rate, Ruler, RulerY, ScrollView,
};
pub use size::Natural;

use parse::*;

/// The typed kind of a widget, with the fields the renderer needs.
#[derive(Debug, Clone)]
pub enum WidgetKind {
    /// A top-level window (a GuiDef root): title, requested size, child layout.
    Window {
        title: Option<String>,
        width: u32,
        height: u32,
        layout: Layout,
        flow: Flow,
    },
    /// A nestable container.
    Panel { layout: Layout, flow: Flow },
    /// A container showing **one child at a time**: the one at `index`, filling
    /// the container's area (its `flow`'s margin inset). The others are hidden
    /// — skipped by the layout, so they are neither drawn nor hit — but they
    /// stay in the tree, which is what makes a switch cheap: a hidden heavy
    /// element keeps its GPU slot and its bus watch, since both are collected
    /// from the tree and not from the placements, so flipping back re-uploads
    /// nothing.
    ///
    /// An `index` outside the children shows nothing, deliberately: it is a
    /// blank page, not a clamped one, so a pager cannot silently show the wrong
    /// child. With `index` bound to a `toggle` or a `menu`
    /// ([`bind`](super::bind)) this is the whole of tabs, a pager, and
    /// alternating two views of one signal — composition, not a widget.
    ///
    /// It carries a `margin` rather than a whole [`Flow`]: a stack makes no
    /// arrangement, so the `gap` between children and a `grid`'s column count
    /// have nothing to mean here.
    Stack { index: i32, margin: Option<f32> },
    /// The 2D workspace: a container whose children live in a **virtual
    /// content area** seen through a scrolling, zooming window ([`ScrollView`]).
    /// General first — the default pans both axes and zooms at the cursor; the
    /// constrained scroll views (`axis`, `zoom: 0`) degrade from it by
    /// configuration. `layout` arranges the children *inside* the content
    /// area (default `free`), exactly as a panel does inside its rect.
    Scroll {
        layout: Layout,
        flow: Flow,
        view: ScrollView,
    },
    /// A multitrack lane: a horizontal strip of the shared timeline holding
    /// `clip` children placed by their `offset`/`dur`. A container (its clips
    /// are its children); `label` names the track in a left header, `height`
    /// its lane weight when several tracks stack under one time axis. The
    /// **graphic unit** — the clip rectangles and the track header — is drawn
    /// by [`super::track`]; the clips share one time axis (aligned tracks), the
    /// span being the longest clip end over the window's tracks. `snap` is the
    /// drag grid in timeline samples (0 = snap to whole samples) a clip's
    /// move/resize rounds to. `editor` is the shared chrome, of which a lane
    /// uses the time `ruler` (a strip under the lane, off by default) and the
    /// `playhead_at` anchor (the engine sample-clock value at timeline sample 0,
    /// so the playhead sweeps the clips as the composition plays) — the same
    /// props, parsing and `/gui_set` keys the heavy timeline views use. A lane
    /// joins no navigation group (its axis is the window's shared clip span), so
    /// those keys apply to the widget itself.
    Track {
        label: Option<String>,
        height: f32,
        snap: f64,
        /// The lane's gutter: how wide it is and what it carries there (see
        /// [`super::track::Header`]).
        header: super::track::Header,
        editor: EditorProps,
    },
    /// A **free-standing time ruler**: the shared axis of a navigation group,
    /// drawn as a strip the *document* places — the DAW's ruler above its
    /// tracks.
    ///
    /// It exists because a ruler over a multitrack belongs to the **axis**, not
    /// to any one lane. A `track`'s own `ruler` strip is reserved out of that
    /// lane's height, so ruling a stack of lanes meant picking one to carry it
    /// (and to pay for it), and the strip then sat wherever that lane happened
    /// to be — between two lanes, unless it was the last. This widget owns its
    /// own box instead: put it above the lanes (or below) and no lane loses a
    /// pixel.
    ///
    /// It is a timeline widget like any other (`is_timeline`): it joins the
    /// group named by `editor.link` and reads that group's window, so it labels
    /// exactly what the lanes show and moves with them. A press locates the
    /// transport, as a lane's own ruler strip does. Its thickness is the `h`
    /// place prop, like any other widget's — the builders default it.
    TimeRuler { editor: EditorProps },
    /// The dedicated editor-grade piano-roll view: a keyboard gutter, a note
    /// grid, and optional velocity / OSC-event strips — the editor sibling of
    /// the compact `clip` roll, sharing its drawing/hit-test primitives
    /// ([`super::pianoroll`]). MIDI `notes` (`start`/`dur` in timeline samples,
    /// `pitch` a MIDI note over `[min, max]`, plus velocity/channel) draw in the
    /// grid; `osc` events draw as flags in their lane. A timeline widget
    /// (`is_timeline`): it joins a navigation group and carries the ruler /
    /// selection / playhead chrome in `editor`, so it zooms/pans/plays in lockstep
    /// with sibling views. Editing (drag a note, resize an edge, Ctrl+click
    /// add/remove) flows back per the edit-back pattern.
    PianoRoll {
        notes: Vec<super::track::Note>,
        osc: Vec<super::pianoroll::OscMark>,
        /// The multi-note selection (note indices) — native view state, never
        /// parsed from the wire: the marquee/Alt+click gestures build it, block
        /// edits (move, delete, velocity) consume it, and it clears when the
        /// script replaces `notes` (the indices would dangle).
        selected: Vec<usize>,
        min: f32,
        max: f32,
        snap: f64,
        velocity_lane: bool,
        osc_lane: bool,
        /// Live MIDI input: when on, the native host opens its virtual MIDI
        /// input port and **paints** incoming notes into this roll — at the
        /// running playhead, or step-entry on the snap grid when stopped.
        midi_in: bool,
        label: Option<String>,
        editor: EditorProps,
    },
    /// One clip on a `track`: a placed rectangle spanning `[offset, offset +
    /// dur]` in timeline sample units (the graphic unit — length = duration),
    /// with a `label`. Interaction (drag to move `offset`, drag an edge to
    /// resize `dur`) writes back through the edit-back path.
    ///
    /// **A clip is a container, and its bodies are its children.** A take is a
    /// **signal** element, a roll of events a [`PianoRoll`], an automation
    /// curve a `curve` element — the same elements that stand on their own
    /// composed here rather than reimplemented, and **layered** back to front
    /// rather than selected by precedence: an envelope drawn over the material
    /// it shapes is one clip, not two. Each keeps its own value axis, because a
    /// roll's `min`/`max` are pitches and a curve's are its parameter's.
    ///
    /// They are built from the clip's own props (`data`/`blob`/`path`/`cache`/
    /// `buffer`, `notes`, `points`) because the wire still describes a clip as
    /// a thing with bodies; moving the wire onto the containment is a separate
    /// step. So they carry **no id**: a script addresses the clip, and a
    /// `/gui_set` of a body prop routes into the child that owns it.
    ///
    /// [`PianoRoll`]: WidgetKind::PianoRoll
    Clip {
        offset: f64,
        dur: f64,
        label: Option<String>,
    },
    /// A **directed, typed** patcher (a GraphDef at level 1, a SynthDef/FaustDef
    /// at level 2): boxes with inlets on their top edge and outlets on their
    /// bottom, and a cord per `outlet → inlet` connection, weighted by rate (audio
    /// heavy, control thin, init dashed). Dragging an outlet to an inlet (either
    /// grab order) draws a cord, refusing a rate mismatch; the edit leaves as a
    /// flat directed `"wire"` event. At level 1 the buses are not drawn — a cord
    /// *is* a bus (the client names them); at level 2 a cord is an internal wire.
    /// A leaf.
    Patch {
        patch: super::patch::PatchDraw,
        /// The multi-box selection (box indices) — native view state, never
        /// parsed from the wire: the click/marquee gestures build it, the move
        /// drag consumes it, and it clears when the script replaces `boxes`
        /// (the indices would dangle).
        selected: Vec<usize>,
        label: Option<String>,
    },
    /// A **registered element**: a leaf this build renders through the
    /// [`Element`] trait rather than through an arm of this enum, built by the
    /// constructor a program registered under the wire type it answers to
    /// ([`element::register`]).
    ///
    /// It sits beside the built-ins rather than replacing them: the built-in
    /// names are matched first, so a registration can neither shadow one nor
    /// change what an existing def means, and a name nothing registered stays
    /// [`Unknown`](WidgetKind::Unknown) — laid out and not painted, the
    /// behavior an older host already has against a newer script.
    Custom(Box<dyn Element>),
    /// A type this build does not render yet. Laid out so it reserves space, but
    /// not painted. Carries the type tag for logs.
    Unknown(String),
}

/// The default window size when a GuiDef omits `w`/`h`.
const DEFAULT_WINDOW: (u32, u32) = (640, 360);
/// The default peak-pyramid bucket for an inline signal element.
use super::signal::DEFAULT_BASE_BUCKET;

/// A typed widget node: its id (the root's comes from the `/gui_def` argument),
/// its kind, and its children (only containers have any).
#[derive(Debug, Clone)]
pub struct Widget {
    pub id: Option<i32>,
    pub kind: WidgetKind,
    /// The generic layout props (`w`/`h`/`weight`/`x`/`y`) this widget carries.
    pub place: Place,
    /// The `theme` prop: a partial role table (`role -> "#rrggbb[aa]"`, the
    /// same shape as the TOML style file) overlaying the parent's theme for
    /// this widget's whole subtree — a **theme group**.
    pub theme_over: Option<serde_json::Map<String, Value>>,
    /// The `color` prop: the single-color shorthand — an overlay of just the
    /// roles that carry this widget's function (see
    /// [`Theme::accent_seeded`](super::theme::Theme::accent_seeded)).
    pub color: Option<super::paint::Color>,
    /// The `gestures` prop: the container's own (modifier → plan) table, replacing
    /// the default its kind carries ([`GestureMap::of_kind`]). `None` on the
    /// overwhelming majority of widgets, which are not containers and whose
    /// press is the element's.
    pub gestures: Option<GestureMap>,
    /// The resolved theme this widget draws with, produced at mutation points
    /// by [`resolve_themes`] (an [`Arc`] clone per widget, so the per-frame
    /// path reads exactly one theme and pays nothing). `None` until the first
    /// resolve — the renderer falls back to the host theme.
    pub theme: Option<Arc<super::theme::Theme>>,
    pub children: Vec<Widget>,
}

/// Applies one `/gui_set` key/value to a widget: its kind's own keys, plus —
/// for a `clip` — the props of the bodies it holds as children. See
/// [`apply::apply_widget`].
pub fn apply_widget(widget: &mut Widget, key: &str, v: &Value) -> bool {
    apply::apply_widget(widget, key, v)
}

/// Resolves every widget's theme reference: walking from `base` (the host
/// theme), a `theme` prop overlays the inherited table for its subtree and a
/// `color` prop re-seeds the function roles for its one widget — both at this
/// **mutation point**, never per frame. Recursive and cheap by construction:
/// a widget with neither prop shares its parent's `Arc`.
pub fn resolve_themes(widget: &mut Widget, base: &Arc<super::theme::Theme>) {
    let group = match &widget.theme_over {
        Some(table) => {
            let mut t = (**base).clone();
            for warning in t.overlay_json(table) {
                tracing::warn!("widget {:?}: {warning}", widget.id);
            }
            Arc::new(t)
        }
        None => base.clone(),
    };
    widget.theme = Some(match widget.color {
        Some(c) => Arc::new(super::theme::Theme::accent_seeded(&group, c)),
        None => group.clone(),
    });
    for child in &mut widget.children {
        resolve_themes(child, &group);
    }
}

impl Widget {
    /// Interprets a generic [`GuiNode`] (and the blobs carried beside it in the
    /// `/gui_def` message) into a typed widget tree. `root_id` is the def id from
    /// the OSC argument, used for the root whose JSON carries no `id`.
    pub fn from_node(root_id: i32, node: &GuiNode, blobs: &[Vec<u8>]) -> Result<Widget, String> {
        let mut widget = Self::build(Some(root_id), node, blobs)?;
        Self::link_lanes(&mut widget, root_id);
        Ok(widget)
    }

    /// Links every un-linked `track` — and every un-linked free-standing
    /// `timeruler` — of a window into one navigation group keyed by the window
    /// root. The multitrack's promise is **one shared time axis** (aligned
    /// lanes), and a navigation group is exactly that — so the lanes of a
    /// window navigate as one by default, zooming and panning together, and
    /// only an explicit `link` splits them (or joins lanes across windows).
    ///
    /// The ruler is in for the same reason and not by analogy: a free-standing
    /// ruler exists to rule the lanes beside it, so one dropped into a window
    /// of lanes with nothing said is asking for *their* axis. Every other
    /// timeline view stays out — a `waveform` in a window of lanes is showing
    /// its own buffer, and joining it to the composition's axis would be a
    /// guess.
    fn link_lanes(widget: &mut Widget, root_id: i32) {
        if let WidgetKind::Track { editor, .. } | WidgetKind::TimeRuler { editor } =
            &mut widget.kind
            && editor.link.is_none()
        {
            editor.link = Some(root_id);
        }
        for child in &mut widget.children {
            Self::link_lanes(child, root_id);
        }
    }

    fn build(id: Option<i32>, node: &GuiNode, blobs: &[Vec<u8>]) -> Result<Widget, String> {
        let id = id.or(node.id);
        let props = &node.props;
        let kind = build::build_kind(node.kind.as_str(), props, !node.children.is_empty(), blobs)?;
        // Only containers carry children into the typed tree; a leaf's children
        // (if any) are ignored. A `track` carries its clips.
        let children = match kind {
            WidgetKind::Window { .. }
            | WidgetKind::Panel { .. }
            | WidgetKind::Scroll { .. }
            | WidgetKind::Stack { .. }
            | WidgetKind::Track { .. } => node
                .children
                .iter()
                .map(|c| Self::build(None, c, blobs))
                .collect::<Result<Vec<_>, _>>()?,
            // A clip is a container too, but its children are not on the wire:
            // the wire still describes a clip as a thing with bodies, so the
            // bodies are built from its own props (see `build::clip_bodies`).
            // Anything nested under a `clip` node is ignored, as under a leaf.
            WidgetKind::Clip { .. } => build::clip_bodies(props, blobs)?,
            _ => Vec::new(),
        };
        let gestures = props.get("gestures").and_then(|v| {
            let mut map = GestureMap::of_kind(&kind);
            map.overlay(v).then_some(map)
        });
        Ok(Widget {
            id,
            kind,
            place: Place::parse(props),
            gestures,
            theme_over: props.get("theme").and_then(Value::as_object).cloned(),
            color: props
                .get("color")
                .and_then(Value::as_str)
                .and_then(super::theme::parse_hex),
            theme: None,
            children,
        })
    }

    /// Applies a `/gui_set` of the style props (`theme`, `color`) to this
    /// widget. A `theme` value rides as a JSON object or its string carrier
    /// (the scalar wire, like `points`); an empty string (or empty object)
    /// clears the group, an empty `color` clears the accent. Returns whether
    /// the key was a style key that applied — the caller re-resolves the
    /// window's themes.
    pub fn style_apply(&mut self, key: &str, v: &Value) -> bool {
        match key {
            "color" => match v.as_str() {
                Some("") => {
                    self.color = None;
                    true
                }
                Some(hex) => match super::theme::parse_hex(hex) {
                    Some(c) => {
                        self.color = Some(c);
                        true
                    }
                    None => false,
                },
                None => false,
            },
            "theme" => {
                let value = match v {
                    Value::String(s) if s.is_empty() => Value::Object(Default::default()),
                    Value::String(s) => match serde_json::from_str::<Value>(s) {
                        Ok(parsed) => parsed,
                        Err(_) => return false,
                    },
                    other => other.clone(),
                };
                match value.as_object() {
                    Some(table) if table.is_empty() => {
                        self.theme_over = None;
                        true
                    }
                    Some(table) => {
                        self.theme_over = Some(table.clone());
                        true
                    }
                    None => false,
                }
            }
            _ => false,
        }
    }

    /// Applies a `/gui_set gestures` to this container: the same overlay the
    /// prop takes at build time, on top of the kind's defaults — so a set names
    /// only the modifiers it changes and an empty table restores the defaults.
    /// Returns whether the value was usable.
    pub fn gestures_apply(&mut self, v: &Value) -> bool {
        let mut map = GestureMap::of_kind(&self.kind);
        if !map.overlay(v) {
            return false;
        }
        self.gestures = Some(map);
        true
    }

    /// Every widget in this subtree, `self` first and each child's subtree in
    /// order — the tree's one traversal.
    ///
    /// Nearly everything a pass wants from the tree is a filter over this: the
    /// live buses a window reads, the timeline views on an axis, the ids the
    /// server leg must query. Writing each of those as its own recursion is
    /// what the walk-shaped helper functions used to be, and every one of them
    /// had to re-state the same two lines to get the order right.
    ///
    /// Order is **pre-order**, which is the order the layout emits and the
    /// order the drawing depends on: a parent is seen before the children it
    /// contains.
    pub fn descendants(&self) -> Descendants<'_> {
        Descendants { stack: vec![self] }
    }

    /// The widget with id `id` anywhere in this tree.
    pub fn find(&self, id: i32) -> Option<&Widget> {
        self.descendants().find(|w| w.id == Some(id))
    }

    /// The widget with id `id` anywhere in this tree, mutably (for `/gui_set`
    /// and interaction).
    pub fn find_mut(&mut self, id: i32) -> Option<&mut Widget> {
        if self.id == Some(id) {
            return Some(self);
        }
        self.children.iter_mut().find_map(|c| c.find_mut(id))
    }
}

/// The pre-order walk of a widget subtree ([`Widget::descendants`]).
///
/// An explicit stack rather than a recursion, so a caller can `filter`,
/// `find` or `any` over the tree and stop where it likes — a deep tree costs
/// no call frames.
pub struct Descendants<'a> {
    stack: Vec<&'a Widget>,
}

impl<'a> Iterator for Descendants<'a> {
    type Item = &'a Widget;

    fn next(&mut self) -> Option<&'a Widget> {
        let widget = self.stack.pop()?;
        // Reversed, so the pop order is the children's own order.
        self.stack.extend(widget.children.iter().rev());
        Some(widget)
    }
}

impl WidgetKind {
    pub fn apply(&mut self, key: &str, v: &Value) -> bool {
        apply::apply_kind(self, key, v)
    }
}
