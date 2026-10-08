//! The keyboard half of the machine: the focus ring, the key that goes to the
//! focused element, the **verbs** the host's key table names, and the ones the
//! window itself performs (play, the loop, the ends, the clipboard over a view,
//! resetting every view to its full extent).
//!
//! Split from the pointer machine because it shares nothing with it but the
//! `Gestures` state: no hit-test, no drag, no cursor -- a key arrives already
//! addressed to whatever the window has focused or selected.
//!
//! **One dispatch, for both fronts**: [`Gestures::press_key`]. A front only
//! translates its own event into a [`Key`] and calls it; what the key means is
//! decided here, once, in this order -- an open popup (modal), Tab (the
//! ring's), Escape (an open dialog's), the **focused element's raw key** (a
//! field types, a list walks), and only then the key table
//! ([`crate::host::keymap`]): the verb it names goes to the focused element,
//! to the element under the pointer, to the window, and finally -- performed by
//! nobody here -- to the window's owner. That order is what lets a field
//! swallow `q` while a roll behind it still quantizes on the same key when
//! nothing is focused.

use super::super::Host;
use super::super::interact::Hit;
use crate::host::diag;
use clausters_core::osc::OscType;
use clausters_editing::audio_playback::space;

use super::super::clipboard::Clip;
use super::super::keymap::Verb;
use super::super::widget::element::{Key, KeyInput, Mods, SampleBlock, refusal};
use super::effects::{emit, emit_view, redraw_all, tell};
use super::nav::{cursor_of, freq_nav_ids, hit, set_x_view, set_y_view, timeline_ids};
use super::{GestureCtx, GestureEffect, Gestures, element, focus};

impl Gestures {
    /// A key arriving at this window: Tab walks the focus ring, anything else
    /// goes to the focused element's
    /// [`Element::key`](crate::host::widget::Element::key) -- which delivers
    /// whatever it reports exactly as a drag would, bound -> straight to the
    /// audio server, else a `/gui_event`.
    ///
    /// A cut, a copy and a paste read and write the host's clipboard
    /// ([`Host::clipboard`]).
    ///
    /// Returns `Some(effects)` when the key was consumed -- the key table is
    /// then not read -- and `None` when nothing here answered it.
    pub fn key(&self, host: &mut Host, ctx: &GestureCtx, key: Key) -> Option<Vec<GestureEffect>> {
        with_clipboard(host, |host, clip| self.key_with(host, ctx, key, clip))
    }

    /// [`key`](Self::key), with the clipboard taken off the host for the
    /// length of the call (an element borrows the host while it edits).
    fn key_with(
        &self,
        host: &mut Host,
        ctx: &GestureCtx,
        key: Key,
        clipboard: &mut Clip,
    ) -> Option<Vec<GestureEffect>> {
        // **An open list is modal**: it takes every key, the walking ones to
        // walk it and the rest to swallow -- Tab included, since the focus
        // cannot leave a list that is still open.
        if let Some(out) = self.popup_key(host, ctx, &key) {
            return Some(out);
        }
        // **A marker's name being typed takes every key** until it is given
        // or left (`naming`), as a field does.
        if let Some(out) = super::naming::key(host, ctx, &key, clipboard) {
            return Some(out);
        }
        if key == Key::Tab {
            return Some(focus::step(host, ctx, ctx.shift));
        }
        // **Escape asks a dialog to go**: with one up it is the dialog's,
        // reported as `"cancel"` from the dialog for whoever defined it to free
        // -- the host does not free a node a client made. It never reaches the
        // front then, whose own Escape would close the whole window behind it.
        if key == Key::Escape {
            let dialog = host
                .layout_window(ctx.def_id, ctx.fb_w, ctx.fb_h)
                .and_then(|placed| {
                    let at = crate::host::chrome::modal_start(&placed)?;
                    placed[at].widget.id
                });
            // nothing open: the focused element's, then the key table's
            // scopes, and with neither the front's own Escape
            if let Some(dialog) = dialog {
                let mut out = Vec::new();
                emit(
                    host,
                    &mut out,
                    ctx.def_id,
                    dialog,
                    vec![OscType::String(super::chrome::CANCEL.into())],
                );
                return Some(out);
            }
        }
        // Only an element focused in *this* window: a key is delivered by the
        // window it was typed into.
        let (fdef, id) = host.focused()?;
        if fdef != ctx.def_id {
            return None;
        }
        let placed = host.layout_window(ctx.def_id, ctx.fb_w, ctx.fb_h)?;
        let (rect, scale, indent) = placed
            .iter()
            .find(|p| p.widget.id == Some(id))
            .map(|p| (p.rect, p.scale, p.indent))?;
        let mut input = KeyInput {
            mods: Mods {
                shift: ctx.shift,
                ctrl: ctx.ctrl,
                alt: ctx.alt,
            },
            clipboard,
            cursor: cursor_of(host, ctx, id),
        };
        let at = element::At::widget(id, rect, scale, indent);
        // The element's own arm first; then, for Space and Enter, the
        // keyboard's press -- do what a click on this control does.
        let pressed = matches!(key, Key::Enter | Key::Char(' '));
        let (events, selected) = element::with(host, ctx, at, |el, placed| {
            let events = el
                .key(&key, &mut input)
                .or_else(|| pressed.then(|| el.activate(placed)).flatten());
            (events, el.selected_text())
        })?;
        let events = events?;
        // **What a field has selected is the primary selection**, whichever
        // way it was selected -- Shift and an arrow as much as a drag.
        if let Some(text) = selected {
            input.clipboard.set_primary(&text);
        }
        let mut out = Vec::new();
        // The element consumed it, so the window repaints whether or not
        // anything was reported: a caret that moved is a picture that changed.
        element::report(host, &mut out, ctx, id, events);
        out.push(GestureEffect::Redraw(ctx.def_id));
        Some(out)
    }

