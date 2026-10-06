//! The gestures of the **popup layer**: what a press, a pointer motion, the
//! wheel and a key do while something is open over the window, and how a menu
//! is opened in the first place.
//!
//! A popup is modal, so this phase runs **before** the tree in every one of
//! them: an open list takes the press (a row picks, anywhere else closes), the
//! wheel (it scrolls), the walking keys, and the pointer's motion (which is
//! what lights the row under it and opens a submenu). Before the popup layer
//! existed only the press was routed, to an element that held its own list --
//! so the wheel reached whatever lay under an open list, no key closed it and
//! a static window never repainted the row under the pointer.
//!
//! It is also where a **menu** is opened, since a menu has no element: a title
//! of the window's bar, a button that carries a `menu`, and the request for a
//! context menu -- the secondary button, or a press held still.

use clausters_core::osc::OscType;

use super::super::Host;
use super::super::layout::Rect;
use super::super::menu::{self, Entry};
use super::super::menubar::{self, Title};
use super::super::popup::{self, Anchor, Keyed, Owner, Stack, Walk};
use super::super::widget::element::{Key, KeyInput, Mods};
use super::effects::emit;
use super::{GestureCtx, GestureEffect, Gestures, element};

/// The tag a picked entry is reported under: `/gui_event <owner> "menu" <verb>`
/// and, for an entry that holds a state, the new state after it.
pub const MENU: &str = "menu";

/// The walking key a host key is, or `None` for one a list does not answer.
fn walk(key: &Key) -> Option<Walk> {
    Some(match key {
        Key::Up => Walk::Up,
        Key::Down => Walk::Down,
        Key::Left => Walk::Left,
        Key::Right => Walk::Right,
        Key::Home => Walk::Home,
        Key::End => Walk::End,
        Key::Enter | Key::Char(' ') => Walk::Enter,
        Key::Escape => Walk::Escape,
        _ => return None,
    })
}

/// The placement of widget `id` in this window, as an address the machine can
/// hand an element.
fn at_widget(host: &Host, ctx: &GestureCtx, id: i32) -> Option<element::At> {
    host.layout_window(ctx.def_id, ctx.fb_w, ctx.fb_h)?
        .iter()
        .find(|p| p.widget.id == Some(id))
        .map(|p| element::At::widget(id, p.rect, p.scale, p.indent))
}

/// The menu bar's titles as they stand in this window.
fn bar_titles(host: &Host, ctx: &GestureCtx) -> Vec<(Title, Rect)> {
    let Some(tree) = host.window_def(ctx.def_id) else {
        return Vec::new();
    };
    let (Some(entries), Some(band)) = (
        menubar::entries(tree),
        host.menu_bar_rect(ctx.def_id, ctx.fb_w, ctx.fb_h),
    ) else {
        return Vec::new();
    };
    menubar::titles(entries, band, host.metrics_for(ctx.def_id))
}

/// Opens the list of bar title `title` -- or, for a title that is a plain
/// entry, picks it. `walked` puts the hover on the first row, which is what a
/// list opened from the keyboard wants.
fn open_title(
    host: &mut Host,
    ctx: &GestureCtx,
    out: &mut Vec<GestureEffect>,
    title: Title,
    rect: Rect,
    walked: bool,
) {
    let Some(entries) = host
        .window_def(ctx.def_id)
        .and_then(menubar::entries)
        .map(<[Entry]>::to_vec)
    else {
        return;
    };
    if !menubar::live(&entries, title) {
        return;
    }
    match menubar::list(&entries, title) {
        Some(list) => {
            let size = host.metrics_for(ctx.def_id).text_scale;
            let mut stack = Stack::new(Owner::Bar(title), list, Anchor::Below(rect), size);
            if walked {
                stack.key(Walk::Down);
            }
            host.open_popup(ctx.def_id, stack);
        }
        // A title with no list of its own is an entry like any other.
        None => {
            host.close_popup(ctx.def_id);
            report_pick(
                host,
                ctx,
                out,
                Owner::Bar(title),
                &menubar::path(title, &[]),
            );
        }
    }
    out.push(GestureEffect::Redraw(ctx.def_id));
}

/// Whether bar title `title` opens a list of its own, as against being a plain
/// entry a press picks.
fn has_list(host: &Host, ctx: &GestureCtx, title: Title) -> bool {
    host.window_def(ctx.def_id)
        .and_then(menubar::entries)
        .is_some_and(|entries| menubar::list(entries, title).is_some())
}

