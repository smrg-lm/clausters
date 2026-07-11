"""`Editor`: the bridge between the compositional model and the multitrack GUI.

The driver of the DAW-style view. It renders a `clausters.model` tree into a
multitrack `GuiDef` (tracks of clips on one shared time axis), applies the clip
edit-backs the host sends straight onto the model, and re-realizes it — the loop
**data ↔ graphic ↔ sound**, which is what makes the composition editable at any
granularity rather than merely displayable.

Three things are worth knowing about how it is built.

**The dependency arrow points this way.** `clausters.model` stays pure and
transport-agnostic; the editor imports the model, never the reverse. This module
is the only one that knows both worlds.

**Beats meet samples here.** The model places materials in *beats*; the
multitrack view places clips in *timeline samples*, because a clip's body is
audio data and its sample 0 sits at the clip's offset. The editor is the only
converter: one beat is `sample_rate / tempo` timeline units, so an audio take
placed at its own length sits 1:1 on the axis. A musical `quant` becomes the
lane's drag grid, so the grid a clip is dropped on is the grid the model
re-schedules on. The arithmetic itself is the core's (`beats_to_secs` →
`secs_to_samples`), not a second implementation.

**One mapping rule, not a heuristic per case.** The root `Group`'s members are
the *lanes*; a lane's members are its *clips*; a `Buffer` clip draws its take, a
material of events draws a piano-roll, and a nested `Group` draws as a labeled
rectangle — its summary — until it is `expand`ed into lanes of its own. That
collapse/expand *is* the model's base level (the zoom that summarizes a group or
resolves it), so it needs no protocol of its own.
"""

import itertools

from .. import _native
from ..model.group import COMPOSITIONAL, Group
from ..model.material import Buffer, Material
from ..model.realize import flatten
from ..seq.event import Event as SeqEvent
from .guidef import clip, track, window

__all__ = ["Editor"]

#: The pitch range a piano-roll lane falls back to when its notes give none
#: (C3..C6 — the span a melodic line usually lives in).
DEFAULT_PITCH = (48.0, 72.0)
#: Semitones of headroom above and below the notes of a piano-roll clip.
PITCH_PAD = 2.0


