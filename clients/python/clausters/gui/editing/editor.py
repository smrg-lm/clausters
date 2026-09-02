"""The editor: what orchestrates a picture, a vocabulary and a history.

`Editor` edits **one structure** — a buffer's samples, a break-point curve, a
timeline of events — and it imports nothing from the arrangement. What makes
that possible is that it performs almost nothing itself: it opens a window
through a `clausters.gui.editing.View`, turns a gesture into a payload through a
`clausters.gui.editing.Domain`, answers the host through a
`clausters.gui.editing.Echo`, and records what happened in the
`clausters.gui.editing.Editing` context the **data** owns rather than one of its
own.

So the boundaries are:

- an editor owns **neither the data nor the history**. It asks the structure for
  its context (`Editing.of`) and never builds one, which is what makes two
  windows over one thing walk one undo order;
- **how an edit inverts is the crate's** (`history::Editable`), reached through
  the domain — never re-derived here, and never twice per language;
- **what a number is measured in is the editor's**: the unit bridge (beats and
  seconds ↔ timeline samples) is here because it is the same bridge for every
  structure, and a view that computed its own would be a second answer.

`clausters.gui.editing.FormEditor` is this class plus what only a tree has: a
held document, several views of one composition, the lanes and clips, and a
transport. **Transport and render are not here** — a bare structure at most
sounds; it has no piece to move over.
"""

import itertools

from ... import _native
from ...base.time import TempoMap
from .context import FIRST_VERSION, Editing
from .echo import Echo

#: The tags that are **not** edits: what a view is looking at, and where the
#: hand is. They are answered generically and never reach a domain, because the
#: crate is explicit that screen state is never part of what is edited.
NOT_AN_EDIT = ("selection", "view", "view_x", "view_y", "layer", "focus",
               "locate", "height")


def _resolve_host(host):
    """The host an ``open`` acts on: the one named, else the ambient one — the
    same resolution `clausters.gui.guidef.View.open`, `clausters.plot` and
    `clausters.scope` share, so an editor is not the one resource that has to be
    handed a host."""
    if host is not None:
        return host
    from ...plot import _ambient_host

    return _ambient_host()


