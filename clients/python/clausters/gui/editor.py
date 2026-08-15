"""`Editor`: the bridge between the arrangement and the multitrack GUI.

The driver of the DAW-style view. It draws a `clausters.form` tree as a
multitrack `GuiDef` (tracks of clips on one shared time axis), applies the clip
edit-backs the host sends straight onto the arrangement, and re-renders it — the
loop **data ↔ graphic ↔ sound**, which is what makes the composition editable at
any granularity rather than merely displayable.

Three things are worth knowing about how it is built.

**The dependency arrow points this way.** `clausters.form` stays pure and
transport-agnostic; the editor imports the arrangement, never the reverse. This
module is the only one that knows both worlds.

**Beats meet samples here.** The arrangement places elements in *beats*; the
multitrack view places clips in *timeline samples*, because a clip's body is
audio data and its sample 0 sits at the clip's offset. The editor is the only
converter: one beat is `sample_rate / tempo` timeline units, so an audio take
placed at its own length sits 1:1 on the axis. A musical `quant` becomes the
lane's drag grid, so the grid a clip is dropped on is the grid the arrangement
re-schedules on. The arithmetic itself is the core's (`beats_to_secs` →
`secs_to_samples`), not a second implementation.

**One mapping rule, not a heuristic per case.** The root `Group`'s members are
the *lanes*; a lane's members are its *clips*; a `Buffer` clip draws its take, a
element of events draws a piano-roll, and a nested `Group` draws as a labeled
rectangle — its summary — until it is `expand`ed into lanes of its own. That
collapse/expand *is* the arrangement's base level (the zoom that summarizes a
group or resolves it), so it needs no protocol of its own.
"""

import itertools

from .. import _native
from .handle import WindowHandle
from ..form.document import FIRST_VERSION, ID_ATTR, to_document
from ..form.group import CONCRETE, LOGICAL, SIMULTANEOUS, Group
from ..form.element import Buffer, Element
from ..defs.ugens import points_to_env
from ..form.render import flatten
from ..seq.automation import Automation
from ..seq.event import Event as SeqEvent
from ..seq.timeline import MidiEvent, OscEvent, Timeline
from .guidef import (_flat_notes, clip, patch, pianoroll, scroll, timeruler,
                     track, waveform, window)
from .transport import Transport

__all__ = ["Editor"]

#: The pitch range a piano-roll lane falls back to when its notes give none
#: (C3..C6 — the span a melodic line usually lives in).
DEFAULT_PITCH = (48.0, 72.0)
#: Semitones of headroom above and below the notes of a piano-roll clip.
PITCH_PAD = 2.0


class _Placed:
    """What a clip widget was drawn from: the placement it shows (``owner`` group
    and ``member`` handle, the arrangement's stable identity), the ``base`` in beats its
    group sits at (a clip's offset is absolute on the shared axis, a placement is
    relative to its group — this bridges the two), and the ``offset``/``dur`` in
    timeline units it was drawn with (so an edit-back can tell what actually
    moved)."""

    __slots__ = ("owner", "member", "base", "offset", "dur")

    def __init__(self, owner, member, base, offset, dur):
        self.owner = owner
        self.member = member
        self.base = float(base)
        self.offset = float(offset)
        self.dur = float(dur)