/// **Reports a pick in a menu**: writes the state the entry holds into the
/// tree it came from, and emits `"menu" <verb> [<state>]` from the widget that
/// carries the menu -- the window itself, for its bar.
fn report_pick(
    host: &mut Host,
    ctx: &GestureCtx,
    out: &mut Vec<GestureEffect>,
    owner: Owner,
    path: &[usize],
) {
    let def_id = ctx.def_id;
    let Some(tree) = host.window_def_mut(def_id) else {
        return;
    };
    let (widget_id, picked) = match owner {
        Owner::Bar(_) => (def_id, tree.menu.as_mut().and_then(|m| menu::pick(m, path))),
        Owner::Button(id) => (
            id,
            tree.find_mut(id)
                .and_then(|w| w.menu.as_mut())
                .and_then(|m| menu::pick(m, path)),
        ),
        Owner::Context(id) => (
            id,
            tree.find_mut(id)
                .and_then(|w| w.context.as_mut())
                .and_then(|m| menu::pick(m, path)),
        ),
        Owner::Element(_) | Owner::Edit(_) | Owner::Keys => return,
    };
    let Some(picked) = picked else {
        return;
    };
    let mut args = vec![OscType::String(MENU.into()), OscType::String(picked.verb)];
    if let Some(state) = picked.state {
        args.push(OscType::Int(i32::from(state)));
    }
    emit(host, out, def_id, widget_id, args);
}

/// **A key bound to a verb the window's bar names is that entry's pick**: it
/// reports exactly what a pick would, a check flipping included. `Some(true)`
/// when it picked, `Some(false)` for an entry that cannot be picked (the key
/// then does nothing, as a disabled entry does), and `None` when the bar names
/// no such verb.
pub(super) fn pick_verb(
    host: &mut Host,
    ctx: &GestureCtx,
    out: &mut Vec<GestureEffect>,
    verb: &str,
) -> Option<bool> {
    let entries = host.window_def(ctx.def_id).and_then(menubar::entries)?;
    let (path, live) = menu::find_verb(entries, verb)?;
    if live {
        let owner = Owner::Bar(Title::Entry(path[0]));
        report_pick(host, ctx, out, owner, &path);
        out.push(GestureEffect::Redraw(ctx.def_id));
    }
    Some(live)
}

/// A row was picked: the stack closes and whoever owns it answers -- the
/// element that opened the list, or the host for a menu.
fn pick(host: &mut Host, ctx: &GestureCtx, out: &mut Vec<GestureEffect>, path: Vec<usize>) {
    let Some(owner) = host.popup(ctx.def_id).map(|s| s.owner.clone()) else {
        return;
    };
    // The verb, read before the list closes: an edit menu's pick is performed
    // by its name.
    let verb = host.popup(ctx.def_id).and_then(|s| {
        let (last, parents) = path.split_last()?;
        menu::list_at(&s.levels[0].entries, parents)?
            .get(*last)?
            .verb
            .clone()
    });
    host.close_popup(ctx.def_id);
    out.push(GestureEffect::Redraw(ctx.def_id));
    match owner {
        Owner::Element(id) => {
            let Some(at) = at_widget(host, ctx, id) else {
                return;
            };
            if let Some(events) = element::with(host, ctx, at, |el, input| el.picked(&path, input))
            {
                element::report(host, out, ctx, id, events);
            }
        }
        Owner::Bar(title) => {
            let full = menubar::path(title, &path);
            report_pick(host, ctx, out, owner, &full);
        }
        Owner::Button(_) | Owner::Context(_) => report_pick(host, ctx, out, owner, &path),
        Owner::Edit(id) => {
            if let Some(verb) = verb {
                edit_pick(host, ctx, out, id, &verb);
            }
        }
        // a sheet is read, and nothing on it is picked
        Owner::Keys => {}
    }
}

/// **Opens the window's key sheet** (the `keys` verb): what each key does in
/// it, a section per scope in force and one for the table's own rows, in the
/// middle of the window ([`popup::Owner::Keys`]).
pub(super) fn open_keys(host: &mut Host, ctx: &GestureCtx) -> Vec<GestureEffect> {
    let scopes = host.window_keys(ctx.def_id);
    let mut entries = Vec::new();
    for section in host.keys.sheet(&scopes) {
        if !entries.is_empty() {
            entries.push(Entry::separator());
        }
        entries.push(Entry::heading(&section.title));
        entries.extend(
            section
                .rows
                .iter()
                .map(|(what, keys)| Entry::note(what, keys)),
        );
    }
    let size = host.metrics_for(ctx.def_id).text_scale;
    host.open_popup(
        ctx.def_id,
        Stack::new(Owner::Keys, entries, Anchor::Centre, size),
    );
    vec![GestureEffect::Redraw(ctx.def_id)]
}