class Editor:
    """One structure on screen, editable back into it.

    Args:
        structure: what is edited — whatever the ``domain`` and the ``view``
            understand.
        sample_rate: the engine's sample rate; with ``tempo`` it fixes the
            data↔timeline-samples conversion.
        tempo: the clock's tempo in **beats per second** (the `TempoClock`
            convention — 2.0 is 120 bpm).
        tempo_map: the piece's beat→second map, when the tempo changes along it.
        domain: the `clausters.gui.editing.Domain` this structure's payloads are
            written in.
        view: the `clausters.gui.editing.View` that draws it.
        context: the editing context to register in. ``None`` — the ordinary
            case — asks the structure for its own, which is what makes two
            windows over one thing share an undo order.
        title: the window title.
        base_id: the first widget id a **host-less** draw counts from (tests and
            tree inspection). Once `open`ed, the ids come from the host's own
            recycling pool instead, so the two never collide and a redraw's ids
            return to the pool.
    """

    def __init__(self, structure=None, *, sample_rate: float, tempo: float = 1.0,
                 tempo_map=None, domain=None, view=None, context=None,
                 title: str = "Editor",
                 width: int = 1000, height: int = 520, base_id: int = 10_000):
        #: What is edited. `FormEditor` calls it `element`, which is the
        #: arrangement's word for the same slot.
        self.structure = structure
        self.sample_rate = float(sample_rate)
        #: The piece's beat->second map (`clausters.base.TempoMap`) — the whole
        #: of the beat side of the unit bridge. Given one, the editor draws
        #: against the same function the clock plays by, which is what makes the
        #: line and the sound agree across a tempo change; given only a
        #: ``tempo``, it is that tempo as a single segment, which is exactly the
        #: affine ratio this bridge always was.
        self.tempo_map = (
            tempo_map.copy() if tempo_map is not None else TempoMap(float(tempo))
        )
        self.title = title
        self.size = (int(width), int(height))
        #: The vocabulary this structure's edits are written in, and the picture
        #: of it. Both are per-structure and neither is per-window: what a
        #: number means and what the thing looks like do not change because a
        #: second window opened.
        self.domain = domain
        self.view = view
        #: The last selection swept in this editor's windows, as the crate's
        #: ``Selection``. It is a plain value and not part of what is edited,
        #: which is the crate's own line: a selection is screen state, never
        #: persisted and never logged.
        self.selection: dict = {}
        #: The base a host-less draw counts ids from; once `open`ed the ids come
        #: from the host's recycling pool instead (`_new_id`).
        self._base_id = int(base_id)
        self._fallback_ids = itertools.count(self._base_id)
        #: The version this editor was at when it last answered a host event --
        #: what turns "the version moved" into "it moved *by someone else*".
        self._applied: int = FIRST_VERSION
        self._window = None
        #: The context to register in when the caller named one; otherwise the
        #: structure's own, asked for on each use.
        self._context = context
        #: This view's end of the acknowledgement protocol — the stamp, the
        #: floor, the corrections and the reason. It reads the version out of
        #: the context rather than keeping one, because two windows over one
        #: structure report one counter.
        self._echo = Echo(host=None, version=lambda: self._version)
        #: The identity this structure was registered in the history under,
        #: minted on the first edit — a structure you built has no id and is not
        #: going to be given a stable one for this.
        self._structure_id = None
        #: Whether the data changed since the last render.
        self.dirty = False

    # ---- the unit bridge: the data ↔ timeline samples ----

    @property
    def tempo(self) -> float:
        """The tempo the piece **starts** at, in beats per second.

        A reading of `tempo_map`, not a second copy of it: under one tempo it is
        the tempo, and under a tempo that changes it is the first segment's.
        Assigning it replaces the map with that single tempo, which is what
        setting a grid does.
        """
        return self.tempo_map.tempo_at(0.0)

    @tempo.setter
    def tempo(self, tempo: float):
        self.tempo_map = TempoMap(float(tempo))

    @property
    def units_per_beat(self) -> float:
        """Timeline samples in the **first** beat — the nominal ratio of the
        data↔view bridge. One timeline unit is one audio sample, so a take
        placed at its own frame count sits 1:1 on the axis.

        It is a ratio at a position, not a constant: under a tempo that changes,
        a later beat is a different number of samples wide, which is why
        `beats_to_units` takes a position and this only names the origin.
        """
        return self.beats_to_units(1.0) - self.beats_to_units(0.0)

    def beats_to_units(self, beats: float) -> float:
        """Beats → timeline samples, through the piece's time map (and the
        core's seconds→samples rounding every client shares).

        The axis is real time, so this is where a beat stops being a logical
        coordinate: a beat after a tempo change lands on the second it actually
        falls on, which is the second the clock will play it at.
        """
        secs = self.tempo_map.secs_at(float(beats))
        return float(_native.secs_to_samples(secs, self.sample_rate))

    def units_to_beats(self, units: float) -> float:
        """Timeline samples → beats: the inverse the edit-back path takes to turn
        a dragged clip back into a placement."""
        secs = _native.samples_to_secs(int(round(units)), self.sample_rate)
        return self.tempo_map.beats_at(secs)

    @property
    def units_per_second(self) -> float:
        """Timeline samples per second — the axis *is* samples, so this is the
        engine's sample rate. It is the other half of the bridge: a length in
        seconds (a take's) crosses on this one, and only an onset crosses on
        `units_per_beat`."""
        return self.sample_rate

    def secs_to_units(self, secs: float) -> float:
        """Seconds → timeline samples: what a length of recorded audio is
        drawn with, since its seconds were fixed before any tempo was."""
        return float(_native.secs_to_samples(float(secs), self.sample_rate))

    def units_to_secs(self, units: float) -> float:
        """Timeline samples → seconds: the inverse, for an edit-back that
        resized something measured in seconds."""
        return _native.samples_to_secs(int(round(units)), self.sample_rate)

    # ---- widget ids: the host's recycling pool, or a host-less fallback ----

    def _new_id(self) -> int:
        """A widget id for the tree being drawn. Once `open`ed, it comes from the
        host's recycling pool (`clausters.gui.host.GuiHost.alloc_id`); host-less
        (a test, or inspecting `draw`), it counts from ``base_id``."""
        return self._host.alloc_id() if self._host is not None else next(self._fallback_ids)

    def _reset_ids(self):
        """Start a fresh draw's id numbering. Host-less, the fallback counter
        restarts at ``base_id``; on a host nothing resets — the ids come from its
        pool, and re-defining the window returns the previous tree's ids there
        (`GuiHost.define`), so the churn recycles instead of climbing."""
        if self._host is None:
            self._fallback_ids = itertools.count(self._base_id)

    # ---- the acknowledgement, delegated to the `Echo` ----

    @property
    def _host(self):
        """The host this editor answers, or ``None`` before it is opened."""
        return self._echo.host

    @_host.setter
    def _host(self, host):
        self._echo.host = host

    @property
    def _corrections(self) -> list:
        return self._echo.corrections

    @_corrections.setter
    def _corrections(self, value):
        self._echo.corrections = list(value)

    @property
    def _floor(self) -> int:
        return self._echo.floor

    @_floor.setter
    def _floor(self, value):
        self._echo.floor = int(value)

    @property
    def _reason(self) -> "str | None":
        return self._echo.reason

    @_reason.setter
    def _reason(self, value):
        self._echo.reason = value

    def _announce(self):
        self._echo.announce()

    def _stale(self, against: int) -> bool:
        return self._echo.stale(against)

    def _correct(self, widget_id: int, **props):
        self._echo.correct(widget_id, **props)

    def _acknowledge(self, seq: int, reason: "str | None" = None):
        self._echo.acknowledge(seq, reason)

    # ---- the history: the data's, not this editor's ----

    @property
    def _editing(self) -> Editing:
        """The structure's editing context — its history, and the views over it.

        Reached through the **data**, so a second window gets the same one. That
        is the whole of what makes an undo in either view update both, and it is
        why none of this is a field here: a history belongs to the data, never
        to a view.
        """
        if self._context is not None:
            return self._context
        return Editing.of(self.structure)

    @property
    def _version(self) -> int:
        """The version — the counter the host names back on its next gesture."""
        return self._editing.version

    @_version.setter
    def _version(self, value: int):
        self._editing.version = int(value)

    def _registered(self) -> int:
        """This structure's identity in the history, minted on first use.

        Asked of the **context** rather than kept here, so two windows over one
        structure name one identity: the pile is one order over the data, not
        one per view.
        """
        if self._structure_id is None:
            self._structure_id = self._editing.identity(
                self.structure, getattr(self.domain, "name", "") or "")
        return self._structure_id

    # ---- the forward draw ----

    def draw(self) -> dict:
        """The structure as a ``window``-rooted GuiDef. Pure — it builds the
        tree and the view's registry, and sends nothing."""
        if self.view is None:
            raise RuntimeError("this editor has no view to draw with")
        self._reset_ids()
        return self.view.draw(self)

    def open(self, host=None, id: "int | None" = None):
        """`draw` the structure and open it on ``host`` (a
        `clausters.gui.host.GuiHost`), or on the **ambient** host when none is
        named — the same rule `clausters.gui.guidef.View.open`, `clausters.plot`
        and `clausters.scope` follow.

        Returns the **window handle** `clausters.gui.host.GuiHost.open` hands
        back: it equals the window id, and it also resolves the tree's named
        widgets."""
        self._host = _resolve_host(host)
        self._window = self._host.open(self.draw(), id=id)
        self._editing.attach(self)
        self._announce()
        return self._window

    # ---- the edit-back ----

    def apply(self, addr: str, args) -> bool:
        """Apply one message from the host to the structure, and **answer it**.
        Returns whether the data changed.

        **Every other window over this structure is told**, on the way out.
        Nothing else would do it: an acknowledgement goes to the window whose
        gesture it answered, so a second view would go on drawing something that
        moved under it — and the shared history would then step an order one of
        its windows could not see.
        """
        with self._editing.turn(self):
            changed = self._deliver(addr, args)
            if changed:
                self._editing.changed()
            return changed

    def _deliver(self, addr: str, args) -> bool:
        """`apply`, without the turn around it: what the message actually
        does."""
        if addr == "/gui_closed":
            if not args or self._window is None or int(args[0]) == self._window:
                self._window = None
                # Closing a *view* is not an event of the history, so the
                # context stays exactly as it is -- what goes is this window's
                # place in the list of who to tell.
                self._editing.detach(self)
            return False
        if addr != "/gui_event" or len(args) < 3:
            return False
        # ``<id> <seq> <version> <tag> <payload…>``: the stamp and the version
        # the gesture was made against are the second and third arguments of
        # every event. The stamp is what an acknowledgement names, so it is read
        # here and answered below -- an owner that applies an edit and says
        # nothing leaves the host drawing what the hand did.
        seq, against = int(args[1]), int(args[2]) if len(args) > 2 else 0
        args = (args[0], *args[3:])
        self._corrections = []
        self._reason = None
        # The window's own shortcuts (Ctrl+Z / Ctrl+Shift+Z), which the host
        # addresses to the **window** rather than to a widget: undo is not
        # aimed at anything under the cursor. They are answered here rather
        # than routed, because a history step is not an edit to the data -- it
        # is a walk through the one the crate keeps.
        if args[1] in ("undo", "redo") and int(args[0]) == self._window:
            # **What it answers is whether anything moved**, not whether the
            # keystroke was understood. A history at its end is the ordinary
            # case -- a person holds Ctrl+Z until it stops -- and reporting a
            # change there told every other view to bring itself in step with an
            # edit that never happened, which is a redraw for nothing. The
            # acknowledgement still goes out: the host asked, and the answer is
            # the state that holds.
            stepped = (self.redo if args[1] == "redo" else self.undo)()
            self._acknowledge(seq)
            return stepped
        # Only what this editor draws is this editor's to answer. A poll loop
        # may be shared with a second editor, and answering for its window would
        # retire a pending edit nobody applied -- the host would adopt a picture
        # the real owner never saw.
        if not self._owns(int(args[0])):
            return False
        # **The answers lag, and that is not a conflict.** A host stamps every
        # event with the version it was last told, and it is told only when an
        # acknowledgement reaches it -- a round trip a hand outruns, so an edit
        # naming a version this editor has already moved past is the ordinary
        # case, not a collision: a drag reporting as it goes, or a second
        # gesture begun inside one round trip. Refusing those refuses the hand,
        # and answers each with a resync that snaps the picture back. So only a
        # route the host knows nothing about raises the floor: the version moved
        # since the last event was answered, and no event is what moved it.
        if self._version != self._applied:
            self._floor = self._version
        if self._stale(against):
            # The data moved under the gesture, by a route no gesture produced.
            # The edit is not applied and not merged: an edit-back payload is
            # absolute *and* whole (a roll's notes are the list, not a diff), so
            # applying one made against an older picture would silently drop
            # whatever arrived in between. What goes back is the state as it
            # stands, which the host adopts exactly as it adopts a snap.
            self._resync(int(args[0]))
            self._acknowledge(seq, reason="the composition changed since this edit")
            return False
        changed = self._route(args)
        self._applied = self._version
        # Answered whatever happened, and answered with a *value*. There is no
        # success flag: the state this editor decided rides as the corrections
        # `_route` collected, and a refusal is simply the previous value among
        # them. Applied, transformed and refused are one message.
        self._acknowledge(seq, reason=self._reason)
        # ...and *then* the redefine, when the gesture added or removed a
        # member: the answer retires what the host had in flight, and the new
        # tree is what shows a widget that was not there.
        self._restructure()
        return changed

    def _owns(self, widget_id: int) -> bool:
        """Whether this editor drew the widget an event names."""
        return self.view is not None and self.view.owns(widget_id)

    def _route(self, args) -> bool:
        """One `/gui_event` payload onto the structure, with the stamp already
        taken off. Returns whether the data changed; `apply` is what answers the
        host.

        The tags that are not edits are answered here and never reach the
        domain; everything else is the domain's to read, and a tag it does not
        recognize is nothing rather than an error — a view emits what it can do,
        and not all of it is an edit of this structure.
        """
        wid, tag, values = int(args[0]), str(args[1]), args[2:]
        if tag in NOT_AN_EDIT:
            return self._observe(wid, tag, values)
        if self.domain is None:
            return False
        payload = self.domain.payload(self.structure, tag, values)
        if payload is None:
            return False
        return self._edit(payload, self.domain.label(payload))

    def _observe(self, wid: int, tag: str, values) -> bool:
        """A tag that says what the view is looking at rather than what changed.

        Nothing here reaches a history: the crate is explicit that a selection,
        a zoom and which layer the hand is on are never part of what is edited.
        The selection is still kept **typed**, because it is the value an
        operation is handed.
        """
        if tag == "selection":
            self.selection = {"widget": int(wid),
                              "start": self.units_to_beats(float(values[0])) if values else 0.0,
                              "len": (self.units_to_beats(float(values[1]))
                                      if len(values) > 1 else 0.0)}
        return False

    def _edit(self, payload: dict, label: str, *, coalesce: bool = False) -> bool:
        """Apply one payload to the structure and record how to put it back.

        The inverse is read **before** the edit lands (`Domain.current`), which
        is the whole reason this is one call: a surface that let you apply first
        and record second would let you record the wrong thing. A payload the
        structure was already at is applied by nobody and recorded by nobody —
        a resend is not an edit.
        """
        if self.domain is None:
            return False
        before = self.domain.current(self.structure, payload)
        if not self.domain.project(self.structure, payload):
            return False
        if before is not None:
            self._editing.history.record(
                [{"structure": self._registered(),
                  "forward": {"edit": payload},
                  "backward": before,
                  "key": self.domain.coalesce_key(payload)}],
                label=label, coalesce=coalesce)
        self._version += 1
        self.dirty = True
        self._editing.moved({"structure": self._registered(), "payload": payload})
        return True

    def _resync(self, widget_id: int):
        """Hand back what the widget should be drawing, without applying
        anything: the answer to an edit that arrived too late."""
        if self.view is None:
            return
        props = self.view.props(self, widget_id)
        if props:
            self._correct(widget_id, **props)

    def _restructure(self) -> bool:
        """Redefine the window when the last edit changed **which widgets
        exist**, and say whether it did. A structure edited in place changes
        none, which is why this is nothing here and something in `FormEditor`.
        """
        return False

    def adopt(self, intents: list, whole: bool) -> None:
        """Another view of this structure edited it: bring this window in step.

        A window that is not open has nothing to bring in step, and says so by
        doing nothing.
        """
        if self._host is None or self._window is None:
            return
        self._corrections = []
        for wid in list(getattr(self.view, "widgets", {})):
            self._resync(wid)
        self._acknowledge(0)
        self._corrections = []

    # ---- the history walk ----

    def undo(self) -> bool:
        """Step back one edit, and tell the host what to draw instead.

        The inverse is an ordinary payload, so undoing needs no second path: it
        is the projection again, on what the crate hands back. Returns whether
        anything was undone.

        Every **other** window over this structure is told, the way it is told
        about an edit: one history, and an undo in either view updates both."""
        with self._editing.turn(self):
            stepped = self._step("undo")
            if stepped:
                self._editing.changed()
            return stepped

    def redo(self) -> bool:
        """Step forward again after `undo`. Returns whether anything was
        redone."""
        with self._editing.turn(self):
            stepped = self._step("redo")
            if stepped:
                self._editing.changed()
            return stepped

    def _step(self, direction: str) -> bool:
        """One step of the pile, projected leg by leg.

        The history holds structures the crate cannot reach, so it applies
        nothing: what comes back is an ordered list of legs, and it is the
        editor that hands each to the domain that owns it. A leg naming a
        structure this editor does not hold is left alone — another view of the
        same context owns it, and one pile over several structures is the point.
        """
        history = self._editing.history
        if history is None:
            return False
        step = history.undo() if direction == "undo" else history.redo()
        if step is None:
            return False
        legs = (step.get("inverses") if direction == "undo"
                else step.get("edits")) or []
        mine = self._registered()
        applied = False
        for leg in legs:
            if int(leg.get("structure", -1)) != mine:
                continue
            payload = leg.get("payload")
            if isinstance(payload, dict) and self.domain is not None:
                applied |= bool(self.domain.project(self.structure, payload))
        if not applied:
            return False
        self._version += 1
        self.dirty = True
        self._corrections = []
        for wid in list(getattr(self.view, "widgets", {})):
            self._resync(wid)
        self._acknowledge(0)
        self._corrections = []
        return True

    @property
    def can_undo(self) -> bool:
        """Whether there is an edit to step back over."""
        history = self._editing.history
        return history is not None and history.can_undo

    @property
    def can_redo(self) -> bool:
        """Whether there is an undone edit to step forward into."""
        history = self._editing.history
        return history is not None and history.can_redo

    @property
    def undo_label(self) -> "str | None":
        """What an undo would be called, for a menu item."""
        history = self._editing.history
        return None if history is None else history.undo_label

    @property
    def redo_label(self) -> "str | None":
        """What a redo would be called, for a menu item.

        The pair of `undo_label`, and it stops being decoration the moment a
        second window is open: with one pile over all of them, a label is how a
        person knows which edit a keystroke is about to move — and both windows
        read the same one.
        """
        history = self._editing.history
        return None if history is None else history.redo_label

    @property
    def window(self):
        """The open window's id, or ``None``."""
        return self._window

    def poll(self, timeout: float = 0.0) -> bool:
        """Drain the host's pending messages into the structure (`apply` each)
        **and on to the window's own handlers**. Returns whether the data
        changed. Call it from the script's loop — **never** from the clock
        thread, which a routine must never block.

        The second half is why a window may carry both: a panel beside the
        editor is the script's, addressed to widgets this editor never drew, and
        its `clausters.gui.handle.WidgetHandle.on_event` callbacks run here
        because this is the loop that took its message off the socket. A drain
        that only fed the data swallowed them — the button was pressed, the host
        reported it, and nothing happened.
        """
        if self._host is None:
            raise RuntimeError("open(host) the editor first")
        changed = False
        while (msg := self._host.poll(timeout)) is not None:
            changed |= self.apply(*msg)
            self._host.dispatch(*msg)
            timeout = 0.0  # only the first wait blocks
        return changed