class Editor:
    """A composition on screen: the arrangement tree drawn as a multitrack view,
    editable back into the tree.

    Args:
        element: the composition — a `clausters.form.group.Group` (its members
            become the lanes) or any single `Element` (one lane).
        sample_rate: the engine's sample rate; with ``tempo`` it fixes the
            beats↔timeline-samples conversion.
        tempo: the clock's tempo in **beats per second** (the `TempoClock`
            convention — 2.0 is 120 bpm).
        quant: the musical drag grid in beats (``0.25`` = a sixteenth); ``0``
            snaps to whole samples.
        follow: re-render on every edit (the live editor).
        extra: extra GuiDef nodes to place under the lanes (a transport panel,
            say). Their events are not the editor's: `apply` ignores them, so a
            script can handle them itself.
        title: the window title.
        base_id: the first widget id a **host-less** draw counts from (tests and
            tree inspection). Once `open`ed, the editor draws ids from the host's
            own recycling pool instead, so the two never collide and a redraw's
            ids return to the pool.

    Usage::

        editor = Editor(song, sample_rate=server.sample_rate, tempo=clock.tempo,
                        quant=0.25)
        editor.open(gui)              # draw and open the window
        editor.apply(*gui.poll())     # a dragged clip moves the element
        editor.render(server, clock)  # play the edited composition
    """

    def __init__(self, element, *, sample_rate: float, tempo: float = 1.0,
                 quant: float = 0.0, follow: bool = False, extra=(),
                 title: str = "Composition",
                 width: int = 1000, height: int = 520, base_id: int = 10_000):
        self.element = element
        self.sample_rate = float(sample_rate)
        self.tempo = float(tempo)
        self.quant = float(quant)
        #: Re-render on every edit (the *live editor*: drag a clip and hear it
        #: where you dropped it). Off by default — an edit then only changes the
        #: arrangement, and `rerender` decides when it is heard.
        self.follow = bool(follow)
        #: Widgets appended to the window after the lanes (a transport panel, a
        #: readout). They are the script's — the editor never touches their ids,
        #: so keep them clear of `base_id`.
        self.extra = list(extra)
        self.title = title
        self.size = (int(width), int(height))
        #: The base a host-less draw counts ids from; once `open`ed the ids come
        #: from the host's recycling pool instead (`_new_id`).
        self._base_id = int(base_id)
        self._fallback_ids = itertools.count(self._base_id)
        #: The elements shown as lanes of their own instead of a summary clip
        #: (the base level: a group resolved rather than collapsed).
        self._expanded: set[int] = set()
        #: widget id -> `_Placed` — where the clip came from in the arrangement and
        #: what was drawn for it, which is what an edit-back writes through.
        self._clips: dict = {}
        #: widget id -> element, for every lane (a `/gui_set` of the lane chrome
        #: — the playhead — addresses these).
        self._lanes: dict = {}
        #: The last selection swept in this editor's windows, as the crate's
        #: ``Selection`` — ``{"start", "len"}`` in **beats**, plus ``"value"``
        #: where the sweep restricted the value axis and ``"nodes"`` where it
        #: named an element rather than the shared time axis. Empty until one is
        #: swept; `resolve_selection` says what is under it.
        #:
        #: It is a plain value and not part of the composition, which is the
        #: crate's own line: a selection is screen state, never persisted and
        #: never logged, and what this holds is the *reading* of it that can be
        #: handed to an operation.
        self.selection: dict = {}
        #: The view: the multitrack (`open`), a dedicated piano-roll of one
        #: events element (`open_pianoroll`) or a dedicated signal view of one
        #: rendered element (`open_signal`). `render` dispatches on it.
        self._mode = "multitrack"
        #: The element the dedicated piano-roll draws (its notes editable when it
        #: is a `Track`).
        self._roll_element = None
        #: The element the dedicated signal view draws.
        self._signal_element = None
        #: What the dedicated signal view measures (`open_signal`), as a tuple
        #: of measure names. Assigning it on an open view is **live**: the
        #: measure is a `/gui_set` prop, so showing or hiding the level body
        #: costs one message and no rebuild (see the `layers` property).
        self._layers: tuple = ("peak", "rms")
        #: widget id -> the element whose samples that widget draws. The signal
        #: view's own registry, the sibling of `_rolls`: a selection swept there
        #: is a selection *of that element*, not of the shared axis.
        self._signals: dict = {}
        #: widget id -> the element whose notes that widget draws, for the note
        #: edit-back route: the dedicated roll, and every clip with a roll body
        #: (a body carries no id, so its notes arrive tagged with the clip's).
        self._rolls: dict = {}
        #: What the host should be drawing instead of what it drew, collected
        #: while one event is routed and sent with its acknowledgement.
        self._corrections: list = []
        #: Why the last routed event did not do what it asked, if it did not.
        self._reason: "str | None" = None
        #: The composition's version — the document half of the two counters.
        #: It moves on every edit this editor applies **and on every redefine**,
        #: and it rides on each acknowledgement so the host can name it back on
        #: the next gesture. One rather than zero, because zero is what an event
        #: means by *unstated*.
        self._version = FIRST_VERSION
        #: The crate's undo log, and the document it inverts — created on the
        #: first edit, because a composition that is only looked at needs
        #: neither. **The history is the crate's, not this editor's**: a log
        #: kept here would see only the gestures this editor made, so a script
        #: editing the arrangement or a second view would leave it describing a
        #: composition that has moved on, and undo would then write a state
        #: nobody was ever in.
        self._log = None
        self._document = None
        #: node id -> the arrangement object an intent naming it writes to. Built
        #: with the document, since `to_document` is what stamps the ids.
        self._by_node: dict = {}
        #: patch widget id -> (logical `Group`, its box-order member handles) —
        #: the directed-patch view of a logical group, for its edit-back route.
        self._patches: dict = {}
        #: id(group) -> {box index: (x, y)} — a patch's box placements, presentation
        #: only (a logical group is a signal graph, so positions live here, not in
        #: the arrangement). Keyed by group identity, so they survive a redraw.
        self._patch_geometry: dict = {}
        self._host = None
        self._window = None
        #: The rendering in flight: where it went and on what clock — what
        #: `rerender` re-schedules after an edit.
        self._destination = None
        self._clock = None
        #: The transport driving the lanes' playhead: play / pause / stop /
        #: locate, and the position the next play starts from. The editor's own
        #: transport methods delegate to it; a script's loop calls its `update`.
        #: Its lanes are read on each use, so a redraw's new widgets get the line.
        self.transport = Transport(
            None, lambda: self._lanes, source=self._render_pass,
            tempo=self.tempo, sample_rate=self.sample_rate, extent=self.extent)
        #: Whether the arrangement changed since the last render — an edit does not
        #: interrupt what is playing, so a transport (play, a resume after pause, a
        #: seek) reads this to know it must re-read the composition.
        self.dirty = False

    # ---- the unit bridge: beats (the data) ↔ timeline samples (the view) ----

    @property
    def units_per_beat(self) -> float:
        """Timeline samples per beat — the whole of the data↔view unit bridge.
        One timeline unit is one audio sample, so a take placed at its own frame
        count sits 1:1 on the axis."""
        return self.beats_to_units(1.0)

    def beats_to_units(self, beats: float) -> float:
        """Beats → timeline samples, through the core's own time arithmetic (the
        seconds→samples rounding every client shares)."""
        secs = _native.beats_to_secs(self.tempo, 0.0, 0.0, float(beats))
        return float(_native.secs_to_samples(secs, self.sample_rate))

    def units_to_beats(self, units: float) -> float:
        """Timeline samples → beats: the inverse the edit-back path takes to turn
        a dragged clip back into a placement."""
        secs = _native.samples_to_secs(int(round(units)), self.sample_rate)
        return _native.secs_to_beats(self.tempo, 0.0, 0.0, secs)

    # ---- the base level: collapse (a summary rectangle) vs expand (lanes) ----

    def expand(self, element) -> "Editor":
        """Resolve a nested `Group` into lanes of its own (instead of the labeled
        rectangle that summarizes it). The arrangement's *base level*, made an edit."""
        self._expanded.add(id(element))
        return self

    def collapse(self, element) -> "Editor":
        """Summarize a nested `Group` back into one labeled rectangle."""
        self._expanded.discard(id(element))
        return self

    def is_expanded(self, element) -> bool:
        return id(element) in self._expanded

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

    # ---- the forward draw: the arrangement -> GuiDef ----

    def draw(self) -> dict:
        """The composition as a ``window``-rooted GuiDef: one `track` lane per
        member of the root group, each holding its members as clips on the shared
        time axis. Pure — it builds the tree and the id registry, and sends
        nothing.

        A **logical** group draws as a directed `patch` (a server patch, not
        a timeline lane): a box per member, its typed ports derived from the
        `SynthDef` the member wraps, cords from the members' shared internal-bus
        controls (`_logical_patch`). A member wrapping a bare def *name* draws
        port-less (its directions need the def object)."""
        if self._mode == "pianoroll":
            return self._draw_pianoroll()
        if self._mode == "signal":
            return self._draw_signal()
        self._reset_ids()
        self._clips = {}
        self._lanes = {}
        self._rolls = {}
        self._patches = {}
        self._signals = {}

        lanes: list = []
        root = self.element
        if isinstance(root, Group) and root.kind == CONCRETE:
            for member in root.handles:
                if isinstance(member.element, Group) and member.element.kind == LOGICAL:
                    lanes.append(self._patch_lane(member.element))
                else:
                    lanes += self._lanes_for(member.element, member.offset, root, member)
        elif isinstance(root, Group) and root.kind == LOGICAL:
            lanes.append(self._patch_lane(root))
        else:
            lanes += self._lanes_for(root, float(root.onset or 0.0), None, None)

        # One ruler under the stack (the DAW convention), as a **free-standing**
        # strip owning its own box: a lane's own `ruler` is reserved out of that
        # lane's height, so ruling the stack used to cost the bottom lane a
        # strip of itself. Un-linked, it joins the lanes' navigation group, so
        # its ticks stand over the samples they name.
        # A lane and a patch workspace are different containers on the wire, and
        # only the first has a time axis to rule: `track` builds the two-axis
        # `field`, `patch` the locked-scale `plane`.
        ruler = [timeruler(ruler="beats", sample_rate=self.sample_rate,
                           tempo=self.tempo)] if any(
            lane.get("type") == "field" for lane in lanes) else []
        return window(*lanes, *ruler, *self.extra, title=self.title,
                      w=self.size[0], h=self.size[1], layout="col")

    def _patch_lane(self, group) -> dict:
        """A logical group drawn as a directed `patch` inside a pan/zoom `scroll`
        workspace — a server patch among the timeline lanes. Registers the patch
        widget id so an edit-back resolves to the group it draws."""
        p, handles = _logical_patch(group)
        wid = self._new_id()
        self._patches[wid] = (group, handles)
        geometry = self._patch_geometry.get(id(group), {})
        content = (900.0, 700.0)
        view = patch(id=wid, **p.to_widget(geometry), label=_name(group),
                     x=0.0, y=0.0, w=content[0], h=content[1])
        return scroll(view, id=self._new_id(),
                      content_w=content[0], content_h=content[1])

    def open(self, host, id: int | None = None) -> "WindowHandle":
        """`draw` the composition and open it on ``host`` (a
        `clausters.gui.host.GuiHost`).

        Returns the **window handle** `clausters.gui.host.GuiHost.open` hands
        back: it equals the window id, and it also resolves the tree's named
        widgets, so the transport buttons are reachable by name
        (``win["play"].on_event(...)``)."""
        self._host = self.transport.host = host
        self._mode = "multitrack"
        self._window = host.open(self.draw(), id=id)
        self._announce()
        return self._window

    def load(self, element) -> None:
        """Point this editor at another composition, redrawing the window it
        already has.

        What a reopened session needs: `clausters.form.document.from_session`
        hands back an arrangement, and without this the only way to show it was
        to build a second editor and a second window. The node ids survive the
        file (`from_document` restores them), so the reopened tree is the same
        composition by identity and not merely by shape.

        **The history is dropped**, deliberately. Its inverses describe states
        of the session that just ended; keeping them would let an undo walk back
        into a composition the file does not contain, which is the one thing a
        history must never do. The transport keeps its position — where you were
        looking is not part of what was loaded.
        """
        self.element = element
        self._expanded.clear()
        self._patch_geometry.clear()
        if self._log is not None:
            self._log.close()
            self._log = None
        if self._document is not None:
            self._document.close()
            self._document = None
        self._by_node = {}
        self._version = FIRST_VERSION
        self.dirty = True
        if self._host is not None and self._window is not None:
            self._reset_ids()
            self._host.define(self._window, self.draw())
            self._announce()

    def _draw_pianoroll(self) -> dict:
        """The dedicated piano-roll view: one `pianoroll` widget drawing a single
        events element's MIDI notes (grid) and OSC events (lane), instead of a
        multitrack of clips. The notes ride the shared beats grid; the pitch
        window frames them (falling back to `DEFAULT_PITCH`). Pure — it builds the
        tree and the edit-back registry."""
        self._reset_ids()
        self._clips = {}
        self._lanes = {}
        self._rolls = {}
        self._signals = {}
        element = self._roll_element
        wid = self._new_id()
        notes = self._notes(element)
        osc = self._osc(element)
        body: dict = {}
        if notes:
            pitches = [n[2] for n in notes]
            body["min"] = min(min(pitches) - PITCH_PAD, DEFAULT_PITCH[1])
            body["max"] = max(max(pitches) + PITCH_PAD, DEFAULT_PITCH[0])
        snap = self.beats_to_units(self.quant) if self.quant > 0 else None
        # The roll is a lane (the playhead/cursor addresses these) and a roll (the
        # note edit-back resolves through these).
        self._lanes[wid] = element
        self._rolls[wid] = element
        roll = pianoroll(id=wid, notes=notes or None, osc=osc or None, ruler="beats",
                         tempo=self.tempo, sample_rate=self.sample_rate, snap=snap,
                         label=_name(element), **body)
        return window(roll, *self.extra, title=self.title,
                      w=self.size[0], h=self.size[1], layout="col")

    @property
    def layers(self) -> tuple:
        """What the dedicated signal view measures — `("peak", "rms")` for the
        editor's picture, `("peak",)` for the bare envelope.

        **Assigning it on an open view sends one message.** The measure is a
        live `/gui_set` prop, so the body appears and disappears over the peaks
        with the picture, the axis, the zoom, the selection and the playhead all
        exactly where they were. Redrawing for this would be the wrong tool
        twice over: a redefine rebuilds every widget (so a handler bound to one
        by name is left holding an id nobody answers to) and the window it
        redefines is reopened.
        """
        return self._layers

    @layers.setter
    def layers(self, measures) -> None:
        stack = tuple(_measure(m) for m in measures)
        if not stack:
            raise ValueError(f"a signal view measures something (one of "
                             f"{', '.join(MEASURES)})")
        self._layers = stack
        if self._mode == "signal" and self._host is not None and self._window is not None:
            for wid in self._signals:
                self._host.set(wid, measure=" ".join(stack))

    def _draw_signal(self) -> dict:
        """The dedicated signal view: the **editor-grade waveform** of a single
        rendered element's samples, instead of a multitrack of clips.

        It is one `clausters.gui.guidef.waveform` — the same heavy view a
        standalone take is shown in — and the stack of measures is a prop of it
        (``layers`` → ``measure``), not a pile of widgets. That is the shape the
        picture forces: every view of a signal paints its own field before it
        draws, so two of them on one rectangle are not layers — the second hides
        the first. Measuring twice into *one* body is also what makes the rest
        of it one thing: one axis, one ruler, one selection, one playhead, one
        upload of the samples.

        Pure — it builds the tree and the edit-back registry, and sends nothing.
        """
        self._reset_ids()
        self._clips = {}
        self._lanes = {}
        self._rolls = {}
        self._signals = {}
        element = self._signal_element
        body = self._material_of(element)
        wid = self._new_id()
        # The view is the editor's one target here: the playhead, the cursor
        # readout and `locate` address it as they address a lane, and the signal
        # registry is what makes a selection swept in it a selection *of this
        # element*.
        self._lanes[wid] = element
        self._signals[wid] = element
        view = waveform(**body, id=wid, label=_name(element),
                        measure=" ".join(self._layers),
                        ruler="time", sample_rate=self.sample_rate,
                        tempo=self.tempo)
        return window(view, *self.extra, title=self.title,
                      w=self.size[0], h=self.size[1], layout="col")

    def _material_of(self, element) -> dict:
        """The source props a signal view draws ``element``'s samples from, or a
        `ValueError` naming what is missing.

        **This is the generated/generator distinction, asked at the door.** A
        rendered element has material a view can address — a buffer the host
        fetches, decimates and navigates; a generator has none until it is
        rendered, and a window drawn over nothing is worse than a refusal that
        says what to do. It is the same question `open_pianoroll` answers by
        showing a bounced generator read-only, and it has a sharper answer here:
        notes can be bounced for a picture, samples cannot be invented.
        """
        body = self._body_for(element) if element is not None else {}
        if "buffer" not in body:
            raise ValueError(
                f"{_name(element)} has no samples to draw: a signal view needs a "
                "rendered element (render the composition, or bounce this one to "
                "a buffer, and open that)")
        return {k: v for k, v in body.items() if k in ("buffer", "channels")}

    def open_signal(self, host, element=None, *, layers=("peak", "rms"),
                    id: int | None = None) -> "WindowHandle":
        """`draw` a single **rendered** element as a dedicated signal view and
        open it on ``host`` — the editor-grade view of one element's samples, as
        opposed to `open`, where the same samples are only a clip's body.

        ``layers`` is what the picture measures, and the `layers` property
        changes it **live** on the open view: ``("peak", "rms")`` is the
        editor's picture — what the signal reached with the level it held drawn
        inside it — and ``("peak",)`` is the bare envelope. They are measures of
        **one** `clausters.gui.guidef.waveform`, not a pile of widgets: a view
        of a signal paints its own field before it draws, so two of them on one
        rectangle would not layer, and one view is also one axis, one ruler, one
        selection, one playhead and one upload of the samples.

        The element must have **material**: a rendered take, not a generator
        (see the error a generator raises). Returns the **window handle**, like
        `open`.
        """
        element = self.element if element is None else element
        # Refused **before** a window exists: an unknown measure and an element
        # with no samples are both answers to the call that was made, and
        # finding out at the first repaint would leave an empty window behind.
        stack = tuple(_measure(m) for m in layers)
        self._material_of(element)
        self._host = self.transport.host = host
        self._mode = "signal"
        self._signal_element = element
        # Straight to the field: there is no window yet, so this is what the
        # first draw measures rather than something to push at one.
        self._layers = stack
        self._window = host.open(self.draw(), id=id)
        self._announce()
        return self._window

    def open_pianoroll(self, host, element=None, id: int | None = None) -> "WindowHandle":
        """`draw` a single events element as a **dedicated piano-roll** window
        and open it on ``host`` — the editor-grade note view (a keyboard, an
        editable note grid, a velocity lane, an OSC-event lane) of one MIDI/OSC
        element, as opposed to `open`, where the same notes are only a clip body.

        Edits write back through `poll` exactly as the multitrack does, **when the
        element is editable** — a `clausters.form.Track` (a
        `clausters.seq.Timeline`): a dragged, added or removed note is rebuilt onto
        its timeline. A **generator** (a `Pbind`/`Routine`) is forward-only, so its
        bounced notes are shown *read-only* (bounce it to a `Track` to edit). OSC
        events are shown but not edited back yet (a marker carries only its time
        and address, not the full message).

        Returns the **window handle**, like `open`."""
        self._host = self.transport.host = host
        self._mode = "pianoroll"
        self._roll_element = self.element if element is None else element
        self._window = host.open(self.draw(), id=id)
        self._announce()
        return self._window

    def extent(self, element=None) -> float:
        """The composition's length in beats, **read from the arrangement** — the
        end of its last placed element. It is not a constant: move a clip past the end
        and the piece gets longer, which is exactly what a transport must ask
        (a hard-coded length would cut the playback short at the old end)."""
        return self._extent(self.element if element is None else element)

    @property
    def playhead(self):
        """The `clausters.seq.Playhead` playing the composition, or ``None`` before
        the first `render` — what the `transport` (play/pause/stop/locate) drives."""
        return self.transport.playhead

    @property
    def window(self):
        """The open window's id, or ``None`` once it is closed (a `/gui_closed`
        seen by `apply`/`poll`) — what a script's loop checks to stop."""
        return self._window

    def update(self):
        """Push the current arrangement back to the open window — a whole-tree
        redefine (`GuiHost.define`), the honest way to show a structural edit (an
        element added, a group expanded). A mere placement change needs no redefine: the
        host already moved the clip that was dragged.

        A redefine **moves the version**, and that is the point rather than a
        side effect: this is the route a change the editor did not apply arrives
        by — a script adding an element, a group expanded, a re-render — and it
        is the case an edit log cannot see. It also rebuilds the widgets, so a
        gesture still in flight was made against a picture that no longer
        exists; the bump is what makes that edit come back as stale instead of
        landing on whatever now holds its id."""
        if self._host is None or self._window is None:
            raise RuntimeError("open(host) the editor first")
        self._version += 1
        self._host.define(self._window, self.draw())
        # The host drops what it had in flight on a redefine, so what it needs
        # from here is the version the new picture is at.
        self._announce()

    # ---- the edit-back: a dragged clip becomes a placement ----

    def apply(self, addr: str, args) -> bool:
        """Apply one message from the host to the **arrangement**, and **answer
        it**. Returns whether the composition changed.

        The clip edit-back (``/gui_event <id> "clip" <offset> <dur>``, the payload
        a drag or a resize sends) is resolved through the widget registry to the
        placement it came from and written with `Group.move`. The clip's offset is
        **absolute** on the shared axis while a placement is relative to its
        group, so the position converts back through the base the clip was drawn
        at; and only what actually moved is written — a drag carries the clip's
        unchanged ``dur`` along, and snapping *that* to the grid would silently
        shorten the element. ``/gui_closed`` drops the window (its own — the
        payload names the window id); anything else is ignored, so a whole poll
        loop can be fed straight in — even one shared with a second editor
        (a dedicated piano-roll beside the multitrack, say): every route resolves
        through this editor's own registries, so another window's events fall
        through untouched.

        A logical group's directed patch routes here too: a ``"wire"`` rewrites the
        two members' controls onto a shared bus (`_apply_patch`), a ``"move"``
        persists a box's canvas position.
        """
        if addr == "/gui_closed":
            if not args or self._window is None or int(args[0]) == self._window:
                self._window = None
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
        # Why an edit did not do what it asked, when there is something to say.
        # It rides with the acknowledgement, because a refusal with no reason
        # teaches "sometimes it does not work" -- the one answer worse than no.
        self._reason = None
        # The window's own shortcuts (Ctrl+Z / Ctrl+Shift+Z), which the host
        # addresses to the **window** rather than to a widget: undo is not
        # aimed at anything under the cursor. They are answered here rather
        # than routed, because a history step is not an edit to the tree -- it
        # is a walk through the one the crate keeps.
        if args[1] in ("undo", "redo") and int(args[0]) == self._window:
            (self.redo if args[1] == "redo" else self.undo)()
            self._acknowledge(seq)
            return True
        # Only what this editor draws is this editor's to answer. A poll loop
        # may be shared with a second editor, and answering for its window would
        # retire a pending edit nobody applied -- the host would adopt a picture
        # the real owner never saw.
        if not self._owns(int(args[0])):
            return False
        if self._stale(against):
            # The composition moved under the gesture, by a route no gesture
            # produced. The edit is not applied and not merged: an edit-back
            # payload is absolute *and* whole (a roll's notes are the list, not
            # a diff), so applying one made against an older picture would
            # silently drop whatever arrived in between. What goes back is the
            # state as it stands, which the host adopts exactly as it adopts a
            # snap -- no new path, and the reason is what distinguishes "someone
            # else changed this" from "not here".
            self._resync(int(args[0]))
            self._acknowledge(seq, reason="the composition changed since this edit")
            return False
        changed = self._route(args)
        # Answered whatever happened, and answered with a *value*. There is no
        # success flag: the state this editor decided rides as the corrections
        # `_route` collected, and a refusal is simply the previous value among
        # them. Applied, transformed and refused are one message.
        self._acknowledge(seq, reason=self._reason)
        return changed

    def _route(self, args) -> bool:
        """One `/gui_event` payload onto the arrangement, with the stamp already
        taken off. Returns whether the composition changed; `apply` is what
        answers the host."""
        if args[1] == "locate":
            # A click on a lane's ruler (or its empty space): seek. A transport
            # action, not an edit — the composition did not change (and another
            # editor's lane is not ours to seek from).
            if int(args[0]) in self._lanes:
                self.locate(self.units_to_beats(float(args[2])))
            return False
        if args[1] == "selection":
            # A marquee swept on a lane or a view. Nothing in the composition
            # changed -- a selection is screen state, and the crate is explicit
            # that it is never part of the document -- but it is the *value* an
            # operation is handed, so it is kept typed and in the arrangement's
            # own unit, ready for `resolve_selection`.
            self._set_selection(int(args[0]), args[2:])
            return False
        if args[1] == "cut":
            return self._apply_cut(int(args[0]), args[2:])
        if args[1] == "paste":
            return self._apply_paste(int(args[0]), args[2:])
        if args[1] == "notes":
            # A note edited in a roll — a clip's body on a lane, or the dedicated
            # piano-roll: rebuild the element's timeline (a generator is
            # read-only, so it is ignored).
            element = self._rolls.get(int(args[0]))
            if element is None:
                return False
            if self._apply_notes(element, args[2:]):
                return True
            # Read-only material: a generator's notes are a *rendering* of an
            # algorithm, so the edit is refused -- and the refusal is the notes
            # as they still are, sent back so the host stops drawing the one the
            # hand moved. This is the case that used to be silent.
            self._correct(int(args[0]), notes=_flat_notes(self._notes(element)))
            return False
        if int(args[0]) in self._patches:
            # A logical group's directed patch: a cord drawn (rewire) or a box
            # moved (presentation).
            return self._apply_patch(int(args[0]), args[1], args[2:])
        placed = self._clips.get(int(args[0]))
        if placed is None:
            return False
        if args[1] == "points":
            return self._apply_points(placed, args[2:])
        if args[1] != "clip" or len(args) < 4:
            return False
        if placed.member is None:
            return False  # the root element itself: nothing places it

        offset, dur = float(args[2]), float(args[3])
        moved = abs(offset - placed.offset) >= 0.5      # half a sample: a real edit
        resized = abs(dur - placed.dur) >= 0.5
        if not (moved or resized):
            return False

        member = placed.member
        # Absolute (the axis) -> relative (the placement). The **grid is not
        # applied here**: the intent states where the hand put it and the crate
        # snaps, which is the rule the whole document exists for -- one place
        # decides what an edit becomes, and the value that comes back is what
        # actually happened.
        asked_offset = (self.units_to_beats(offset) - placed.base if moved
                        else member.offset)
        asked_dur = self.units_to_beats(dur) if resized else member.dur
        node = self._node_id(member.element)
        if node is None:
            return False
        intent = {"intent": "place", "node": node, "offset": float(asked_offset)}
        if asked_dur is not None:
            intent["dur"] = float(asked_dur)
        outcome = self._record(intent, "move the clip" if moved else "resize the clip")
        if outcome is None or not outcome["applied"]:
            # Refused -- a node the document does not know, or an edit made
            # against a version it has left behind. What goes back is the
            # placement as it stands, which the host adopts like any other push.
            self._resync(int(args[0]))
            return False
        effective = outcome["effective"]
        self._project(effective)
        # What the crate did to the gesture. The host drew the clip where it was
        # released; if the snap moved it, saying so is the whole point of an
        # acknowledgement carrying a value.
        new_dur = effective.get("dur")
        snapped_offset = self.beats_to_units(float(effective["offset"]) + placed.base)
        snapped_dur = dur if new_dur is None else self.beats_to_units(float(new_dur))
        if abs(snapped_offset - offset) >= 0.5 or abs(snapped_dur - dur) >= 0.5:
            self._correct(int(args[0]), offset=snapped_offset, dur=snapped_dur)
            offset, dur = snapped_offset, snapped_dur
        # The clip was drawn where it now is: keep the registry truthful, or the
        # next edit would measure its move against a stale placement.
        placed.offset, placed.dur = offset, dur
        self._version = self._document.version
        self.dirty = True
        self._follow_render()
        return True

    def _owns(self, widget_id: int) -> bool:
        """Whether this editor drew the widget an event names -- the same
        registries every route resolves through."""
        return (widget_id in self._clips or widget_id in self._rolls
                or widget_id in self._patches or widget_id in self._lanes
                or widget_id in self._signals)

    def _announce(self):
        """Tell the host which version it is drawing, before any edit.

        A stamp of zero retires nothing -- the host's own numbering starts at
        one -- so this is purely the version, and it is what keeps the *first*
        gesture checked like every later one. Without it the host would name
        zero until the first acknowledgement came back, and the opening edit
        would be the one edit nobody could tell was stale."""
        if self._host is not None:
            self._host.ack(0, doc_version=self._version)

    def _stale(self, against: int) -> bool:
        """Whether an edit made against document version ``against`` has been
        overtaken.

        Zero is *unstated* rather than a version -- an older host, or one no
        owner has reported a version to -- and unstated applies unchecked, which
        is the behavior there was before there were versions at all."""
        return bool(against) and against != self._version

    def _resync(self, widget_id: int):
        """Hand back what the widget should be drawing, without applying
        anything: the answer to an edit that arrived too late.

        It reads the same registries every route resolves through, so a stale
        gesture is answered in the widget's own terms — a clip gets its
        placement, a roll gets its notes — and the host adopts it with the
        ordinary drop-and-adopt rule. **Everything the widget has**, not only
        what the gesture touched: one widget is often both (a clip with a roll
        body is a placement *and* a note list), and the stale edit is the one
        case where the host's whole picture of it is in doubt."""
        props: dict = {}
        placed = self._clips.get(widget_id)
        if placed is not None:
            props.update(offset=placed.offset, dur=placed.dur)
        element = self._rolls.get(widget_id)
        if element is not None:
            props["notes"] = _flat_notes(self._notes(element))
        if props:
            self._correct(widget_id, **props)

    def _correct(self, widget_id: int, **props):
        """What the host should be drawing instead of what it drew.

        Called by `_route` when this editor did not do what the gesture asked --
        snapped it to the grid, or refused it outright. The value travels with
        the acknowledgement in one bundle, which is what lets the host adopt it
        without a redefine."""
        self._corrections.append((int(widget_id), props))

    def _acknowledge(self, seq: int, reason: "str | None" = None):
        """Answer the host for everything up to ``seq``.

        The half that was missing until now: this editor snaps a placement to
        the musical grid and refuses an edit to a generator, and the host could
        learn neither -- so a note dragged onto read-only material stayed drawn
        where the hand put it, and a clip landed half a grid step from where it
        was released. The stamp closes both, because it lets the host retire
        what it drew and adopt what actually happened.

        Every acknowledgement carries the composition's version, which is what
        the host names back on its next gesture -- that round trip is the whole
        of the staleness check, and it costs one integer."""
        if self._host is None:
            return
        if not seq and not self._corrections:
            return
        # A stamp of zero retires nothing, which is exactly what an **unasked**
        # push needs: an undo answers no gesture, so it carries values and a
        # version and takes no pending edit with it.
        if self._corrections:
            self._host.push(seq, *self._corrections,
                            doc_version=self._version, reason=reason)
        else:
            self._host.ack(seq, doc_version=self._version, reason=reason)
    def _set_selection(self, wid: int, values) -> None:
        """Keep the swept selection as the crate's `Selection`, in beats.

        The host reports ``start len`` in timeline samples, plus ``min max``
        where the sweep restricted the value axis too. Three things happen here
        and each is a translation the crate deliberately does not do. The time
        numbers become **beats**, the unit the arrangement is written in (the
        crate holds whatever unit it is handed and converts nothing, because the
        beats↔samples bridge belongs to whoever renders). The value range is
        carried **as it came**: it is in the element's own domain and no unit of
        this editor's applies to it. And the selection is made *of* something
        where the widget names one element — a clip's body, a roll — and of the
        shared time axis where it does not, which is a lane: the empty ``nodes``
        the crate describes as a selection dragged across a multitrack.
        """
        if len(values) < 2:
            return
        start, length = float(values[0]), float(values[1])
        selection: dict = {"start": self.units_to_beats(start),
                           "len": self.units_to_beats(length)}
        if len(values) >= 4:
            selection["value"] = {"min": float(values[2]), "max": float(values[3])}
        element = self._rolls.get(wid) or self._signals.get(wid)
        if element is None:
            placed = self._clips.get(wid)
            element = None if placed is None else (
                placed.member.element if placed.member is not None else self.element)
        node = None if element is None else self._node_id(element)
        if node is not None:
            selection["nodes"] = [node]
        self.selection = selection

    def _apply_cut(self, wid: int, values) -> bool:
        """A cut asked for over the selection: the host owns none of the data,
        so this is where it becomes an edit.

        **What this editor cuts is a placement**, because that is what it owns:
        an arrangement is elements placed in time, and a clip the selection
        covers entirely leaves the group it was in — undoably, through the
        crate, like every other edit here. What it does *not* do is trim: a
        selection cutting across a clip implies a new length for the material
        under it, and writing material is the owner of that material's business
        (the working copy the document's plan describes), not a placement edit
        wearing the same name. That case is refused **out loud**, because a cut
        that silently did nothing would read as a broken key.
        """
        placed = self._clips.get(wid)
        if placed is None or placed.member is None or len(values) < 2:
            return False
        start = self.units_to_beats(float(values[0]))
        end = start + self.units_to_beats(float(values[1]))
        member = placed.member
        # The clip's span on the **shared** axis: a placement is relative to its
        # group, and the selection is absolute, which is the same bridge a drag
        # crosses in the other direction.
        at = placed.base + member.offset
        span = at, at + (member.length or 0.0)
        if not (start <= span[0] and end >= span[1]):
            self._resync(wid)
            self._reason = (
                "a cut across a clip is a new length for its material, "
                "which is the material owner's edit"
            )
            return False
        owner = placed.owner
        if owner is None:
            return False
        node = self._node_id(owner)
        if node is None:
            return False
        # The members as they would stand: the document's own serialization,
        # minus the one being cut. Built from the public conversion rather than
        # by hand, so the shape is whatever the format currently says it is.
        whole = to_document(owner, version=self._version)["root"]
        keep = [m for h, m in zip(owner.handles, whole.get("members", []))
                if h is not member]
        outcome = self._record({"intent": "setmembers", "node": node,
                                "members": keep}, "cut the clip")
        if outcome is None or not outcome["applied"]:
            self._resync(wid)
            return False
        self._project(outcome["effective"])
        return True

    def _apply_paste(self, wid: int, values) -> bool:
        """A paste asked for over a view, carrying the clipboard with it.

        The clipboard travels *with* the request — the host's, so that a block
        copied in one window pastes into another — and what arrives is the
        crate's typed document: its kind, its JSON, and its bulk beside it.

        **What this editor can place is elements.** A block of *samples* is
        material, and material is written by whoever owns it against a working
        copy; an arrangement editor placing a nameless block of audio would be
        inventing both a source and a source's owner. So a sample paste is
        refused with the reason, which is the honest answer until the material
        half of the track lands.
        """
        if wid not in self._clips and wid not in self._lanes:
            return False
        kind = str(values[1]) if len(values) > 1 else ""
        self._reason = (
            f"this editor places elements; a {kind or 'clipboard'} block is "
            "material, and material is written by its owner"
        )
        return False

    def resolve_selection(self) -> list:
        """The **material under the current selection**, through the crate.

        The other half of what a selection is for: `Editor.selection` says what
        was swept, and this says what is underneath it — one entry per leaf,
        with the placement's base, the element's trim and the clamp at both ends
        already applied (`clausters._native.Document.resolve`). Empty when
        nothing material was under the sweep, and when there is no selection at
        all.

        The value range travels with the selection but does not narrow this:
        what is under a band of amplitudes is the same material as what is under
        the whole span, and *reading only those samples* is an operation over
        the range rather than a resolution of it.
        """
        if not self.selection:
            return []
        _, document = self._history()
        return document.resolve(self.selection,
                                frames_per_beat=self.units_per_beat,
                                in_beats=True)

    def _apply_points(self, placed, values) -> bool:
        """A curve edited in place on an automation clip (the flat ``"points"``
        payload the `bpf` view also sends): the break-points go back onto the
        element's `clausters.seq.Automation`, with their times converted from
        timeline units to beats. The `Env` is the automation's source of truth, so
        this *is* the edit — the next render plays the curve as drawn."""
        auto = _automation(placed.member.element if placed.member is not None
                           else self.element)
        if auto is None or not values:
            return False
        flat = []
        for t, v, shape, curve in _quads(list(values)):
            flat += [self.units_to_beats(t), float(v), int(shape), float(curve)]
        auto.env = points_to_env(flat)
        self._changed()
        return True

    def _apply_notes(self, element, values) -> bool:
        """Notes edited in a roll — a clip's body or the dedicated piano-roll
        alike, since both send it (the flat ``"notes"`` payload,
        `start dur pitch velocity channel` quintuples): rebuilt onto the element's
        editable `clausters.seq.Timeline` as `Event`s, times converted to beats,
        preserving any OSC/MIDI items already on it. Returns ``False`` for a
        forward-only generator element (read-only), so the edit is a no-op."""
        timeline = _editable_timeline(element)
        if timeline is None:
            return False
        new = []
        for start, dur, pitch, vel, channel in _quintuples(list(values)):
            params = dict(midinote=int(pitch), dur=self.units_to_beats(dur),
                          amp=max(0.0, min(1.0, int(vel) / 127.0)),
                          velocity=int(vel), legato=1.0)
            if int(channel):
                params["channel"] = int(channel)
            new.append((self.units_to_beats(start), SeqEvent(params)))
        # Replace the notes, keep the OSC/MIDI events (they share the timeline).
        _rewrite_timeline(timeline, lambda it: _pitch(it) is None, new)
        self._changed()
        return True

    def _apply_patch(self, wid: int, tag, values) -> bool:
        """One edit on a logical group's directed patch. A ``"wire"`` (``src_box
        outlet dst_box inlet``) rewrites the two members' controls so they share a
        bus — the connection *is* a bus, the same fact `Group.to_graphdef` reads,
        so the next render wires the GraphDef the way the cord is drawn. A ``"move"``
        (``box x y``) only persists the box's canvas position (a signal graph has
        no timeline, so positions are the editor's, not the arrangement's)."""
        group, handles = self._patches[wid]
        if tag == "wire" and len(values) >= 4:
            return self._apply_wire(group, handles, values[:4])
        if tag == "move" and len(values) >= 3:
            self._patch_geometry.setdefault(id(group), {})[int(values[0])] = (
                float(values[1]), float(values[2]))
            return False
        return False

    def _apply_wire(self, group, handles, values) -> bool:
        """Draw a cord ``src.outlet -> dst.inlet`` onto the arrangement: name the
        bus the connection implies (reusing one either end already writes/reads,
        else a fresh name declared on the group) and point both members' controls
        at it. The bus rate comes from the source outlet's def."""
        src_box, outlet, dst_box, inlet = int(values[0]), str(values[1]), int(values[2]), str(values[3])
        if not (0 <= src_box < len(handles) and 0 <= dst_box < len(handles)):
            return False
        src, dst = handles[src_box].element, handles[dst_box].element
        rate = self._outlet_rate(src, outlet)
        if rate is None:
            return False  # a port-less (bare-name) member, or an unknown outlet
        src_ctls = dict(src.controls or {})
        dst_ctls = dict(dst.controls or {})
        bus = _named_bus(src_ctls.get(outlet)) or _named_bus(dst_ctls.get(inlet)) \
            or self._fresh_bus(group)
        src_ctls[outlet], dst_ctls[inlet] = bus, bus
        src.controls, dst.controls = src_ctls, dst_ctls
        group.declare_bus(bus, rate=rate)
        self._changed()
        return True

    def _outlet_rate(self, member, name: str):
        """The rate (``"audio"``/``"control"``) of ``member``'s outlet ``name``,
        derived from the `SynthDef` it wraps — or ``None`` when the member wraps a
        bare def name or has no such outlet."""
        from ..defs import synthdef_ports
        from ..defs.synthdef import SynthDef

        wraps = getattr(member, "wraps", None)
        if not isinstance(wraps, SynthDef):
            return None
        _inlets, outlets = synthdef_ports(wraps)
        for port in outlets:
            if _port_name(port) == name:
                return port[1] if isinstance(port, tuple) else "audio"
        return None

    def _fresh_bus(self, group) -> str:
        """A bus name not yet declared on ``group`` (``w0``, ``w1``, …) — the
        private wire a brand-new cord introduces."""
        taken = set(group.bus_names)
        i = 0
        while f"w{i}" in taken:
            i += 1
        return f"w{i}"

    def _osc(self, element) -> list:
        """The OSC (and raw MIDI) events of an element as ``(time_units, label)``
        pairs — the piano-roll's event lane. An `OscEvent` labels with its address,
        a `MidiEvent` with a short tag. Display only: a marker carries the time and
        a label, not the full message, so it is not written back (see
        `open_pianoroll`)."""
        if isinstance(element, (Group, Buffer)):
            return []
        try:
            events = flatten(element, 0.0)
        except (NotImplementedError, TypeError):
            return []
        out = []
        for beat, item in events:
            if isinstance(item, OscEvent):
                out.append((self.beats_to_units(beat), str(item.addr)))
            elif isinstance(item, MidiEvent):
                out.append((self.beats_to_units(beat), "midi"))
        return out

    def _changed(self) -> bool:
        """The arrangement was edited: mark it, and re-render now when `follow` is
        on. Otherwise the edit simply waits — a render already in flight is not
        interrupted, and the next one (a play, a resume, a seek) plays the piece as
        it now stands, because rendering always re-flattens the tree."""
        self.dirty = True
        self._version += 1
        self._follow_render()
        return True

    # ---- the history: the crate's log, over the crate's document -----------

    def _history(self):
        """The log and the document, built on first use and kept in step.

        The document is rebuilt from the arrangement rather than carried,
        because `to_document` stamps each element with the id it keeps
        (`ID_ATTR`) — so a second conversion gives the same node the same
        number, and an entry recorded against one conversion still names the
        right thing in the next. That is what lets a redraw, or a script editing
        the tree, happen without the history losing its footing."""
        if self._log is None:
            self._log = _native.Log()
        document = to_document(self.element, version=self._version)
        if self._document is None:
            self._document = _native.Document(document)
        else:
            # Replace the held tree: the arrangement is this editor's own
            # authority and may have moved by a route no intent took.
            self._document.close()
            self._document = _native.Document(document)
        self._index(self.element)
        return self._log, self._document

    def _index(self, element, owner=None, member=None):
        """Walk the arrangement collecting node id -> what an intent writes to.

        A `place` needs the owning group and the member handle (a placement is
        the group's, not the element's); everything else needs the element. The
        walk mirrors `clausters.form.document`'s own, which is what keeps the
        two agreeing about what has an id."""
        node = getattr(element, ID_ATTR, None)
        if node is not None:
            self._by_node[int(node)] = (owner, member, element)
        if isinstance(element, Group):
            for handle in element.handles:
                self._index(handle.element, element, handle)

    def _node_id(self, element) -> "int | None":
        """The document id of an arrangement element, building the document if
        that is what it takes.

        `to_document` is what *stamps* the id, so asking for one before the
        first conversion has to trigger it — otherwise the first gesture of a
        session names a node nobody has numbered."""
        node = getattr(element, ID_ATTR, None)
        if node is None:
            self._history()
            node = getattr(element, ID_ATTR, None)
        return None if node is None else int(node)

    def _record(self, intent: dict, label: str) -> "dict | None":
        """Apply one edit **through the crate**, recording its inverse.

        This is what makes the editor's own gestures undoable, and it is also
        where the deciding happens: the crate snaps a placement to `quant`,
        refuses an edit to a node it cannot find, and reports an edit made
        against a version the document has left behind. What comes back is the
        **effective** value, which `_project` then writes onto the arrangement —
        so this editor decides nothing an intent could decide, and the log and
        the tree cannot disagree about what happened.

        Returns the outcome, or ``None`` when there is no document to apply to
        (an arrangement whose elements carry no ids yet)."""
        log, document = self._history()
        outcome = log.apply(document, intent, against={"version": self._version},
                            quant=self.quant, label=label)
        return outcome

    def _project(self, intent: dict) -> "int | None":
        """Write an intent's value onto the arrangement, and say which widget
        was drawing it.

        The editor is to the document what the host is to the editor: it emits
        an intent and adopts the value that comes back. Nothing here decides
        anything — the snap, the clamp and the refusal already happened in the
        crate — so this is a projection and not a second implementation of what
        an edit means. It is also the whole of what an undo has to do, since an
        inverse is an ordinary intent."""
        found = self._by_node.get(int(intent.get("node", -1)))
        if found is None:
            return None
        owner, member, element = found
        kind = intent.get("intent")
        if kind == "place" and owner is not None and member is not None:
            owner.move(member, float(intent["offset"]), intent.get("dur"))
        elif kind == "configure":
            auto = _automation(element)
            if auto is None:
                return None
            flat = intent.get("config", {}).get("points")
            if flat is None:
                return None
            auto.env = points_to_env(list(flat))
        elif kind == "setmembers":
            members = intent.get("members", [])
            # Two things carry members and they are not the same thing: a
            # `Group`'s placements, and the notes of an editable timeline. The
            # element decides which, because the intent names a node and the
            # node is whichever of the two it is.
            if isinstance(element, Group):
                if not self._set_placements(element, members):
                    return None
            elif not self._set_notes(element, members):
                return None
        else:
            return None
        return self._widget_of(element, member)

    def _set_placements(self, group, members: list) -> bool:
        """A `setmembers` onto a `Group`: the placements as the document states
        them, whole.

        **Only what the document still names survives**, which is what makes a
        cut a removal and an undo of it a restoration: the members arrive by
        node id, so a handle whose id is no longer among them leaves, and the
        ones that stay keep their identity rather than being rebuilt (a rebuilt
        handle would be a different object to the widget registry, and every
        pending edit against it would address a placement nobody is drawing).
        """
        keep = {int(m["node"]["id"]) for m in members if "id" in (m.get("node") or {})}
        by_id = {}
        for handle in list(group.handles):
            node = getattr(handle.element, ID_ATTR, None)
            if node is None:
                continue
            by_id[int(node)] = handle
            if int(node) not in keep:
                group.remove(handle)
        # ...and the offsets the document states, for the ones that stayed —
        # plus the ones that are **back**, which is what an undo of a cut is.
        # The element itself outlives its placement (the node index still names
        # it), so restoring is placing that same object again rather than
        # rebuilding one: the identity a pending edit or a widget registry holds
        # is the element's, and a copy of it would answer for nothing.
        for m in members:
            node = (m.get("node") or {}).get("id")
            if node is None:
                continue
            handle = by_id.get(int(node))
            offset = float(m.get("offset", 0.0))
            if handle is not None:
                group.move(handle, offset)
                continue
            found = self._by_node.get(int(node))
            if found is not None:
                group.add(found[2], offset, m.get("dur"))
        return True

    def _set_notes(self, element, members: list) -> bool:
        """A `setmembers` onto an element's editable timeline: the notes as the
        document states them, whole. Returns whether it landed."""
        timeline = _editable_timeline(element)
        if timeline is None:
            return False
        new = []
        for placed in members:
            node = placed.get("node") or {}
            config = node.get("config") or {}
            if "midinote" not in config:
                continue
            new.append((float(placed.get("offset", 0.0)), SeqEvent(dict(config))))
        _rewrite_timeline(timeline, lambda it: _pitch(it) is None, new)
        return True

    def _widget_of(self, element, member=None) -> "int | None":
        """Which widget is drawing this element — the id→object route read
        backwards, which is what an undo needs in order to correct the picture
        without redefining the window."""
        for wid, placed in self._clips.items():
            if placed.member is member and member is not None:
                return wid
            if placed.member is not None and placed.member.element is element:
                return wid
        for wid, drawn in self._rolls.items():
            if drawn is element:
                return wid
        return None

    def undo(self) -> bool:
        """Step back one edit, and tell the host what to draw instead.

        The inverse is an ordinary intent, so undoing needs no second path: it
        is `_project` again, on what the crate hands back. Returns whether
        anything was undone."""
        return self._step(lambda log, doc: log.undo(doc), "undone")

    def redo(self) -> bool:
        """Step forward again after `undo`. Returns whether anything was
        redone.

        A step the crate **cannot perform** — a deterministic operation kept as
        its parameters rather than as a span — comes back in ``remaining`` for
        its owner to re-run. Nothing in the multitrack editor produces one yet,
        so this reports it rather than acting on it."""
        return self._step(lambda log, doc: log.redo(doc), "remaining")

    def _step(self, walk, key: str) -> bool:
        if self._log is None:
            return False
        log, document = self._history()
        step = walk(log, document)
        if step is None:
            return False
        widgets = set()
        if key == "undone":
            for intent in step["undone"]:
                wid = self._project(intent)
                if wid is not None:
                    widgets.add(wid)
        else:
            # A redo applied its ordinary edits to the document already, so what
            # the arrangement needs is the document as it now stands rather than
            # a list of intents to replay.
            self._adopt(document.snapshot(), widgets)
        self._version = document.version
        self.dirty = True
        self._follow_render()
        self._corrections = []
        for wid in widgets:
            self._resync(wid)
        self._acknowledge(0)
        self._corrections = []
        return True

    def _adopt(self, snapshot: dict, widgets: set):
        """Write a whole document back onto the arrangement — the redo path,
        where the crate applied the steps itself rather than handing them over.

        It walks placements only, which is what a redo of this editor's own
        gestures can have changed; anything else is left to the redraw."""
        def walk(node, owner=None, member=None):
            found = self._by_node.get(int(node.get("id", -1)))
            if found is not None and member is not None:
                _, handle, element = found
                if handle is not None and owner is not None:
                    owner.move(handle, float(member.get("offset", 0.0)),
                               member.get("dur"))
                    wid = self._widget_of(element, handle)
                    if wid is not None:
                        widgets.add(wid)
            for placed in node.get("members", []) or []:
                child = placed.get("node")
                if child is None:
                    continue
                parent = self._by_node.get(int(node.get("id", -1)))
                walk(child, parent[2] if parent else None, placed)
        walk(snapshot["root"])

    @property
    def can_undo(self) -> bool:
        """Whether there is an edit to step back over."""
        return self._log is not None and self._log.can_undo

    @property
    def can_redo(self) -> bool:
        """Whether there is an undone edit to step forward into."""
        return self._log is not None and self._log.can_redo

    @property
    def undo_label(self) -> "str | None":
        """What an undo would be called, for a menu item."""
        return None if self._log is None else self._log.undo_label

    def _follow_render(self):
        """Re-schedule after an edit when `follow` is on **and there is
        something to re-schedule**.

        The guard is the whole of it: `rerender` needs a destination and a
        clock, which only a `render` or a `play` supplies, so a live editor
        edited before anything was ever played used to raise on the first drag.
        An edit made before the first play is not lost by doing nothing here --
        it marked the composition (`dirty`), and the next play re-reads it,
        because rendering always re-flattens the tree."""
        if self.follow and self._destination is not None:
            self.rerender()

    def poll(self, timeout: float = 0.0) -> bool:
        """Drain the host's pending messages into the arrangement (`apply` each)
        **and on to the window's own handlers**. Returns whether the composition
        changed. Call it from the script's loop — **never** from the clock
        thread, which a routine must never block.

        The second half is why a window may carry both: a transport bar beside
        the editor is the script's, addressed to widgets this editor never drew,
        and its `clausters.gui.handle.WidgetHandle.on_event` callbacks run here
        because this is the loop that took its message off the socket. A drain
        that only fed the arrangement swallowed them — the button was pressed,
        the host reported it, and nothing happened.
        """
        if self._host is None:
            raise RuntimeError("open(host) the editor first")
        changed = False
        while (msg := self._host.poll(timeout)) is not None:
            changed |= self.apply(*msg)
            self._host.dispatch(*msg)
            timeout = 0.0  # only the first wait blocks
        return changed

    def _snap(self, beats: float) -> float:
        """Snap a beat value to the musical `quant` grid (the same grid the lane
        snapped the drag to, now in the arrangement's units — so the round trip
        draw → drag → apply → draw is exact, free of the wire's float noise)."""
        if self.quant <= 0.0:
            return beats
        return round(beats / self.quant) * self.quant

    # ---- rendering: the edited arrangement back to sound ----

    def render(self, destination, clock=None, *, at: float = 0.0, quant=None):
        """Render the composition onto ``destination`` — RT (a `Server` and a
        running clock) or NRT (a score) — and anchor the lanes' playhead so the
        line sweeps the clips as it plays. Returns the `clausters.seq.Playhead`.

        This is the arrangement's own `render` (flatten to absolute beats, play
        through a playhead): the editor adds no rendering path of its own, it only
        remembers the destination so `rerender` can re-schedule after an edit.
        """
        self._destination, self._clock = destination, clock
        playhead = self.transport.play(destination, at=at, quant=quant)
        self.dirty = False            # what plays now *is* the arrangement
        return playhead

    def _render_pass(self, at: float, quant=None):
        """One pass for the `transport`: the arrangement, flattened and played
        from beat ``at``. Called afresh on every play, which is what makes a
        play — or a resume, or a seek — read the composition as it now stands."""
        from ..form.render import render as render_element

        return render_element(self.element, self._destination, self._clock,
                              at=at, quant=quant)

    def rerender(self, *, at: float | None = None):
        """Re-schedule the (edited) composition from the playhead's current
        position: stop, re-flatten, play again.

        The honest semantics are **re-schedule from here**, not a sample-exact
        splice — a synth already sounding keeps sounding, and what changes is what
        has not been scheduled yet. In NRT there is no "already", so it is simply
        a fresh score.
        """
        if self._destination is None:
            raise RuntimeError("render(destination, clock) the editor first")
        return self.render(self._destination, self._clock,
                           at=self.position if at is None else at)

    # ---- the transport: play, pause, stop, locate ----
    #
    # The machinery is `clausters.gui.transport.Transport`, shared with every
    # other view that shows a playhead; what the editor adds is that a pass is a
    # *render* of the arrangement (`_render_pass`), so play, resume and seek all
    # read the composition as it now stands.

    @property
    def position(self) -> float:
        """The transport's position in beats: where the playhead is while it plays,
        and where the next `play` starts when it does not."""
        return self.transport.position

    def play(self, destination=None, clock=None, *, at: float | None = None):
        """Play (or resume) from the transport's position — a fresh render, so
        it plays the composition **as it now stands** (moved clips, new lengths,
        redrawn curves). Reuses the destination and clock of the last `render`
        when they are not given."""
        destination = self._destination if destination is None else destination
        clock = self._clock if clock is None else clock
        if destination is None:
            raise RuntimeError("nothing to play onto: render(destination, clock) first")
        return self.render(destination, clock,
                           at=self.transport.at if at is None else float(at))

    def pause(self):
        """Halt where we are: the playhead stops scheduling and the position stays,
        so a `play` resumes from here. What is already sounding keeps sounding —
        stopping a playhead is not a panic button (the script owns its voices)."""
        return self.transport.pause()

    def stop(self):
        """Halt and return to the top."""
        self.transport.stop()
        return self

    def locate(self, beat: float):
        """Seek: put the transport at ``beat``. Playing, it re-renders from there
        (so a seek also picks up any edit); stopped, it just moves the cursor the
        lanes draw. This is what a click on a lane's ruler does.

        A composition holding a **resident generator** has no position to seek
        to — its material is produced on the server, so its position is that
        def's internal state and no number moves it. Rather than move the cursor
        somewhere the sound will not follow, this refuses and says why. Render
        the element first (`clausters.form.render`) and it becomes material like
        any other."""
        if not self.locatable:
            raise ValueError(
                "this composition contains a resident generator, which has no "
                "position to locate to; render it first to give it one"
            )
        self.transport.locate(beat)
        return self

    @property
    def locatable(self) -> bool:
        """Whether the composition can be seeked at all — false when any element
        is a resident generator. The view draws those lanes as their own class,
        since a ruler click means nothing on one."""
        return self.element.locatable

    def resume(self):
        """Continue where `pause` left off, **without re-rendering**.

        MIDI's `continue` against `play`'s `start`: play reads the composition
        as it now stands and starts it again, resume picks the frozen sound back
        up. Under a server transport that governs the material, every node kept
        its internal state through the pause, so a texture carries on
        mid-gesture instead of restarting."""
        return self.transport.resume()

    def anchor(self, server, *, at: float = 0.0) -> bool:
        """Anchor every lane's playhead to the engine clock, so the line starts at
        beat ``at`` of the timeline and sweeps on with the audio. Returns whether
        it could (a destination with no clock — an NRT score — has no playhead)."""
        return self.transport.anchor(server, at=at)

    def unanchor(self):
        """Take the sweeping playhead line off the lanes (the transport's cursor,
        if any, stays). The host's anchored playhead *tracks the engine clock*, so
        a line left anchored keeps sweeping after the music stopped."""
        self.transport.unanchor()

    # ---- the tree walk ----

    def _lanes_for(self, element, base: float, owner, member) -> list:
        """The lanes an element contributes: a concrete `Group` becomes one
        lane holding its members as clips (plus a lane of its own for every
        *expanded* nested group); anything else becomes a lane with one clip.
        ``base`` is its start in beats, ``owner``/``member`` the placement an
        edit-back writes through."""
        if (isinstance(element, Group) and element.kind == CONCRETE
                and len(element) > 1
                and element.temporal_relation() == SIMULTANEOUS
                and not self.is_expanded(element)):
            # Its members start and end together: they are *one* thing on the
            # timeline, so they are one clip with layered bodies — not a lane of
            # clips that must be dragged one by one.
            return [self._lane([self._clip_for(element, base, owner, member)],
                               _name(element))]
        if isinstance(element, Group) and element.kind == CONCRETE:
            clips, extra = [], []
            for child in element.handles:
                child_base = base + child.offset
                if isinstance(child.element, Group) and self.is_expanded(child.element):
                    extra += self._lanes_for(child.element, child_base, element, child)
                else:
                    clips.append(self._clip_for(child.element, child_base, element, child))
            lane = [self._lane(clips, _name(element))] if clips else []
            return lane + extra
        return [self._lane([self._clip_for(element, base, owner, member)], _name(element))]

    def _lane(self, clips: list, label: str) -> dict:
        """One `track` lane holding ``clips``, with the shared time chrome."""
        wid = self._new_id()
        lane = track(*clips, id=wid, label=label, sample_rate=self.sample_rate,
                     tempo=self.tempo,
                     snap=self.beats_to_units(self.quant) if self.quant > 0 else None)
        self._lanes[wid] = label
        return lane

    def _clip_for(self, element, base: float, owner, member) -> dict:
        """One `clip`: the element placed at ``base`` beats (absolute on the shared
        axis), with the body (or **bodies**) its kind calls for. Registers what it
        drew, which is what the edit-back path resolves against."""
        wid = self._new_id()
        offset = self.beats_to_units(base)
        # The length shown, in beats: the placement's when it overrides, else the
        # element's own.
        dur_beats = member.dur if (member is not None and member.dur is not None) else None
        if dur_beats is None and isinstance(element, Element):
            dur_beats = element.duration
        if dur_beats is None:
            dur_beats = self._extent(element)

        body = self._body_for(element)
        # A take with no duration given is as long as it is (1 unit = 1 sample).
        if "buffer" in body and dur_beats <= 0.0:
            dur = float(element.wraps.frames)
        else:
            dur = self.beats_to_units(dur_beats)

        # The placement's own base: a clip's offset is absolute on the shared axis,
        # a member's offset is relative to its group.
        parent_base = base - (member.offset if member is not None else 0.0)
        self._clips[wid] = _Placed(owner, member, parent_base, offset, dur)
        # A roll body is the `notes` element itself, and it edits: a body carries
        # no id of its own, so a note dragged inside one arrives tagged with *this
        # clip's* id. Registering what the body draws is what lets that edit reach
        # the arrangement — without it the note moves on screen and nowhere else.
        roll = _roll_owner(element)
        if "notes" in body and roll is not None:
            self._rolls[wid] = roll
        return clip(id=wid, offset=offset, dur=dur, label=_name(element), **body)

    def _body_for(self, element) -> dict:
        """The clip-body props an element draws with — and a **simultaneous** group
        draws with *all of its members'*, layered in one clip.

        That is the arrangement's own answer to "attach an envelope to the event it
        shapes": a group whose members start and end together *is* one thing on the
        timeline (its temporal relation says so), so it is one clip — dragging it
        moves the whole group, and the bodies overlay instead of hiding each other.
        The curve keeps its own value axis (`points_min`/`points_max`), since an
        envelope's units are not the pitches under it.
        """
        # A simultaneous group first: it is one thing on the timeline, and its
        # members' bodies layer (each keeps its own value axis).
        if (isinstance(element, Group) and len(element) > 1
                and element.temporal_relation() == SIMULTANEOUS):
            body: dict = {}
            for m in element.handles:
                body.update(self._body_for(m.element))
            return body

        auto = _automation(element)
        if auto is not None:
            points = [(self.beats_to_units(t), v, shape, curve)
                      for t, v, shape, curve in _quads(auto.to_points())]
            lo, hi = _curve_range(points)
            return dict(points=points, points_min=lo, points_max=hi)

        if isinstance(element, Buffer):
            buf = element.wraps
            # The take rides the bulk path: the host fetches the server buffer and
            # decimates it through its peak pyramid.
            #
            # Material this process does not hold draws as a **clip with no
            # waveform** rather than not at all: a session reopened without its
            # sources resolved wraps each one in a `FrozenSource`, which knows
            # the buffer number the document recorded and nothing about its
            # shape. Laid out, not dropped -- the same rule an unknown widget
            # gets, and the reason a piece whose take has gone missing still
            # shows where the take was.
            channels = getattr(buf, "channels", None)
            if channels is None:
                return {}
            return dict(buffer=buf.bufnum, channels=max(1, channels))

        notes = self._notes(element)
        if notes:
            pitches = [n[2] for n in notes]
            return dict(notes=notes,
                        min=min(min(pitches) - PITCH_PAD, DEFAULT_PITCH[1]),
                        max=max(max(pitches) + PITCH_PAD, DEFAULT_PITCH[0]))
        # No body: a collapsed group (or an element with nothing to draw) is the
        # labeled rectangle — the summary of the level above it.
        return {}

    def _notes(self, element) -> list:
        """The ``(start, dur, pitch)`` note events of an element, in timeline
        units relative to the element — the piano-roll body. A `Group` is a
        summary, not a roll (it collapses to a rectangle), and a note is any
        flattened event that resolves a pitch: the *change of state* of a
        contained generator happens right here (a pattern is bounced by
        `clausters.form.render.flatten`), so a generator lane shows the notes it
        will play."""
        if isinstance(element, (Group, Buffer)):
            return []
        try:
            events = flatten(element, 0.0)
        except (NotImplementedError, TypeError):
            return []
        notes = []
        for beat, item in events:
            pitch = _pitch(item)
            if pitch is None:
                continue
            notes.append((self.beats_to_units(beat),
                          self.beats_to_units(_event_dur(item)),
                          pitch, _velocity(item), 0))
        return notes

    def _extent(self, element) -> float:
        """An element's length in beats: its own ``duration`` when it has one,
        else what it spans — a group over its placed members, an envelope over its
        curve, anything else over its flattened events (a bounced pattern
        included)."""
        if isinstance(element, Element) and element.duration is not None:
            return float(element.duration)
        auto = _automation(element)
        if auto is not None:
            return auto.duration()
        if isinstance(element, Group):
            return max((m.offset + (m.dur if m.dur is not None
                                    else self._extent(m.element))
                        for m in element.handles), default=0.0)
        if isinstance(element, Buffer):
            buf = element.wraps
            rate = buf.sample_rate or self.sample_rate
            return self.units_to_beats(buf.frames * (self.sample_rate / rate))
        try:
            events = flatten(element, 0.0)
        except (NotImplementedError, TypeError):
            return 0.0
        return max((beat + _event_dur(item) for beat, item in events), default=0.0)