    /// **A key, all the way**: what both fronts call with the key they read and
    /// where the pointer is (`None` when it is unknown -- off the window, or
    /// not yet moved over it).
    ///
    /// First [`key`](Self::key) -- the popup layer, the ring, a dialog's
    /// Escape and the focused element's raw key -- and then the **key table**:
    /// a chord bound to a verb is performed, and is
    /// consumed whether or not anything here acted on it, since the window's
    /// owner is told what nobody performed. Returns `None` for a key that is
    /// neither, which the front may still answer (a desktop window's Escape
    /// closes it).
    pub fn press_key(
        &mut self,
        host: &mut Host,
        ctx: &GestureCtx,
        key: Key,
        pointer: Option<(f64, f64)>,
    ) -> Option<Vec<GestureEffect>> {
        with_clipboard(host, |host, clip| {
            self.press_key_with(host, ctx, key, pointer, clip)
        })
    }

    fn press_key_with(
        &mut self,
        host: &mut Host,
        ctx: &GestureCtx,
        key: Key,
        pointer: Option<(f64, f64)>,
        clipboard: &mut Clip,
    ) -> Option<Vec<GestureEffect>> {
        let mods = Mods {
            shift: ctx.shift,
            ctrl: ctx.ctrl,
            alt: ctx.alt,
        };
        // **A command chord goes through an open menu**: the menu closes and
        // the command runs, as a key equivalent does during a platform's menu
        // tracking. Only a chord is -- Ctrl or Alt held, or a function key --
        // since a bare letter, the arrows, Enter and Escape are the open
        // list's to walk, pick and dismiss with.
        let mut out = Vec::new();
        let command = mods.ctrl || mods.alt || matches!(key, Key::F(_));
        let scopes = host.window_keys(ctx.def_id);
        if command
            && host.popup(ctx.def_id).is_some()
            && host.keys.lookup_in(&key, mods, &scopes).is_some()
        {
            host.close_popup(ctx.def_id);
            out.push(GestureEffect::Redraw(ctx.def_id));
        }
        if let Some(more) = self.key_with(host, ctx, key.clone(), clipboard) {
            out.extend(more);
            return Some(out);
        }
        let Some(verb) = host.keys.lookup_in(&key, mods, &scopes).map(str::to_string) else {
            diag::note!(host, ctx.def_id, "key", "key {key:?}: bound to nothing");
            return None;
        };
        out.extend(self.perform(host, ctx, &verb, pointer, clipboard));
        Some(out)
    }

    /// **Performs the verb `name` for a tool, a menu entry or a client's
    /// `/gui_verb`**: what a key bound to it would do, with no pointer -- the
    /// hand is on the tool, or nowhere on the window, not on what the verb
    /// acts on.
    pub(crate) fn command(
        &mut self,
        host: &mut Host,
        ctx: &GestureCtx,
        name: &str,
    ) -> Vec<GestureEffect> {
        with_clipboard(host, |host, clip| self.perform(host, ctx, name, None, clip))
    }

    /// **An entry of element `id`'s own context menu**: its command, or the
    /// host's verb by that name performed on it, or -- when it takes neither
    /// -- the verb performed as a key would.
    pub(super) fn context_command(
        &mut self,
        host: &mut Host,
        ctx: &GestureCtx,
        id: i32,
        name: &str,
    ) -> Vec<GestureEffect> {
        with_clipboard(host, |host, clip| {
            if let Some(out) = self.command_to(host, ctx, id, name, clip) {
                return out;
            }
            if let Some(verb) = Verb::named(name)
                && let Some(at) = super::popups::at_widget(host, ctx, id)
                && let Some(out) = self.verb_to(host, ctx, verb, id, at, clip)
            {
                return out;
            }
            self.perform(host, ctx, name, None, clip)
        })
    }