/// **A pick in a field's edit menu is the edit itself**, done the way its key
/// does it -- Cut is Ctrl+X on the field, Delete is Delete -- so the menu and
/// the keyboard cannot disagree about what a cut is.
fn edit_pick(host: &mut Host, ctx: &GestureCtx, out: &mut Vec<GestureEffect>, id: i32, verb: &str) {
    let (key, ctrl) = match verb {
        "cut" => (Key::Char('x'), true),
        "copy" => (Key::Char('c'), true),
        "paste" => (Key::Char('v'), true),
        "delete" => (Key::Delete, false),
        "select_all" => (Key::Char('a'), true),
        _ => return,
    };
    let Some(at) = at_widget(host, ctx, id) else {
        return;
    };
    super::keys::with_clipboard(host, |host, clipboard| {
        let mut input = KeyInput {
            mods: Mods {
                ctrl,
                ..Mods::default()
            },
            clipboard,
            cursor: None,
        };
        let Some((events, selected)) = element::with(host, ctx, at, |el, _| {
            (el.key(&key, &mut input), el.selected_text())
        }) else {
            return;
        };
        if let Some(text) = selected {
            input.clipboard.set_primary(&text);
        }
        if let Some(events) = events {
            element::report(host, out, ctx, id, events);
        }
    });
    out.push(GestureEffect::Redraw(ctx.def_id));
}

impl Gestures {
    /// A press while a stack is open, or on the menu bar. Returns whether it
    /// was the popup layer's -- in which case the tree never sees it.
    ///
    /// **A list is modal**: a row picks, a row with a list of its own opens
    /// it, and a press anywhere else closes the stack and is swallowed, so the
    /// click that dismisses a menu does not also land on what was under it.
    /// The one exception is the bar itself, where a press on another title is
    /// a move to that title rather than a dismissal.
    pub(super) fn popup_press(
        &mut self,
        host: &mut Host,
        ctx: &GestureCtx,
        cx: f64,
        cy: f64,
        out: &mut Vec<GestureEffect>,
    ) -> bool {
        let def_id = ctx.def_id;
        let title = bar_titles(host, ctx)
            .into_iter()
            .find(|(_, r)| r.contains(cx, cy));
        let open = host.popup(def_id).map(|s| s.owner.clone());
        if let Some(open) = open {
            out.push(GestureEffect::Redraw(def_id));
            let placed = host
                .popup_placed(def_id, ctx.fb_w, ctx.fb_h)
                .unwrap_or_default();
            // **The key sheet goes by its close mark alone**: a press
            // anywhere else is swallowed, on it or off it.
            if open == Owner::Keys {
                if placed
                    .first()
                    .and_then(popup::Placed::close)
                    .is_some_and(|mark| mark.contains(cx, cy))
                {
                    host.close_popup(def_id);
                }
                return true;
            }
            let target = host
                .popup(def_id)
                .and_then(|stack| popup::hit(stack, &placed, cx, cy));
            match target {
                Some((level, Some(row))) => {
                    let stack = host.popup_mut(def_id).expect("the stack is open");
                    if stack.levels[level].entries[row].submenu().is_some() {
                        stack.point(level, Some(row));
                    } else {
                        let path = stack.path_of(level, row);
                        pick(host, ctx, out, path);
                    }
                }
                // On a list, on nothing that can be picked: it stays.
                Some((_, None)) => {}
                None => {
                    host.close_popup(def_id);
                    // Another title of the bar is a move, not a dismissal; the
                    // title already open closes and that is all.
                    if let Some((title, rect)) = title
                        && open != Owner::Bar(title)
                    {
                        open_title(host, ctx, out, title, rect, false);
                    }
                }
            }
            return true;
        }
        if let Some((title, rect)) = title {
            open_title(host, ctx, out, title, rect, false);
            return true;
        }
        // The bar's own band, off every title: chrome, and nothing under it.
        host.menu_bar_rect(def_id, ctx.fb_w, ctx.fb_h)
            .is_some_and(|band| band.contains(cx, cy))
    }