def _logical_patch(group):
    """A logical `Group` as a `clausters.defs.GraphPatch`, through the headless
    decode `GraphPatch.from_graphdef`: the group renders to a `GraphDef` (its
    members and their shared-bus controls — the arrangement's 1:1 logical mapping),
    and the decode reads that back into a directed patch, typing each box's ports
    from the `SynthDef` the member wraps. The `Group -> patch` mapping itself lives
    in `clausters.defs`, not here — the editor is only a consumer of it.

    A member wrapping a bare def *name* (not a `SynthDef` object) draws port-less —
    its directions are unknowable without the def. Returns the patch and the member
    handles in box order (box index == member order), so an edit-back maps a box
    index back to the member whose controls it rewrites."""
    from ..defs import GraphPatch
    from ..defs.synthdef import SynthDef

    handles = list(group.handles)
    gdef = group.to_graphdef(name=getattr(group, "name", None) or "_patch")
    defs = {
        h.element.def_name: h.element.wraps
        for h in handles
        if isinstance(getattr(h.element, "wraps", None), SynthDef)
    }
    return GraphPatch.from_graphdef(gdef, defs), handles


def _port_name(port) -> str:
    """A port spec's name, whether audio (a bare string) or control (``(name,
    rate)``)."""
    return port if isinstance(port, str) else port[0]


