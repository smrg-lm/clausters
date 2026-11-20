//! **What the pointer is over, and how long it has been there**: the hover
//! every control draws through, the tip a widget shows once the pointer rests
//! on it, and the press held still that asks for a context menu.
//!
//! Both of the timed ones need a clock the agnostic core does not have, so the
//! front hands it over with every call ([`GestureCtx::now_ms`]) and the
//! **rules** stay here: how long is a rest, how long is a hold. A front is
//! told whether a timer is armed ([`Gestures::pending`]) so it keeps its frame
//! tick alive for exactly that long, and reports back on each tick
//! ([`Gestures::elapsed`]).

use super::super::Host;
use super::super::popup::Tip;
use super::super::widget::WidgetKind;
use super::nav::hit;
use super::{GestureCtx, GestureEffect, Gestures, card, element};

/// How long the pointer rests on a widget before its tip shows, in
/// milliseconds.
const REST_MS: f64 = 600.0;

/// How long the pointer rests on an element told of it before it is told,
/// in milliseconds: a code editor's help over a name, as the current
/// editor's.
const REST_ON_MS: f64 = 350.0;

/// **How long after a tip went another shows at once**, in milliseconds: a
/// pointer crossing the gap between two tools reads them as a row.
const SKIP_MS: f64 = 300.0;

/// How long a press is held still before it is a request for a context menu,
/// in milliseconds.
const HOLD_MS: f64 = 500.0;

/// A tip waiting for the pointer to have rested long enough.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Rest {
    tip: Tip,
    since: f64,
}

/// **The pointer over an element told where it rests**: where, since when,
/// and whether the element was told.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct RestOn {
    pub(super) widget: i32,
    at: (f64, f64),
    since: f64,
    pub(super) told: bool,
    /// Since when the pointer has been off the hover card the element
    /// shows and off what it explains ([`card`]).
    leaving: Option<f64>,
}

/// A press held still, waiting to become a context request.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Hold {
    at: (f64, f64),
    since: f64,
}

impl Gestures {
    /// The pointer is at `at` with no button held, or has left the window
    /// (`None`): the widget under it is the one that draws hovered, and a
    /// widget with a `tip` starts waiting to show it.
    ///
    /// A repaint is asked for only when the hovered widget **changes** -- a
    /// pointer crossing a button costs two frames, not one per pixel.
    pub fn hover(
        &mut self,
        host: &mut Host,
        ctx: &GestureCtx,
        at: Option<(f64, f64)>,
    ) -> Vec<GestureEffect> {
        let def_id = ctx.def_id;
        let mut out = Vec::new();
        // a hover card covers what is under it
        let at_tree = at.filter(|&(x, y)| !card::on_card(host, ctx, x, y));
        let over = at_tree.and_then(|(x, y)| hovered(host, ctx, x, y));
        let mut changed = host.set_hover(def_id, over);
        // a dialog's close button lights under the hand, as a ghost button
        let close = at_tree.and_then(|(x, y)| {
            let placed = host.layout_window(def_id, ctx.fb_w, ctx.fb_h)?;
            crate::host::chrome::close_at(&placed, x, y)
        });
        changed |= host.set_close_hover(def_id, close);
        self.rest_moved(host, ctx, at, &mut out);
        if at.is_none() {
            changed |= host.set_bar_hover(def_id, None);
        }
        // **No tip while a list is open**: the list is what is read.
        let tip = at_tree
            .filter(|_| host.popup(def_id).is_none())
            .and_then(|(x, y)| host.tip_at(def_id, ctx.fb_w, ctx.fb_h, x, y));
        let showing = host
            .popups(def_id)
            .and_then(|p| p.tip.as_ref())
            .map(|t| t.widget);
        let skipping = self.tip_gone.is_some_and(|t| ctx.now_ms - t < SKIP_MS);
        match (tip, showing) {
            // Off every widget that has one: nothing waits and nothing shows.
            (None, shown) => {
                self.rest = None;
                if shown.is_some() {
                    self.tip_gone = Some(ctx.now_ms);
                }
                changed |= host.set_tip(def_id, None);
            }
            // Still on the widget whose tip is up: it stays where it is.
            (Some(tip), Some(shown)) if tip.widget == shown => {}
            // From one tipped widget onto another while a tip is up, or just
            // after one went: the next one shows at once, which is how a row
            // of tools is read.
            (Some(tip), shown) if shown.is_some() || skipping => {
                self.rest = None;
                changed |= host.set_tip(def_id, Some(tip));
            }
            // Nothing showing: the wait starts over at every move, since a
            // pointer that is moving has not rested.
            (Some(tip), _) => {
                self.rest = Some(Rest {
                    tip,
                    since: ctx.now_ms,
                });
            }
        }
        if changed {
            out.push(GestureEffect::Redraw(def_id));
        }
        out
    }

