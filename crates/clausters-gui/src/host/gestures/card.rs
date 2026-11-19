//! **The hover card**: a note the pointer may enter
//! ([`Note::card`]), a code editor's help on the name the pointer
//! rests on. It stays while the pointer is on the name or on it, and goes a
//! moment after the pointer leaves both, on Escape, on a press elsewhere or
//! on a key that goes on to the text. Over it the wheel and its scroll bar
//! scroll its documentation and a drag selects its text, which the copy key
//! copies.
//!
//! The element only says what the note holds; its scroll and its selection
//! are the host's ([`Card`](super::super::popup::Card)), kept beside the window's other popups.

use super::super::Host;
use super::super::clipboard::Clip;
use super::super::popup::{Note, NotePlace};
use super::super::widget::element::{Key, Mods};
use super::{Drag, GestureCtx, GestureEffect, Gestures};

/// How long a hover card stays once the pointer left it and the name it
/// explains, in milliseconds: a pointer crossing the gap between the two
/// does not take it down.
pub(super) const LEAVE_MS: f64 = 300.0;

/// The lines a notch of the wheel scrolls a card's documentation by.
const LINES_A_NOTCH: f32 = 3.0;

/// The card over the window, when `owner`'s note is one.
fn shown(host: &Host, ctx: &GestureCtx, owner: i32) -> Option<(Note, NotePlace)> {
    host.card_place(ctx.def_id, ctx.fb_w, ctx.fb_h)
        .filter(|(id, ..)| *id == owner)
        .map(|(_, note, place)| (note, place))
}

/// **Whether the pointer at `at` keeps `owner`'s card up**: on the card or on
/// what it explains. `None` while no card of `owner`'s is shown.
pub(super) fn keeps(
    host: &Host,
    ctx: &GestureCtx,
    owner: i32,
    at: Option<(f64, f64)>,
) -> Option<bool> {
    let (note, place) = shown(host, ctx, owner)?;
    Some(at.is_some_and(|(x, y)| place.rect.contains(x, y) || note.anchor.contains(x, y)))
}

/// Whether `(x, y)` is on a hover card over the window.
pub(super) fn on_card(host: &Host, ctx: &GestureCtx, x: f64, y: f64) -> bool {
    host.card_place(ctx.def_id, ctx.fb_w, ctx.fb_h)
        .is_some_and(|(_, _, place)| place.rect.contains(x, y))
}

impl Gestures {
    /// **A press on the card**: a selection of its text starts there, and
    /// nothing under it hears the press. Returns whether it was on one.
    pub(super) fn card_press(
        &mut self,
        host: &mut Host,
        ctx: &GestureCtx,
        x: f64,
        y: f64,
        out: &mut Vec<GestureEffect>,
    ) -> bool {
        let Some((owner, note, place)) = host
            .card_place(ctx.def_id, ctx.fb_w, ctx.fb_h)
            .filter(|(_, _, place)| place.rect.contains(x, y))
        else {
            return false;
        };
        let m = *host.metrics_for(ctx.def_id);
        let card = host.card_mut(ctx.def_id, owner);
        let scroll = card.scroll.min(place.max_scroll());
        // its scroll bar's thumb in hand
        if let Some((grip, to)) = place
            .bar(scroll, &m)
            .and_then(|bar| bar.press((x, y), scroll))
        {
            card.scroll = to;
            self.drag = Some(Drag::Card {
                owner,
                thumb: Some(grip),
            });
            out.push(GestureEffect::Redraw(ctx.def_id));
            return true;
        }
        let at = note.pos_at(&place, scroll, x as f32, y as f32);
        card.select = Some((at, at));
        self.drag = Some(Drag::Card { owner, thumb: None });
        out.push(GestureEffect::Redraw(ctx.def_id));
        true
    }