    /// **Performs the verb `name`**, offered in the order a key reaches the
    /// window: the focused element, the element under the pointer, the window
    /// itself, and -- when the host performs no such verb, or nothing here
    /// took it -- the window's owner. The window's `main` element is asked
    /// after the one under the pointer; a name the key table does not hold
    /// is offered to the focused and the `main` element as a command of
    /// their own. The owner is told as `"menu" <verb>` when the window's bar
    /// has an entry for it (the entry and its key are one command) and as the
    /// bare `<verb>` otherwise.
    fn perform(
        &mut self,
        host: &mut Host,
        ctx: &GestureCtx,
        name: &str,
        pointer: Option<(f64, f64)>,
        clipboard: &mut Clip,
    ) -> Vec<GestureEffect> {
        if let Some(verb) = Verb::named(name) {
            // **A paste reads the platform's clipboard first**: what another
            // program copied since is what is pasted. Here, once, and never
            // per key.
            if matches!(verb, Verb::Paste | Verb::Mix) {
                clipboard.refresh();
            }
            if let Some(out) = self.verb_focused(host, ctx, verb, clipboard) {
                return out;
            }
            if let Some((cx, cy)) = pointer
                && let Some(out) = self.verb_at(host, ctx, verb, cx, cy, clipboard)
            {
                return out;
            }
            if let Some(out) = self.verb_main(host, ctx, verb, clipboard) {
                return out;
            }
            if let Some(out) = self.window_verb(host, ctx, verb, pointer, clipboard) {
                return out;
            }
        } else if let Some(out) = self.command_held(host, ctx, name, clipboard) {
            return out;
        }
        let mut out = Vec::new();
        if super::popups::pick_verb(host, ctx, &mut out, name).is_none() {
            emit(
                host,
                &mut out,
                ctx.def_id,
                ctx.def_id,
                vec![OscType::String(name.to_string())],
            );
        }
        out
    }

    /// The verb offered to the element holding this window's focus.
    fn verb_focused(
        &self,
        host: &mut Host,
        ctx: &GestureCtx,
        verb: Verb,
        clipboard: &mut Clip,
    ) -> Option<Vec<GestureEffect>> {
        let (fdef, id) = host.focused()?;
        if fdef != ctx.def_id {
            return None;
        }
        let placed = host.layout_window(ctx.def_id, ctx.fb_w, ctx.fb_h)?;
        let (rect, scale, indent) = placed
            .iter()
            .find(|p| p.widget.id == Some(id))
            .map(|p| (p.rect, p.scale, p.indent))?;
        let at = element::At::widget(id, rect, scale, indent);
        self.verb_to(host, ctx, verb, id, at, clipboard)
    }

    /// The verb offered to the window's **`main` element**, where it is not the
    /// one holding the focus (which was asked already): what a menu entry or a
    /// tool addresses before a hand has been in the window.
    fn verb_main(
        &self,
        host: &mut Host,
        ctx: &GestureCtx,
        verb: Verb,
        clipboard: &mut Clip,
    ) -> Option<Vec<GestureEffect>> {
        let id = self.main_unfocused(host, ctx)?;
        let at = super::popups::at_widget(host, ctx, id)?;
        self.verb_to(host, ctx, verb, id, at, clipboard)
    }

    /// The window's `main` element, unless it holds the focus already.
    fn main_unfocused(&self, host: &Host, ctx: &GestureCtx) -> Option<i32> {
        host.window_main(ctx.def_id)
            .filter(|id| host.focused() != Some((ctx.def_id, *id)))
    }

    /// **A name the key table does not hold**, offered as a command of the
    /// element's own ([`Element::command`](crate::host::widget::Element::command))
    /// to the focused element and then to the window's `main` one.
    fn command_held(
        &self,
        host: &mut Host,
        ctx: &GestureCtx,
        name: &str,
        clipboard: &mut Clip,
    ) -> Option<Vec<GestureEffect>> {
        let focused = host
            .focused()
            .filter(|(d, _)| *d == ctx.def_id)
            .map(|(_, id)| id);
        for id in focused.into_iter().chain(self.main_unfocused(host, ctx)) {
            if let Some(out) = self.command_to(host, ctx, id, name, clipboard) {
                return Some(out);
            }
        }
        None
    }