class _Placed:
    """What a clip widget was drawn from: the placement it shows (``owner`` group
    and ``member`` handle, the model's stable identity), the ``base`` in beats its
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
    """A composition on screen: the model tree rendered as a multitrack view,
    editable back into the model.

    Args:
        material: the composition — a `clausters.model.group.Group` (its members
            become the lanes) or any single `Material` (one lane).
        sample_rate: the engine's sample rate; with ``tempo`` it fixes the
            beats↔timeline-samples conversion.
        tempo: the clock's tempo in **beats per second** (the `TempoClock`
            convention — 2.0 is 120 bpm).
        quant: the musical drag grid in beats (``0.25`` = a sixteenth); ``0``
            snaps to whole samples.
        follow: re-realize on every edit (the live editor).
        title: the window title.
        base_id: the first widget id the editor allocates. The default sits well
            above the ids `clausters.gui.host.GuiHost` assigns to windows it opens
            (from 1000), so the two never collide.

    Usage::

        editor = Editor(song, sample_rate=server.sample_rate, tempo=clock.tempo,
                        quant=0.25)
        editor.open(gui)              # render and open the window
        editor.apply(*gui.poll())     # a dragged clip moves the material
        editor.realize(server, clock) # play the edited composition
    """

    def __init__(self, material, *, sample_rate: float, tempo: float = 1.0,
                 quant: float = 0.0, follow: bool = False,
                 title: str = "Composition",
                 width: int = 1000, height: int = 520, base_id: int = 10_000):
        self.material = material
        self.sample_rate = float(sample_rate)
        self.tempo = float(tempo)
        self.quant = float(quant)
        #: Re-realize on every edit (the *live editor*: drag a clip and hear it
        #: where you dropped it). Off by default — an edit then only changes the
        #: model, and `rerealize` decides when it is heard.
        self.follow = bool(follow)
        self.title = title
        self.size = (int(width), int(height))
        self._base_id = int(base_id)
        #: The materials shown as lanes of their own instead of a summary clip
        #: (the base level: a group resolved rather than collapsed).
        self._expanded: set[int] = set()
        #: widget id -> `_Placed` — where the clip came from in the model and
        #: what was drawn for it, which is what an edit-back writes through.
        self._clips: dict = {}
        #: widget id -> material, for every lane (a `/gui_set` of the lane chrome
        #: — the playhead — addresses these).
        self._lanes: dict = {}
        self._host = None
        self._window = None
        #: The realization in flight: where it went, on what clock, and the
        #: playhead playing it — what `rerealize` re-schedules after an edit.
        self._destination = None
        self._clock = None
        self._playhead = None

    # ---- the unit bridge: beats (the model) ↔ timeline samples (the view) ----

    @property
    def units_per_beat(self) -> float:
        """Timeline samples per beat — the whole of the model↔view unit bridge.
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

    def expand(self, material) -> "Editor":
        """Resolve a nested `Group` into lanes of its own (instead of the labeled
        rectangle that summarizes it). The model's *base level*, made an edit."""
        self._expanded.add(id(material))
        return self

    def collapse(self, material) -> "Editor":
        """Summarize a nested `Group` back into one labeled rectangle."""
        self._expanded.discard(id(material))
        return self

    def is_expanded(self, material) -> bool:
        return id(material) in self._expanded

    # ---- the forward render: model -> GuiDef ----

    def render(self) -> dict:
        """The composition as a ``window``-rooted GuiDef: one `track` lane per
        member of the root group, each holding its members as clips on the shared
        time axis. Pure — it builds the tree and the id registry, and sends
        nothing."""
        self._ids = itertools.count(self._base_id)
        self._clips = {}
        self._lanes = {}

        lanes: list = []
        root = self.material
        if isinstance(root, Group) and root.kind == COMPOSITIONAL:
            for member in root.handles:
                lanes += self._lanes_for(member.material, member.offset, root, member)
        else:
            lanes += self._lanes_for(root, float(root.onset or 0.0), None, None)

        # The bottom lane rules the shared axis (one ruler under the stack is the
        # DAW convention); every lane carries the tempo/rate its ticks read.
        if lanes:
            lanes[-1]["ruler"] = "beats"
        return window(*lanes, title=self.title, w=self.size[0], h=self.size[1],
                      layout="col")

    def open(self, host, id: int | None = None) -> int:
        """`render` the composition and open it on ``host`` (a
        `clausters.gui.host.GuiHost`). Returns the window id."""
        self._host = host
        self._window = host.open(self.render(), id=id)
        return self._window

    @property
    def window(self):
        """The open window's id, or ``None`` once it is closed (a `/gui_closed`
        seen by `apply`/`poll`) — what a script's loop checks to stop."""
        return self._window

    def update(self):
        """Push the current model back to the open window — a whole-tree redefine
        (`GuiHost.define`), the honest way to show a structural edit (a material
        added, a group expanded). A mere placement change needs no redefine: the
        host already moved the clip that was dragged."""
        if self._host is None or self._window is None:
            raise RuntimeError("open(host) the editor first")
        self._host.define(self._window, self.render())

    # ---- the edit-back: a dragged clip becomes a placement ----

    def apply(self, addr: str, args) -> bool:
        """Apply one message from the host to the **model**. Returns whether the
        composition changed.

        The clip edit-back (``/gui_event <id> "clip" <offset> <dur>``, the payload
        a drag or a resize sends) is resolved through the widget registry to the
        placement it came from and written with `Group.move`. The clip's offset is
        **absolute** on the shared axis while a placement is relative to its
        group, so the position converts back through the base the clip was drawn
        at; and only what actually moved is written — a drag carries the clip's
        unchanged ``dur`` along, and snapping *that* to the grid would silently
        shorten the material. ``/gui_closed`` drops the window; anything else is
        ignored, so a whole poll loop can be fed straight in.
        """
        if addr == "/gui_closed":
            self._window = None
            return False
        if addr != "/gui_event" or len(args) < 4 or args[1] != "clip":
            return False
        placed = self._clips.get(int(args[0]))
        if placed is None or placed.member is None:
            return False  # unknown widget, or the root material (nothing places it)

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
        if self.follow:
            self.rerealize()
        return True

    def poll(self, timeout: float = 0.0) -> bool:
        """Drain the host's pending messages into the model (`apply` each).
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
        snapped the drag to, now in the model's units — so the round trip
        render → drag → apply → render is exact, free of the wire's float noise)."""
        if self.quant <= 0.0:
            return beats
        return round(beats / self.quant) * self.quant

    # ---- realization: the edited model back to sound ----

    def realize(self, destination, clock=None, *, at: float = 0.0, quant=None):
        """Realize the composition onto ``destination`` — RT (a `Server` and a
        running clock) or NRT (a score) — and anchor the lanes' playhead so the
        line sweeps the clips as it plays. Returns the `clausters.seq.Playhead`.

        This is the model's own `realize` (flatten to absolute beats, play through
        a playhead): the editor adds no realization path, it only remembers the
        destination so `rerealize` can re-schedule after an edit.
        """
        from ..model.realize import realize as model_realize

        self._destination, self._clock = destination, clock
        self._playhead = model_realize(self.material, destination, clock,
                                       at=at, quant=quant)
        self.anchor(destination, at=at)
        return self._playhead

    def rerealize(self, *, at: float | None = None):
        """Re-schedule the (edited) composition from the playhead's current
        position: stop, re-flatten, play again.

        The honest semantics are **re-schedule from here**, not a sample-exact
        splice — a synth already sounding keeps sounding, and what changes is what
        has not been scheduled yet. In NRT there is no "already", so it is simply
        a fresh score.
        """
        if self._destination is None:
            raise RuntimeError("realize(destination, clock) the editor first")
        if at is None:
            at = self._playhead.position() if self._playhead is not None else 0.0
        if self._playhead is not None:
            self._playhead.stop()
        return self.realize(self._destination, self._clock, at=at)

    def anchor(self, server, *, at: float = 0.0):
        """Anchor every lane's playhead to the engine clock, so the line starts at
        beat ``at`` of the timeline and sweeps on with the audio.

        ``playhead_at`` is the sample-clock value at timeline position 0, which is
        *now* minus the beats already played. A destination with no clock reply
        (an NRT score) simply gets no playhead.
        """
        if self._host is None or not hasattr(server, "request"):
            return
        try:
            _addr, args = server.request("/clock", expect=("/clock.reply",))
        except Exception:
            return  # NRT, or a server that does not answer: no live playhead
        now = float(args[0])
        origin = now - self.beats_to_units(at)
        for lane in self._lanes:
            self._host.set(lane, playhead_at=origin)

    # ---- the tree walk ----

    def _lanes_for(self, material, base: float, owner, member) -> list:
        """The lanes a material contributes: a compositional `Group` becomes one
        lane holding its members as clips (plus a lane of its own for every
        *expanded* nested group); anything else becomes a lane with one clip.
        ``base`` is its start in beats, ``owner``/``member`` the placement an
        edit-back writes through."""
        if isinstance(material, Group) and material.kind == COMPOSITIONAL:
            clips, extra = [], []
            for child in material.handles:
                child_base = base + child.offset
                if isinstance(child.material, Group) and self.is_expanded(child.material):
                    extra += self._lanes_for(child.material, child_base, material, child)
                else:
                    clips.append(self._clip_for(child.material, child_base, material, child))
            lane = [self._lane(clips, _name(material))] if clips else []
            return lane + extra
        return [self._lane([self._clip_for(material, base, owner, member)], _name(material))]

    def _lane(self, clips: list, label: str) -> dict:
        """One `track` lane holding ``clips``, with the shared time chrome."""
        wid = next(self._ids)
        lane = track(wid, *clips, label=label, sample_rate=self.sample_rate,
                     tempo=self.tempo,
                     snap=self.beats_to_units(self.quant) if self.quant > 0 else None)
        self._lanes[wid] = label
        return lane

    def _clip_for(self, material, base: float, owner, member) -> dict:
        """One `clip`: the material placed at ``base`` beats (absolute on the
        shared axis), with the body its kind calls for — a take, a piano-roll, or
        the labeled rectangle that summarizes a collapsed group. Registers what it
        drew, which is what the edit-back path resolves against."""
        wid = next(self._ids)
        offset = self.beats_to_units(base)
        # The length shown, in beats: the placement's when it overrides, else the
        # material's own.
        dur_beats = member.dur if (member is not None and member.dur is not None) else None
        if dur_beats is None and isinstance(material, Material):
            dur_beats = material.duration
        label = _name(material)

        if isinstance(material, Buffer):
            buf = material.wraps
            # The take rides the bulk path: the host fetches the server buffer and
            # decimates it through its peak pyramid. With no duration given, the
            # take's own length is it (1 timeline unit = 1 audio sample) — which
            # needs the frame count, so a buffer read but never queried (its shape
            # unknown client-side) must carry a `duration`.
            dur = (self.beats_to_units(dur_beats) if dur_beats is not None
                   else float(buf.frames))
            body = dict(buffer=buf.bufnum, channels=max(1, buf.channels))
        else:
            dur = self.beats_to_units(
                dur_beats if dur_beats is not None else self._extent(material))
            notes = self._notes(material)
            if notes:
                pitches = [n[2] for n in notes]
                body = dict(notes=notes,
                            min=min(min(pitches) - PITCH_PAD, DEFAULT_PITCH[1]),
                            max=max(max(pitches) + PITCH_PAD, DEFAULT_PITCH[0]))
            else:
                # No body: a collapsed group (or a material with nothing to draw)
                # is the labeled rectangle — the summary of the level above it.
                body = {}

        # The placement's own base: a clip's offset is absolute on the shared axis,
        # a member's offset is relative to its group.
        parent_base = base - (member.offset if member is not None else 0.0)
        self._clips[wid] = _Placed(owner, member, parent_base, offset, dur)
        return clip(wid, offset=offset, dur=dur, label=label, **body)

    def _notes(self, material) -> list:
        """The ``(start, dur, pitch)`` note events of a material, in timeline
        units relative to the material — the piano-roll body. A `Group` is a
        summary, not a roll (it collapses to a rectangle), and a note is any
        flattened event that resolves a pitch: the *change of state* of a
        contained generator happens right here (a pattern is bounced by
        `clausters.model.realize.flatten`), so a generator lane shows the notes it
        will play."""
        if isinstance(material, (Group, Buffer)):
            return []
        try:
            events = flatten(material, 0.0)
        except (NotImplementedError, TypeError):
            return []
        notes = []
        for beat, item in events:
            pitch = _pitch(item)
            if pitch is None:
                continue
            notes.append((self.beats_to_units(beat),
                          self.beats_to_units(_event_dur(item)), pitch))
        return notes

    def _extent(self, material) -> float:
        """A material's length in beats: its own ``duration`` when it has one,
        else what it spans — a group over its placed members, anything else over
        its flattened events (a bounced pattern included)."""
        if isinstance(material, Material) and material.duration is not None:
            return float(material.duration)
        if isinstance(material, Group):
            return max((m.offset + (m.dur if m.dur is not None
                                    else self._extent(m.material))
                        for m in material.handles), default=0.0)
        if isinstance(material, Buffer):
            buf = material.wraps
            rate = buf.sample_rate or self.sample_rate
            return self.units_to_beats(buf.frames * (self.sample_rate / rate))
        try:
            events = flatten(material, 0.0)
        except (NotImplementedError, TypeError):
            return 0.0
        return max((beat + _event_dur(item) for beat, item in events), default=0.0)


def _name(material) -> str:
    """A material's display name: its own ``name`` when it has one (a logical
    group names its GraphDef), else its kind — enough to read a lane header."""
    name = getattr(material, "name", None)
    if isinstance(name, str) and name:
        return name
    return type(material).__name__.lower()


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


def _event_dur(item) -> float:
    """A flattened item's length in beats: an event's **sounding** time
    (``sustain``, which is what a note bar should show), 0 when it is punctual."""
    if isinstance(item, SeqEvent):
        try:
            return float(item.sustain())
        except (KeyError, TypeError, ValueError):
            return 0.0
    return 0.0
