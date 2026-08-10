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
    /// a window of its own ([`WidgetKind::freq_nav`]).
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

    /// The signal element this widget is, when it navigates its **own**
    /// frequency axis rather than the window's time — the one element that
    /// carries an x window instead of joining a navigation group.
    pub fn freq_nav(&self) -> Option<&SignalElement> {
        self.signal().filter(|el| el.navigates_freq())
    }

    /// The current value as an OSC primitive for a `/gui_event`, or `None` for a
    /// non-interactive widget. A `button` reports `1` (it is momentary; the press
    /// is the event).
    pub fn event_value(&self) -> Option<OscType> {
        match self {
            WidgetKind::Text { value, .. } => Some(OscType::String(value.clone())),
            WidgetKind::Custom(el) => el.value(),
            _ => None,
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
            ..Default::default()
        };
        self.audio_buses_read(&mut needs.taps);
        needs
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