    /// Element `id` asked to perform its own command `name`; what it reports
    /// is delivered as a verb's is.
    pub(super) fn command_to(
        &self,
        host: &mut Host,
        ctx: &GestureCtx,
        id: i32,
        name: &str,
        clipboard: &mut Clip,
    ) -> Option<Vec<GestureEffect>> {
        let at = super::popups::at_widget(host, ctx, id)?;
        let mut input = KeyInput {
            mods: Mods::default(),
            clipboard,
            cursor: cursor_of(host, ctx, id),
        };
        let events =
            element::with(host, ctx, at, |el, _| el.command(name, &mut input)).flatten()?;
        let mut out = Vec::new();
        element::report(host, &mut out, ctx, id, events);
        host.sync_track_totals_keeping_view();
        out.push(GestureEffect::Redraw(ctx.def_id));
        Some(out)
    }

    /// The verb offered to the **element under the pointer** -- the other
    /// addressee, and the reason a block operation lands where the pointer
    /// already is: a selection is made there.
    fn verb_at(
        &self,
        host: &mut Host,
        ctx: &GestureCtx,
        verb: Verb,
        cx: f64,
        cy: f64,
        clipboard: &mut Clip,
    ) -> Option<Vec<GestureEffect>> {
        let Hit {
            id,
            rect,
            scale,
            indent,
            ..
        } = hit(host, ctx, cx, cy)?;
        let at = element::At::widget(id, rect, scale, indent);
        let out = self.verb_to(host, ctx, verb, id, at, clipboard);
        if out.is_none() {
            // **A verb nothing claimed is the quietest failure there is**, and
            // it is the shape of the defect reported twice on 2026-09-12: a
            // verb refused correctly by an element that had nothing to act on
            // and a key no element answers to are indistinguishable at the
            // window. The element's own refusals are said out loud; this is
            // the other case, and it is the machine's business rather than the
            // hand's -- so it is a note, and only a debug build carries it.
            diag::note!(
                host,
                ctx.def_id,
                "key",
                "{}: widget {id} did not take it",
                verb.name()
            );
        }
        out
    }

    /// Element `id`, placed at `at`, asked to perform `verb`; what it reports
    /// is delivered as a drag's is, and the window repaints whether or not
    /// anything was.
    fn verb_to(
        &self,
        host: &mut Host,
        ctx: &GestureCtx,
        verb: Verb,
        id: i32,
        at: element::At,
        clipboard: &mut Clip,
    ) -> Option<Vec<GestureEffect>> {
        let mut input = KeyInput {
            mods: Mods {
                shift: ctx.shift,
                ctrl: ctx.ctrl,
                alt: ctx.alt,
            },
            clipboard,
            cursor: cursor_of(host, ctx, id),
        };
        let events = element::with(host, ctx, at, |el, _| el.verb(verb, &mut input)).flatten()?;
        let mut out = Vec::new();
        element::report(host, &mut out, ctx, id, events);
        // A content edit moves the extent the shared axis spans.
        host.sync_track_totals_keeping_view();
        out.push(GestureEffect::Redraw(ctx.def_id));
        Some(out)
    }

    /// The verbs the **window** performs, when no element took them: the
    /// transport, the loop, the ends of what is under the pointer, the
    /// clipboard over a view, and resetting every view.
    fn window_verb(
        &mut self,
        host: &mut Host,
        ctx: &GestureCtx,
        verb: Verb,
        pointer: Option<(f64, f64)>,
        clipboard: &mut Clip,
    ) -> Option<Vec<GestureEffect>> {
        // An unknown pointer is off the window: a verb addressed by it then
        // means the window's one take, if it has exactly one.
        let (cx, cy) = pointer.unwrap_or((-1.0, -1.0));
        match verb {
            Verb::ViewAll => Some(self.reset_timelines(host, ctx)),
            // **A multitrack is the window's, not the pointer's.** Its readers
            // are resident and follow the transport, so with no take to play
            // the window is told, and whoever edits the multitrack answers:
            // this host's own editor, or a script's.
            Verb::Play => Some(self.play_key(host, ctx, cx, cy).unwrap_or_else(|| {
                let mut out = Vec::new();
                tell(host, &mut out, ctx.def_id, host.play_verb());
                out
            })),
            // The window is told the loop changed too, so whoever plays it
            // changes the pass in progress.
            Verb::Loop => {
                let mut out = self.loop_key(host, ctx);
                tell(host, &mut out, ctx.def_id, host.loop_verb());
                Some(out)
            }
            Verb::ToStart | Verb::ToEnd => self.ends_key(host, ctx, verb == Verb::ToEnd, cx, cy),
            Verb::Copy | Verb::Cut | Verb::Paste | Verb::Mix => {
                let clip = match verb {
                    Verb::Copy => ClipVerb::Copy,
                    Verb::Cut => ClipVerb::Cut,
                    Verb::Mix => ClipVerb::Mix,
                    _ => ClipVerb::Paste,
                };
                self.clipboard_key(host, ctx, clip, cx, cy, clipboard)
            }
            Verb::Keys => Some(super::popups::open_keys(host, ctx)),
            Verb::SelectAll => self.take_span_key(host, ctx, cx, cy, None),
            Verb::Delete => self.take_span_key(host, ctx, cx, cy, Some("delete")),
            Verb::Quantize | Verb::Split | Verb::Join => None,
        }
    }