    /// **The pointer moved with no button held.** The popup layer's half of
    /// it: the row under the pointer lights up and a submenu's row opens its
    /// list, and with a bar's list open, the pointer crossing another title
    /// moves the list there -- the way a menu bar is read.
    ///
    /// Returns `None` when nothing is open and the pointer is not on the bar,
    /// which is where the tree's own hover takes over.
    pub(super) fn popup_motion(
        &mut self,
        host: &mut Host,
        ctx: &GestureCtx,
        cx: f64,
        cy: f64,
    ) -> Option<Vec<GestureEffect>> {
        let def_id = ctx.def_id;
        let mut out = Vec::new();
        let title = bar_titles(host, ctx)
            .into_iter()
            .find(|(_, r)| r.contains(cx, cy));
        let Some(owner) = host.popup(def_id).map(|s| s.owner.clone()) else {
            if host.set_bar_hover(def_id, title.map(|(t, _)| t)) {
                out.push(GestureEffect::Redraw(def_id));
            }
            return title.is_some().then_some(out);
        };
        let placed = host
            .popup_placed(def_id, ctx.fb_w, ctx.fb_h)
            .unwrap_or_default();
        let target = host
            .popup(def_id)
            .and_then(|stack| popup::hit(stack, &placed, cx, cy));
        match target {
            Some((level, row)) => {
                let stack = host.popup_mut(def_id).expect("the stack is open");
                if stack.point(level, row) {
                    out.push(GestureEffect::Redraw(def_id));
                }
            }
            None => {
                if let (Owner::Bar(open), Some((title, rect))) = (owner, title)
                    && open != title
                {
                    // Crossing a title moves the open list there; a title that
                    // is a plain entry has none, so the list closes and the
                    // title only lights up. Passing over an action is never
                    // taking it -- that is the press's.
                    if has_list(host, ctx, title) {
                        open_title(host, ctx, &mut out, title, rect, false);
                    } else {
                        host.close_popup(def_id);
                        host.set_bar_hover(def_id, Some(title));
                        out.push(GestureEffect::Redraw(def_id));
                    }
                }
            }
        }
        Some(out)
    }

    /// **The pointer moved with no button held**: the front's one call for
    /// it, on both fronts. The popup layer answers first; with nothing open,
    /// the tree's own hover does ([`Self::hover`]).
    pub fn motion(
        &mut self,
        host: &mut Host,
        ctx: &GestureCtx,
        cx: f64,
        cy: f64,
    ) -> Vec<GestureEffect> {
        // Behind a dialog nothing lights up: it cannot be reached.
        if host.popup(ctx.def_id).is_none() && self.behind_dialog(host, ctx, cx, cy) {
            return self.hover(host, ctx, None);
        }
        if let Some(out) = self.popup_motion(host, ctx, cx, cy) {
            return out;
        }
        self.hover(host, ctx, Some((cx, cy)))
    }

    /// The wheel over an open stack: the list under the pointer scrolls (the
    /// deepest one, off them all), a notch a row. `None` when nothing is open.
    pub(super) fn popup_wheel(
        &mut self,
        host: &mut Host,
        ctx: &GestureCtx,
        cx: f64,
        cy: f64,
        steps: f64,
    ) -> Option<Vec<GestureEffect>> {
        let def_id = ctx.def_id;
        host.popup(def_id)?;
        let placed = host
            .popup_placed(def_id, ctx.fb_w, ctx.fb_h)
            .unwrap_or_default();
        let row = popup::row_h(host.popup(def_id)?.text_size, host.metrics_for(def_id));
        let stack = host.popup_mut(def_id)?;
        let level =
            popup::hit(stack, &placed, cx, cy).map_or(stack.levels.len() - 1, |(level, _)| level);
        let Some(at) = placed.get(level) else {
            return Some(Vec::new());
        };
        let to = (at.scroll - steps as f32 * row).clamp(0.0, at.max_scroll);
        if (to - at.scroll).abs() <= f32::EPSILON {
            return Some(Vec::new());
        }
        stack.levels[level].scroll = to;
        stack.levels[level].reveal = false;
        // The rows moved under a still pointer, so the hover is read again
        // against where they are now.
        let placed = host
            .popup_placed(def_id, ctx.fb_w, ctx.fb_h)
            .unwrap_or_default();
        let stack = host.popup_mut(def_id)?;
        if let Some((level, row)) = popup::hit(stack, &placed, cx, cy) {
            stack.point(level, row);
        }
        Some(vec![GestureEffect::Redraw(def_id)])
    }