def _named_bus(value):
    """A control value that is an internal-bus **name** — a non-empty string that
    is not the hardware sentinel ``"OUT"`` — or ``None`` (a number, or ``"OUT"``)."""
    return value if (isinstance(value, str) and value and value != "OUT") else None


#: The measures a signal view can stack, in the order a reader thinks of them:
#: what the signal reached, and what it held inside that.
MEASURES = ("peak", "rms")


def _measure(name: str) -> str:
    """One layer's measure, or a `ValueError` naming it — a stack is written by
    hand, and a silent typo is a layer that quietly does not appear."""
    if name not in MEASURES:
        raise ValueError(f"unknown measure {name!r} (one of {', '.join(MEASURES)})")
    return name


def _name(element) -> str:
    """An element's display name: its own ``name`` when it has one (a group names
    itself, an automation names the control it drives), else what it *is* — an
    automation is an "envelope", not the `Element` that happens to wrap it."""
    name = getattr(element, "name", None)
    if isinstance(name, str) and name:
        return name
    auto = _automation(element)
    if auto is not None:
        return auto.name or "envelope"
    return type(element).__name__.lower()


def _automation(element):
    """The `clausters.seq.Automation` an element carries, or ``None``. An automation
    is a *curve* — the List/Buffer duality of the arrangement — so it needs no primitive
    of its own: any element wrapping one draws (and edits) as an envelope.

    A **simultaneous** group is searched too: an envelope attached to the event it
    shapes is one clip, and a curve edited on it must find the automation inside.
    """
    if isinstance(getattr(element, "wraps", None), Automation):
        return element.wraps
    if (isinstance(element, Group) and len(element) > 1
            and element.temporal_relation() == SIMULTANEOUS):
        for _offset, _dur, child in element.members:
            auto = _automation(child)
            if auto is not None:
                return auto
    return None


