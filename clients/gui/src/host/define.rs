//! **A widget tree defined, and freed**: `/gui_def` and its in-process door
//! ([`Host::define`]), `/gui_free`, and what the host forgets when a widget is
//! gone. A definition is reconciled with the tree it replaces, so what the
//! host put on a surviving widget -- its zoom, its selection -- is kept.

use super::wire::{blob_args, int_arg, json_arg};
use super::*;

impl Host {
    /// `/gui_def <id> <json> [blob...]` -- build a whole widget tree from one JSON
    /// GuiDef (with any bulk data, e.g. waveform samples, as trailing blobs). A
    /// `window` root also opens (or rebuilds) a window.
    pub(super) fn on_def(
        &mut self,
        args: &[OscType],
        from: ClientId,
        effects: &mut Vec<HostEffect>,
    ) {
        let Some(id) = int_arg(args, 0) else {
            return diag::warn!("{from}: {GUI_DEF} needs an integer id");
        };
        let Some(bytes) = json_arg(args, 1) else {
            return diag::warn!("{from}: {GUI_DEF} needs a JSON string or blob argument");
        };
        let node = match GuiNode::parse(bytes) {
            Ok(node) => node,
            Err(e) => return diag::warn!("{from}: {GUI_DEF} {id}: invalid GuiDef JSON: {e}"),
        };
        let blobs = blob_args(&args[2.min(args.len())..]);
        self.define_node(id, node, bytes.to_vec(), &blobs, &from, effects);
    }

    /// Defines a GuiDef tree **from Rust**, with no document to write and parse
    /// back: the counterpart of `/gui_def` for a program that links the crate,
    /// taking the node the parser would have produced (build one with
    /// [`crate::tree`]). A `window` root opens or rebuilds its window, so the
    /// returned effects are the ones [`Self::handle_packet`] returns.
    ///
    /// The def is still recorded as the document it is -- persisted by `name`,
    /// reloadable, answerable by `/gui_query` -- because the JSON is *derived*
    /// here rather than skipped: there is one definition path, and this is its
    /// other entrance.
    pub fn define(&mut self, root_id: i32, root: impl Into<GuiNode>) -> Vec<HostEffect> {
        self.define_with_blobs(root_id, root, &[])
    }

    /// [`Self::define`] with the bulk payloads a `"blob": <index>` prop refers
    /// to -- the in-process equivalent of the blobs trailing a `/gui_def`
    /// message.
    pub fn define_with_blobs(
        &mut self,
        root_id: i32,
        root: impl Into<GuiNode>,
        blobs: &[Vec<u8>],
    ) -> Vec<HostEffect> {
        let node = root.into();
        // The verbatim document, derived from the node before anything
        // rewrites it -- the same bytes the wire would have carried, which is
        // what persistence and reload are the source of truth over.
        let bytes = match serde_json::to_vec(&node) {
            Ok(bytes) => bytes,
            Err(e) => {
                diag::warn!("in-process: {GUI_DEF} {root_id}: cannot serialize the tree: {e}");
                return Vec::new();
            }
        };
        let mut effects = Vec::new();
        self.define_node(root_id, node, bytes, blobs, &"in-process", &mut effects);
        effects
    }