    /// A key while a stack is open: the walking keys walk it, and every other
    /// key is swallowed -- a list is modal. `None` when nothing is open.
    pub(super) fn popup_key(
        &self,
        host: &mut Host,
        ctx: &GestureCtx,
        key: &Key,
    ) -> Option<Vec<GestureEffect>> {
        let def_id = ctx.def_id;
        let row = popup::row_h(host.popup(def_id)?.text_size, host.metrics_for(def_id));
        let most = host
            .popup_placed(def_id, ctx.fb_w, ctx.fb_h)
            .and_then(|placed| placed.first().map(|p| p.max_scroll))
            .unwrap_or(0.0);
        let stack = host.popup_mut(def_id)?;
        let mut out = vec![GestureEffect::Redraw(def_id)];
        let Some(key) = walk(key) else {
            return Some(Vec::new());
        };
        // **The key sheet has no row to land on**, so the walking keys scroll
        // it -- a row at a time, or to an end -- and the layout clamps.
        if stack.owner == Owner::Keys && key != Walk::Escape {
            let level = &mut stack.levels[0];
            level.reveal = false;
            level.scroll = match key {
                Walk::Up => level.scroll - row,
                Walk::Down => level.scroll + row,
                Walk::Home => 0.0,
                Walk::End => most,
                _ => return Some(Vec::new()),
            }
            .clamp(0.0, most);
            return Some(out);
        }
        match stack.key(key) {
            Keyed::Moved | Keyed::Ignored => {}
            Keyed::Closed => {
                host.close_popup(def_id);
            }
            Keyed::Pick(path) => pick(host, ctx, &mut out, path),
            Keyed::Step(dir) => {
                let Owner::Bar(open) = stack.owner else {
                    return Some(out);
                };
                let titles = bar_titles(host, ctx);
                let n = titles.len() as i32;
                if let Some(i) = titles.iter().position(|(t, _)| *t == open) {
                    let (title, rect) = titles[(i as i32 + dir).rem_euclid(n) as usize];
                    host.close_popup(def_id);
                    open_title(host, ctx, &mut out, title, rect, true);
                }
            }
        }
        Some(out)
    }

    /// **The request for a context menu**, at `(cx, cy)`: the secondary
    /// button's press, or a press held still. The widget under the point
    /// answers with its `context`, or the nearest ancestor that carries one.
    ///
    /// Returns the effects when a menu opened, and `None` where nothing under
    /// the point carries one -- so a front can let the platform's own menu
    /// through there.
    pub fn context(
        &mut self,
        host: &mut Host,
        ctx: &GestureCtx,
        cx: f64,
        cy: f64,
    ) -> Option<Vec<GestureEffect>> {
        let def_id = ctx.def_id;
        // One pointer, one gesture: with a drag in hand the other button is
        // not a new request.
        if self.dragging() {
            return None;
        }
        // A list already open is dismissed by any press, this one included.
        let closed = host.close_popup(def_id);
        let found = host.context_at(def_id, ctx.fb_w, ctx.fb_h, cx, cy);
        let Some((owner, entries)) = found else {
            return closed.then(|| vec![GestureEffect::Redraw(def_id)]);
        };
        let mut out = Vec::new();
        // A field's edit menu edits that field, so the field takes the focus
        // its menu acts on -- the caret and the selection show while it is up.
        if let Owner::Edit(id) = owner {
            super::focus::set(host, &mut out, ctx, Some(id));
        }
        let size = host.metrics_for(def_id).text_scale;
        host.open_popup(
            def_id,
            Stack::new(owner, entries, Anchor::At(cx as f32, cy as f32), size),
        );
        out.push(GestureEffect::Redraw(def_id));
        Some(out)
    }

    /// Opens the `menu` of button `id` under `rect`, when it carries one.
    /// Answers whether it did -- the button's own press is then not delivered.
    pub(super) fn button_menu(
        &mut self,
        host: &mut Host,
        ctx: &GestureCtx,
        id: i32,
        rect: Rect,
        out: &mut Vec<GestureEffect>,
    ) -> bool {
        let Some(entries) = host
            .window_def(ctx.def_id)
            .and_then(|t| t.find(id))
            .filter(|w| w.live)
            .and_then(|w| w.menu.clone())
            .filter(|m| !m.is_empty())
        else {
            return false;
        };
        let size = host.metrics_for(ctx.def_id).text_scale;
        let mut stack = Stack::new(Owner::Button(id), entries, Anchor::Below(rect), size);
        stack.origin = Some((rect.x, rect.y));
        host.open_popup(ctx.def_id, stack);
        out.push(GestureEffect::Redraw(ctx.def_id));
        true
    }
}