def _quads(flat) -> list:
    """A flat ``[t, v, shape, curve, …]`` break-point list as ``(t, v, shape,
    curve)`` tuples (a trailing partial quad is dropped)."""
    flat = list(flat)
    return [tuple(flat[i:i + 4]) for i in range(0, len(flat) - 3, 4)]


def _quintuples(flat) -> list:
    """A flat ``[start, dur, pitch, velocity, channel, …]`` note list as
    quintuple tuples (a trailing partial group is dropped) — the inverse of the
    piano-roll's `notes` wire form."""
    flat = list(flat)
    return [tuple(flat[i:i + 5]) for i in range(0, len(flat) - 4, 5)]


def _roll_owner(element):
    """The element whose notes a clip's roll body draws — what a ``"notes"``
    edit-back is written onto.

    Usually the element itself (a generator among them: it registers and the
    edit is refused later, which is where read-only is decided). A
    **simultaneous** group is the one that needs asking: it draws as one clip
    with its members' bodies layered, so the notes under the cursor belong to
    the member that carries them, not to the group. ``None`` when no member
    has an editable timeline — a layered roll nobody can write to."""
    if (isinstance(element, Group) and len(element) > 1
            and element.temporal_relation() == SIMULTANEOUS):
        for m in element.handles:
            if _editable_timeline(m.element) is not None:
                return m.element
        return None
    return element