    /// The definition itself, from the point where the document has been
    /// parsed -- shared by the wire (`/gui_def`) and by [`Self::define`], so a
    /// tree built in Rust is recorded, rendered, bound and persisted by
    /// exactly the same steps as one that arrived as JSON. `source` only names
    /// who asked, for the log.
    pub(super) fn define_node(
        &mut self,
        id: i32,
        mut node: GuiNode,
        bytes: Vec<u8>,
        blobs: &[Vec<u8>],
        source: &dyn std::fmt::Display,
        effects: &mut Vec<HostEffect>,
    ) {
        // The log names whoever asked -- a client address on the wire, the
        // process itself in a Rust program.
        let from = source;
        // The axis chrome lands flat before anything records it, so the
        // registry -- and the `/gui_info` a query answers with -- carries the
        // props the host itself reads, whichever spelling the tree used. The
        // node's *type* is kept as written: a query answers in the vocabulary
        // the script wrote.
        widget::flatten_tree_axes(&mut node);
        // Keep the verbatim JSON: the source of truth for persistence and reload.
        self.def_json.insert(id, bytes.clone());
        // A def states what this widget now is, reconciled or not, so an edit
        // still in flight against what it was has nothing left to resolve to:
        // its widget may be gone, or its id may name something else now. Drop
        // the pending set here for the same reason `/gui_free` does -- an
        // acknowledgement that is never coming holds the outbox open forever,
        // and the new tree is authoritative by definition.
        self.outbox.borrow_mut().forget(id);
        // **Which window this widget belongs to, read before it is redefined.**
        // Defining an id makes it a root of the registry's own tree, so asking
        // afterwards answers the widget itself and the window is lost.
        let inside = if node.kind == "window" {
            None
        } else {
            self.registry.root_of(id)
        };
        // What each id was registered as, read before this def overwrites the
        // record: the reconcile below needs the wire's own word for the kind a
        // surviving id used to name.
        let was = self.registry.kinds();
        let outcome = self.registry.define(id, &node);
        // The acceptance criterion: log the parsed tree.
        diag::info!(
            "{from}: {GUI_DEF} {id}: {} widget(s){}{}\n{}",
            outcome.inserted,
            if outcome.replaced { " (replaced)" } else { "" },
            if outcome.skipped > 0 {
                format!(", {} skipped", outcome.skipped)
            } else {
                String::new()
            },
            node.dump(id).trim_end(),
        );
        // A window root becomes a renderable typed document; the front opens it.
        if node.kind == "window" {
            let held = self.window_defs.get(&id);
            match self.build_tree(id, &node, blobs, held, &was) {
                Ok(tree) => {
                    self.window_defs.insert(id, tree);
                    self.tree_changed(id, effects);
                }
                Err(e) => diag::warn!("{from}: {GUI_DEF} {id}: cannot build window: {e}"),
            }
        } else if let Some(root) = inside {
            // **A widget inside an open window is redefined in place.** The
            // wire has always said a def names any id -- "re-sending an existing
            // id redefines it" -- and until now only a `window` reached the
            // typed tree the front draws, so a def of anything else was
            // recorded, logged, and invisible.
            //
            // It is the one channel a widget that was not there can arrive by,
            // and doing it to the **window** rebuilds every widget in it: a
            // clip appearing in one lane took the zoom, the scroll and the
            // selection of every other lane with it. Splicing the subtree keeps
            // all of that, because everything outside it is the same object it
            // was.
            let held = self.window_defs.get(&root).and_then(|tree| tree.find(id));
            match self.build_tree(id, &node, blobs, held, &was) {
                Ok(subtree) => {
                    if let Some(tree) = self.window_defs.get_mut(&root)
                        && let Some(held) = tree.find_mut(id)
                    {
                        *held = subtree;
                        // **The window is brought up to the tree, not merely
                        // repainted** ([`Self::tree_changed`]): a subtree that
                        // changed shape has not been measured at all.
                        self.tree_changed(root, effects);
                    } else {
                        diag::warn!(
                            "{from}: {GUI_DEF} {id}: no widget by that id in the \
                             window it belongs to"
                        );
                    }
                }
                Err(e) => diag::warn!("{from}: {GUI_DEF} {id}: cannot build widget: {e}"),
            }
        }
        // A redefine frees the old subtree first: what the host kept for a
        // widget that did not survive into the new tree goes with it.
        if outcome.replaced {
            self.forget_gone();
        }
        // Inline `bind` props register a binding declaratively, so a saved GuiDef
        // carries its own bindings (the standalone path) and a live script may
        // bind without a separate `/gui_bind`.
        self.register_inline_bindings(&node);
        // A GuiDef with a `name` persists to the store the way a named SynthDef
        // does on `/def_send synth` -- no separate save command.
        if let Some(name) = node.props.get("name").and_then(Value::as_str)
            && let Some(store) = self.store.as_ref()
        {
            match store.save(name, id, &bytes) {
                Ok(()) => diag::info!("{from}: {GUI_DEF} {id}: saved as \"{name}\""),
                Err(e) => diag::warn!("{from}: {GUI_DEF} {id}: cannot save \"{name}\": {e}"),
            }
        }
    }

    /// **Builds widget `id`'s tree from `node`**, the way both a window and a
    /// widget inside one are defined: what the host put on every widget of
    /// `held` that survives is carried across, and the style is resolved.
    ///
    /// **A def says what to look like, not what to destroy.** The old tree is
    /// walked beside the new one and everything the host itself put on a
    /// widget that survived -- its window on the axis, its selection, its
    /// layer -- is carried across, which is why a clip appearing in one lane
    /// no longer takes the zoom of every other lane with it. Theme groups and
    /// per-widget accents resolve here -- at the mutation point, never per
    /// frame.
    pub(super) fn build_tree(
        &self,
        id: i32,
        node: &GuiNode,
        blobs: &[Vec<u8>],
        held: Option<&Widget>,
        was: &HashMap<i32, String>,
    ) -> Result<Widget, String> {
        let mut tree = Widget::from_node(id, node, blobs).map_err(|e| e.to_string())?;
        if let Some(held) = held {
            widget::reconcile::reconcile(held, &mut tree, node, id, was);
        }
        widget::resolve_style(&mut tree, &Arc::new(self.theme.clone()));
        Ok(tree)
    }

