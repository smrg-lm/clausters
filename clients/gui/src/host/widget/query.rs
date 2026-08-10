//! **The query pass**: what a widget currently *is*, asked one kind at a time.
//!
//! Every method here is a third arm-per-kind pass over the model, the shape
//! [`size`](super::size) already has and in the same direction: the frame, the
//! gesture machine and the server leg all need to ask a widget something the
//! enum answers per variant — its event value, the bus it reads, the editor
//! chrome it carries, whether it sits on the window's time axis — and none of
//! those questions belongs in the file that declares the model.
//!
//! The second half is the **clip routing**, which is the same question asked
//! through a container: a clip's bodies carry no id, so anything that resolves
//! a widget by id and then wants its data ([`signal_target`], [`kind_or_body`],
//! [`clip_body`]) lands on the clip and reaches the body that owns it. That is
//! the containment stated once, rather than at each caller.
//!
//! [`signal_target`]: Widget::signal_target
//! [`kind_or_body`]: Widget::kind_or_body
//! [`clip_body`]: Widget::clip_body

use clausters_core::osc::OscType;
use serde_json::Value;

use super::super::signal::{Presentation, SignalElement};
use super::element::BodyRole;
use super::{EditorProps, GestureMap, Widget, WidgetKind, build};

impl Widget {
    /// This widget's gesture table: the `gestures` prop when it carries one,
    /// else the default its kind implies. The one door the press walk reads, so
    /// a container never has to know whether it was configured.
    pub fn gesture_map(&self) -> GestureMap {
        self.gestures
            .unwrap_or_else(|| GestureMap::of_kind(&self.kind))
    }

    /// The signal element this widget is, if it is one.
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