    /// **The verbs a view of samples answers over its whole extent or its
    /// selection**: *select all* sweeps the whole take, and *delete* asks its
    /// owner to take the selected span out -- a cut that puts nothing on the
    /// clipboard, as a cut is a copy and a delete. Addressed as the clipboard's
    /// verbs are: the view under the pointer, else the window's last
    /// selection, else its one take. `tag` is what the owner is asked; `None`
    /// is the sweep.
    fn take_span_key(
        &self,
        host: &mut Host,
        ctx: &GestureCtx,
        cx: f64,
        cy: f64,
        tag: Option<&str>,
    ) -> Option<Vec<GestureEffect>> {
        let id = match hit(host, ctx, cx, cy).filter(|h| host.timeline_key(h.id).is_some()) {
            Some(Hit { id, .. }) => id,
            None => host
                .selection_addressee(ctx.def_id)
                .or_else(|| sole_take(host, ctx.def_id))?,
        };
        let frames = host.buffer_frames(ctx.def_id, id)?;
        let mut out = Vec::new();
        match tag {
            None => {
                super::nav::set_selection(host, &mut out, ctx.def_id, id, 0.0, frames as f64, None)
            }
            Some(tag) => {
                let key = host.timeline_key(id)?;
                let state = *host.timelines().state(key)?;
                let (start, len) = state.selection()?;
                if len <= 0.0 {
                    return None;
                }
                emit(
                    host,
                    &mut out,
                    ctx.def_id,
                    id,
                    vec![
                        OscType::String(tag.into()),
                        OscType::Double(start),
                        OscType::Double(len),
                    ],
                );
            }
        }
        Some(out)
    }

    /// **Plays the contents under the cursor, or stops what is playing** -- the
    /// editor's monitor, on the space bar.
    ///
    /// Addressed by the pointer for the same reason a copy is: a window may
    /// hold several takes, and what the hand is over is the one it means. It is
    /// the host's own action and not an intent -- sounding a take changes
    /// nothing, so there is nobody to report it to (a host driven by a script
    /// plays through that script's own transport).
    ///
    /// Returns `None` when there is nothing under the cursor to play, so the
    /// key falls through to whatever else the window does with it.
    ///
    /// **Where it starts and whether it repeats are read off the view**, not
    /// asked for: a selection plays as a loop over exactly the span it covers,
    /// and with no selection the take plays from the position cursor. The transport is
    /// what carries both -- a locate and a loop span -- so nothing here computes
    /// a time or keeps one in step.
    pub fn play_key(
        &self,
        host: &mut Host,
        ctx: &GestureCtx,
        cx: f64,
        cy: f64,
    ) -> Option<Vec<GestureEffect>> {
        // **Space is play/stop, and a stop goes back to the position cursor.**
        // A monitor already sounding is stopped and the transport is located
        // at the mark the reader put down -- not left wherever the pass ended
        // -- so the next press plays from the same place, as a multitrack's
        // does.
        //
        // The one thing still addressed by the pointer is *which* take to load:
        // space over a take the monitor is not holding plays that one instead,
        // so a window of several takes is driven by pointing at them. Over
        // nothing at all it is the transport that is meant, and there is one.
        // **A window whose owner plays it is the owner's**: an application
        // plays its take through its own playback, so the key is the window's
        // verb and the monitor stays out.
        if host.window_plays(ctx.def_id) {
            return None;
        }
        let over = hit(host, ctx, cx, cy).map(|Hit { id, .. }| id);
        if let Some(loaded) = host.monitor()
            && over.is_none_or(|id| id == loaded.widget)
        {
            let mark =
                position_cursor(host, loaded.widget).unwrap_or(start_of(host, loaded.widget).0);
            host.stop_playback();
            host.locate(mark);
            return Some(vec![GestureEffect::Redraw(ctx.def_id)]);
        }
        // **With no pointer, the window's one take.** A window that holds a
        // single view of samples means that one whether or not the pointer
        // has been over it -- the first press after a window opens has no
        // pointer at all, since it is unknown until it moves. A window of
        // several views (a multitrack, its ruler, its take panes) is still
        // addressed by pointing, and over nothing it is the window's own verb.
        let id = over.or_else(|| sole_take(host, ctx.def_id))?;
        let (_, span) = start_of(host, id);
        let frames = host.buffer_frames(ctx.def_id, id)?;
        // **Where it starts and how it ends are the crate's rule**, the one an
        // audio editor's application reads too: a selection plays its span, no
        // selection plays from the position cursor, and the loop switch (`L`)
        // decides whether it repeats or stops and goes back to the cursor.
        let (start, pass) = space(
            host.monitor_loops(),
            span,
            position_cursor(host, id),
            frames,
        );
        host.play_buffer(ctx.def_id, id, start, pass)
            .then(|| vec![GestureEffect::Redraw(ctx.def_id)])
    }