    /// Whether a timer is armed, so the front must keep ticking
    /// ([`Self::elapsed`]) though nothing on screen is moving.
    pub fn pending(&self) -> bool {
        self.rest.is_some()
            || self.hold.is_some()
            || self.rest_on.is_some_and(|r| !r.told || r.leaving.is_some())
    }

    /// **The pointer moved to `at`** (or off the window): an element told of
    /// rests and told of one hears the pointer moved on, and one under the
    /// new place starts waiting.
    fn rest_moved(
        &mut self,
        host: &mut Host,
        ctx: &GestureCtx,
        at: Option<(f64, f64)>,
        out: &mut Vec<GestureEffect>,
    ) {
        if let (Some(r), Some(at)) = (self.rest_on, at)
            && r.at == at
        {
            return;
        }
        // **A hover card stays while the pointer is on it or on what it
        // explains**, and once it is off both it goes a moment later
        // (`elapsed`), so the pointer can cross into it.
        if let Some(r) = self.rest_on.as_mut().filter(|r| r.told)
            && let Some(keeps) = card::keeps(host, ctx, r.widget, at)
        {
            if keeps {
                r.leaving = None;
            } else if r.leaving.is_none() {
                r.leaving = Some(ctx.now_ms);
            }
            return;
        }
        self.end_rest(host, ctx, out);
        let Some((x, y)) = at else {
            return;
        };
        let Some(found) = hit(host, ctx, x, y) else {
            return;
        };
        if let WidgetKind::Custom(el) = &found.kind
            && el.takes_rests()
        {
            self.rest_on = Some(RestOn {
                widget: found.id,
                at: (x, y),
                since: ctx.now_ms,
                told: false,
                leaving: None,
            });
        }
    }

    /// The rest is over: an element told of it hears the pointer moved on,
    /// and the hover card it showed goes with its scroll and selection.
    pub(super) fn end_rest(
        &mut self,
        host: &mut Host,
        ctx: &GestureCtx,
        out: &mut Vec<GestureEffect>,
    ) {
        let Some(r) = self.rest_on.take().filter(|r| r.told) else {
            return;
        };
        self.tell_rest(host, ctx, r.widget, None, out);
        if host.drop_card(ctx.def_id) {
            out.push(GestureEffect::Redraw(ctx.def_id));
        }
    }

    /// Element `widget` told the pointer rests at `at`, or moved on.
    fn tell_rest(
        &mut self,
        host: &mut Host,
        ctx: &GestureCtx,
        widget: i32,
        at: Option<(f64, f64)>,
        out: &mut Vec<GestureEffect>,
    ) {
        let Some(placed) = host.layout_window(ctx.def_id, ctx.fb_w, ctx.fb_h) else {
            return;
        };
        let Some(spot) = placed
            .iter()
            .find(|p| p.widget.id == Some(widget))
            .map(|p| element::At::widget(widget, p.rect, p.scale, p.indent))
        else {
            return;
        };
        if let Some(Some(events)) = element::with(host, ctx, spot, |el, input| el.rest(at, input)) {
            element::report(host, out, ctx, widget, events);
            out.push(GestureEffect::Redraw(ctx.def_id));
        }
    }

