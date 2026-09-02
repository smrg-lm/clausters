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

**Beats meet samples here, on two ratios and not one.** The arrangement places
elements in *beats* and measures each one's length in the unit of its own data
(seconds for a take, beats for a phrase of events); the multitrack view places
clips in *timeline samples*, because a clip's body is audio data and its sample
0 sits at the clip's offset. The editor is the only converter, and it holds both
ratios: an **onset** crosses on `units_per_beat` (`sample_rate / tempo`) and a
**length in seconds** on `units_per_second` (the rate itself), so a take is as
wide as it sounds whatever the tempo is and only its placement follows the
grid. A musical `quant` becomes the lane's drag grid, so the grid a clip is
dropped on is the grid the arrangement re-schedules on. The arithmetic itself is
the core's (`beats_to_secs` → `secs_to_samples`), not a second implementation.

**One mapping rule, not a heuristic per case.** The root `Aggregate`'s members are
the *lanes*; a lane's members are its *clips*; a `Vector` clip draws its take, a
element of events draws a piano-roll, and a nested `Aggregate` draws as a labeled
rectangle — its summary — until it is `expand`ed into lanes of its own. That
collapse/expand *is* the arrangement's base level (the zoom that summarizes a
aggregate or resolves it), so it needs no protocol of its own.
"""

import copy
import itertools
import weakref

from .. import _native
from ..base.time import TempoMap
from .handle import WindowHandle
from ..form.document import (FIRST_VERSION, ID_ATTR, leaf_config, leaf_node,
                             next_node_id, to_document)
from ..form.aggregate import CONCRETE, LOGICAL, SIMULTANEOUS, Aggregate
from ..form.element import (BEATS, SECONDS, Element, Clang, Segment, Segments,
                            Vector, to_beats)
from ..defs.ugens import points_to_env
from ..form.render import flatten
from ..seq.automation import Automation
from ..seq.event import Event as SeqEvent
from ..seq.timeline import MidiItem, OscItem, Timeline
from .editing import Editing
from .guidef import (_flat_notes, _flat_points, clip, patch, pianoroll, signal,
                     scroll, timeruler, track, waveform, window)
from .transport import Transport

__all__ = ["Editor"]

#: The pitch range a piano-roll lane falls back to when its notes give none
#: (C3..C6 — the span a melodic line usually lives in).
DEFAULT_PITCH = (48.0, 72.0)
#: Semitones of headroom above and below the notes of a piano-roll clip.
PITCH_PAD = 2.0


class _Placed:
    """What a clip widget was drawn from: the placement it shows (``owner`` aggregate
    and ``member`` handle, the arrangement's stable identity), the ``base`` in beats its
    aggregate sits at (a clip's offset is absolute on the shared axis, a placement is
    relative to its aggregate — this bridges the two), and the ``offset``/``dur`` in
    timeline units it was drawn with (so an edit-back can tell what actually
    moved)."""

    __slots__ = ("owner", "member", "base", "offset", "dur")

    def __init__(self, owner, member, base, offset, dur):
        self.owner = owner
        self.member = member
        self.base = float(base)
        self.offset = float(offset)
        self.dur = float(dur)


def _resolve_host(host):
    """The host an ``open`` acts on: the one named, else the ambient one — the
    same resolution `clausters.gui.guidef.View.open`, `clausters.plot` and
    `clausters.scope` share, so an editor is not the one resource that has to be
    handed a host."""
    if host is not None:
        return host
    from ..plot import _ambient_host

    return _ambient_host()


class Editor:
    """A composition on screen: the arrangement tree drawn as a multitrack view,
    editable back into the tree.

    Args:
        element: the composition — a `clausters.form.aggregate.Aggregate` (its members
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
                 tempo_map=None,
                 quant: float = 0.0, follow: bool = False, extra=(),
                 title: str = "Composition",
                 width: int = 1000, height: int = 520, base_id: int = 10_000):
        self.element = element
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
        #: (the base level: an aggregate resolved rather than collapsed).
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
        # The composition's version, the held document, the history over it and
        # the index between them are **not fields of this editor**: they belong
        # to the arrangement, and a second window over one composition reaches
        # the same `Editing` context through `_editing`. A log kept here would
        # see only the gestures this editor made, so a script editing the
        # arrangement or a second view would leave it describing a composition
        # that has moved on, and undo would then write a state nobody was ever
        # in. The properties below read that context.
        #: Which **edit layer** of each clip the hand is on -- the placement, a
        #: roll, a curve. Screen state like a selection: the composition does
        #: not change when it moves, and the document is explicit that what a
        #: view is currently editing is never part of it.
        #:
        #: Keyed by the **placement's node**, not by the widget drawing it. A
        #: widget id is the *drawing's* name for something and is minted afresh
        #: every time the window is redefined, so anything keyed by one is
        #: silently emptied by a structural edit -- and a missing key and "the
        #: default layer" are the same answer, which is why nothing noticed. A
        #: node id is the arrangement's own, stamped by `to_document` and kept
        #: across a re-derivation, so it outlives the picture the way this state
        #: is supposed to. `edit_layer` reads it.
        self._edit_layer: dict = {}
        #: Whether the last edit changed **which members exist** -- a split, a
        #: join, a cord. A placement is a prop the host can be told about; a
        #: widget that was not there is not, so this is what says a redefine is
        #: owed. Read and cleared by `_restructure`.
        self._restructured = False
        #: The **oldest version an incoming edit may name**: raised whenever
        #: the composition moves by a route that is not a host event, and by
        #: nothing else. See `_stale`, the only thing that reads it.
        self._floor: int = FIRST_VERSION
        #: The version this editor was at when it last answered a host event --
        #: what turns "the version moved" into "it moved *by someone else*".
        self._applied: int = FIRST_VERSION
        #: The **value axis** each curve is drawn against, by `Automation` —
        #: remembered rather than recomputed, which is screen state for the same
        #: reason the edit layer is.
        #:
        #: A break-point's position on screen is its value *against this axis*,
        #: so an axis derived from the break-points moves every point whenever
        #: any one of them is dragged: the curve jumps under the hand that is
        #: editing it, and the point being dragged is the only one that appears
        #: not to move. So the axis is fixed the first time a curve is drawn and
        #: kept, widening only when the data no longer fits inside it — never
        #: shrinking, so a point dragged down and back up leaves the picture
        #: where it was.
        self._curve_axis: "weakref.WeakKeyDictionary" = weakref.WeakKeyDictionary()
        #: patch widget id -> (logical `Aggregate`, its box-order member handles) —
        #: the directed-patch view of a logical aggregate, for its edit-back route.
        self._patches: dict = {}
        #: id(aggregate) -> {box index: (x, y)} — a patch's box placements, presentation
        #: only (a logical aggregate is a signal graph, so positions live here, not in
        #: the arrangement). Keyed by aggregate identity, so they survive a redraw.
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
            tempo_map=self.tempo_map, sample_rate=self.sample_rate,
            extent=self.extent)
        #: Whether the arrangement changed since the last render — an edit does not
        #: interrupt what is playing, so a transport (play, a resume after pause, a
        #: seek) reads this to know it must re-read the composition.
        self.dirty = False

    # ---- the unit bridge: beats (the data) ↔ timeline samples (the view) ----

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

    def length_to_units(self, length: float, element, at: float = 0.0) -> float:
        """A length of ``element``, in that element's own unit, as timeline
        samples — the one place the editor decides which of the two ratios a
        number crosses on.

        ``at`` is where the length **starts**, in beats. A length in seconds
        does not need it (its seconds are already fixed), but a length in beats
        does: beats are a logical coordinate, so the same count of them is a
        different stretch of time depending on where it sits, and only two
        positions can say how long it is.
        """
        if getattr(element, "duration_unit", BEATS) == SECONDS:
            return self.secs_to_units(length)
        return self.beats_to_units(at + length) - self.beats_to_units(at)

    def units_to_length(self, units: float, element, at: float = 0.0) -> float:
        """Timeline samples as a length of ``element``, in that element's own
        unit — the inverse of `length_to_units`, and what an edit-back writes
        back onto the arrangement. ``at`` is the length's start in beats, for
        the same reason."""
        if getattr(element, "duration_unit", BEATS) == SECONDS:
            return self.units_to_secs(units)
        return self.units_to_beats(self.beats_to_units(at) + units) - at

    # ---- the base level: collapse (a summary rectangle) vs expand (lanes) ----

    def expand(self, element) -> "Editor":
        """Resolve a nested `Aggregate` into lanes of its own (instead of the labeled
        rectangle that summarizes it). The arrangement's *base level*, made an edit."""
        self._expanded.add(id(element))
        return self

    def collapse(self, element) -> "Editor":
        """Summarize a nested `Aggregate` back into one labeled rectangle."""
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
        member of the root aggregate, each holding its members as clips on the shared
        time axis. Pure — it builds the tree and the id registry, and sends
        nothing.

        A **logical** aggregate draws as a directed `patch` (a server patch, not
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
        if isinstance(root, Aggregate) and root.kind == CONCRETE:
            for member in root.handles:
                if isinstance(member.element, Aggregate) and member.element.kind == LOGICAL:
                    lanes.append(self._patch_lane(member.element))
                else:
                    lanes += self._lanes_for(member.element, member.offset, root, member)
        elif isinstance(root, Aggregate) and root.kind == LOGICAL:
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

    def _patch_lane(self, aggregate) -> dict:
        """A logical aggregate drawn as a directed `patch` inside a pan/zoom `scroll`
        workspace — a server patch among the timeline lanes. Registers the patch
        widget id so an edit-back resolves to the aggregate it draws."""
        p, handles = _logical_patch(aggregate)
        wid = self._new_id()
        self._patches[wid] = (aggregate, handles)
        geometry = self._patch_geometry.get(id(aggregate), {})
        content = (900.0, 700.0)
        view = patch(id=wid, **p.to_widget(geometry), label=_name(aggregate),
                     x=0.0, y=0.0, w=content[0], h=content[1])
        return scroll(view, id=self._new_id(),
                      content_w=content[0], content_h=content[1])

    def open(self, host=None, id: int | None = None) -> "WindowHandle":
        """`draw` the composition and open it on ``host`` (a
        `clausters.gui.host.GuiHost`), or on the **ambient** host when none is
        named — the same rule `clausters.gui.guidef.View.open`, `clausters.plot`
        and `clausters.scope` follow.

        Returns the **window handle** `clausters.gui.host.GuiHost.open` hands
        back: it equals the window id, and it also resolves the tree's named
        widgets, so the transport buttons are reachable by name
        (``win["play"].on_event(...)``)."""
        host = _resolve_host(host)
        self._host = self.transport.host = host
        self._mode = "multitrack"
        self._window = host.open(self.draw(), id=id)
        self._editing.attach(self)
        self._announce()
        return self._window

    def adopt(self, intents: list, whole: bool) -> None:
        """Another view of this composition edited it: bring this window in step.

        **Props where props can carry it.** A placement, a length, a curve and a
        note list are values, so a foreign edit reaches this window the way its
        own edits do — the drawn record is brought back in step and the widgets
        are resynced. A redefine here would rebuild every widget and drop what
        the host had in flight, which makes a window flicker under a hand that
        is not even in it; it is the same reason `_restructure` is deliberately
        not a redraw after every edit.

        ``whole`` is the case no prop can carry: a widget that was not there a
        moment ago — a cut, a split, a join, an undo of one — or a turn that
        changed something and projected no intent at all. Then it is `update`,
        which is exactly what that method was written for: *the route a change
        the editor did not apply arrives by*.

        A window that is not open has nothing to bring in step, and says so by
        doing nothing: an editor that was never opened still shares the history,
        it just has no picture. So does one that draws none of what moved.
        """
        if self._host is None or self._window is None:
            return
        if whole:
            self.update()
            return
        widgets = set()
        for intent in intents:
            widgets |= self._reflect(intent)
        if not widgets:
            return
        self._corrections = []
        for wid in widgets:
            self._resync(wid)
        # A stamp of zero retires nothing, which is what an unasked push needs:
        # this window answered no gesture.
        self._acknowledge(0)
        self._corrections = []

    def _reflect(self, intent: dict) -> set:
        """The drawn half of `_project`, for an edit **another view applied**.

        The arrangement is already written — the two views hold the same
        objects — so what is left is this window's own record of what it drew,
        and which of its widgets is now drawing something else. A view that does
        not draw the node an intent names answers with nothing, which is the
        ordinary case for a dedicated roll beside a multitrack.
        """
        found = self._by_node.get(int(intent.get("node", -1)))
        if found is None:
            return set()
        _owner, member, element = found
        if intent.get("intent") == "place" and member is not None:
            wid = self._redrawn(element, member)
        else:
            wid = self._widget_of(element, member)
        return set() if wid is None else {wid}

    def edit_layer(self, element, member=None) -> "str | None":
        """Which layer of a clip the hand last worked on — the placement, a
        roll, a curve — or ``None`` where nothing has been touched.

        Screen state, so it is read rather than persisted or logged, and it is
        the answer to "what is this window currently editing". Takes what
        `Aggregate.add` handed back (the placement) the way every other route
        here does, since a clip is a window onto an element and the layer
        belongs to that window.
        """
        node = self._node_id(element, member)
        return None if node is None else self._edit_layer.get(int(node))

    def refresh(self) -> None:
        """Tell this editor that the arrangement moved by a route it did not
        take, so the document it holds has to be derived again.

        **The door a held document needs.** The editor keeps one `Document` for
        the composition's life, which is what makes a gesture cost the edit
        rather than the composition. The price is that a script mutating the
        arrangement while a window is open — adding a clip, rewriting a
        timeline — is no longer absorbed by a rebuild that used to happen on
        every gesture: without this the next edit would be made against a
        composition that has moved, and the crate would refuse it as stale (or,
        for a node the script removed, not find it at all).

        Cheap and safe to call: `to_document` stamps each element with the id it
        keeps, so a re-derivation names the same nodes and the history keeps its
        footing. Call it after editing the tree behind the editor's back; the
        editor calls it for itself wherever *it* writes the arrangement directly.
        """
        self._rederive = True
        # The arrangement moved by a route no gesture took -- a picture a step
        # still in flight was made against is now gone, and `_stale` has to say
        # so. It takes no version of its own, so the floor is raised here rather
        # than noticed later.
        self._floor = self._version

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
        # The history is **not** dropped here, and that is the point of where it
        # lives: it belongs to the composition, so pointing this window at
        # another one simply reaches that composition's context. An undo can no
        # more walk back into a piece this window is not showing than it could
        # walk into one another window is.
        self._floor = FIRST_VERSION
        self._applied = FIRST_VERSION
        self.dirty = True
        if self._host is not None and self._window is not None:
            self._reset_ids()
            self._host.define(self._window, self.draw())
            self._announce()

    def _draw_pianoroll(self) -> dict:
        """The dedicated piano-roll view: one `pianoroll` widget drawing a single
        events element's MIDI notes (grid) and OSC markers (lane), instead of a
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
        body = self._source_of(element)
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

    def _source_of(self, element) -> dict:
        """The source props a signal view draws ``element``'s samples from, or a
        `ValueError` naming what is missing.

        **This is the generated/generator distinction, asked at the door.** A
        rendered element has samples a view can address — a buffer the host
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

    def open_signal(self, host=None, element=None, *, layers=("peak", "rms"),
                    id: int | None = None) -> "WindowHandle":
        """`draw` a single **rendered** element as a dedicated signal view and
        open it on ``host`` (or the ambient host, like `open`) — the
        editor-grade view of one element's samples, as opposed to `open`, where
        the same samples are only a clip's body.

        ``layers`` is what the picture measures, and the `layers` property
        changes it **live** on the open view: ``("peak", "rms")`` is the
        editor's picture — what the signal reached with the level it held drawn
        inside it — and ``("peak",)`` is the bare envelope. They are measures of
        **one** `clausters.gui.guidef.waveform`, not a pile of widgets: a view
        of a signal paints its own field before it draws, so two of them on one
        rectangle would not layer, and one view is also one axis, one ruler, one
        selection, one playhead and one upload of the samples.

        The element must have **samples**: a rendered take, not a generator
        (see the error a generator raises). Returns the **window handle**, like
        `open`.
        """
        element = self.element if element is None else element
        # Refused **before** a window exists: an unknown measure and an element
        # with no samples are both answers to the call that was made, and
        # finding out at the first repaint would leave an empty window behind.
        stack = tuple(_measure(m) for m in layers)
        self._source_of(element)
        host = _resolve_host(host)
        self._host = self.transport.host = host
        self._mode = "signal"
        self._signal_element = element
        # Straight to the field: there is no window yet, so this is what the
        # first draw measures rather than something to push at one.
        self._layers = stack
        self._window = host.open(self.draw(), id=id)
        self._editing.attach(self)
        self._announce()
        return self._window

    def open_pianoroll(self, host=None, element=None,
                       id: int | None = None) -> "WindowHandle":
        """`draw` a single events element as a **dedicated piano-roll** window
        and open it on ``host`` (or the ambient host, like `open`) — the
        editor-grade note view (a keyboard, an
        editable note grid, a velocity lane, an OSC lane) of one MIDI/OSC
        element, as opposed to `open`, where the same notes are only a clip body.

        Edits write back through `poll` exactly as the multitrack does, **when the
        element is editable** — a `clausters.form.Track` (a
        `clausters.seq.Timeline`): a dragged, added or removed note is rebuilt onto
        its timeline. A **generator** (a `Pbind`/`Routine`) is forward-only, so its
        bounced notes are shown *read-only* (bounce it to a `Track` to edit). OSC
        markers are shown but not edited back yet (one carries only its time and
        address, not the full message).

        Returns the **window handle**, like `open`."""
        host = _resolve_host(host)
        self._host = self.transport.host = host
        self._mode = "pianoroll"
        self._roll_element = self.element if element is None else element
        self._window = host.open(self.draw(), id=id)
        self._editing.attach(self)
        self._announce()
        return self._window

    def extent(self, element=None) -> float:
        """The composition's length in beats, **read from the arrangement** — the
        end of its last placed element. It is not a constant: move a clip past the end
        and the piece gets longer, which is exactly what a transport must ask
        (a hard-coded length would cut the playback short at the old end).

        Beats whatever the element is measured in: a transport is on a clock, so
        an element that is its own length in seconds (a lone take opened as a
        composition) crosses here."""
        element = self.element if element is None else element
        unit = element.duration_unit if isinstance(element, Element) else BEATS
        return to_beats(self._extent(element), unit, self.tempo)

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
        element added, an aggregate expanded). A mere placement change needs no redefine: the
        host already moved the clip that was dragged.

        A redefine **moves the version**, and that is the point rather than a
        side effect: this is the route a change the editor did not apply arrives
        by — a script adding an element, an aggregate expanded, a re-render — and it
        is the case an edit log cannot see. It also rebuilds the widgets, so a
        gesture still in flight was made against a picture that no longer
        exists; the bump is what makes that edit come back as stale instead of
        landing on whatever now holds its id."""
        if self._host is None or self._window is None:
            raise RuntimeError("open(host) the editor first")
        self._version += 1
        # **And the document moves with it.** The crate refuses an edit whose
        # `against` version is not the document's -- ahead of it as loudly as
        # behind, since the two would not be talking about the same piece -- so
        # a version bumped here and nowhere else left the editor answering
        # "this edit was made against a different document" to every gesture
        # after the redefine, silently and forever. Re-deriving stamps the
        # document with the version this window is now drawing; it is what
        # `refresh` does, and a redefine is the case that most needs it (the
        # tree it redraws is usually one the log did not make).
        self._rederive = True
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
        placement it came from and written with `Aggregate.move`. The clip's offset is
        **absolute** on the shared axis while a placement is relative to its
        aggregate, so the position converts back through the base the clip was drawn
        at; and only what actually moved is written — a drag carries the clip's
        unchanged ``dur`` along, and snapping *that* to the grid would silently
        shorten the element. ``/gui_closed`` drops the window (its own — the
        payload names the window id); anything else is ignored, so a whole poll
        loop can be fed straight in — even one shared with a second editor
        (a dedicated piano-roll beside the multitrack, say): every route resolves
        through this editor's own registries, so another window's events fall
        through untouched.

        A logical aggregate's directed patch routes here too: a ``"wire"`` rewrites the
        two members' controls onto a shared bus (`_apply_patch`), a ``"move"``
        persists a box's canvas position.

        **Every other window over this composition is told**, on the way out.
        Nothing else would do it: an acknowledgement goes to the window whose
        gesture it answered, so a second view would go on drawing a piece that
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
            # **What it answers is whether anything moved**, not whether the
            # keystroke was understood. A history at its end is the ordinary
            # case -- a person holds Ctrl+Z until it stops -- and reporting a
            # change there told every other view of this composition to bring
            # itself in step with an edit that never happened, which is a redraw
            # for nothing and, before the axis learned to survive one, a zoom
            # reset for nothing. The acknowledgement still goes out: the host
            # asked, and the answer is the state that holds.
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
        self._applied = self._version
        # Answered whatever happened, and answered with a *value*. There is no
        # success flag: the state this editor decided rides as the corrections
        # `_route` collected, and a refusal is simply the previous value among
        # them. Applied, transformed and refused are one message.
        self._acknowledge(seq, reason=self._reason)
        # ...and *then* the redefine, when the gesture added or removed a
        # member: the answer retires what the host had in flight, and the new
        # tree is what shows a clip that was not there.
        self._restructure()
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
            # Read-only samples: a generator's notes are a *rendering* of an
            # algorithm, so the edit is refused -- and the refusal is the notes
            # as they still are, sent back so the host stops drawing the one the
            # hand moved. This is the case that used to be silent.
            #
            # It says **why**, which is the whole of what the reason is for: a
            # note that springs back with nothing attached teaches "sometimes it
            # does not work" rather than "not here". (The picture still moves
            # under the hand and snaps back, because nothing tells the host this
            # body is read-only *before* the gesture -- a widget capability the
            # protocol does not carry yet.) Two different things are read-only
            # here and naming the wrong one is worse than naming none: a
            # **generator**'s notes are a rendering of an algorithm, while a
            # single placed **event** has no timeline to write onto at all --
            # its position is the placement's, so the clip is what moves.
            self._reason = (
                "this clip draws what a generator produced; render it to a "
                "track to edit its notes"
                if _editable_timeline(element) is None
                and not isinstance(element, Clang)
                else "this clip is one placed event; drag the clip to move it, "
                     "a track is what holds editable notes")
            self._correct(int(args[0]), notes=_flat_notes(self._notes(element)))
            return False
        if int(args[0]) in self._patches:
            # A logical aggregate's directed patch: a cord drawn (rewire) or a box
            # moved (presentation).
            return self._apply_patch(int(args[0]), args[1], args[2:])
        placed = self._clips.get(int(args[0]))
        if placed is None:
            return False
        if args[1] == "points":
            return self._apply_points(placed, args[2:])
        if args[1] == "layer":
            # Which layer of a clip the hand is on is **screen state**, like a
            # selection: the composition did not change, and the document is
            # explicit that what a view is currently editing is never part of
            # it. It is kept so a driver can ask -- under the *placement's* node
            # rather than the widget's id, so it survives the window being
            # redrawn (see the field).
            node = self._node_id(placed.member.element, placed.member) \
                if placed.member is not None else None
            if node is not None:
                self._edit_layer[int(node)] = str(args[2])
            return False
        if args[1] == "split":
            return self._apply_split(placed, float(args[2]))
        if args[1] == "join":
            return self._apply_join(placed, [int(a) for a in args[2:]])
        if args[1] != "clip" or len(args) < 4:
            return False
        if placed.member is None:
            return False  # the root element itself: nothing places it

        offset, dur = float(args[2]), float(args[3])
        # The **window** the trim left behind, when the host stated one: where
        # in its samples the clip now reads. A host older than windows sends
        # three arguments and means "from the beginning", which is what a take
        # with no window has always been.
        window = float(args[4]) if len(args) > 4 else None
        moved = abs(offset - placed.offset) >= 0.5      # half a sample: a real edit
        resized = abs(dur - placed.dur) >= 0.5
        trimmed = self._window_moved(placed, window)
        if not (moved or resized or trimmed):
            return False
        if trimmed:
            # A trim is a gesture of its own -- the placement *and* the window
            # over the samples, in one edit -- so it does not go through the
            # placement road below.
            if not self._trim(placed, offset, dur, window):
                return False
            placed.offset, placed.dur = offset, dur
            self._version = self._document.version
            self.dirty = True
            self._follow_render()
            return True

        member = placed.member
        # Absolute (the axis) -> relative (the placement). The **grid is not
        # applied here**: the intent states where the hand put it and the crate
        # snaps, which is the rule the whole document exists for -- one place
        # decides what an edit becomes, and the value that comes back is what
        # actually happened.
        asked_offset = (self.units_to_beats(offset) - placed.base if moved
                        else member.offset)
        # **The length goes back in the unit of what it measures.** A placement
        # is musical and crosses on the beat; a clip of samples is as long as
        # its seconds, and dividing those by the beat would write a length the
        # next tempo change moves.
        asked_dur = (self.units_to_length(dur, member.element,
                                          self.units_to_beats(offset)) if resized
                     else member.dur)
        node = self._node_id(member.element, member)
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
        snapped_dur = (dur if new_dur is None
                       else self.length_to_units(float(new_dur), member.element,
                                                 float(effective["offset"]) + placed.base))
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
        is the behavior there was before there were versions at all.

        Overtaken means *by a route the host never saw*. Every version this
        editor made while answering the host's own events is one the host is
        either about to be told or has been told already, so an edit naming one
        of them is an answer that had not arrived yet -- a drag's later frames,
        a second gesture begun inside one round trip. What raises the floor is a
        script's edit, a second editor's, a redefine, an undo: the cases where
        the picture the gesture was made against is gone."""
        return against != 0 and against < self._floor

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
            auto = _automation(placed.member.element if placed.member is not None
                               else self.element, self.tempo)
            if auto is not None:
                # A curve is as much of "what this widget should be drawing" as
                # a placement is, and an undone one is the case that needs it:
                # nothing else would tell the host the break-points moved back.
                # Flat: already-resolved quads, as `_body_for` sends them.
                props["points"] = _flat_points(
                    [x for t, v, shape, curve in _quads(auto.to_points())
                     for x in (self.secs_to_units(t), v, shape, curve)])
            held = (placed.member.element if placed.member is not None
                    else self.element)
            if isinstance(held, Vector):
                # A take's **window** is as much of what this widget draws as
                # its placement is: an undone trim puts the frames back, and
                # nothing else would say so. Stated even when it is zero --
                # absence is a value here as everywhere, and a prop left out is
                # a prop left standing.
                props["start"] = float(held.start)
                props["loop"] = bool(held.loop)
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
        learn neither -- so a note dragged onto read-only samples stayed drawn
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
        covers entirely leaves the aggregate it was in — undoably, through the
        crate, like every other edit here. What it does *not* do is trim: a
        selection cutting across a clip implies a new length for the samples
        under it, and writing samples is the business of whoever owns them
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
        # aggregate, and the selection is absolute, which is the same bridge a drag
        # crosses in the other direction.
        at = placed.base + member.offset
        span = at, at + (member.length or 0.0)
        if not (start <= span[0] and end >= span[1]):
            self._resync(wid)
            self._reason = (
                "a cut across a clip is a new length for its samples, "
                "which is the buffer owner's edit"
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
        samples, and samples are written by whoever owns them against a working
        copy; an arrangement editor placing a nameless block of audio would be
        inventing both a source and a source's owner. So a sample paste is
        refused with the reason, which is the honest answer until the samples
        half of the track lands.
        """
        if wid not in self._clips and wid not in self._lanes:
            return False
        kind = str(values[1]) if len(values) > 1 else ""
        self._reason = (
            f"this editor places elements; a {kind or 'clipboard'} block is "
            "samples, and samples are written by their owner"
        )
        return False

    def resolve_selection(self) -> list:
        """The **samples under the current selection**, through the crate.

        The other half of what a selection is for: `Editor.selection` says what
        was swept, and this says what is underneath it — one entry per leaf,
        with the placement's base, the element's trim and the clamp at both ends
        already applied (`clausters._native.Document.resolve`). Empty when
        nothing with samples was under the sweep, and when there is no selection at
        all.

        The value range travels with the selection but does not narrow this:
        what is under a band of amplitudes is the same samples as what is under
        the whole span, and *reading only those samples* is an operation over
        the range rather than a resolution of it.
        """
        if not self.selection:
            return []
        _, document = self._history()
        return document.resolve(self.selection,
                                frames_per_beat=self.units_per_beat,
                                frames_per_second=self.units_per_second,
                                in_beats=True)

    def _window_moved(self, placed, start) -> bool:
        """Whether the host reported a **window** that is not the one the
        element already reads — half a frame's worth, the same threshold a move
        uses."""
        if start is None or placed.member is None:
            return False
        element = placed.member.element
        return (isinstance(element, Vector)
                and abs(float(start) - float(element.start)) >= 0.5)

    def _trim(self, placed, offset: float, dur: float, start: float) -> bool:
        """A **trim**: the clip begins later or ends earlier, and the window over
        its samples moves with the edge.

        One intent, not two. Where a clip sits is its placement's and what it
        reads is its element's, so a trim touches both -- and a gesture that
        recorded them separately would take two undos to reverse, the first of
        which leaves a clip showing frames it does not play. The parent's
        members carry both (each member is a placement *and* the node it holds),
        so `setmembers` states the result of the whole gesture and an undo puts
        back the frames the trim hid.
        """
        member, owner = placed.member, placed.owner
        if member is None or owner is None:
            return False
        node = self._node_id(owner)
        index = next((i for i, h in enumerate(owner.handles) if h is member), None)
        if node is None or index is None:
            return False
        whole = to_document(owner, version=self._version)["root"]
        members = [dict(m) for m in whole.get("members", [])]
        if index >= len(members):
            return False
        edited = members[index]
        edited["offset"] = self.units_to_beats(offset) - placed.base
        edited["dur"] = self.units_to_length(dur, member.element,
                                             self.units_to_beats(offset))
        edited["node"] = dict(edited["node"])
        config = dict(edited["node"].get("config") or {})
        config["start"] = float(start)
        edited["node"]["config"] = config
        outcome = self._record({"intent": "setmembers", "node": node,
                                "members": members}, "trim the clip")
        if outcome is None or not outcome["applied"]:
            self._resync(self._widget_of(member.element, member) or 0)
            return False
        self._project(outcome["effective"])
        return True

    def _apply_split(self, placed, at_units: float) -> bool:
        """A clip cut in two at ``at_units`` of its own time.

        Both halves keep the **same samples** and take a window of it: the
        first reads what it always did and stops early, the second begins where
        the first left off. That is the whole of what a split is on a memory
        view, and it is why the frames neither of them shows are still there --
        stretching either half back brings them out again.

        The second half is built here rather than left for the projection to
        invent, because an element is an object this client holds and a document
        node only *describes* one. What goes into the log is **one** intent over
        the parent's members, so an undo puts the clip back whole rather than
        leaving half of it behind.
        """
        member, owner = placed.member, placed.owner
        if member is None or owner is None:
            return False
        element = member.element
        if not isinstance(element, (Vector, Segments)):
            # Only a window onto samples can be cut into windows. Splitting a
            # pattern or an aggregate would have to say what half of an algorithm
            # is, which is a different question and not this one.
            self._reason = ("only a clip over samples can be split: this one "
                            "holds " + _name(element))
            return False
        length = member.length if member.length is not None else element.duration
        # In the element's own unit -- seconds, for the only elements a split
        # applies to -- because that is what the placement's length is in.
        at = self.units_to_length(at_units, element,
                                  placed.base + (member.offset or 0.0))
        if length is None or not (0.0 < at < float(length)):
            return False
        node = self._node_id(owner)
        if node is None:
            return False
        second_element = self._tail(element, float(at), float(length))
        # The cut, on the arrangement: the first half stops early — its
        # *placement* does, the element is untouched — and the second is placed
        # where it stops. Stamped with an id of its own **before** any
        # conversion sees it, or the next one would renumber the tree around it.
        was_dur = member.dur
        member.dur = float(at)
        # The onset is the aggregate's, so it is in beats: the cut's own
        # seconds cross here and nowhere else.
        onset = member.offset + to_beats(float(at), element.duration_unit, self.tempo)
        handle = owner.add(second_element, onset, float(length) - float(at))
        setattr(handle, ID_ATTR, self._mint_id())
        whole = to_document(owner, version=self._version)["root"]
        outcome = self._record({"intent": "setmembers", "node": node,
                                "members": whole.get("members", [])},
                               "split the clip")
        if outcome is None or not outcome["applied"]:
            # Put the arrangement back: the log refused, so nothing happened.
            owner.remove(handle)
            member.dur = was_dur
            self._resync(self._widget_of(element, member) or 0)
            return False
        # No projection: the tree already *is* what the intent says. What the
        # index has to learn is the element that was not there a moment ago --
        # and so does the window, which has one clip where there are now two.
        self._rederive = True
        self._restructured = True
        return self._changed()

    def _tail(self, element, at: float, length: float):
        """The element the **second half** of a cut reads: the same samples,
        from ``at`` seconds in (``at`` and ``length`` are the element's own
        unit, which for samples is seconds).

        The first half is not built at all — it is the element it always was,
        with its placement shortened, which is the arrangement's own rule (a
        placement is a window onto an element, never a rewrite of it) and what
        makes an undo of a split one step. A `Vector` gives a window that starts
        further in; a `Segments` gives the segments past the cut, with the one
        the cut falls inside cut into two.
        """
        if isinstance(element, Segments):
            after = []
            for offset, seg in element.placed():
                end = offset + seg.duration
                if offset >= at - 1e-9:
                    after.append(seg)
                elif end > at + 1e-9:
                    head = at - offset
                    after.append(Segment(seg.buffer,
                                         seg.start + self.secs_to_units(head),
                                         seg.duration - head))
            return self._joined_element([element], after)
        return Vector(element.wraps, duration=length - at,
                      instrument=element.instrument, controls=element.controls,
                      start=element.start + self.secs_to_units(at),
                      loop=element.loop, name=element.name)

    def _segments_within(self, element, length) -> list:
        """The segments a placement of ``length`` seconds actually shows of a
        `Segments` — the placement being a window onto the samples like every
        other placement here, so a half whose placement was shortened by a split
        holds the samples it *plays*, not everything the element still knows
        about.
        """
        if length is None:
            return element.segments
        out = []
        for offset, seg in element.placed():
            if offset >= float(length) - 1e-9:
                break
            room = float(length) - offset
            out.append(seg if seg.duration <= room + 1e-9
                       else Segment(seg.buffer, seg.start, room))
        return out

    def _apply_join(self, placed, ids: list) -> bool:
        """Clips read as one.

        Two shapes, one verb, and which one it takes is a fact about the
        samples rather than a mode: fragments that are **one run of one
        buffer** (what a split makes) join back into the single window they were
        cut from, and anything else becomes a `Segments` — the element whose
        contents are a list of windows onto whatever buffers they come from, read
        back to back. The second is what a multitrack means by joining in
        general: nothing is copied, and cutting it apart again gives the same
        windows back.
        """
        member, owner = placed.member, placed.owner
        if member is None or owner is None:
            return False
        run = [self._clips.get(i) for i in ids]
        run = [p for p in run
               if p is not None and p.member is not None and p.owner is owner]
        if len(run) < 2:
            return False
        run.sort(key=lambda p: p.member.offset)
        elements = [p.member.element for p in run]
        if not all(isinstance(e, (Vector, Segments)) for e in elements):
            self._reason = "only clips over samples can be joined"
            return False
        # The segments the run holds, in reading order: a `Vector` is one, a
        # `Segments` is however many it already carries.
        segments: list = []
        for p, element in zip(run, elements):
            length = p.member.length if p.member.length is not None else element.duration
            if length is None:
                self._reason = "a clip with no length has no samples to join"
                return False
            if isinstance(element, Segments):
                segments += self._segments_within(element, length)
            else:
                segments.append(Segment(element.wraps, element.start, float(length)))
        node = self._node_id(owner)
        if node is None:
            return False
        joined = self._joined_element(elements, segments)
        keep, dropped = run[0].member, {id(p.member) for p in run[1:]}
        total = sum(seg.duration for seg in segments)
        # The members as they would stand -- the document's own serialization,
        # with the run's first holding the joined samples and the rest gone.
        # Built rather than mutated, which is the cut's shape too: nothing on
        # this side moves until the crate has said what the edit becomes.
        whole = to_document(owner, version=self._version)["root"]
        members = []
        for handle, m in zip(owner.handles, whole.get("members", [])):
            if id(handle) in dropped:
                continue
            m = dict(m)
            if handle is keep:
                m["dur"] = total
                m["node"] = dict(m["node"])
                m["node"].update(leaf_node(joined))
            members.append(m)
        outcome = self._record({"intent": "setmembers", "node": node,
                                "members": members}, "join the clips")
        if outcome is None or not outcome["applied"]:
            return False
        # The kept placement now holds a different element, which the projection
        # cannot invent from a node: it is written here, where the object is.
        keep.element = joined
        keep.dur = total
        self._project(outcome["effective"])
        self._rederive = True
        self._restructured = True
        return self._changed()

    def _joined_element(self, elements: list, segments: list):
        """What a run of clips joins **into**: the single window they were cut
        from when they are one run of one buffer, else the `Segments` that reads
        their windows back to back.

        The first case is not an optimization — it is what makes a join the
        inverse of a split, so cutting and rejoining leaves the composition it
        started with rather than a list of one.
        """
        first = elements[0]
        instrument = getattr(first, "instrument", None)
        controls = getattr(first, "controls", None)
        contiguous = True
        expected = segments[0].start
        for seg in segments:
            if seg.buffer is not segments[0].buffer or abs(seg.start - expected) >= 0.5:
                contiguous = False
                break
            expected += self.secs_to_units(seg.duration)
        if contiguous:
            return Vector(segments[0].buffer,
                          duration=sum(seg.duration for seg in segments),
                          instrument=instrument, controls=controls,
                          start=segments[0].start,
                          loop=getattr(first, "loop", False),
                          name=first.name)
        return Segments(segments, instrument=instrument, controls=controls,
                        name=first.name)

    def _apply_points(self, placed, values) -> bool:
        """A curve edited in place on an automation clip (the flat ``"points"``
        payload the `bpf` view also sends): the break-points go back onto the
        element's `clausters.seq.Automation`, with their times converted from
        timeline units to the seconds an `Env` measures its segments in. The `Env` is the automation's source of truth, so
        this *is* the edit — the next render plays the curve as drawn."""
        clip = (placed.member.element if placed.member is not None
                else self.element)
        # **The leaf that carries the curve, not the clip that draws it.** A
        # simultaneous aggregate is one clip with its members' bodies layered,
        # so an envelope drawn on it belongs to a *member* — and a `Configure`
        # addressed to the aggregate replaced an empty configuration with a
        # `points` the crate had nowhere to keep: the edit reported success,
        # changed nothing and left no undo behind.
        element, member = _curve_owner(clip, placed.member, self.tempo)
        auto = _automation(element, self.tempo)
        if auto is None or not values:
            return False
        flat = []
        for t, v, shape, curve in _quads(list(values)):
            flat += [self.units_to_secs(t), float(v), int(shape), float(curve)]
        node = self._node_id(element, member)
        if node is None:
            return False
        # **Through the log, like every other edit.** A curve's break-points are
        # a leaf's configuration, so the intent is a `Configure` — and it
        # replaces the configuration whole, which is why it starts from what the
        # leaf already carries rather than from the points alone. Writing the
        # `Env` here instead is what made undo work for clips and for nothing
        # inside one: the edit landed on the object and left no entry behind.
        config = leaf_config(element)
        config["points"] = flat
        outcome = self._record({"intent": "configure", "node": node,
                                "config": config}, "edit the curve")
        if outcome is None:
            return False
        # The effective value is the crate's, and `_configure` is the one door
        # that writes it onto the automation *and* refills the control buffer
        # the lane synth reads — so the envelope, the sound and the picture
        # cannot disagree about which of the three happened.
        self._project(outcome["effective"])
        return self._changed(outcome["applied"])

    def _apply_notes(self, element, values) -> bool:
        """Notes edited in a roll — a clip's body or the dedicated piano-roll
        alike, since both send it (the flat ``"notes"`` payload,
        `start dur pitch velocity channel` quintuples): written onto the element's
        editable `clausters.seq.Timeline` as `clausters.seq.Event`s, times converted to beats,
        preserving any OSC/MIDI items already on it. Returns ``False`` for a
        forward-only generator element (read-only), so the edit is a no-op.

        **An edit updates the note it names; it does not rebuild it.** The i-th
        note of the payload is the i-th note the roll drew (order is the only
        identity the payload carries), so its event is *copied* and the edited
        fields written onto the copy — which keeps everything the roll cannot
        say: the instrument, and whatever else the author put on that event.
        Rebuilding one from the five numbers instead dropped all of it.

        **And the length it carries is the note's `sustain`.** A roll draws what
        a note *sounds* (`Event.sustain`, which is ``dur * legato`` when nothing
        says otherwise), so that is what a drag on its edge sets — the key that
        says how long it sounds, leaving `dur` (its length on the grid) and
        `legato` (the articulation) as the author wrote them. Writing the drawn
        length into `dur` with a `legato` of 1 was a round trip that lost both:
        every note in the lane, edited or not, came back fully legato with its
        grid length quietly shortened to what it had been sounding, so a melody
        that had been articulated started running its notes together."""
        timeline = _editable_timeline(element)
        if timeline is None:
            return False
        node = self._node_id(element)
        if node is None:
            return False
        # The notes as they stand, in the order the roll drew them — what the
        # i-th note of the payload is an edit *of*.
        held = [item for _, item in timeline if _pitch(item) is not None]
        new = []
        for i, (start, dur, pitch, vel, channel) in enumerate(_quintuples(list(values))):
            length = self.units_to_beats(dur)
            was = held[i] if i < len(held) else None
            if was is not None:
                params = dict(was)
                params.update(midinote=int(pitch), sustain=length)
                # The velocity round-trips through the drawing, so writing it
                # back unconditionally would re-quantize an `amp` nobody
                # touched. It is written only when the hand actually moved it.
                if int(vel) != _velocity(was):
                    params.update(velocity=int(vel),
                                  amp=max(0.0, min(1.0, int(vel) / 127.0)))
            else:
                # A note the lane did not hold: there is nothing to keep, so it
                # is built from what the payload says, sounding its full length.
                params = dict(midinote=int(pitch), dur=length, legato=1.0,
                              amp=max(0.0, min(1.0, int(vel) / 127.0)),
                              velocity=int(vel))
            if int(channel):
                params["channel"] = int(channel)
            new.append((self.units_to_beats(start), SeqEvent(params)))
        # **Through the log**: the roll's edit is a `SetMembers`, which is what
        # that intent was written for — "notes added, moved and removed arrive
        # as the resulting list. Members keep their ids".
        #
        # Keeping them is the whole difficulty, because the payload carries no
        # ids: a roll sends the resulting notes in order, so **order is the only
        # information there is**. The i-th note therefore inherits the i-th
        # note's id and the extras are minted past everything the arrangement
        # holds — which is what makes a note the same node across an edit, to
        # the log and to a view.
        kept = [getattr(item, ID_ATTR, None) for _, item in timeline
                if _pitch(item) is not None]
        members = []
        for i, (beat, event) in enumerate(new):
            nid = kept[i] if i < len(kept) and kept[i] is not None else self._mint_id()
            # **Through the conversion's own door.** A note's event is not
            # plain data — a played one carries its `server`, and the intent
            # travels as JSON — so the config is written the way `to_document`
            # writes a clang's, which turns what is not JSON-able into the
            # reference the document keeps for it. Handing `dict(event)` over
            # raw is a `TypeError` in the middle of a drag.
            members.append({"offset": float(beat),
                            "node": {"id": int(nid), "kind": "clang",
                                     "config": leaf_config(Clang(event))}})
        outcome = self._record({"intent": "setmembers", "node": node,
                                "members": members}, "edit the notes")
        if outcome is None:
            return False
        self._project(outcome["effective"])
        return self._changed(outcome["applied"])

    def _mint_id(self) -> int:
        """A node id nothing in this arrangement holds, for a note a gesture
        added. Follows the conversion's own rule, so a minted id and a converted
        one cannot collide."""
        nid = self._editing.mint(self.element)
        return nid

    def _apply_patch(self, wid: int, tag, values) -> bool:
        """One edit on a logical aggregate's directed patch. A ``"wire"`` (``src_box
        outlet dst_box inlet``) rewrites the two members' controls so they share a
        bus — the connection *is* a bus, the same fact `Aggregate.to_graphdef` reads,
        so the next render wires the GraphDef the way the cord is drawn. A ``"move"``
        (``box x y``) only persists the box's canvas position (a signal graph has
        no timeline, so positions are the editor's, not the arrangement's)."""
        aggregate, handles = self._patches[wid]
        if tag == "wire" and len(values) >= 4:
            return self._apply_wire(aggregate, handles, values[:4])
        if tag == "move" and len(values) >= 3:
            self._patch_geometry.setdefault(id(aggregate), {})[int(values[0])] = (
                float(values[1]), float(values[2]))
            return False
        return False

    def _apply_wire(self, aggregate, handles, values) -> bool:
        """Draw a cord ``src.outlet -> dst.inlet`` onto the arrangement: name the
        bus the connection implies (reusing one either end already writes/reads,
        else a fresh name declared on the aggregate) and point both members' controls
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
            or self._fresh_bus(aggregate)
        src_ctls[outlet], dst_ctls[inlet] = bus, bus
        src.controls, dst.controls = src_ctls, dst_ctls
        aggregate.declare_bus(bus, rate=rate)
        # The one gesture left that writes the arrangement *directly*: a cord is
        # a pair of controls naming a bus, which no intent describes yet. The
        # held document is behind after it, and says so.
        self._rederive = True
        self._restructured = True
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

    def _fresh_bus(self, aggregate) -> str:
        """A bus name not yet declared on ``aggregate`` (``w0``, ``w1``, …) — the
        private wire a brand-new cord introduces."""
        taken = set(aggregate.bus_names)
        i = 0
        while f"w{i}" in taken:
            i += 1
        return f"w{i}"

    def _osc(self, element) -> list:
        """The OSC (and raw MIDI) items of an element as ``(time_units, label)``
        pairs — the piano-roll's OSC lane. An `OscItem` labels with its address,
        a `MidiItem` with a short tag. Display only: a marker carries the time and
        a label, not the full message, so it is not written back (see
        `open_pianoroll`)."""
        if isinstance(element, (Aggregate, Vector)):
            return []
        try:
            events = flatten(element, 0.0)
        except (NotImplementedError, TypeError):
            return []
        out = []
        for beat, item in events:
            if isinstance(item, OscItem):
                out.append((self.beats_to_units(beat), str(item.addr)))
            elif isinstance(item, MidiItem):
                out.append((self.beats_to_units(beat), "midi"))
        return out

    def _changed(self, applied: bool = True) -> bool:
        """The arrangement was edited: mark it, and re-render now when `follow` is
        on. Otherwise the edit simply waits — a render already in flight is not
        interrupted, and the next one (a play, a resume, a seek) plays the piece as
        it now stands, because rendering always re-flattens the tree.

        ``applied`` is the crate's own answer, and passing it is not optional
        bookkeeping: **a resend is not an edit**. The document says so — it
        refuses one and leaves its version where it was — and an editor that
        moved its own version anyway, and answered "the composition changed",
        told every other view of it to come into step with nothing. The same
        sentence the crate writes about a refusal: it does not move the
        version."""
        if not applied:
            return False
        self.dirty = True
        self._version += 1
        self._follow_render()
        return True

    # ---- the history: the arrangement's, not this editor's ----------------

    @property
    def _editing(self) -> Editing:
        """The composition's editing context — its held document, its history
        and the index between them.

        Reached through the **element**, so a second window over one composition
        gets the same one. That is the whole of what makes an undo in either
        view update both, and it is why none of this is a field here: a history
        belongs to the data, never to a view.
        """
        return Editing.of(self.element)

    def _history(self):
        """The log and the document — **one of each, held** for as long as the
        composition is open, in however many windows.

        It used to be rebuilt on every gesture, and that handed back the whole
        of what holding the tree in the crate had won: converting the
        arrangement and opening a fresh handle cost 36 ms and 71 ms on a
        10240-event composition, against 0.014 ms for the edit itself. Held, a
        drag costs the edit.

        What a rebuild was quietly doing is explicit in `Editing.held`.
        `to_document` stamps each element with the id it keeps (`ID_ATTR`), so a
        re-derivation names the same nodes and the history keeps its footing —
        that is what makes `refresh` cheap and safe, and it is called wherever
        the arrangement moves by a route no intent took (a script's edit, a new
        composition, a gesture the editor still applies directly)."""
        return self._editing.held(self.element)

    @property
    def _log(self):
        """The arrangement's face of the composition's history."""
        return self._editing.log

    @property
    def _document(self):
        """The held document, or ``None`` before the first edit derived it."""
        return self._editing.document

    @property
    def _version(self) -> int:
        """The composition's version — the document half of the two counters.

        It moves on every edit applied to this composition **and on every
        redefine**, and it rides on each acknowledgement so the host can name it
        back on the next gesture."""
        return self._editing.version

    @_version.setter
    def _version(self, value: int):
        self._editing.version = int(value)

    @property
    def _rederive(self) -> bool:
        """Whether the held document has to be derived from the arrangement
        again before the next edit. Set wherever the tree moves by a route that
        is not an intent — `refresh`, and the gestures this editor still applies
        to the objects directly."""
        return self._editing.rederive

    @_rederive.setter
    def _rederive(self, value: bool):
        self._editing.rederive = bool(value)

    @property
    def _by_node(self) -> dict:
        """node id -> the arrangement object an intent naming it writes to."""
        return self._editing.by_node

    def _node_id(self, element, member=None) -> "int | None":
        """The document id of an arrangement element, building the document if
        that is what it takes.

        `to_document` is what *stamps* the id, so asking for one before the
        first conversion has to trigger it — otherwise the first gesture of a
        session names a node nobody has numbered.

        **The id is the placement's**, so a caller holding the member handle
        passes it and gets that window's node. Without one the element is looked
        up in the index, which answers when it is placed **once** and declines
        when it is placed twice — there being no way to tell from an element
        alone which of its windows an edit meant."""
        if member is not None:
            node = getattr(member, ID_ATTR, None)
            if node is None:
                self._history()
                node = getattr(member, ID_ATTR, None)
            return None if node is None else int(node)
        # **The index first, the stamp second.** An aggregate's id is the
        # *placement's* -- the handle that holds it in its parent -- so the
        # number on the aggregate object is not the document's answer, and it is a
        # number some other conversion left there: converting a subtree on its
        # own (which is how a cut, a split and a join read the members they are
        # about to state) numbers that subtree as a root and stamps its top. The
        # index is derived from the whole tree, so it is the one that knows.
        # Through the current document: an index left over from before an
        # element arrived would answer for a tree that is one gesture old.
        self._history()
        found = [nid for nid, (_, _, held) in self._by_node.items() if held is element]
        if len(found) == 1:
            return int(found[0])
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

    def _project(self, intent: dict) -> set:
        """Write an intent's value onto the arrangement, and say which widgets
        were drawing it.

        **Several**, when the intent is one an aggregate carries for all of its
        members: a trim, a split, a join and a cut are one `setmembers` over a
        lane, and the widgets whose picture it changed are the lane's clips.
        Answering with only the lane left every one of them drawn as the hand
        had left it.

        The editor is to the document what the host is to the editor: it emits
        an intent and adopts the value that comes back. Nothing here decides
        anything — the snap, the clamp and the refusal already happened in the
        crate — so this is a projection and not a second implementation of what
        an edit means. It is also the whole of what an undo has to do, since an
        inverse is an ordinary intent.

        It also keeps the **drawn record** in step, which the clip route does
        for itself after a drag (*"keep the registry truthful, or the next edit
        would measure its move against a stale placement"*). An undo reaches
        the arrangement through here and nowhere else, so without it the
        registry still held the position the hand dropped the clip at -- and a
        correction is read straight out of that registry, so an undo moved the
        model, told the host to go on drawing the clip exactly where it was,
        and looked like a dead button."""
        found = self._by_node.get(int(intent.get("node", -1)))
        if found is None:
            return set()
        owner, member, element = found
        kind = intent.get("intent")
        moved = set()
        if kind == "place" and owner is not None and member is not None:
            owner.move(member, float(intent["offset"]))
            # **A `place` states the whole placement, so absence is a value.**
            # An intent with no `dur` says this member takes the element's own
            # length again -- which is exactly what the inverse of the *first*
            # resize of a clip has to say, since before it there was no
            # placement length at all. `Aggregate.move` reads a `None` as
            # "leave the length alone", the opposite, so it is written here
            # instead of passed to it: with it passed, the document went back
            # and the clip kept the size the hand had given it, and undo looked
            # like a dead button on every clip that had never been resized.
            member.dur = None if intent.get("dur") is None else float(intent["dur"])
        elif kind == "configure":
            if not self._configure(element, intent.get("config") or {}):
                return set()
        elif kind == "setmembers":
            members = intent.get("members", [])
            # Two things carry members and they are not the same thing: a
            # `Aggregate`'s placements, and the notes of an editable timeline. The
            # element decides which, because the intent names a node and the
            # node is whichever of the two it is.
            if isinstance(element, Aggregate):
                if not self._set_placements(element, members):
                    return set()
                # ...and the drawn record of every member it states, since the
                # clip a trim or a split moved is not the aggregate the intent
                # names.
                for handle in element.handles:
                    wid = self._redrawn(handle.element, handle)
                    if wid is not None:
                        moved.add(wid)
            elif not self._set_notes(element, members):
                return set()
        else:
            return set()
        wid = self._redrawn(element, member)
        if wid is not None:
            moved.add(wid)
        # Every other window over this composition draws the same data, and this
        # is the one place that knows *what* moved -- so the intent is reported
        # here rather than reduced to "something changed", which would cost them
        # a redefine per edit.
        self._editing.moved(intent)
        return moved

    def _configure(self, element, config: dict) -> bool:
        """Write a leaf's configuration onto what the arrangement holds.

        One curve, one door: the projection of an inverse, the adoption of a
        redone document and the edit itself all land here, so the envelope the
        script holds, the buffer it sounds through and the picture cannot
        disagree about which of the three happened."""
        if isinstance(element, Vector):
            # A take's configuration is the **window** it reads: which frame of
            # its samples it begins at, and whether that window wraps. The
            # configuration is written whole, so a key the intent does not carry
            # is the default -- reading from the first frame, once.
            element.start = float(config.get("start", 0.0))
            element.loop = bool(config.get("loop", False))
            return True
        auto = _automation(element, self.tempo)
        flat = config.get("points")
        if auto is None or flat is None:
            return False
        auto.env = points_to_env(list(flat))
        auto.refill()
        return True

    def _restructure(self) -> bool:
        """Redefine the window when the last edit changed **which members
        exist**, and say whether it did.

        A placement, a length, a curve and a note list are **props**: the host
        is told and it draws them. A widget that was not there a moment ago --
        the second half of a split, the clip an undone cut brings back -- is
        not a prop, and no acknowledgement can carry one. The only channel for
        it is a redefine, so the editor that drew the window is what owes it:
        without this the document and the objects the script holds had two
        clips while the picture had one, until something happened to redraw.

        It is deliberately **not** a redraw after every edit. A redefine
        rebuilds every widget and drops what the host had in flight, which is
        exactly wrong for a drag and exactly right for a structural edit."""
        if not self._restructured:
            return False
        self._restructured = False
        # The case no prop can carry, for the other windows as much as for this
        # one: a widget that was not there a moment ago is not a value.
        self._editing.restructured()
        if self._host is None or self._window is None:
            return False
        self.update()
        return True

    def _redrawn(self, element, member) -> "int | None":
        """Bring the **drawn record** of one placement back in step with the
        arrangement, and say which widget draws it.

        Every path that moves a placement without a gesture ends here — an
        inverse projected onto the tree, a whole document adopted after a redo —
        because a correction is read straight out of this registry
        (`_resync`). A path that moved the model and left the record behind
        tells the host to go on drawing the clip exactly where it was, which is
        indistinguishable from a dead button."""
        wid = self._widget_of(element, member)
        placed = self._clips.get(wid) if wid is not None else None
        if placed is not None and member is not None:
            placed.offset = self.beats_to_units(member.offset + placed.base)
            # Through the same rule the draw used, not a shorter one of its own:
            # a placement whose length went back to *unstated* is drawn at the
            # element's own length, and a member with neither reaches the
            # element's extent -- which `member.length` alone cannot say, so the
            # record kept the size the gesture left.
            placed.dur = self._drawn_dur(element, member,
                                         at=member.offset + placed.base)
        return wid

    def _set_placements(self, aggregate, members: list) -> bool:
        """A `setmembers` onto an `Aggregate`: the placements as the document states
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
        for handle in list(aggregate.handles):
            node = getattr(handle, ID_ATTR, None)
            if node is None:
                continue
            by_id[int(node)] = handle
            if int(node) not in keep:
                aggregate.remove(handle)
                # A placement that is gone takes a widget with it, and no prop
                # says "this clip is not there any more".
                self._restructured = True
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
            # A member carries its node, and a node carries what the leaf is
            # configured as: a trimmed take's window, an edited curve's points.
            # Written here so one `setmembers` states the whole of what a
            # gesture did -- and so an undo of it restores both. **Absence is a
            # value**: a node with no configuration is a leaf configured as it
            # was made, which is what the state before a trim is, so the empty
            # table is written rather than skipped. Skipping it left the window
            # over the samples where the trim had put it while the placement
            # went back -- a clip the right size showing the wrong frames.
            config = (m.get("node") or {}).get("config") or {}
            found = self._by_node.get(int(node))
            if found is not None:
                self._configure(found[2], config)
            if handle is not None:
                aggregate.move(handle, offset)
                # ...and the length the document states, which a split, a join
                # and an undo of either all change. Without this a placement
                # that came back the right length on paper stayed the length
                # the gesture had left it.
                handle.dur = None if m.get("dur") is None else float(m["dur"])
                continue
            found = self._by_node.get(int(node))
            if found is not None:
                # ...and one that is **back** -- the undo of a cut, of a split,
                # of a join -- needs a widget nobody drew.
                self._restructured = True
                restored = aggregate.add(found[2], offset, m.get("dur"))
                # **The placement keeps the id the document gave it.** A handle
                # that came back unstamped is a new node to the next conversion,
                # which renumbers the tree under every intent still naming the
                # old one -- and the aggregate holding it stops being found at all.
                if restored is not None:
                    setattr(restored, ID_ATTR, int(node))
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
            event = SeqEvent(dict(config))
            # **The note keeps the id the document gave it.** The payload a roll
            # sends carries no ids, so the i-th note inherits the i-th note's --
            # read off *these* objects. A note that came back unstamped is a new
            # node to the next edit, which mints one: the same notes resent then
            # arrive as different members, the document changes for nothing, and
            # every other view of it redraws to come into step with an edit that
            # was not one. It is the placement's rule (O14), one level down.
            if node.get("id") is not None:
                setattr(event, ID_ATTR, int(node["id"]))
            new.append((float(placed.get("offset", 0.0)), event))
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
        # **A layered clip draws an aggregate, and an edit inside it names a
        # member.** A simultaneous aggregate is one clip with its members'
        # bodies over each other, so the curve an edit configures is not the
        # element any clip is registered against -- and without this an undo of
        # that curve moved the model and told the host nothing, which is a dead
        # button with the drawing left on the edited shape.
        for wid, placed in self._clips.items():
            held = placed.member.element if placed.member is not None else self.element
            if isinstance(held, Aggregate) and any(
                h is member or h.element is element for h in held.handles
            ):
                return wid
        return None

    def undo(self) -> bool:
        """Step back one edit, and tell the host what to draw instead.

        The inverse is an ordinary intent, so undoing needs no second path: it
        is `_project` again, on what the crate hands back. Returns whether
        anything was undone.

        Every **other** window over this composition is told, the way it is told
        about an edit: one history, and an undo in either view updates both."""
        with self._editing.turn(self):
            stepped = self._step(lambda log, doc: log.undo(doc), "undone")
            if stepped:
                self._editing.changed()
            return stepped

    def redo(self) -> bool:
        """Step forward again after `undo`. Returns whether anything was
        redone.

        A step the crate **cannot perform** — a deterministic operation kept as
        its parameters rather than as a span — comes back in ``remaining`` for
        its owner to re-run. Nothing in the multitrack editor produces one yet,
        so this reports it rather than acting on it."""
        with self._editing.turn(self):
            stepped = self._step(lambda log, doc: log.redo(doc), "remaining")
            if stepped:
                self._editing.changed()
            return stepped

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
                widgets |= self._project(intent)
        else:
            # A redo now reports the intents it applied, so it is the *same
            # shape* as an undo and takes the same path. It used to adopt the
            # whole document instead, which cost O(document) per step and was a
            # second implementation of what an edit means -- and it is what made
            # a redo move the model while telling the host to keep drawing the
            # old position, because only one of the two routes kept the drawn
            # record.
            for intent in step.get("redone") or []:
                widgets |= self._project(intent)
        self._version = document.version
        self.dirty = True
        self._follow_render()
        self._corrections = []
        # A step that changed which members exist is answered with a new tree
        # rather than with props: the widgets the corrections name are about to
        # be rebuilt, and the clip an undone split takes away has no prop that
        # says so.
        if self._restructure():
            return True
        for wid in widgets:
            self._resync(wid)
        self._acknowledge(0)
        self._corrections = []
        return True

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

    @property
    def redo_label(self) -> "str | None":
        """What a redo would be called, for a menu item.

        The pair of `undo_label`, and it stops being decoration the moment a
        second window is open on the composition: with one pile over all of
        them, a label is how a person knows which edit a keystroke is about to
        move — and both windows read the same one.
        """
        return None if self._log is None else self._log.redo_label

    def _follow_render(self):
        """Re-schedule after an edit when `follow` is on **and there is
        something to re-schedule**.

        The guard is the whole of it, and it has two halves. `rerender` needs a
        destination and a clock, which only a `render` or a `play` supplies, so
        a live editor edited before anything was ever played used to raise on
        the first drag. And what `follow` means is *what is sounding follows the
        edit* -- so it re-schedules a pass in flight and *starts* nothing: an
        edit made while the transport is stopped (or after a pass ran out) would
        otherwise have the drag itself press play, which is a window that plays
        itself by another route.

        An edit not re-scheduled here is not lost: it marked the composition
        (`dirty`), and the next play re-reads it, because rendering always
        re-flattens the tree."""
        if self.follow and self._destination is not None and self.transport.playing:
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

    # ---- rendering: the edited arrangement back to sound ----

    def render(self, destination, clock=None, *, at: float = 0.0, quant=None):
        """Render the composition onto ``destination`` — RT (a `Server` and a
        running clock) or NRT (a score) — and anchor the lanes' playhead so the
        line sweeps the clips as it plays. Returns the `clausters.seq.Playhead`.

        This is the arrangement's own `render` (flatten to absolute beats, play
        through a playhead): the editor adds no rendering path of its own, it only
        remembers the destination so `rerender` can re-schedule after an edit.

        **The clock's tempo map wins.** A view of a piece and the clock playing
        it cannot hold two answers for when a beat falls, so handing a clock
        here adopts its map and redraws whatever moved. Without a clock the
        editor keeps its own, which is what lets a composition be laid out
        before anything plays.
        """
        self._destination, self._clock = destination, clock
        self._adopt_map(getattr(clock, "map", None))
        playhead = self.transport.play(destination, at=at, quant=quant)
        self.dirty = False            # what plays now *is* the arrangement
        return playhead

    def _adopt_map(self, tempo_map) -> bool:
        """Take ``tempo_map`` as the editor's, redrawing if it says anything
        different from the one held. Returns whether it moved.

        The one place the view's time and the clock's are reconciled: a line
        drawn by one function and a sound played by another disagree by whatever
        a tempo change moved, and no amount of redrawing the *lanes* fixes that
        — it is the axis underneath them.
        """
        if tempo_map is None:
            return False
        if self.tempo_map.segments() == tempo_map.segments():
            return False
        self.tempo_map = tempo_map.copy()
        self.transport.tempo_map = self.tempo_map
        if self._host is not None and self._window is not None:
            self._host.define(self._window, self.draw())
        return True

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
        to — its samples are produced on the server, so its position is that
        def's internal state and no number moves it. Rather than move the cursor
        somewhere the sound will not follow, this refuses and says why. Render
        the element first (`clausters.form.render`) and it becomes samples like
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
        up. Under a server transport that governs the samples, every node kept
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
        """The lanes an element contributes: a concrete `Aggregate` becomes one
        lane holding its members as clips (plus a lane of its own for every
        *expanded* nested aggregate); anything else becomes a lane with one clip.
        ``base`` is its start in beats, ``owner``/``member`` the placement an
        edit-back writes through."""
        if (isinstance(element, Aggregate) and element.kind == CONCRETE
                and len(element) > 1
                and element.temporal_relation(self.tempo) == SIMULTANEOUS
                and not self.is_expanded(element)):
            # Its members start and end together: they are *one* thing on the
            # timeline, so they are one clip with layered bodies — not a lane of
            # clips that must be dragged one by one.
            return [self._lane([self._clip_for(element, base, owner, member)],
                               _name(element))]
        if isinstance(element, Aggregate) and element.kind == CONCRETE:
            clips, extra = [], []
            for child in element.handles:
                child_base = base + child.offset
                if isinstance(child.element, Aggregate) and self.is_expanded(child.element):
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

    def _drawn_length(self, element, member) -> float:
        """The length one clip is drawn at, **in the element's own unit** — the
        placement's when it overrides, else the element's own, else what the
        element extends to.

        One rule, in one place, because two of them is how a picture and a
        model come to disagree: the draw asks this, and so does every path that
        has to put a placement back (`_redrawn`, after an inverse or a redo)."""
        length = member.dur if (member is not None and member.dur is not None) else None
        if length is None and isinstance(element, Element):
            length = element.duration
        if length is None:
            length = self._extent(element)
        return length

    def _drawn_dur(self, element, member, body=None, at: float = 0.0) -> float:
        """The same length in **timeline units**, which needs the body: a take
        with no duration given is as long as it is (1 unit = 1 sample).

        ``at`` is the clip's own onset in beats, which a length **in beats**
        needs to have a length at all — the same two-position rule
        `length_to_units` states."""
        length = self._drawn_length(element, member)
        if body is None:
            body = self._body_for(element, length)
        if "buffer" in body and length <= 0.0:
            return float(element.wraps.frames)
        return self.length_to_units(length, element, at)

    def _clip_for(self, element, base: float, owner, member) -> dict:
        """One `clip`: the element placed at ``base`` beats (absolute on the shared
        axis), with the body (or **bodies**) its kind calls for. Registers what it
        drew, which is what the edit-back path resolves against."""
        wid = self._new_id()
        offset = self.beats_to_units(base)
        dur_length = self._drawn_length(element, member)
        body = self._body_for(element, dur_length)
        dur = self._drawn_dur(element, member, body, at=base)

        # The placement's own base: a clip's offset is absolute on the shared axis,
        # a member's offset is relative to its aggregate.
        parent_base = base - (member.offset if member is not None else 0.0)
        self._clips[wid] = _Placed(owner, member, parent_base, offset, dur)
        # A roll body is the `notes` element itself, and it edits: a body carries
        # no id of its own, so a note dragged inside one arrives tagged with *this
        # clip's* id. Registering what the body draws is what lets that edit reach
        # the arrangement — without it the note moves on screen and nowhere else.
        roll = _roll_owner(element, self.tempo)
        if "notes" in body and roll is not None:
            self._rolls[wid] = roll
        return clip(id=wid, offset=offset, dur=dur, label=_name(element), **body)

    def _axis_for(self, auto, points) -> tuple:
        """The value axis this curve is drawn against — the one it was **first**
        drawn against, kept.

        `_curve_range` answers what the break-points alone would ask for, and
        that is the right answer exactly once: recomputed on every redraw it
        makes an edit rescale the picture, so dragging one point visibly moves
        every other one. The axis is therefore remembered per `Automation` and
        only ever **widened**, when a curve no longer fits inside it (a script
        replaced the envelope, an undo restored a taller one) — never narrowed,
        so a point dragged down and back up leaves the drawing where it was.
        """
        lo, hi = _curve_range(points)
        kept = self._curve_axis.get(auto)
        if kept is not None:
            klo, khi = kept
            values = [float(p[1]) for p in points] or [0.0]
            # **One side at a time.** Only the end that stopped holding the data
            # moves; taking the union of the two padded ranges would drop the
            # floor as well whenever the ceiling grew, which is the same jump
            # one step removed.
            lo = klo if min(values) >= klo else lo
            hi = khi if max(values) <= khi else hi
            if (lo, hi) == kept:
                return kept
        self._curve_axis[auto] = (lo, hi)
        return lo, hi

    def _body_for(self, element, limit=None) -> dict:
        """The clip-body props an element draws with — and a **simultaneous** aggregate
        draws with *all of its members'*, layered in one clip.

        ``limit`` is the **placement's** length in the element's own unit when
        it has one (seconds for samples, beats for events): a
        placement is a window onto an element, so a clip shortened over samples
        assembled from segments draws the segments it plays and not the ones it
        no longer reaches.

        That is the arrangement's own answer to "attach an envelope to the event it
        shapes": an aggregate whose members start and end together *is* one thing on the
        timeline (its temporal relation says so), so it is one clip — dragging it
        moves the whole aggregate, and the bodies overlay instead of hiding each other.
        The curve keeps its own value axis (`points_min`/`points_max`), since an
        envelope's units are not the pitches under it.
        """
        # A simultaneous aggregate first: it is one thing on the timeline, and its
        # members' bodies layer (each keeps its own value axis).
        if (isinstance(element, Aggregate) and len(element) > 1
                and element.temporal_relation(self.tempo) == SIMULTANEOUS):
            body: dict = {}
            for m in element.handles:
                body.update(self._body_for(m.element, limit))
            return body

        auto = _automation(element, self.tempo)
        if auto is not None:
            # A curve's times are an `Env`'s, so they are seconds and cross on
            # the rate: the shape is drawn where it sounds, whatever the tempo.
            points = [(self.secs_to_units(t), v, shape, curve)
                      for t, v, shape, curve in _quads(auto.to_points())]
            lo, hi = self._axis_for(auto, points)
            # **Flat, because these quads are already resolved.** A `points`
            # argument of *tuples* is read as `(t, v, curve_spec)` and resolved,
            # so handing it `(t, v, shape, curve)` re-reads the shape number as a
            # curvature: a linear segment (shape 1) came out as the custom shape
            # with curvature 1.0 -- drawn curved, and edited back curved, so an
            # envelope changed shape by being looked at. The flat form is kept
            # verbatim.
            return dict(points=[x for p in points for x in p],
                        points_min=lo, points_max=hi)

        if isinstance(element, Segments):
            segments = self._segments_within(element, limit)
            # **One clip, one take per segment.** The samples are several
            # windows read as one thing, so the clip holds one body per segment,
            # each over its own stretch of the clip and each reading its own
            # buffer from its own frame — which is what makes a joined clip draw
            # the pieces it actually plays instead of the first of them.
            children, cursor = [], 0.0
            for seg in segments:
                offset, cursor = cursor, cursor + seg.duration
                buf = seg.buffer
                channels = getattr(buf, "channels", None)
                if channels is None:
                    continue
                take = dict(view="trace", buffer=buf.bufnum,
                            channels=max(1, channels),
                            at=self.secs_to_units(offset),
                            dur=self.secs_to_units(seg.duration))
                if seg.start:
                    take["start"] = float(seg.start)
                children.append(signal(**take))
            return {"children": children} if children else {}

        if isinstance(element, Vector):
            buf = element.wraps
            # The take rides the bulk path: the host fetches the server buffer and
            # decimates it through its peak pyramid.
            #
            # An element this process does not hold draws as a **clip with no
            # waveform** rather than not at all: a session reopened without its
            # sources resolved wraps each one in a `FrozenSource`, which knows
            # the buffer number the document recorded and nothing about its
            # shape. Laid out, not dropped -- the same rule an unknown widget
            # gets, and the reason a piece whose take has gone missing still
            # shows where the take was.
            channels = getattr(buf, "channels", None)
            if channels is None:
                return {}
            body = dict(buffer=buf.bufnum, channels=max(1, channels))
            # The **window** onto those samples: a clip shows the segment its
            # element reads, so a trimmed take draws the frames it plays and
            # not the buffer squeezed into a rectangle. Sent only when there is
            # a window to state, which keeps a whole-take clip's props exactly
            # what they were.
            if element.start:
                body["start"] = float(element.start)
            if element.loop:
                body["loop"] = True
            return body

        notes = self._notes(element)
        if notes:
            pitches = [n[2] for n in notes]
            body = dict(notes=notes,
                        min=min(min(pitches) - PITCH_PAD, DEFAULT_PITCH[1]),
                        max=max(max(pitches) + PITCH_PAD, DEFAULT_PITCH[0]))
            # **Say it before the hand tries.** These notes are a *rendering* of
            # a forward-only generator when there is no editable timeline behind
            # them, so the roll refuses the press instead of offering a drag it
            # will unwind — which is what "the notes flicker, jump and return"
            # was. The refusal itself has always been correct; what was missing
            # is that nothing told the widget in advance.
            #
            # **The roll's own key, not the clip's.** ``editable`` is a
            # statement about the whole clip and reaches every body it carries,
            # so saying it here locked the *envelope* drawn over these notes as
            # well — a sweep whose curve drew and could not be touched. A body
            # says its own with ``notes_editable``, as it already keeps its own
            # value axis with ``points_min``/``points_max``.
            if _editable_timeline(element) is None:
                body["notes_editable"] = False
            return body
        # No body: a collapsed aggregate (or an element with nothing to draw) is the
        # labeled rectangle — the summary of the level above it.
        return {}

    def _notes(self, element) -> list:
        """The ``(start, dur, pitch)`` note events of an element, in timeline
        units relative to the element — the piano-roll body. An `Aggregate` is a
        summary, not a roll (it collapses to a rectangle), and a note is any
        flattened event that resolves a pitch: the *change of state* of a
        contained generator happens right here (a pattern is bounced by
        `clausters.form.render.flatten`), so a generator lane shows the notes it
        will play."""
        if isinstance(element, (Aggregate, Vector)):
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
        """An element's length **in its own unit** (`duration_unit`: seconds for
        samples and for a curve, beats for events): its own ``duration`` when it
        has one, else what it spans — an aggregate over its placed members, an
        envelope over its curve, anything else over its flattened events (a
        bounced pattern included).

        An aggregate spans *beats*, since that is what its members' offsets are
        in, so each member's length crosses on the way into the sum."""
        if isinstance(element, Element) and element.duration is not None:
            return float(element.duration)
        # **The aggregate rule comes first**, and the curve's own length second:
        # a simultaneous aggregate holding a curve spans *beats*, like every
        # aggregate, and answering with the envelope's seconds would hand a
        # caller a number in one unit under a name that says the other.
        if isinstance(element, Aggregate):
            return max((m.offset + to_beats(
                m.dur if m.dur is not None else self._extent(m.element),
                m.element.duration_unit if isinstance(m.element, Element) else BEATS,
                self.tempo)
                for m in element.handles), default=0.0)
        auto = _automation(element, self.tempo)
        if auto is not None:
            return auto.duration()
        if isinstance(element, Segments):
            # Its contents are a list, and its extent is the whole of it.
            return sum(seg.duration for seg in element.segments)
        if isinstance(element, Vector):
            buf = element.wraps
            rate = buf.sample_rate or self.sample_rate
            # Its own seconds: the frames it holds over the rate they were
            # recorded at, which no tempo enters.
            return float(buf.frames) / float(rate)
        try:
            events = flatten(element, 0.0)
        except (NotImplementedError, TypeError):
            return 0.0
        return max((beat + _event_dur(item) for beat, item in events), default=0.0)