    /// Whether this widget navigates the window's shared time axis: a navigable
    /// signal element, or one of the containers placed on that axis.
    pub fn is_timeline(&self) -> bool {
        self.is_nav_signal()
            || matches!(
                self.kind,
                WidgetKind::Track { .. }
                    | WidgetKind::PianoRoll { .. }
                    | WidgetKind::TimeRuler { .. }
            )
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
                // A lane and the strip that rules it: their empty space *is*
                // the axis, which is the case the fall-through was written for.
                | WidgetKind::Track { .. }
                | WidgetKind::TimeRuler { .. }
                | WidgetKind::Unknown(_)
        ) || matches!(self, WidgetKind::Custom(el) if el.is_bare_surface())
    }

    /// The **body role** this widget fills inside a container that layers its
    /// contents — the one question a `clip` asks about a child of its own.
    ///
    /// A built-in answers from its variant, an element from
    /// [`Element::body_role`](super::Element::body_role); anything that fills
    /// no role answers `None` and is simply not one of a clip's bodies. This
    /// is the single door, so the layering, the `/gui_set` routing, the drawing
    /// and the hit-test all recognize a body the same way and a new element can
    /// be one.
    pub fn body_role(&self) -> Option<BodyRole> {
        match self {
            WidgetKind::Signal(_) => Some(BodyRole::Take),
            WidgetKind::PianoRoll { .. } => Some(BodyRole::Notes),
            WidgetKind::Custom(el) => el.body_role(),
            _ => None,
        }
    }

    /// Whether this widget is a stop on its window's **tab ring**, and whether
    /// a press on it moves the keyboard focus there.
    ///
    /// Only an element can be one ([`Element::accepts_focus`]): focus is where
    /// keys go, and a key reaches a widget through
    /// [`Element::key`](super::Element::key). A container is not a stop — it
    /// arranges, it does not read.
    ///
    /// [`Element::accepts_focus`]: super::Element::accepts_focus
    pub fn accepts_focus(&self) -> bool {
        matches!(self, WidgetKind::Custom(el) if el.accepts_focus())
    }

    /// The area this widget occupies **outside its own rect** — an open list, a
    /// popup — or `None` for one that stays inside its placement.
    ///
    /// Only an element can have one, and it *declares* it, which is what lets
    /// the frame draw it last and the press route to it first without either
    /// pass keeping state about who opened what.
    pub fn overlay_rect(&self) -> Option<super::super::layout::Rect> {
        match self {
            WidgetKind::Custom(el) => el.overlay_rect(),
            _ => None,
        }
    }

    /// Whether this widget navigates a **measured x axis of its own** — a
    /// frequency axis — instead of joining the window's shared time. The one
    /// widget that carries an x window rather than a navigation group.
    pub fn navigates_freq(&self) -> bool {
        match self {
            WidgetKind::Signal(el) => el.navigates_freq(),
            WidgetKind::Custom(el) => el.navigates_freq(),
            _ => false,
        }
    }

    /// That axis inside the rect this widget was placed in
    /// ([`Element::freq_axis`](super::Element::freq_axis)) — where it lies,
    /// what it shows, and at what rate.
    pub fn freq_axis(
        &self,
        rect: super::super::layout::Rect,
        m: &super::super::metrics::Metrics,
        sample_rate: f64,
    ) -> Option<super::element::FreqAxis> {
        match self {
            WidgetKind::Signal(el) => el.freq_axis(rect, m, sample_rate),
            WidgetKind::Custom(el) => el.freq_axis(rect, m, sample_rate),
            _ => None,
        }
    }

    /// What that axis would show for `want`, or shows now for `None` — the
    /// request opened up to what the analysis behind it resolves
    /// ([`Element::freq_window_of`](super::Element::freq_window_of)).
    pub fn freq_window_of(&self, sample_rate: f64, want: Option<(f64, f64)>) -> Option<(f64, f64)> {
        match self {
            WidgetKind::Signal(el) => el.freq_window_shown(sample_rate, want),
            WidgetKind::Custom(el) => el.freq_window_of(sample_rate, want),
            _ => None,
        }
    }

    /// The narrowest window that axis may be **asked** for at `start`
    /// ([`Element::freq_min_span`](super::Element::freq_min_span)).
    pub fn freq_min_span(&self, sample_rate: f64, start: f64) -> Option<f64> {
        match self {
            WidgetKind::Signal(el) => el.freq_min_span(sample_rate, start),
            WidgetKind::Custom(el) => el.freq_min_span(sample_rate, start),
            _ => None,
        }
    }

    /// The current value as an OSC primitive for a `/gui_event`, or `None` for a
    /// non-interactive widget. A `button` reports `1` (it is momentary; the press
    /// is the event).
    pub fn event_value(&self) -> Option<OscType> {
        match self {
            WidgetKind::Custom(el) => el.value(),
            _ => None,
        }
    }

    /// **What a gesture has changed on this widget**, in the props' own
    /// vocabulary — the keys a script could set, with the values it would have
    /// to set to reproduce what is on screen.
    ///
    /// The one door `/gui_query` overlays on the document, so a widget answers
    /// with what it *is* rather than with what it was defined as. An element
    /// answers for itself ([`Element::info`](super::Element::info)); a built-in
    /// is an arm here, and its row disappears as the leaf moves behind the
    /// trait — the shape [`Self::needs`] already has.
    ///
    /// **Only what a gesture can change belongs here.** A prop the script alone
    /// writes is already current in the document, since a `/gui_set` updates it;
    /// restating it would be a second source of truth for the same value. And a
    /// non-scalar rides as the JSON string its `/gui_set` already accepts, so
    /// what a query gives back is what a set would take.
    pub fn info(&self) -> Vec<(String, Value)> {
        match self {
            WidgetKind::Custom(el) => el.info(),
            WidgetKind::Clip { offset, dur, .. } => vec![
                ("offset".into(), Value::from(*offset)),
                ("dur".into(), Value::from(*dur)),
            ],
            WidgetKind::Track { header, .. } => header
                .mute
                .map(|b| ("mute".into(), Value::from(b)))
                .into_iter()
                .chain(header.solo.map(|b| ("solo".into(), Value::from(b))))
                .chain(header.level.map(|v| ("level".into(), Value::from(v))))
                .collect(),
            WidgetKind::PianoRoll { notes, osc, .. } => vec![
                (
                    "notes".into(),
                    Value::from(super::super::pianoroll::notes_json(notes).to_string()),
                ),
                (
                    "osc".into(),
                    Value::from(super::super::pianoroll::osc_json(osc).to_string()),
                ),
            ],
            WidgetKind::Scroll { view, .. } => view.info(),
            _ => Vec::new(),
        }
    }

    /// **What this widget reads from outside itself** — the one door every tree
    /// collector asks, so none of them matches on a kind any more. An element
    /// answers for itself; a built-in is assembled here out of the per-kind
    /// queries below, each answered by the arm that knows the prop it comes
    /// from, and its rows disappear as the leaf moves behind the trait.
    pub fn needs(&self) -> super::Needs {
        if let WidgetKind::Custom(el) = self {
            return el.needs();
        }
        let mut needs = super::Needs {
            buses: self.live_bus().into_iter().collect(),
            retention: self.signal().map_or(0.0, SignalElement::retention),
            bulk: self.signal().and_then(SignalElement::want),
            slot: self.signal().and_then(SignalElement::slot_kind),
            ..Default::default()
        };
        self.audio_buses_read(&mut needs.taps);
        needs
    }

    /// **The drag table an element declares for itself**, or `None` for a
    /// widget whose table is its container kind's
    /// ([`GestureMap::of_kind`](super::GestureMap::of_kind), which asks this
    /// first).
    pub fn element_gesture_map(&self) -> Option<super::GestureMap> {
        match self {
            WidgetKind::Signal(el) => el.gesture_map(),
            WidgetKind::Custom(el) => el.gesture_map(),
            _ => None,
        }
    }

    /// **The look of a body whose picture is a texture** — the one body the
    /// frame routes to the GPU pass itself, keyed by the clip that holds it
    /// ([`Element::texture_body`](super::Element::texture_body)).
    pub fn texture_body(&self) -> Option<super::element::TextureLook> {
        match self {
            WidgetKind::Signal(el) => el.texture_body(),
            WidgetKind::Custom(el) => el.texture_body(),
            _ => None,
        }
    }

    /// **What this widget reserves left of its body** on a shared time axis: a
    /// lane's header, a roll's keyboard, an element's value ruler. A
    /// `timeruler` asks for nothing — it has no chrome, it only labels whatever
    /// axis it follows.
    ///
    /// A container answers from its variant, an element for itself
    /// ([`Element::gutter`](super::Element::gutter)).
    pub fn gutter(&self, m: &super::super::metrics::Metrics) -> f32 {
        match self {
            WidgetKind::Track { header, .. } => header.width(m),
            WidgetKind::PianoRoll { .. } => super::super::pianoroll::KEYBOARD_W,
            WidgetKind::Signal(el) => el.gutter(m),
            WidgetKind::Custom(el) => el.gutter(m),
            _ => 0.0,
        }
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
        match self {
            WidgetKind::Signal(el) => el.measured_gutter(rect, m),
            WidgetKind::Custom(el) => el.measured_gutter(rect, m),
            _ => None,
        }
    }

    /// **How many lanes this widget stacks**, out of the `uploaded` channel
    /// count the front read off its GPU slot — the divisor for a lane-relative
    /// y gesture. A widget with no slot was given nothing and is one lane.
    ///
    /// A built-in answers from its variant, an element for itself
    /// ([`Element::lanes`](super::Element::lanes)).
    pub fn lanes(&self, uploaded: usize) -> usize {
        match self {
            WidgetKind::Signal(el) => el.lanes(uploaded),
            WidgetKind::Custom(el) => el.lanes(uploaded),
            _ => uploaded.max(1),
        }
    }

    /// Whether a y zoom over this widget anchors at the centre of a lane
    /// instead of under the pointer — an **amplitude** axis, whose zero sits at
    /// the centre of every lane.
    ///
    /// A built-in answers from its variant, an element for itself
    /// ([`Element::centres_y_zoom`](super::Element::centres_y_zoom)).
    pub fn centres_y_zoom(&self) -> bool {
        match self {
            WidgetKind::Signal(el) => el.centres_y_zoom(),
            WidgetKind::Custom(el) => el.centres_y_zoom(),
            _ => false,
        }
    }

    /// **The window one read of this widget's taps has to bring**, in frames at
    /// `sample_rate` — the one door the page's tap subscription is sized from.
    ///
    /// A built-in answers from its variant, an element for itself
    /// ([`Element::tap_frames`](super::Element::tap_frames)). It replaced three
    /// collectors that each walked the tree building a per-kind read spec — a
    /// scope's, a goniometer's, a spectrum's — only to take the largest of the
    /// three and throw the specs away.
    pub fn tap_frames(&self, sample_rate: f64) -> usize {
        match self {
            WidgetKind::Signal(el) => el.tap_frames(sample_rate),
            WidgetKind::Custom(el) => el.tap_frames(sample_rate),
            _ => 0,
        }
    }

    /// The control bus a live (shared-memory-backed) **trace** reads each
    /// frame, if this is one — the one bus a `scope` history is advanced from,
    /// which is why this stays a single answer where [`Self::needs`] is a set.
    /// An audio-rate view reads recorded samples instead — see
    /// [`Self::audio_buses_read`].
    pub fn live_bus(&self) -> Option<i32> {
        match self {
            WidgetKind::Signal(el) if el.presentation == Presentation::Signal => el
                .source
                .bus()
                .filter(|b| !b.rate.is_audio())
                .map(|b| b.bus),
            _ => None,
        }
    }

    /// Appends every audio bus whose **samples** this widget reads each frame —
    /// `channels` adjacent buses for an audio-rate `scope` or a `spectrum`, two
    /// (left and right) for a `phasescope`. This is the set the host asks the
    /// server to record (`/bus_tap`) and the set it animates for, so all three
    /// sample consumers are covered uniformly.
    pub fn audio_buses_read(&self, out: &mut Vec<i32>) {
        let Some(el) = self.signal() else { return };
        let Some(bus) = el.source.bus() else { return };
        match el.presentation {
            // The phase view is a stereo pair by construction: a bus and the
            // one beside it, whatever `channels` says.
            Presentation::Phase => out.extend([bus.bus, bus.bus + 1]),
            // A control-rate trace is read as a bus value, not as samples.
            Presentation::Signal if !bus.rate.is_audio() => {}
            _ => out.extend((0..bus.channels as i32).map(|k| bus.bus + k)),
        }
    }

    /// The editor chrome of a view that carries one — a timeline view
    /// (waveform/spectrogram) or a `track` lane, which reuses the same props for
    /// its ruler and playhead. The shared read path for the frame renderer and
    /// the fronts. (Group membership is `is_timeline`, not this: a lane has the
    /// chrome but navigates with the window's clip span.)
    pub fn editor(&self) -> Option<&EditorProps> {
        match self {
            WidgetKind::Signal(el) => Some(&el.editor),
            WidgetKind::Track { editor, .. }
            | WidgetKind::PianoRoll { editor, .. }
            | WidgetKind::TimeRuler { editor, .. } => Some(editor),
            _ => None,
        }
    }

    /// Mutable access to a view's editor chrome (the selection drag writes
    /// through here).
    pub fn editor_mut(&mut self) -> Option<&mut EditorProps> {
        match self {
            WidgetKind::Signal(el) => Some(&mut el.editor),
            WidgetKind::Track { editor, .. }
            | WidgetKind::PianoRoll { editor, .. }
            | WidgetKind::TimeRuler { editor, .. } => Some(editor),
            _ => None,
        }
    }

    /// Applies one `/gui_set` key/value to a live widget, returning whether it
    /// changed anything the renderer cares about.
    /// The signal element this kind is, if it is one.
    pub fn signal(&self) -> Option<&SignalElement> {
        match self {
            WidgetKind::Signal(el) => Some(el),
            _ => None,
        }
    }

    /// The signal element this kind is, mutably — a bulk load and a `/gui_set`
    /// both write through here.
    pub fn signal_mut(&mut self) -> Option<&mut SignalElement> {
        match self {
            WidgetKind::Signal(el) => Some(el),
            _ => None,
        }
    }

    /// **One tick** of whatever this widget accumulates from a live source.
    ///
    /// A built-in answers from its variant, an element for itself
    /// ([`Element::tick`](super::Element::tick)) — the single door, so the
    /// front drives one walk instead of one per kind of live view.
    pub fn tick(&mut self, live: &super::element::Live) {
        match self {
            WidgetKind::Signal(el) => el.tick(live),
            WidgetKind::Custom(el) => el.tick(live),
            _ => {}
        }
    }

    /// **A declared bulk resource has arrived**: the element takes it home.
    ///
    /// A built-in answers from its variant, an element for itself
    /// ([`Element::bulk`](super::Element::bulk)) — the single door, so a loader
    /// resolves a resource and never reaches into a widget to place it.
    pub fn take_bulk(&mut self, data: super::element::Loaded) -> bool {
        match self {
            WidgetKind::Signal(el) => el.take(data),
            WidgetKind::Custom(el) => el.bulk(data),
            _ => false,
        }
    }

    /// **What this widget's claimed GPU slot is fed**, when it has something
    /// new for it.
    ///
    /// A built-in answers from its variant, an element for itself
    /// ([`Element::fill`](super::Element::fill)) — the single door, so the
    /// front's upload walk asks the tree what to upload instead of deriving it
    /// from what each kind happens to be.
    pub fn fill(&mut self) -> Option<super::element::SlotFill> {
        match self {
            WidgetKind::Signal(el) => el.fill(),
            WidgetKind::Custom(el) => el.fill(),
            _ => None,
        }
    }

    /// **The window's GPU slots are gone** (a device rebuilt, a canvas
    /// re-attached): whatever this widget handed over has to be handed over
    /// again.
    pub fn slot_dropped(&mut self) {
        match self {
            WidgetKind::Signal(el) => el.slot_dirty = true,
            WidgetKind::Custom(el) => el.slot_dropped(),
            _ => {}
        }
    }

    /// Recomputes a stored spectrum's cached analysis from its current samples
    /// and props — a no-op for every other widget and every other
    /// presentation. Called at the element's mutation points (parse, a bulk
    /// load landing samples, a live `/gui_set` touching what the analysis
    /// reads), which keeps the per-frame render pure and allocation-light.
    pub fn refresh_analysis(&mut self) {
        if let WidgetKind::Signal(el) = self {
            el.refresh_analysis();
        }
    }
}

