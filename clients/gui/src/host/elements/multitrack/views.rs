//! **A box of samples is the audio editor's view of its take, placed.**
//!
//! Each box over a take holds a signal element -- the very one the audio
//! editor draws a file with -- and is drawn through that element's own slot,
//! at the box's window onto the take: the same layers, the same summary and
//! the same reads past it, so zooming a box to the samples, its spectral
//! picture and whatever that view learns next are the audio editor's and are
//! never written here again. What the multitrack keeps is what is its own:
//! where a box is, its frame, its fades, its label and its grips.
//!
//! The take's summary is read once, by the take's own body ([`Multitrack`]'s
//! `takes`), and every view of it shares that summary; what a view holds of
//! its own is what its zoom asked for -- the window of samples and the finer
//! grid under the span it shows.

use super::*;

/// **One box's view of its take**: the signal element it is drawn through,
/// under the slot key that addresses its picture, and the take it is of.
#[derive(Debug, Clone)]
pub(super) struct BoxView {
    pub key: SlotKey,
    pub source: i32,
    pub body: SignalElement,
}

impl Multitrack {
    /// **Keeps a view per box of samples**: one made for a box that has a
    /// take to view and none yet (or one of another take), and the view of a
    /// box that is gone let go of.
    ///
    /// A view is made from its take's body as it stands, so a box drawn
    /// before its summary arrived is drawn once it does
    /// ([`Self::refresh_views`]).
    pub(super) fn sync_views(&mut self) {
        let names: HashSet<&str> = self.clips.iter().map(|c| c.name.as_str()).collect();
        self.views.retain(|name, _| names.contains(name.as_str()));
        for clip in &self.clips {
            if self
                .views
                .get(&clip.name)
                .is_some_and(|v| v.source == clip.source)
            {
                continue;
            }
            let Some(take) = self.takes.get(&clip.source) else {
                continue;
            };
            let key = match self.views.get(&clip.name) {
                Some(view) => view.key,
                None => {
                    self.next_view += 1;
                    SlotKey(self.next_view)
                }
            };
            self.views.insert(
                clip.name.clone(),
                BoxView {
                    key,
                    source: clip.source,
                    body: fresh(take),
                },
            );
        }
    }

    /// **Every view of take `source` takes it as it now stands** -- its
    /// summary arrived, or arrived again after the take changed. What a view
    /// had read past the summary was of the take before, and goes with it.
    pub(super) fn refresh_views(&mut self, source: i32) {
        self.sync_views();
        let Some(take) = self.takes.get(&source) else {
            return;
        };
        for view in self.views.values_mut().filter(|v| v.source == source) {
            view.body = fresh(take);
        }
    }

    /// The view slot `key` draws.
    pub(super) fn view_of(&self, key: SlotKey) -> Option<&BoxView> {
        self.views.values().find(|v| v.key == key)
    }

    /// [`view_of`](Self::view_of), mutably.
    pub(super) fn view_of_mut(&mut self, key: SlotKey) -> Option<&mut BoxView> {
        self.views.values_mut().find(|v| v.key == key)
    }

    /// **What each box's view draws this frame**: the view's own slot frame,
    /// in the box's rectangle and over the box's window onto the take -- one
    /// per run of it, since a box that loops reads its take once per lap.
    pub(super) fn view_slots(&self, ctx: &Ctx) -> Vec<(SlotKey, SlotFrame)> {
        let mut out = Vec::new();
        for (n, cr, local) in self.boxes_on_screen(ctx.rect, ctx.indent, ctx.metrics, ctx.time) {
            let clip = &self.clips[n];
            let Some(view) = self.views.get(&clip.name) else {
                continue;
            };
            let Some(frames) = take_frames(&view.body) else {
                continue;
            };
            let window = SourceWindow {
                start: clip.place.start,
                rate: self.rate_of(&clip.name),
                looping: self.wraps(&clip.name),
                ..SourceWindow::default()
            };
            let x = |t: f64| cr.x + ((t - local.start) / local.len) as f32 * cr.w;
            let rate = window.frames_per_unit();
            for (from, to, source) in
                window.runs(local.start, local.start + local.len, clip.place.dur, frames)
            {
                let rect = Rect::new(x(from), cr.y, x(to) - x(from), cr.h);
                if rect.w <= 0.0 {
                    continue;
                }
                let inside = Ctx {
                    world: ctx.world,
                    metrics: ctx.metrics,
                    rect,
                    indent: 0.0,
                    clip: ctx.clip,
                    scale: ctx.scale,
                    time: None,
                    focused: false,
                    hovered: false,
                    clock: ctx.clock,
                };
                for (_, frame) in view.body.slots(&inside) {
                    if let SlotFrame::Signal {
                        layers,
                        axis,
                        domain,
                        y,
                        overlay,
                        lanes,
                        loudness,
                        look,
                        ..
                    } = frame
                    {
                        out.push((
                            view.key,
                            SlotFrame::Signal {
                                body: rect,
                                layers,
                                axis,
                                domain,
                                y,
                                overlay,
                                lanes,
                                loudness,
                                look,
                                placed: Some(View {
                                    start: source,
                                    len: (to - from) * rate,
                                }),
                            },
                        ));
                    }
                }
            }
        }
        out
    }

    /// **What the views have for their slots**, each under its own key.
    ///
    /// The views are kept here first: a fill is what runs before every
    /// repaint, so a box a hand just made -- the tail of a split, a paste --
    /// has its view by the frame that draws it.
    pub(super) fn view_fills(&mut self) -> Vec<(SlotKey, SlotFill)> {
        self.sync_views();
        self.views
            .values_mut()
            .flat_map(|view| {
                let key = view.key;
                view.body
                    .fills()
                    .into_iter()
                    .map(move |(_, fill)| (key, fill))
            })
            .collect()
    }
}

/// **How many frames `take` is**: what its samples say, or -- for a take
/// drawn from its files, before its summary has arrived -- what its list
/// does, which is known the moment the list is named.
pub(super) fn take_frames(take: &SignalElement) -> Option<f64> {
    take.sample_shape()
        .map(|(_, frames)| frames as f64)
        .or_else(|| take.segments().map(|list| list.frames as f64))
}

/// A view of `take` as it stands, owing its slot the picture: a view is
/// filled when it is made, as a window's view is when it opens.
fn fresh(take: &SignalElement) -> SignalElement {
    let mut body = take.clone();
    Slotted::slot_dropped(&mut body);
    body
}
