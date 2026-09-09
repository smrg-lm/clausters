//! **The query pass**: what a widget currently *is*, asked one kind at a time.
//!
//! Every method here is a third arm-per-kind pass over the model, the shape
//! [`size`](super::size) already has and in the same direction: the frame, the
//! gesture machine and the server leg all need to ask a widget something the
//! enum answers per variant — its event value, the bus it reads, the editor
//! chrome it carries, whether it sits on the window's time axis — and none of
//! those questions belongs in the file that declares the model.
//!
//! The second half is the **data routing**: anything that resolves a widget by
//! id and then wants its samples ([`signal_target`], [`bulk_target`]) asks
//! through a door rather than unpacking a variant, which is the containment
//! stated once rather than at each caller.
//!
//! [`signal_target`]: Widget::signal_target
//! [`bulk_target`]: Widget::bulk_target

use clausters_core::osc::OscType;
use serde_json::Value;

use super::super::elements::signal::SignalElement;
use super::element::BodyRole;
use super::element::Element;
use super::{EditorProps, GestureMap, Widget, WidgetKind};

impl Widget {
    /// This widget's gesture table: the `gestures` prop when it carries one,
    /// else the default its kind implies. The one door the press walk reads, so
    /// a container never has to know whether it was configured.
    pub fn gesture_map(&self) -> GestureMap {
        self.gestures
            .unwrap_or_else(|| GestureMap::of_kind(&self.kind))
    }

    /// The signal element this widget is, if it is one — through the trait's
    /// downcast door ([`Element::as_any`]).
    ///
    /// **Nothing in the passes calls this.** Every question a pass asks has a
    /// door of its own, which is the whole point of the seam; what is left is
    /// the element's own tests, which own the model they assert on.
    pub fn signal(&self) -> Option<&SignalElement> {
        self.kind.signal()
    }

    /// Whether this is a navigable signal element **on the time axis** — the
    /// view that zooms, pans, selects and shows a playhead over its own
    /// samples. A navigable *spectrum* is not one: it navigates frequency, on
    /// a window of its own ([`WidgetKind::navigates_freq`]).
    pub fn is_nav_signal(&self) -> bool {
        self.signal().is_some_and(SignalElement::navigates_time)
    }

    /// Whether this widget navigates the window's shared time axis: an element
    /// that says it does ([`Element::navigates_time`]),
    /// or one of the containers placed on that axis.
    pub fn is_timeline(&self) -> bool {
        self.kind.navigates_time() || matches!(self.kind, WidgetKind::TimeRuler { .. })
    }

    /// Whether this tree contains a widget whose overlay follows the pointer —
    /// the cursor readout a signal element over *stored* samples draws, and the
    /// timeline containers'. The windowed front asks on cursor motion: such a
    /// window needs a frame per move (a fully static one, like a plot's, has no
    /// other frame source; a live one is already redrawn every tick).
    pub fn has_hover_readout(&self) -> bool {
        self.descendants()
            .any(|w| w.is_timeline() || w.signal().is_some_and(|el| !el.is_live()))
    }
}

impl WidgetKind {
    /// **Whether these are pixels with nothing drawn on them**: a container's
    /// own surface (its margin, the gap between its children, the slack under
    /// the last one), a label, a lane's empty space, or a type this host does
    /// not paint at all.
    ///
    /// The question a gesture asks before falling *through* a widget to
    /// something behind it. In a window with one navigation group, empty pixels
    /// are that axis with nothing on them, so the wheel over them means what it
    /// means over a lane — but an element that draws a picture of its own and
    /// simply has no wheel of its own is not empty, and turning the wheel over
    /// a goniometer must not zoom the waterfall underneath it.
    ///
    /// Each gesture decides for itself whether to ask. The press does **not**:
    /// Shift+drag pans the axis from anywhere at all, over any element, which
    /// is the documented reach of that gesture rather than a fall-through.
    pub fn is_bare_surface(&self) -> bool {
        matches!(
            self,
            WidgetKind::Window { .. }
                | WidgetKind::Panel { .. }
                | WidgetKind::Stack { .. }
                | WidgetKind::Scroll { .. }
                // The strip that rules a stack: its empty space *is* the
                // axis, which is the case the fall-through was written for.
                | WidgetKind::TimeRuler { .. }
                | WidgetKind::Unknown(_)
        ) || matches!(self, WidgetKind::Custom(el) if el.is_bare_surface())
    }

