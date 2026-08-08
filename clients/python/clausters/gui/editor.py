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
from ..form.group import CONCRETE, LOGICAL, SIMULTANEOUS, Group
from ..form.element import Buffer, Element
from ..defs.ugens import points_to_env
from ..form.render import flatten
from ..seq.automation import Automation
from ..seq.event import Event as SeqEvent
from ..seq.timeline import MidiEvent, OscEvent, Timeline
from .guidef import clip, patch, pianoroll, scroll, timeruler, track, window
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
        #: The view: the multitrack (`open`) or a dedicated piano-roll of one
        #: events element (`open_pianoroll`). `render` dispatches on it.
        self._mode = "multitrack"
        #: The element the dedicated piano-roll draws (its notes editable when it
        #: is a `Track`), and widget id -> that element for the edit-back route.
        self._roll_element = None
        self._rolls: dict = {}
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
        self._reset_ids()
        self._clips = {}
        self._lanes = {}
        self._rolls = {}
        self._patches = {}

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
        return self._window

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
        host already moved the clip that was dragged."""
        if self._host is None or self._window is None:
            raise RuntimeError("open(host) the editor first")
        self._host.define(self._window, self.draw())

    # ---- the edit-back: a dragged clip becomes a placement ----

    def apply(self, addr: str, args) -> bool:
        """Apply one message from the host to the **arrangement**. Returns whether
        the composition changed.

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
        if addr != "/gui_event" or len(args) < 2:
            return False
        if args[1] == "locate":
            # A click on a lane's ruler (or its empty space): seek. A transport
            # action, not an edit — the composition did not change (and another
            # editor's lane is not ours to seek from).
            if int(args[0]) in self._lanes:
                self.locate(self.units_to_beats(float(args[2])))
            return False
        if args[1] == "notes":
            # A note edited in the dedicated piano-roll: rebuild the element's
            # timeline (a generator is read-only, so it is ignored).
            element = self._rolls.get(int(args[0]))
            return element is not None and self._apply_notes(element, args[2:])
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
        new_offset = member.offset
        if moved:
            # Absolute (the axis) -> relative (the placement), snapped to the grid.
            new_offset = self._snap(self.units_to_beats(offset)) - placed.base
        new_dur = self._snap(self.units_to_beats(dur)) if resized else None
        placed.owner.move(member, new_offset, new_dur)
        # The clip was drawn where it now is: keep the registry truthful, or the
        # next edit would measure its move against a stale placement.
        placed.offset, placed.dur = offset, dur
        self._changed()
        return True

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
        """Notes edited in the dedicated piano-roll (the flat ``"notes"`` payload,
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
        if self.follow:
            self.rerender()
        return True

    def poll(self, timeout: float = 0.0) -> bool:
        """Drain the host's pending messages into the arrangement (`apply` each).
        Returns whether the composition changed. Call it from the script's loop —
        **never** from the clock thread, which a routine must never block."""
        if self._host is None:
            raise RuntimeError("open(host) the editor first")
        changed = False
        while (msg := self._host.poll(timeout)) is not None:
            changed |= self.apply(*msg)
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
            return dict(buffer=buf.bufnum, channels=max(1, buf.channels))

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
