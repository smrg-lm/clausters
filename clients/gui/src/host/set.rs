//! **A live widget's props changed** (`/gui_set`, a binding, a correction an
//! owner pushed): the registry, the typed tree the front draws, the navigation
//! group a timeline view shares, and the subscriptions a changed source moves.
//! `focus` is the one key that is not a prop, and is taken out here.

use super::*;

impl Host {
    /// Replaces an `axes` pair among `props` with the per-axis keys it names,
    /// leaving every other pair where it is. A `/gui_set` that names no axes --
    /// which is nearly all of them -- allocates nothing.
    pub(super) fn expand_axes(props: Vec<(String, Value)>) -> Vec<(String, Value)> {
        if !props.iter().any(|(k, _)| k == widget::AXES) {
            return props;
        }
        let mut out = Vec::with_capacity(props.len());
        for (key, value) in props {
            if key != widget::AXES {
                out.push((key, value));
                continue;
            }
            // The pair rides as an object or as its string carrier, the way
            // `theme` and `points` do -- OSC has no structural argument.
            let carried;
            let axes = match &value {
                Value::Object(map) => Some(map),
                Value::String(json) => {
                    carried = serde_json::from_str::<Value>(json).ok();
                    carried.as_ref().and_then(Value::as_object)
                }
                _ => None,
            };
            match axes {
                Some(axes) => {
                    let mut flat = serde_json::Map::new();
                    widget::flatten_axes(axes, &mut flat);
                    out.extend(flat);
                }
                None => diag::warn!("{GUI_SET}: {} is not a pair of axes", widget::AXES),
            }
        }
        out
    }

    /// Splits a `focus` pair out of `props`, returning the rest and what it
    /// asked for. A `/gui_set` that does not name it -- which is nearly all of
    /// them -- keeps its vector.
    pub(super) fn take_focus(props: Vec<(String, Value)>) -> (Vec<(String, Value)>, Option<bool>) {
        if !props.iter().any(|(k, _)| k == "focus") {
            return (props, None);
        }
        let mut focus = None;
        let mut out = Vec::with_capacity(props.len());
        for (key, value) in props {
            if key != "focus" {
                out.push((key, value));
                continue;
            }
            match widget::parse::truthy(&value) {
                Some(on) => focus = Some(on),
                None => diag::warn!("{GUI_SET}: focus is not a flag"),
            }
        }
        (out, focus)
    }

    /// Points the keyboard at widget `id` (`focus 1`) or takes it away from it
    /// (`focus 0`) -- the script's half of what Tab and a press do.
    ///
    /// A widget that is not a stop on the ring is refused rather than focused
    /// silently: focus that nothing can read is a script waiting for keystrokes
    /// that will never arrive.
    pub(super) fn set_focused(&mut self, id: i32, on: bool, effects: &mut Vec<HostEffect>) {
        let Some(root) = self.registry.root_of(id) else {
            return;
        };
        if !on {
            if self.focused == Some((root, id))
                && let Some(old) = self.clear_focus()
            {
                effects.push(HostEffect::Redraw(old));
            }
            return;
        }
        let accepts = self
            .window_defs
            .get(&root)
            .and_then(|tree| tree.find(id))
            .is_some_and(|w| w.kind.accepts_focus());
        if !accepts {
            return diag::warn!("{GUI_SET} {id}: this widget does not take the keyboard focus");
        }
        if let Some(other) = self.focus(root, id) {
            effects.push(HostEffect::Redraw(other));
        }
        effects.push(HostEffect::Redraw(root));
    }