def _logical_patch(aggregate):
    """A logical `Aggregate` as a `clausters.defs.GraphPatch`, through the headless
    decode `GraphPatch.from_graphdef`: the aggregate renders to a `GraphDef` (its
    members and their shared-bus controls — the arrangement's 1:1 logical mapping),
    and the decode reads that back into a directed patch, typing each box's ports
    from the `SynthDef` the member wraps. The `Aggregate -> patch` mapping itself lives
    in `clausters.defs`, not here — the editor is only a consumer of it.

    A member wrapping a bare def *name* (not a `SynthDef` object) draws port-less —
    its directions are unknowable without the def. Returns the patch and the member
    handles in box order (box index == member order), so an edit-back maps a box
    index back to the member whose controls it rewrites."""
    from ..defs import GraphPatch
    from ..defs.synthdef import SynthDef

    handles = list(aggregate.handles)
    gdef = aggregate.to_graphdef(name=getattr(aggregate, "name", None) or "_patch")
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
    """An element's display name: its own ``name`` when it has one (an aggregate names
    itself, an automation names the control it drives), else what it *is* — an
    automation is an "envelope", not the `Element` that happens to wrap it."""
    name = getattr(element, "name", None)
    if isinstance(name, str) and name:
        return name
    auto = _automation(element)
    if auto is not None:
        return auto.name or "envelope"
    return type(element).__name__.lower()