    /// The **body role** this widget fills inside a container that layers its
    /// contents — the one question a `clip` asks about a child of its own.
    ///
    /// A built-in answers from its variant, an element from
    /// [`Element::body_role`]; anything that fills
    /// no role answers `None` and is simply not one of a clip's bodies. This
    /// is the single door, so the layering, the `/gui_set` routing, the drawing
    /// and the hit-test all recognize a body the same way and a new element can
    /// be one.
    pub fn body_role(&self) -> Option<BodyRole> {
        self.as_element().and_then(Element::body_role)
    }

    /// Whether this widget is a stop on its window's **tab ring**, and whether
    /// a press on it moves the keyboard focus there.
    ///
    /// Only an element can be one ([`Element::accepts_focus`]): focus is where
    /// keys go, and a key reaches a widget through
    /// [`Element::key`]. A container is not a stop — it
    /// arranges, it does not read.
    ///
    pub fn accepts_focus(&self) -> bool {
        self.as_element().is_some_and(Element::accepts_focus)
    }

    /// Whether this widget, focused, is taking **typed text**
    /// ([`Element::takes_text`]) — what the browser shell reads to decide
    /// where the keyboard goes, since composition needs an editable element
    /// and a canvas is not one.
    pub fn takes_text(&self) -> bool {
        self.as_element().is_some_and(Element::takes_text)
    }

    /// The area this widget occupies **outside its own rect** — an open list, a
    /// popup — or `None` for one that stays inside its placement.
    ///
    /// Only an element can have one, and it *declares* it, which is what lets
    /// the frame draw it last and the press route to it first without either
    /// pass keeping state about who opened what.
    pub fn overlay_rect(&self) -> Option<super::super::layout::Rect> {
        self.as_element().and_then(Element::overlay_rect)
    }

    /// Whether this widget navigates a **measured x axis of its own** — a
    /// frequency axis — instead of joining the window's shared time. The one
    /// widget that carries an x window rather than a navigation group.
    pub fn navigates_freq(&self) -> bool {
        self.as_element().is_some_and(Element::navigates_freq)
    }

    /// That axis inside the rect this widget was placed in
    /// ([`Element::freq_axis`]) — where it lies,
    /// what it shows, and at what rate.
    pub fn freq_axis(
        &self,
        rect: super::super::layout::Rect,
        m: &super::super::metrics::Metrics,
        sample_rate: f64,
    ) -> Option<super::element::FreqAxis> {
        self.as_element()?.freq_axis(rect, m, sample_rate)
    }

    /// The run this widget is holding for the hand ([`Element::pending_edit`]).
    pub fn pending_edit(&self) -> Option<&super::element::PendingEdit> {
        self.as_element()?.pending_edit()
    }

    /// Hands it one, or takes it back ([`Element::set_pending_edit`]);
    /// `false` where the widget is not the kind that can hold one.
    pub fn set_pending_edit(&mut self, held: Option<super::element::PendingEdit>) -> bool {
        self.as_element_mut()
            .is_some_and(|e| e.set_pending_edit(held))
    }

    /// One sample of its samples ([`Element::sample_value`]).
    pub fn sample_value(&self, channel: usize, frame: usize) -> Option<f32> {
        self.as_element()?.sample_value(channel, frame)
    }