    /// **`L` switches the loop**, and the status bar says which way it went --
    /// the one place the state is shown, since the applications have no
    /// transport row. A take the monitor is playing follows it at once, as a
    /// pass read off the view the way the space bar reads one; the window's
    /// owner is told beside it (`Host::loop_verb`), by the front.
    pub fn loop_key(&self, host: &mut Host, ctx: &GestureCtx) -> Vec<GestureEffect> {
        let looping = host.toggle_monitor_loop();
        if let Some(loaded) = host.monitor()
            && loaded.rolling
            && let Some(frames) = host.buffer_frames(ctx.def_id, loaded.widget)
        {
            let (_, span) = start_of(host, loaded.widget);
            let (_, pass) = space(looping, span, position_cursor(host, loaded.widget), frames);
            host.repass_monitor(pass);
        }
        host.say(
            ctx.def_id,
            crate::host::status::Line {
                kind: crate::host::status::Kind::Did,
                widget: None,
                verb: "loop".into(),
                text: if looping { "loop on" } else { "loop off" }.into(),
            },
        );
        vec![GestureEffect::Redraw(ctx.def_id)]
    }

    /// **Home and End: the position cursor to the start or the end of what
    /// is under the pointer** -- the samples of a take, the notes of a roll,
    /// the regions of a multitrack -- the same placing a click on the ruler
    /// makes, so the owner is told where the mark went and a play that follows
    /// starts there.
    ///
    /// Over anything that is on no axis it is the window's one take, or else
    /// its first view on an axis, and `None` when there is neither, so the key
    /// falls through to whatever else the window does with it.
    pub fn ends_key(
        &self,
        host: &mut Host,
        ctx: &GestureCtx,
        to_end: bool,
        cx: f64,
        cy: f64,
    ) -> Option<Vec<GestureEffect>> {
        // Over something that draws no samples -- a meter beside the take --
        // the key still means the window's one take.
        let take = hit(host, ctx, cx, cy)
            .map(|Hit { id, .. }| id)
            .filter(|id| host.buffer_frames(ctx.def_id, *id).is_some())
            .or_else(|| sole_take(host, ctx.def_id));
        let (id, pos) = match take {
            Some(id) => {
                let frames = host.buffer_frames(ctx.def_id, id)?;
                // End is the take's last frame, the one a cursor can stand on.
                let end = frames.saturating_sub(1) as f64;
                (id, if to_end { end } else { 0.0 })
            }
            // **A view with no samples of its own** -- a roll, a multitrack --
            // goes to where its contents end: the last note's, the last
            // region's. The view under the pointer, or the window's first on
            // an axis when the pointer is over neither.
            None => {
                let id = hit(host, ctx, cx, cy)
                    .map(|Hit { id, .. }| id)
                    .filter(|id| host.timeline_key(*id).is_some())
                    .or_else(|| {
                        timeline_ids(host.window_def(ctx.def_id)?)
                            .into_iter()
                            .next()
                    })?;
                let end = host.timeline_content(host.timeline_key(id)?) as f64;
                (id, if to_end { end } else { 0.0 })
            }
        };
        let mut out = Vec::new();
        super::nav::locate_at(host, &mut out, ctx, id, pos);
        Some(out)
    }

