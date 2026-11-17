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
use super::{GestureCtx, GestureEffect, Gestures, element};

/// How long the pointer rests on a widget before its tip shows, in
/// milliseconds.
const REST_MS: f64 = 600.0;

/// How long the pointer rests on an element told of it before it is told,
/// in milliseconds: a code editor's help over a name, as the current
/// editor's.
const REST_ON_MS: f64 = 350.0;

/// How long a press is held still before it is a request for a context menu,
/// in milliseconds.
const HOLD_MS: f64 = 500.0;

/// A tip waiting for the pointer to have rested long enough.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Rest {
    widget: i32,
    text: String,
    at: (f64, f64),
    since: f64,
}

/// **The pointer over an element told where it rests**: where, since when,
/// and whether the element was told.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct RestOn {
    widget: i32,
    at: (f64, f64),
    since: f64,
    told: bool,
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
        let over = at.and_then(|(x, y)| hovered(host, ctx, x, y));
        let mut changed = host.set_hover(def_id, over);
        self.rest_moved(host, ctx, at, &mut out);
        if at.is_none() {
            changed |= host.set_bar_hover(def_id, None);
        }
        let tip = at.and_then(|(x, y)| {
            host.tip_at(def_id, ctx.fb_w, ctx.fb_h, x, y)
                .map(|(widget, text)| (widget, text, (x, y)))
        });
        let showing = host
            .popups(def_id)
            .and_then(|p| p.tip.as_ref())
            .map(|t| t.widget);
        match (tip, showing) {
            // Off every widget that has one: nothing waits and nothing shows.
            (None, _) => {
                self.rest = None;
                changed |= host.set_tip(def_id, None);
            }
            // Still on the widget whose tip is up: it stays where it rested.
            (Some((widget, ..)), Some(shown)) if widget == shown => {}
            // From one tipped widget onto another while a tip is up: the next
            // one shows at once, which is how a row of tools is read.
            (Some((widget, text, at)), Some(_)) => {
                self.rest = None;
                changed |= host.set_tip(
                    def_id,
                    Some(Tip {
                        widget,
                        text,
                        at: (at.0 as f32, at.1 as f32),
                    }),
                );
            }
            // Nothing showing: the wait starts over at every move, since a
            // pointer that is moving has not rested.
            (Some((widget, text, at)), None) => {
                self.rest = Some(Rest {
                    widget,
                    text,
                    at,
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
        self.rest.is_some() || self.hold.is_some() || self.rest_on.is_some_and(|r| !r.told)
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
            });
        }
    }

    /// The rest is over: an element told of it hears the pointer moved on.
    fn end_rest(&mut self, host: &mut Host, ctx: &GestureCtx, out: &mut Vec<GestureEffect>) {
        let Some(r) = self.rest_on.take().filter(|r| r.told) else {
            return;
        };
        self.tell_rest(host, ctx, r.widget, None, out);
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
            && host.set_tip(
                ctx.def_id,
                Some(Tip {
                    widget: rest.widget,
                    text: rest.text,
                    at: (rest.at.0 as f32, rest.at.1 as f32),
                }),
            )
        {
            out.push(GestureEffect::Redraw(ctx.def_id));
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
        let Some((widget, text)) = host.tip_at(def_id, ctx.fb_w, ctx.fb_h, at.0, at.1) else {
            return Vec::new();
        };
        let tip = Tip {
            widget,
            text,
            at: (at.0 as f32, at.1 as f32),
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
    /// still has to take the tip down.
    pub fn key_began(&mut self, host: &mut Host, ctx: &GestureCtx) -> Vec<GestureEffect> {
        self.rest = None;
        let mut out = Vec::new();
        self.end_rest(host, ctx, &mut out);
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