def _automation(element, tempo: float = 1.0):
    """The `clausters.seq.Automation` an element carries, or ``None``. An automation
    is a *curve* — the List/Vector duality of the arrangement — so it needs no primitive
    of its own: any element wrapping one draws (and edits) as an envelope.

    A **simultaneous** aggregate is searched too: an envelope attached to the event it
    shapes is one clip, and a curve edited on it must find the automation inside.
    """
    if isinstance(getattr(element, "wraps", None), Automation):
        return element.wraps
    if (isinstance(element, Aggregate) and len(element) > 1
            and element.temporal_relation(tempo) == SIMULTANEOUS):
        for _offset, _dur, child in element.members:
            auto = _automation(child, tempo)
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


def _roll_owner(element, tempo: float = 1.0):
    """The element whose notes a clip's roll body draws — what a ``"notes"``
    edit-back is written onto.

    Usually the element itself (a generator among them: it registers and the
    edit is refused later, which is where read-only is decided). A
    **simultaneous** aggregate is the one that needs asking: it draws as one clip
    with its members' bodies layered, so the notes under the cursor belong to
    the member that carries them, not to the aggregate. ``None`` when no member
    has an editable timeline — a layered roll nobody can write to."""
    if (isinstance(element, Aggregate) and len(element) > 1
            and element.temporal_relation(tempo) == SIMULTANEOUS):
        for m in element.handles:
            if _editable_timeline(m.element) is not None:
                return m.element
        return None
    return element


def _curve_owner(element, member=None, tempo: float = 1.0):
    """The element whose `clausters.seq.Automation` a clip's curve body draws —
    what a ``"points"`` edit-back is written onto — and the member handle that
    places it.

    The mirror of `_roll_owner`, and needed for the same reason: a
    **simultaneous** aggregate draws as one clip with its members' bodies
    layered, so the curve under the cursor is a member's and the intent has to
    name that member's node. Anything else answers with itself and the handle it
    was placed by.
    """
    if (isinstance(element, Aggregate) and len(element) > 1
            and element.temporal_relation(tempo) == SIMULTANEOUS):
        for handle in element.handles:
            if _automation(handle.element, tempo) is not None:
                return handle.element, handle
    return element, member


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
    `OscItem`/`MidiItem`, a rest, an automation lane. Flattening yields the
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