    /// The widget's **value axis** inside the rect it was placed in
    /// ([`Element::value_axis`]) — the second measuring axis a marquee may
    /// restrict a selection on.
    pub fn value_axis(
        &self,
        rect: super::super::layout::Rect,
        indent: f32,
        m: &super::super::metrics::Metrics,
        lanes: usize,
    ) -> Option<super::element::ValueAxis> {
        self.as_element()?.value_axis(rect, indent, m, lanes)
    }

    /// What that axis would show for `want`, or shows now for `None` — the
    /// request opened up to what the analysis behind it resolves
    /// ([`Element::freq_window_of`]).
    pub fn freq_window_of(&self, sample_rate: f64, want: Option<(f64, f64)>) -> Option<(f64, f64)> {
        self.as_element()?.freq_window_of(sample_rate, want)
    }

    /// The narrowest window that axis may be **asked** for at `start`
    /// ([`Element::freq_min_span`]).
    pub fn freq_min_span(&self, sample_rate: f64, start: f64) -> Option<f64> {
        self.as_element()?.freq_min_span(sample_rate, start)
    }

    /// The current value as an OSC primitive for a `/gui_event`, or `None` for a
    /// non-interactive widget. A `button` reports its `on` value (it is
    /// momentary either way; the press is the event).
    pub fn event_value(&self) -> Option<OscType> {
        self.as_element().and_then(Element::value)
    }

    /// **What a gesture has changed on this widget**, in the props' own
    /// vocabulary — the keys a script could set, with the values it would have
    /// to set to reproduce what is on screen.
    ///
    /// The one door `/gui_query` overlays on the document, so a widget answers
    /// with what it *is* rather than with what it was defined as. An element
    /// answers for itself ([`Element::info`]); a built-in
    /// is an arm here, and its row disappears as the leaf moves behind the
    /// trait — the shape [`Self::needs`] already has.
    ///
    /// **Only what a gesture can change belongs here.** A prop the script alone
    /// writes is already current in the document, since a `/gui_set` updates it;
    /// restating it would be a second source of truth for the same value. And a
    /// non-scalar rides as the JSON string its `/gui_set` already accepts, so
    /// what a query gives back is what a set would take.
    pub fn info(&self) -> Vec<(String, Value)> {
        // **The markers, wherever a ruler carries them.** They are the axis'
        // chrome rather than one kind's, and a hand puts them there, so every
        // widget that has an editor answers for them -- as the JSON string a
        // `/gui_set markers` takes.
        let markers = self
            .editor()
            .filter(|e| !e.markers.is_empty())
            .map(|e| {
                (
                    "markers".to_string(),
                    Value::from(super::markers_json(&e.markers).to_string()),
                )
            })
            .into_iter();
        let own = match self {
            WidgetKind::Custom(el) => el.info(),
            WidgetKind::Scroll { view, .. } => view.info(),
            _ => Vec::new(),
        };
        own.into_iter().chain(markers).collect()
    }

    /// **What this widget reads from outside itself** — the one door every tree
    /// collector asks, so none of them matches on a kind.
    ///
    /// Only an element reads anything: a container arranges, and the leaves
    /// that read a bus, tap a ring, watch a node tree or claim a slot are all
    /// behind the trait. What used to be assembled here, arm by arm, is the
    /// element's own [`Element::needs`].
    pub fn needs(&self) -> super::Needs {
        self.as_element()
            .map_or_else(Default::default, Element::needs)
    }

    /// **The drag table an element declares for itself**, or `None` for a
    /// widget whose table is its container kind's
    /// ([`GestureMap::of_kind`](super::GestureMap::of_kind), which asks this
    /// first).
    pub fn element_gesture_map(&self) -> Option<super::GestureMap> {
        self.as_element().and_then(Element::gesture_map)
    }

    /// **The look of a body whose picture is a texture** — the one body the
    /// frame routes to the GPU pass itself, keyed by the clip that holds it
    /// ([`Element::texture_body`]).
    pub fn texture_body(&self) -> Option<super::element::TextureLook> {
        self.as_element().and_then(Element::texture_body)
    }