    /// The selecting drag, or the scroll bar's thumb, reached `(x, y)`.
    pub(super) fn card_drag(
        &mut self,
        host: &mut Host,
        ctx: &GestureCtx,
        owner: i32,
        thumb: Option<crate::host::graphics::controls::ThumbGrip>,
        x: f64,
        y: f64,
    ) -> Vec<GestureEffect> {
        let Some((note, place)) = shown(host, ctx, owner) else {
            return Vec::new();
        };
        let m = *host.metrics_for(ctx.def_id);
        let card = host.card_mut(ctx.def_id, owner);
        let scroll = card.scroll.min(place.max_scroll());
        if let Some(grip) = thumb {
            let Some(bar) = place.bar(scroll, &m) else {
                return Vec::new();
            };
            card.scroll = bar.drag(grip, (x, y));
            return vec![GestureEffect::Redraw(ctx.def_id)];
        }
        let at = note.pos_at(&place, scroll, x as f32, y as f32);
        match card.select.as_mut() {
            Some((_, end)) if *end != at => *end = at,
            _ => return Vec::new(),
        }
        vec![GestureEffect::Redraw(ctx.def_id)]
    }

    /// The selecting drag let go: what it selected is the primary selection.
    pub(super) fn card_release(&mut self, host: &mut Host, ctx: &GestureCtx, owner: i32) {
        let Some((note, place)) = shown(host, ctx, owner) else {
            return;
        };
        let Some((a, b)) = host
            .card_mut(ctx.def_id, owner)
            .select
            .filter(|(a, b)| a != b)
        else {
            return;
        };
        let text = note.text_between(&place, a, b);
        host.clipboard.set_primary(&text);
    }

    /// **The wheel over the card** scrolls its documentation, a few lines a
    /// notch; nothing under it scrolls. `None` off every card.
    pub(super) fn card_wheel(
        &mut self,
        host: &mut Host,
        ctx: &GestureCtx,
        x: f64,
        y: f64,
        steps: f64,
    ) -> Option<Vec<GestureEffect>> {
        let (owner, _, place) = host
            .card_place(ctx.def_id, ctx.fb_w, ctx.fb_h)
            .filter(|(_, _, place)| place.rect.contains(x, y))?;
        let card = host.card_mut(ctx.def_id, owner);
        let from = card.scroll.min(place.max_scroll());
        let to =
            (from - steps as f32 * LINES_A_NOTCH * place.line_h).clamp(0.0, place.max_scroll());
        if (to - from).abs() <= f32::EPSILON {
            return Some(Vec::new());
        }
        card.scroll = to;
        Some(vec![GestureEffect::Redraw(ctx.def_id)])
    }

    /// **A key while a card is up**: the copy key copies what of it is
    /// selected, Escape takes it down, and any other key takes it down and
    /// goes on to whoever has the keys. `None` for a key that goes on.
    pub(super) fn card_key(
        &mut self,
        host: &mut Host,
        ctx: &GestureCtx,
        key: &Key,
        mods: Mods,
        clipboard: &mut Clip,
    ) -> Option<Vec<GestureEffect>> {
        let owner = self.rest_on.filter(|r| r.told)?.widget;
        let (note, place) = shown(host, ctx, owner)?;
        let scopes = host.window_keys(ctx.def_id);
        let copy = host.keys.lookup_in(key, mods, &scopes) == Some("copy");
        let selected = host
            .popups(ctx.def_id)
            .and_then(|p| p.card.as_ref())
            .filter(|c| c.owner == owner)
            .and_then(|c| c.select)
            .filter(|(a, b)| a != b);
        if copy && let Some((a, b)) = selected {
            clipboard.set_text(&note.text_between(&place, a, b));
            return Some(Vec::new());
        }
        let mut out = Vec::new();
        self.close_card(host, ctx, &mut out);
        (*key == Key::Escape).then_some(out)
    }

    /// The card goes: the element hears the pointer moved on, and the card's
    /// scroll and selection go with it.
    pub(super) fn close_card(
        &mut self,
        host: &mut Host,
        ctx: &GestureCtx,
        out: &mut Vec<GestureEffect>,
    ) {
        self.end_rest(host, ctx, out);
        if host.drop_card(ctx.def_id) {
            out.push(GestureEffect::Redraw(ctx.def_id));
        }
    }
}