    /// **Copy, cut and paste over the selection**, addressed to the view under
    /// the cursor -- the window's own verbs, reached only when nothing focused
    /// and nothing under the cursor performed them first (a field's Ctrl+C is
    /// still the field's).
    ///
    /// The three verbs split exactly where the host's authority does. A **copy**
    /// is a read, and the host may honestly do it: it takes the selected span
    /// out of the contents it has *mapped* and puts it on the clipboard. A
    /// source it cannot read -- a mapped pyramid is an overview, a live view has
    /// no addressable past -- **declines, visibly**, because putting silence on
    /// the clipboard is the one answer worse than saying no. A **cut** and a
    /// **paste** change data, which the host does not own, so they leave as
    /// intents and the owner answers with what the document now is.
    ///
    /// A paste carries the clipboard **with** it (`"paste" position kind json
    /// [blob...]`), rather than the owner keeping a clipboard of its own: the
    /// clipboard is the host's precisely so that a block copied in one window
    /// pastes in another, against a different owner or none.
    pub fn clipboard_key(
        &self,
        host: &mut Host,
        ctx: &GestureCtx,
        verb: ClipVerb,
        cx: f64,
        cy: f64,
        clip: &mut Clip,
    ) -> Option<Vec<GestureEffect>> {
        // The pointer names the addressee whenever it is over a view; when it
        // is over the window's margin -- or off the window, which is where a
        // sweep to the first or last sample leaves it -- the window's most
        // recent selection does (`Host::selection_addressee`).
        let id = match hit(host, ctx, cx, cy).filter(|h| host.timeline_key(h.id).is_some()) {
            Some(Hit { id, .. }) => id,
            None => host.selection_addressee(ctx.def_id)?,
        };
        let key = host.timeline_key(id)?;
        let state = *host.timelines().state(key)?;
        let (start, len) = state.selection().unzip();
        // **Where a paste lands is the cursor, span or no span**: a click
        // leaves a selection of zero length whose start is the cursor, which
        // `selection()` -- the spans only -- does not answer, and a paste read
        // that way always landed at frame 0. The same reading `play_key` makes.
        let cursor = state.sel_start.max(0.0);
        let mut out = Vec::new();
        match verb {
            ClipVerb::Copy => {
                let (start, len) = (start?, len?);
                copy_selection(host, ctx, id, start, len, clip, &mut out, "copy")?;
            }
            // **A cut is a copy and a removal**, and the copy half is the
            // host's to make, as a copy is: the block goes on the clipboard
            // first, and only a cut whose block is on it asks the owner to take
            // the span out. One the host cannot read declines, like a copy --
            // taking something out that nothing holds any more would be the
            // one cut worse than none.
            ClipVerb::Cut => {
                let (start, len) = (start?, len?);
                if !copy_selection(host, ctx, id, start, len, clip, &mut out, "cut")? {
                    out.push(GestureEffect::Redraw(ctx.def_id));
                    return Some(out);
                }
                emit(
                    host,
                    &mut out,
                    ctx.def_id,
                    id,
                    vec![
                        OscType::String("cut".into()),
                        OscType::Double(start),
                        OscType::Double(len),
                    ],
                );
            }
            ClipVerb::Paste | ClipVerb::Mix => {
                let tag = if verb == ClipVerb::Mix {
                    "mix"
                } else {
                    "paste"
                };
                let doc = clip.doc()?;
                if !clip.is_whole() {
                    // A header whose payload did not travel: declining is the
                    // whole reason `blobs()` is on the clipboard at all.
                    emit(
                        host,
                        &mut out,
                        ctx.def_id,
                        id,
                        refusal(tag, "the clipboard's payload did not travel with it"),
                    );
                    return Some(out);
                }
                let mut args = vec![
                    OscType::String(tag.into()),
                    // Where: the selection's start, which is where a locate or a
                    // sweep last put the axis -- a paste has no pointer of its
                    // own, and the cursor is what the reader was looking at.
                    OscType::Double(cursor),
                    OscType::String(doc.kind().into()),
                    OscType::String(doc.to_json()),
                ];
                for i in 0..doc.blobs() {
                    if let Some(bytes) = clip.blob_bytes(i) {
                        args.push(OscType::Blob(bytes));
                    }
                }
                emit(host, &mut out, ctx.def_id, id, args);
            }
        }
        out.push(GestureEffect::Redraw(ctx.def_id));
        Some(out)
    }