    /// **What this widget reserves left of its body** on a shared time axis: a
    /// lane's header, a roll's keyboard, an element's value ruler. A
    /// `timeruler` asks for nothing — it has no chrome, it only labels whatever
    /// axis it follows.
    ///
    /// A container answers from its variant, an element for itself
    /// ([`Element::gutter`]).
    pub fn gutter(&self, m: &super::super::metrics::Metrics) -> f32 {
        match self {
            WidgetKind::Custom(el) => el.gutter(m),
            _ => 0.0,
        }
    }

    /// [`axis_body`](super::Element::axis_body) of an element, or `None` for a
    /// container (whose body is the container's own geometry).
    pub fn axis_body(
        &self,
        rect: super::super::layout::Rect,
        indent: f32,
        m: &super::super::metrics::Metrics,
    ) -> Option<(super::super::layout::Rect, bool)> {
        self.as_element()?.axis_body(rect, indent, m)
    }

    /// [`content_span`](super::Element::content_span) of an element.
    pub fn content_span(&self) -> Option<f64> {
        self.as_element().and_then(Element::content_span)
    }

    /// Whether this widget navigates the window's shared time axis
    /// ([`Element::navigates_time`]).
    pub fn navigates_time(&self) -> bool {
        self.as_element().is_some_and(Element::navigates_time)
    }

    /// Whether this widget's time axis is not bounded by what it holds
    /// ([`Element::unbounded_axis`]).
    pub fn unbounded_axis(&self) -> bool {
        self.as_element().is_some_and(Element::unbounded_axis)
    }

    /// [`gutter`](Self::gutter) asked again of a widget that has been
    /// **placed**, for the chrome whose width is a property of the data rather
    /// than of the props. `None` — every container, and most elements — is a
    /// widget whose first answer stands.
    pub fn measured_gutter(
        &self,
        rect: super::super::layout::Rect,
        m: &super::super::metrics::Metrics,
    ) -> Option<f32> {
        self.as_element()?.measured_gutter(rect, m)
    }

    /// **How many rows this widget stacks**, out of the `uploaded` channel
    /// count the front read off its GPU slot — the divisor for a lane-relative
    /// y gesture. A widget with no slot was given nothing and is one lane.
    ///
    /// A built-in answers from its variant, an element for itself
    /// ([`Element::rows`]).
    pub fn rows(&self, uploaded: usize) -> usize {
        self.as_element()
            .map_or_else(|| uploaded.max(1), |el| el.rows(uploaded))
    }

    /// Whether a y zoom over this widget anchors at the centre of a lane
    /// instead of under the pointer — an **amplitude** axis, whose zero sits at
    /// the centre of every lane.
    ///
    /// A built-in answers from its variant, an element for itself
    /// ([`Element::centres_y_zoom`]).
    pub fn centres_y_zoom(&self) -> bool {
        self.as_element().is_some_and(Element::centres_y_zoom)
    }

    /// **The window one read of this widget's taps has to bring**, in frames at
    /// `sample_rate` — the one door the page's tap subscription is sized from.
    ///
    /// A built-in answers from its variant, an element for itself
    /// ([`Element::tap_frames`]). It replaced three
    /// collectors that each walked the tree building a per-kind read spec — a
    /// scope's, a goniometer's, a spectrum's — only to take the largest of the
    /// three and throw the specs away.
    pub fn tap_frames(&self, sample_rate: f64) -> usize {
        self.as_element().map_or(0, |el| el.tap_frames(sample_rate))
    }

    /// The editor chrome of a view that carries one — a timeline view
    /// (waveform/spectrogram) or a `track` lane, which reuses the same props for
    /// its ruler and playhead. The shared read path for the frame renderer and
    /// the fronts. (Group membership is `is_timeline`, not this: a lane has the
    /// chrome but navigates with the window's clip span.)
    pub fn editor(&self) -> Option<&EditorProps> {
        match self {
            WidgetKind::Custom(el) => el.editor(),
            WidgetKind::TimeRuler { editor, .. } => Some(editor),
            _ => None,
        }
    }