    /// **Window `window`'s tree changed shape**: what the server records for
    /// it and the navigation groups its views join are brought in step, and
    /// the front is asked to bring the window up to the tree.
    ///
    /// `OpenWindow` and not `Redraw`, even for a window that is open: a
    /// `Redraw` asks the front for another frame of what it already measured,
    /// and new widgets that were never measured come out with no size, draw
    /// nothing and cannot be hit -- a window that reads as having stopped
    /// working. `OpenWindow` on an open window keeps the shell (the surface,
    /// the cursor, the gestures) and rebuilds the def's state over the tree as
    /// it now is, which holds every widget outside a spliced subtree
    /// unchanged, with the zoom and the scroll it had.
    pub(super) fn tree_changed(&mut self, window: i32, effects: &mut Vec<HostEffect>) {
        self.sync_subscriptions();
        self.sync_timeline_groups(Some(window));
        effects.push(HostEffect::OpenWindow(window));
    }

    /// `/gui_free <id>` -- destroy a widget and its subtree (and its window, if
    /// `id` is a window-rooted def).
    pub(super) fn on_free(
        &mut self,
        args: &[OscType],
        from: ClientId,
        effects: &mut Vec<HostEffect>,
    ) {
        let Some(id) = int_arg(args, 0) else {
            return diag::warn!("{from}: {GUI_FREE} needs an integer id");
        };
        let removed = self.registry.free(id);
        self.def_json.remove(&id);
        if self.window_defs.remove(&id).is_some() {
            // The window goes, and with it the scale a shell reported for it.
            self.resolved_metrics.remove(&id);
            // ...and whatever it had in flight: an acknowledgement for a widget
            // that no longer exists is never coming, and a pending edit that
            // waits for one holds the outbox open forever.
            self.outbox.borrow_mut().forget(id);
            // The status bar is the window's own history and goes with it: a
            // window reopened on the same id starts with nothing to say.
            self.status.borrow_mut().remove(&id);
            effects.push(HostEffect::CloseWindow(id));
        }
        self.sync_subscriptions();
        self.forget_gone();
        if removed > 0 {
            diag::info!("{from}: {GUI_FREE} {id}: freed {removed} widget(s)");
        } else {
            diag::warn!("{from}: {GUI_FREE} {id}: no such widget");
        }
    }

    /// **Forgets what the host kept for widgets that are gone**, after a
    /// `/gui_free` or a redefining `/gui_def` -- one pass, so the two cannot
    /// disagree about what a missing widget leaves behind.
    ///
    /// Everything the host keeps by widget id: a binding (a freed id must not
    /// keep forwarding), the focus (keystrokes must not reach a widget that is
    /// not there), the counter a playhead was named to read, the navigation
    /// group's state and extents, the monitor's nodes for a take nobody draws,
    /// and the live voices of a piano -- which must not leave keys sounding.
    pub(super) fn forget_gone(&mut self) {
        let registry = &self.registry;
        let windows = &self.window_defs;
        self.bindings.retain(|id, _| registry.contains(*id));
        self.head_clocks
            .retain(|id, _| windows.contains_key(id) || registry.contains(*id));
        if let Some((_, id)) = self.focused
            && !self.registry.contains(id)
        {
            self.focused = None;
        }
        self.prune_timeline_groups();
        self.prune_monitor();
        self.prune_voices();
    }

    /// Registers a [`Binding`] for every widget that declares an inline `bind`
    /// array in the GuiDef (`{"id":...,"type":...,"bind":["/node_set",node,"freq"]}`).
    pub(super) fn register_inline_bindings(&mut self, node: &GuiNode) {
        if let Some(id) = node.id
            && let Some(Value::Array(items)) = node.props.get("bind")
        {
            match Binding::from_json(items) {
                Ok(binding) => {
                    self.bindings.insert(id, binding);
                }
                Err(e) => diag::warn!("widget {id}: invalid inline `bind`: {e}"),
            }
        }
        for child in &node.children {
            self.register_inline_bindings(child);
        }
    }
}