    /// One tick of the front's clock: a rest long enough shows its tip, and a
    /// press held long enough becomes the request for a context menu.
    pub fn elapsed(&mut self, host: &mut Host, ctx: &GestureCtx) -> Vec<GestureEffect> {
        let mut out = Vec::new();
        if let Some(rest) = self.rest.take_if(|r| ctx.now_ms - r.since >= REST_MS)
            && host.set_tip(ctx.def_id, Some(rest.tip))
        {
            out.push(GestureEffect::Redraw(ctx.def_id));
        }
        if self
            .rest_on
            .is_some_and(|r| r.leaving.is_some_and(|t| ctx.now_ms - t >= card::LEAVE_MS))
        {
            self.close_card(host, ctx, &mut out);
        }
        if let Some(r) = self
            .rest_on
            .as_mut()
            .filter(|r| !r.told && ctx.now_ms - r.since >= REST_ON_MS)
        {
            r.told = true;
            let (widget, at) = (r.widget, r.at);
            self.tell_rest(host, ctx, widget, Some(at), &mut out);
        }
        if let Some(hold) = self.hold.take_if(|h| ctx.now_ms - h.since >= HOLD_MS) {
            out.extend(self.held(host, ctx, hold.at));
        }
        out
    }

    /// A press was held still at `at`: where a context menu answers, the press
    /// is let go of and the menu opens; where none does, the tip of what is
    /// under the finger shows instead -- touch has no resting pointer to show
    /// one by.
    fn held(&mut self, host: &mut Host, ctx: &GestureCtx, at: (f64, f64)) -> Vec<GestureEffect> {
        let def_id = ctx.def_id;
        if host
            .context_at(def_id, ctx.fb_w, ctx.fb_h, at.0, at.1)
            .is_some()
        {
            let mut out = self.abandon(host, ctx, at.0, at.1);
            out.extend(self.context(host, ctx, at.0, at.1).unwrap_or_default());
            return out;
        }
        let Some(tip) = host.tip_at(def_id, ctx.fb_w, ctx.fb_h, at.0, at.1) else {
            return Vec::new();
        };
        if host.set_tip(def_id, Some(tip)) {
            vec![GestureEffect::Redraw(def_id)]
        } else {
            Vec::new()
        }
    }

    /// A press landed: whatever was waiting or showing goes, and -- for a
    /// finger -- the hold starts.
    pub(super) fn press_began(
        &mut self,
        host: &mut Host,
        ctx: &GestureCtx,
        at: (f64, f64),
        out: &mut Vec<GestureEffect>,
    ) {
        self.rest = None;
        self.tip_gone = None;
        self.end_rest(host, ctx, out);
        if host.set_tip(ctx.def_id, None) {
            out.push(GestureEffect::Redraw(ctx.def_id));
        }
        self.hold = ctx.touch.then_some(Hold {
            at,
            since: ctx.now_ms,
        });
    }

    /// The held pointer moved to `at`: past the slop it is a drag, and a drag
    /// is not a hold.
    pub(super) fn press_moved(&mut self, slop: f64, at: (f64, f64)) {
        if self
            .hold
            .is_some_and(|h| (at.0 - h.at.0).abs() > slop || (at.1 - h.at.1).abs() > slop)
        {
            self.hold = None;
        }
    }

    /// The button came up: nothing is being held any more.
    pub(super) fn press_ended(&mut self) {
        self.hold = None;
    }

    /// **A key was pressed**: a tip goes, the way it goes on a press. The front
    /// calls it before it routes the key, since a key that nothing consumes
    /// still has to take the tip down. A hover card that is up is the key's
    /// to answer ([`card`]): the copy key copies from it.
    pub fn key_began(&mut self, host: &mut Host, ctx: &GestureCtx) -> Vec<GestureEffect> {
        self.rest = None;
        self.tip_gone = None;
        let mut out = Vec::new();
        let card_up = self
            .rest_on
            .is_some_and(|r| r.told && card::keeps(host, ctx, r.widget, None).is_some());
        if !card_up {
            self.end_rest(host, ctx, &mut out);
        }
        if host.set_tip(ctx.def_id, None) {
            out.push(GestureEffect::Redraw(ctx.def_id));
        }
        out
    }
}

/// The widget that draws hovered at `(x, y)`: an element a hand can act on,
/// with the point on the shape it declared -- the press's own question, asked
/// with no button down.
fn hovered(host: &mut Host, ctx: &GestureCtx, x: f64, y: f64) -> Option<i32> {
    let found = hit(host, ctx, x, y)?;
    if !matches!(found.kind, WidgetKind::Custom(_)) {
        return None;
    }
    let at = element::At::widget(found.id, found.rect, found.scale, found.indent);
    element::inside(host, ctx, at, x, y).then_some(found.id)
}