    /// Mutable access to a view's editor chrome (the selection drag writes
    /// through here).
    pub fn editor_mut(&mut self) -> Option<&mut EditorProps> {
        match self {
            WidgetKind::Custom(el) => el.editor_mut(),
            WidgetKind::TimeRuler { editor, .. } => Some(editor),
            _ => None,
        }
    }

    /// **The element this kind is**, if it is one — and the one match every
    /// question above is asked through.
    ///
    /// That is the shape the whole file collapsed to once the port finished: a
    /// question a *leaf* answers is not a pass over the enum at all, it is this
    /// door and then the trait, so the method beside it carries only the
    /// **container's** answer — which for almost every question is the neutral
    /// one, because a container arranges and reads nothing.
    /// ([`Element::as_any`] does the rest for a caller
    /// that wants the concrete leaf.)
    pub fn as_element(&self) -> Option<&dyn Element> {
        match self {
            WidgetKind::Custom(el) => Some(&**el),
            _ => None,
        }
    }

    /// The same door, mutably — what a tick, a bulk load and a slot fill write
    /// through.
    pub fn as_element_mut(&mut self) -> Option<&mut dyn Element> {
        match self {
            WidgetKind::Custom(el) => Some(&mut **el),
            _ => None,
        }
    }

    /// The signal element this kind is, if it is one — see
    /// [`Widget::signal`] for why this is a downcast and not a match.
    pub fn signal(&self) -> Option<&SignalElement> {
        self.as_element()?.as_any()?.downcast_ref::<SignalElement>()
    }

    /// **One tick** of whatever this widget accumulates from a live source.
    ///
    /// A built-in answers from its variant, an element for itself
    /// ([`Element::tick`]) — the single door, so the
    /// front drives one walk instead of one per kind of live view.
    pub fn tick(&mut self, live: &super::element::Live) {
        if let Some(el) = self.as_element_mut() {
            el.tick(live);
        }
    }

    /// **A declared bulk resource has arrived**: the element takes it home.
    ///
    /// A built-in answers from its variant, an element for itself
    /// ([`Element::bulk`]) — the single door, so a loader
    /// resolves a resource and never reaches into a widget to place it.
    pub fn take_bulk(&mut self, data: super::element::Loaded) -> bool {
        self.as_element_mut().is_some_and(|el| el.bulk(data))
    }

    /// [`take_bulk`](Self::take_bulk) for one of a plural asker's
    /// [`Needs::takes`](super::element::Needs::takes): the buffer says which.
    ///
    /// It falls back to the unlabelled door, so a widget that asked for exactly
    /// one source is served by either path and nothing has to know which kind
    /// of asker it was.
    pub fn take_bulk_of(
        &mut self,
        bufnum: i32,
        data: &impl Fn() -> super::element::Loaded,
    ) -> bool {
        match self.as_element_mut() {
            Some(el) => el.bulk_of(bufnum, data()) || el.bulk(data()),
            None => false,
        }
    }

    /// **What this widget's claimed GPU slot is fed**, when it has something
    /// new for it.
    ///
    /// A built-in answers from its variant, an element for itself
    /// ([`Element::fills`]) — the single door, so the front's upload walk asks
    /// the tree what to upload instead of deriving it from what each kind
    /// happens to be.
    pub fn fills(&mut self) -> Vec<(super::element::SlotKey, super::element::SlotFill)> {
        match self.as_element_mut() {
            Some(el) => el.fills(),
            None => Vec::new(),
        }
    }

    /// **What this widget was told to read again**, when it was
    /// ([`Element::wants_reload`]). Asking clears the ask.
    pub fn wants_reload(&mut self) -> Option<super::element::Bulk> {
        self.as_element_mut()?.wants_reload()
    }