    /// `R` over a window: reset every navigable view's axes -- a timeline's
    /// navigation (the whole group, linked members in other windows too) and
    /// its vertical window, and a navigable spectrum's frequency window. The
    /// views are found by walking the window's tree, so no front slot list is
    /// needed.
    pub fn reset_timelines(&mut self, host: &mut Host, ctx: &GestureCtx) -> Vec<GestureEffect> {
        let mut out = Vec::new();
        let def_id = ctx.def_id;
        let ids = host
            .window_def(def_id)
            .map(timeline_ids)
            .unwrap_or_default();
        for id in ids {
            // The whole group resets (linked members in other windows too).
            let roots = host.reset_timeline(id);
            redraw_all(&mut out, &roots);
            emit_view(host, &mut out, def_id, id);
            // The reset also restores the full vertical axis (and reports it).
            set_y_view(host, &mut out, def_id, id, 0.0, 1.0);
        }
        // A spectrum is in no group, so its frequency window resets on its own
        // -- the same key, since to a reader it is the same "show me all of it".
        let spectra = host
            .window_def(def_id)
            .map(freq_nav_ids)
            .unwrap_or_default();
        for id in spectra {
            set_x_view(host, &mut out, def_id, id, 0.0, 1.0, ctx.sample_rate);
        }
        out.push(GestureEffect::Redraw(def_id));
        out
    }
}

/// Which of the clipboard verbs a key asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipVerb {
    Copy,
    Cut,
    Paste,
    /// A paste that **adds** the block onto what is there rather than putting
    /// it in -- the same payload, answered by the owner as a mix.
    Mix,
}

/// **The selected span onto the clipboard**, or the refusal said to the owner
/// under `verb` when the host cannot read it. Answers whether it copied.
#[allow(clippy::too_many_arguments)] // the gesture's own context, spelled out
fn copy_selection(
    host: &mut Host,
    ctx: &GestureCtx,
    id: i32,
    start: f64,
    len: f64,
    clip: &mut Clip,
    out: &mut Vec<GestureEffect>,
    verb: &str,
) -> Option<bool> {
    let offset = host.widget_kind(ctx.def_id, id)?.editor()?.offset;
    // The selection is in **timeline** samples and an element reads its own
    // frames: a clip placed late holds sample 0 at its offset, which is the
    // one conversion between the axis and the contents on it.
    let from = (start - offset).max(0.0) as u64;
    match element_block(host, ctx, id, from, len as u64) {
        Some(block) => {
            clip.put_samples(block.samples.into(), block.channels, block.sample_rate);
            Some(true)
        }
        // Said out loud, in the one direction the host has: the owner learns
        // the reader could not read, which is what a refusal is for.
        None => {
            emit(
                host,
                out,
                ctx.def_id,
                id,
                refusal(verb, "this source has no samples the host can read"),
            );
            Some(false)
        }
    }
}

/// The contents behind widget `id` over `frames` of its own frames from
/// `start` -- the element's own answer ([`crate::host::widget::element::Samples::sample_block`]), since only
/// it knows what it holds and whether it may be read.
fn element_block(
    host: &mut Host,
    ctx: &GestureCtx,
    id: i32,
    start: u64,
    frames: u64,
) -> Option<SampleBlock> {
    host.widget_kind(ctx.def_id, id)?
        .as_element()?
        .samples()?
        .sample_block(start, frames, ctx.sample_rate)
}

/// The window's **only** timeline view, when there is exactly one and it
/// draws samples -- what a key with no pointer is addressed to.
fn sole_take(host: &Host, def_id: i32) -> Option<i32> {
    let views = timeline_ids(host.window_def(def_id)?);
    match views.as_slice() {
        [id] if host.buffer_frames(def_id, *id).is_some() => Some(*id),
        _ => None,
    }
}

/// The position cursor of view `id`, when one is placed.
fn position_cursor(host: &Host, id: i32) -> Option<u64> {
    let key = host.timeline_key(id)?;
    let cursor = host.timelines().state(key)?.cursor()?;
    Some(cursor.max(0.0) as u64)
}

/// **Where a play over view `id` starts, and the span it loops** -- read off
/// the view, never asked for. A selection plays as a loop over exactly the
/// span it covers, from its start; with none it plays from the position
/// cursor, and from frame 0 where no cursor has been placed.
fn start_of(host: &Host, id: i32) -> (u64, Option<(u64, u64)>) {
    let state = host
        .timeline_key(id)
        .and_then(|key| host.timelines().state(key))
        .copied();
    let span = state
        .and_then(|s| s.selection())
        .map(|(from, len)| (from.max(0.0) as u64, (from + len).max(0.0) as u64));
    let start = match span {
        Some((from, _)) => from,
        None => state.and_then(|s| s.cursor()).unwrap_or(0.0).max(0.0) as u64,
    };
    (start, span)
}

/// Runs `f` with the host's clipboard taken off it for the length of the call
/// and put back after -- an element edits while it borrows the host, and it
/// is handed the clipboard beside it.
pub(super) fn with_clipboard<R>(host: &mut Host, f: impl FnOnce(&mut Host, &mut Clip) -> R) -> R {
    let mut clip = std::mem::take(&mut host.clipboard);
    let out = f(host, &mut clip);
    host.clipboard = clip;
    out
}