impl Widget {
    /// The signal element this widget draws with: its own, or — for a `clip` —
    /// the **take** among its bodies.
    ///
    /// A clip's bodies carry no id, so everything that resolves a widget by id
    /// and then wants its samples (a bulk load landing, a buffer fetch coming
    /// back) lands on the clip and reaches the take through here. That is the
    /// containment stated once: a body's id *is* its container's.
    pub fn signal_target(&self) -> Option<&SignalElement> {
        match &self.kind {
            WidgetKind::Signal(el) => Some(el),
            WidgetKind::Clip { .. } => self.children.iter().find_map(|c| match &c.kind {
                WidgetKind::Signal(el) => Some(&**el),
                _ => None,
            }),
            _ => None,
        }
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

    /// Hands a loaded resource to whichever of this widget or its bodies takes
    /// it, returning whether one did.
    pub fn take_bulk(&mut self, data: super::element::Loaded) -> bool {
        if self.kind.take_bulk(data) {
            return true;
        }
        false
    }

    /// [`signal_target`](Self::signal_target), mutably — the door a bulk load
    /// writes its samples or its pyramid through.
    pub fn signal_target_mut(&mut self) -> Option<&mut SignalElement> {
        match &mut self.kind {
            WidgetKind::Signal(el) => Some(el),
            WidgetKind::Clip { .. } => self.children.iter_mut().find_map(|c| match &mut c.kind {
                WidgetKind::Signal(el) => Some(&mut **el),
                _ => None,
            }),
            _ => None,
        }
    }

    /// **What a gesture has changed on this widget** — its own kind's
    /// ([`WidgetKind::info`]) plus, for a `clip`, its **bodies'**.
    ///
    /// The reader's half of the routing `apply_widget` does for writes, and for
    /// the same reason: a body carries no id, so a script addresses the clip
    /// and the prop that answers is whichever body owns it. A curve edited on a
    /// lane reports its points through the clip that holds it.
    pub fn info(&self) -> Vec<(String, Value)> {
        let mut out = self.kind.info();
        if matches!(self.kind, WidgetKind::Clip { .. }) {
            for body in &self.children {
                out.extend(body.kind.info());
            }
        }
        out
    }

    /// The body filling `role` among a clip's children, mutably — the door a
    /// `/gui_set` of a body prop and an edit-back both write through.
    pub(crate) fn clip_body_mut(&mut self, role: BodyRole) -> Option<&mut WidgetKind> {
        self.children
            .iter_mut()
            .map(|c| &mut c.kind)
            .find(|k| k.body_role() == Some(role))
    }

    /// Adds the body `role` names to this clip when it has none yet, empty, so
    /// a `/gui_set` that introduces a body has somewhere to land. Layering
    /// order is the role's own ([`BodyRole`]), and a body added later keeps it:
    /// an envelope set on a clip that already has a take is drawn *over* it,
    /// which is the whole point of the bodies being a composition.
    pub(crate) fn ensure_body(&mut self, role: BodyRole) {
        if !matches!(self.kind, WidgetKind::Clip { .. }) || self.clip_body(role).is_some() {
            return;
        }
        let Some(kind) = build::empty_clip_body(role) else {
            return;
        };
        let at = self
            .children
            .iter()
            .position(|c| c.kind.body_role() > Some(role))
            .unwrap_or(self.children.len());
        self.children.insert(at, build::body_widget(kind));
    }

    /// This widget's own kind when it fills `role`, else the body filling it
    /// among its children. The reader's half of the routing `apply_widget`
    /// does for writes: an edit-back payload asks the widget it was addressed
    /// to, and a clip answers with the body that owns the data.
    pub(crate) fn kind_or_body(&self, role: BodyRole) -> Option<&WidgetKind> {
        if self.kind.body_role() == Some(role) {
            return Some(&self.kind);
        }
        self.clip_body(role)
    }

    /// The body filling `role` among a clip's children.
    pub(crate) fn clip_body(&self, role: BodyRole) -> Option<&WidgetKind> {
        self.children
            .iter()
            .map(|c| &c.kind)
            .find(|k| k.body_role() == Some(role))
    }
}