def _editable_timeline(element):
    """The `clausters.seq.Timeline` an element's notes can be edited onto, or
    ``None``. A `clausters.form.Track` wraps one — the random-access, editable
    events container; a generator (a `Pbind`/`Routine`) does not, so it is
    forward-only and the piano-roll shows it read-only."""
    wraps = getattr(element, "wraps", None)
    return wraps if isinstance(wraps, Timeline) else None


def _rewrite_timeline(timeline, keep, new):
    """Rewrite a timeline in place: keep the items ``keep(item)`` is true for,
    drop the rest, and add ``new`` (a list of ``(beat, item)``) — so one kind of
    item (the notes) is replaced while the others (OSC/MIDI events) are preserved.
    Uses only the public timeline API (`range`/`clear`/`add`)."""
    kept = [(b, it) for (b, it) in timeline.range(0.0, float("inf")) if keep(it)]
    timeline.clear()
    for beat, item in kept + list(new):
        timeline.add(beat, item)


def _curve_range(points) -> tuple:
    """The value axis of a curve clip: the break-points' own range with a tenth of
    headroom (a flat curve still gets a band to be dragged in)."""
    values = [float(p[1]) for p in points] or [0.0]
    lo, hi = min(values), max(values)
    pad = (hi - lo) * 0.1 or (abs(hi) * 0.1 or 1.0)
    return lo - pad, hi + pad