    /// **The window's GPU slots are gone** (a device rebuilt, a canvas
    /// re-attached): whatever this widget handed over has to be handed over
    /// again.
    pub fn slot_dropped(&mut self) {
        if let Some(el) = self.as_element_mut() {
            el.slot_dropped();
        }
    }
}

impl Widget {
    /// The signal element this widget draws with.
    ///
    /// A test accessor like [`Widget::signal`]: what a *pass* wants of a
    /// widget's samples it asks through a door ([`Widget::bulk_target`]).
    pub fn signal_target(&self) -> Option<&SignalElement> {
        self.kind
            .signal()
            .or_else(|| self.children.iter().find_map(|c| c.kind.signal()))
    }

    /// **Who a bulk load is really for**: this widget, or the body of it that
    /// declared the want — a clip's take carries no id, so the fetch was keyed
    /// by the container's.
    ///
    /// It asks the declaration rather than the variant, which is what lets a
    /// registered element be a clip's body and be loaded like any other.
    pub fn bulk_target(&self) -> &Widget {
        let declares = |w: &Widget| {
            let needs = w.kind.needs();
            needs.bulk.is_some() || needs.slot.is_some()
        };
        if declares(self) {
            return self;
        }
        self.children.iter().find(|c| declares(c)).unwrap_or(self)
    }

    /// [`Self::bulk_target`] for a write — the same routing, mutably: a run of
    /// samples fetched for a clip's take lands in the body that asked for it.
    pub fn bulk_target_mut(&mut self) -> &mut Widget {
        let declares = |w: &Widget| {
            let needs = w.kind.needs();
            needs.bulk.is_some() || needs.slot.is_some()
        };
        if declares(self) {
            return self;
        }
        match self.children.iter().position(declares) {
            Some(i) => &mut self.children[i],
            None => self,
        }
    }

    /// Hands a loaded resource to whichever of this widget **or its bodies**
    /// takes it, returning whether one did.
    ///
    /// The routing is here and not at the call site because a clip's body
    /// carries no id: a fetch was addressed to the container, so the answer
    /// has to look one level in — and both fronts were walking that level
    /// themselves, which is one walk written twice.
    /// `data` is a **maker** rather than a value because a `Loaded` is the
    /// payload itself: it is built only for the widget that takes it, and never
    /// copied past one that declined.
    pub fn take_bulk(&mut self, data: impl Fn() -> super::element::Loaded) -> bool {
        if self.kind.take_bulk(data()) {
            return true;
        }
        self.children.iter_mut().any(|b| b.kind.take_bulk(data()))
    }

    /// [`take_bulk`](Self::take_bulk) for one of a plural asker's
    /// [`Needs::takes`](super::element::Needs::takes), routed the same way: the
    /// buffer number says which of several a widget asked for.
    pub fn take_bulk_of(&mut self, bufnum: i32, data: impl Fn() -> super::element::Loaded) -> bool {
        if self.kind.take_bulk_of(bufnum, &data) {
            return true;
        }
        self.children
            .iter_mut()
            .any(|b| b.kind.take_bulk_of(bufnum, &data))
    }

    /// **What a gesture has changed on this widget**, from its own kind
    /// ([`WidgetKind::info`]).
    pub fn info(&self) -> Vec<(String, Value)> {
        self.kind.info()
    }

    /// The body a **layer address** names among this container's layered
    /// contents, mutably — the door a press on a layer and a drag holding one
    /// both write through.
    ///
    /// `None` for the placement layer (which is the container itself, not one
    /// of its contents) and for an address the container no longer has, which
    /// a drag whose body was freed under it produces and which every caller
    /// already survives.
    pub(crate) fn layer_body_mut(
        &mut self,
        layer: crate::host::layers::Layer,
    ) -> Option<&mut WidgetKind> {
        let crate::host::layers::Layer::Content(n) = layer else {
            return None;
        };
        self.children
            .iter_mut()
            .filter(|c| c.kind.body_role().is_some())
            .nth(n)
            .map(|c| &mut c.kind)
    }
}