    /// The mutation a `/gui_set` performs, without its wire form: apply `props`
    /// to widget `id` in the generic registry and, when it is inside an open
    /// window, in the typed render tree. Returns whether the widget exists.
    ///
    /// It is a method of its own because a **widget binding** performs exactly
    /// this and nothing else -- one apply, never another delivery -- so the two
    /// paths cannot drift and a binding cannot cascade
    /// ([`bind`]).
    pub fn set_props(
        &mut self,
        id: i32,
        props: Vec<(String, Value)>,
        effects: &mut Vec<HostEffect>,
    ) -> bool {
        // An `axes` pair sets the chrome of the container's axes; it is the
        // same relocation `/gui_def` accepts, so it goes through the same
        // table rather than a second one (see `widget::axes`).
        let props = Self::expand_axes(props);
        // `focus` is the one key that is not a prop: it says where the keyboard
        // points, which is the host's state and not the widget's. So it is taken
        // out before the document is written -- a query must not report it, and a
        // reloaded def must not restore a focus nobody asked for.
        let (props, focus) = Self::take_focus(props);
        let keys: Vec<&String> = props.iter().map(|(k, _)| k).collect();
        if !self.registry.set(id, props.clone()) {
            return false;
        }
        if let Some(on) = focus {
            self.set_focused(id, on, effects);
        }
        // A set can retarget a view's source or widen its channel run, which
        // changes what has to be recorded; the diff below is a no-op otherwise.
        let touches_source = keys
            .iter()
            .any(|k| matches!(k.as_str(), "bus" | "rate" | "channels"));
        // And a set can start or stop a *recording* being followed: `fills` is
        // the client saying this buffer is being written into, and `buffer` is
        // which one it is.
        let touches_stream = keys
            .iter()
            .any(|k| matches!(k.as_str(), "fills" | "buffer"));
        // Mirror the change into the typed window tree the front renders. A
        // timeline view's shared keys (`view_*`, `sel_*`, `playhead_at`,
        // `link`) route through its navigation group instead, so a set on any
        // member applies group-wide (linked views).
        let mut is_timeline = false;
        // The extent an authored surface reaches before the props are applied,
        // so a set that *wrote into* it can be told from one that did not.
        let mut span_before = None;
        if let Some(root) = self.registry.root_of(id)
            && let Some(tree) = self.window_defs.get_mut(&root)
        {
            let mut changed = false;
            let mut styled = false;
            if let Some(widget) = tree.find_mut(id) {
                is_timeline = widget.is_timeline();
                span_before = widget.kind.content_span();
                for (k, v) in &props {
                    if !(is_timeline && timeline::is_timeline_key(k)) {
                        // The generic place props (`w`/`h`/`weight`/`x`/`y`)
                        // and the style props (`theme`/`color`) apply to any
                        // widget; everything else is the kind's own.
                        let style = widget.style_apply(k, v);
                        styled |= style;
                        changed |= style
                            || (k == "gestures" && widget.gestures_apply(v))
                            || widget.place.apply(k, v)
                            || widget::apply_widget(widget, k, v);
                    }
                }
            }
            // A style change re-resolves the window's theme references -- the
            // mutation point where a theme group cascades to its subtree.
            if styled {
                widget::resolve_style(tree, &Arc::new(self.theme.clone()));
            }
            if changed {
                effects.push(HostEffect::Redraw(root));
            }
        }
        if is_timeline {
            self.set_timeline_props(id, &props, effects);
        }
        // A content change moves the extent the shared axis spans, so it has to
        // be re-registered: a moved or resized clip lengthens its lane, and an
        // **authored** surface's extent *is* what has been written into it.
        // Without this a roll stays on the axis it was defined with, so
        // everything painted into an empty one lands outside the window.
        let span_after = self
            .widget_kind(self.registry.root_of(id).unwrap_or(id), id)
            .and_then(|k| k.content_span());
        if span_after.is_some() && span_after != span_before {
            // Keeping the window, not refitting it: a roll is *written into*, a
            // note at a time, so a take that grows must scroll under a still
            // axis rather than zoom it out from under the notes just drawn --
            // the same rule a dragged clip follows, for the same reason. And
            // once the take runs past the right edge, the axis pages forward to
            // where it is still being written.
            self.sync_track_totals_keeping_view();
            self.follow_timeline_end(id, effects);
        }
        if touches_source {
            self.sync_bus_watches();
        }
        if touches_stream || touches_source {
            self.sync_buffer_streams();
        }
        true
    }
}