def _pitch(item):
    """The MIDI pitch of a flattened item, or ``None`` when it carries none — an
    `OscEvent`/`MidiEvent`, a rest, an automation lane. Flattening yields the
    wrapped items, so a note is a `clausters.seq.Event`, whose `midinote`
    resolves an explicit pitch or a scale degree."""
    if not isinstance(item, SeqEvent) or item.get("type") == "rest":
        return None
    try:
        return float(item.midinote())
    except (KeyError, TypeError, ValueError):
        return None


def _velocity(item) -> int:
    """The MIDI velocity (``0..127``) of a flattened note event: an explicit
    ``velocity`` key if given, else the event's linear ``amp`` mapped to the
    velocity range, else the default 100 — so the piano-roll's velocity lane
    reflects the dynamics of the events."""
    vel = item.get("velocity")
    if vel is not None:
        return max(0, min(127, int(vel)))
    amp = item.get("amp")
    if amp is not None:
        return max(1, min(127, round(float(amp) * 127)))
    return 100


def _event_dur(item) -> float:
    """A flattened item's length in beats: an event's **sounding** time
    (``sustain``, which is what a note bar should show), 0 when it is punctual."""
    if isinstance(item, SeqEvent):
        try:
            return float(item.sustain())
        except (KeyError, TypeError, ValueError):
            return 0.0
    return 0.0
