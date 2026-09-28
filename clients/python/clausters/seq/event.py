"""Events (port of ``sc3/seq/event.py``, adapted to Clausters).

An `Event` is a dict of parameters with sensible defaults that knows how
to **play itself** against a `Server`. The default
``'note'`` event creates a synth and schedules its release. Timing is the
clock's job: an event emits at the running routine's exact logical beat (via
``server.send_bundle``), and the player advances by the event's `delta`.

By default a note **frees** its synth after ``sustain`` (``/node_free``) rather
than closing a gate -- unless ``has_gate`` is set, in which case it sends
``gate 0`` (for defs whose `env_gen` envelope has a release node and a
``doneAction`` that frees the synth once the release finishes). The built-in
``"default"`` instrument is the exception: it carries such an envelope and is
always released by its gate, so it ramps out without a click even with the
global ``has_gate`` default left False.
"""

from .. import _native
from ..defs.node import Node

#: Keys that drive timing/structure and are never sent as synth controls.
#: ``node`` and ``server`` are the play-completed keys (see `Event.play`).
_RESERVED = {
    "type", "instrument", "dur", "legato", "stretch", "sustain", "delta",
    "add_action", "target", "group", "server", "has_gate",
    "midinote", "degree", "alter", "octave", "root", "scale", "node", "db",
    # What the note says on a page. None of it is a synth control, and a
    # `bool` is an `int` in Python -- so a `tie=True` that was not reserved
    # would be sent as a control ``1.0`` and ignored in silence.
    "articulations", "dynamic", "ornament", "grace", "stem",
    "spelling", "accidental", "tie",
}

#: The reserved keys that say what the note is on a **page** rather than what it
#: does in the air, read by `clausters.gui.notation.sheet_from_notes` and
#: written back by `clausters.gui.notation.to_timeline`. Every one is a musical
#: fact -- ``articulations=["stacc"]``, not an instruction to shorten a drawn
#: value -- which is what lets the same key be read in both directions.
NOTATION_KEYS = (
    "articulations", "dynamic", "ornament", "grace", "stem",
    "spelling", "accidental", "tie",
)

#: Default parameters merged into every `Event`. ``type`` selects behaviour
#: (``note`` or ``rest``); ``instrument`` is the def name; ``dur`` is the beats
#: to the next event, scaled by ``legato``/``stretch`` into the sounding time;
#: ``amp`` is linear amplitude; ``add_action``/``target`` place the synth in the
#: node tree; ``has_gate`` picks release-by-free vs ``gate 0``; and
#: ``octave``/``root``/``scale`` define the pitch space that ``degree`` indexes.
DEFAULTS = {
    "type": "note",
    "instrument": "default",
    "dur": 1.0,
    "legato": 0.8,
    "stretch": 1.0,
    "amp": 0.1,
    "add_action": 1,        # tail
    "target": 0,            # root group
    "has_gate": False,      # Clausters: free on release by default
    "octave": 5.0,
    "root": 0.0,
    "scale": (0, 2, 4, 5, 7, 9, 11),  # major
}

#: The two key families whose keys are spellings of one quantity, and the
#: order a caller's keys are written in when an event is built: lowest first,
#: so where a caller states two spellings the last one -- ``freq``, ``amp`` --
#: is the one the others follow, SuperCollider's precedence.
_PITCH_ORDER = ("scale", "root", "octave", "degree", "alter", "midinote", "freq")
_LEVEL_ORDER = ("db", "velocity", "amp")
_PITCH = frozenset(_PITCH_ORDER)
_LEVEL = frozenset(_LEVEL_ORDER)
#: The pitch keys an edit to another pitch key can rewrite.
_SPELLINGS = ("freq", "midinote", "degree")


class Event(dict):
    """A note event: a ``dict`` of parameters that knows how to play itself.

    Built from `DEFAULTS` overlaid with whatever you pass, exactly like a dict
    -- keyword arguments (``Event(freq=440, amp=0.2)``), a mapping
    (``Event({"freq": 440, "amp": 0.2})``), or both merged, with keywords
    winning -- so unknown keys are simply stored. The keys
    split in two: a fixed **reserved** set drives timing and structure (``dur``,
    ``legato``, ``stretch``, ``add_action``/``target``, the pitch keys, ...) and
    is never sent to the synth; every other numeric key is forwarded as a synth
    control.

    The derived quantities compute the values actually used: `midinote` and
    `freq` resolve pitch (an explicit ``freq`` wins, else ``midinote``, else
    ``degree`` altered by ``alter`` within ``octave``/``root``/``scale``),
    `amp` and `velocity` the level (an explicit ``amp`` wins, else
    ``velocity``, else ``db``), `delta` is the beats to the next event and
    `sustain` the beats the synth sounds. `play` renders the event on a
    destination -- a `Server` or a MIDI destination. The rules are the shared
    core's, so every client's event sounds the same.

    **A family's keys stay coherent.** ``freq``, ``midinote`` and ``degree`` +
    ``alter`` are spellings of one pitch, and ``amp``, ``velocity`` and ``db``
    of one level: writing one of them rewrites the others the event holds, so
    a note moved by ``midinote`` does not go on sounding the ``freq`` it was
    written with. A key the event does not hold is not added. Built with two
    spellings of one family, the event takes ``freq`` over ``midinote`` over
    the degree, and ``amp`` over ``velocity`` over ``db``.

    **A degree is altered by ``alter``**, in semitones (real, so a microtone
    is one too). SuperCollider's fraction (``degree=1.1`` for degree 1 sharp)
    and a pair (``degree=(1, 1)``) are both read as the two keys.

    An event may also carry what the note is **on a page** (`NOTATION_KEYS`):
    ``articulations``, ``dynamic``, ``ornament``, ``grace``, ``stem``,
    ``spelling``, ``accidental`` and ``tie``. They change nothing about how the
    event sounds -- an articulation is honoured when a *score* is read, not when
    an event is played -- and they are reserved, so none of them reaches the
    synth as a control. What reads them is
    `clausters.gui.notation.sheet_from_notes`.
    """

    def __init__(self, *args, **kwargs):
        given = dict(*args, **kwargs)
        super().__init__(DEFAULTS)
        for key, value in given.items():
            if key not in _PITCH and key not in _LEVEL:
                dict.__setitem__(self, key, value)
        for key in _PITCH_ORDER:
            if key in given:
                self[key] = given[key]
        for key in _LEVEL_ORDER:
            if key in given:
                self[key] = given[key]

    # ---- writing a key ----

    def __setitem__(self, key, value):
        if value is None:
            dict.__setitem__(self, key, value)
        elif key in _PITCH:
            self._set_pitch(key, value)
        elif key in _LEVEL:
            self._set_level(key, value)
        else:
            dict.__setitem__(self, key, value)

    def update(self, *args, **kwargs):
        """Writes each key as ``event[key] = value`` does, so a family stays
        coherent."""
        for key, value in dict(*args, **kwargs).items():
            self[key] = value

    def _pitch_keys(self):
        return [self.get(k) for k in _native.PITCH_KEYS[:6]]

    def _set_pitch(self, key, value):
        if key == "degree" and isinstance(value, (tuple, list)):
            degree, alter = value
            self._set_pitch("degree", degree)
            self._set_pitch("alter", alter)
            return
        if key == "scale":
            dict.__setitem__(self, key, value)
        held = any(self.get(k) is not None for k in _SPELLINGS if k != key)
        if not held:
            # Nothing else spells this pitch, so nothing follows: only a
            # fractional degree has to be split.
            if (key == "degree" and isinstance(value, (int, float))
                    and float(value) != int(float(value))):
                degree, alter = _native.split_degree(float(value))
                dict.__setitem__(self, "degree", degree)
                dict.__setitem__(self, "alter", alter)
            elif key != "scale":
                dict.__setitem__(self, key, value)
            return
        keys = _native.pitch_set(self._pitch_keys(), key,
                                 0.0 if key == "scale" else float(value),
                                 self.get("scale") or (), self.get("spelling"))
        for name, v in zip(_native.PITCH_KEYS, keys):
            if v is not None:
                dict.__setitem__(self, name, v)

    def _set_level(self, key, value):
        keys = _native.level_set([self.get(k) for k in _native.LEVEL_KEYS], key,
                                 float(value))
        for name, v in zip(_native.LEVEL_KEYS, keys):
            if v is not None:
                dict.__setitem__(self, name, v)

    # ---- derived quantities ----

    def midinote(self) -> float:
        """The MIDI note number this event sounds (the value `freq` derives
        from): an explicit ``freq`` inverted, else ``midinote``, else ``degree``
        altered by ``alter`` within ``octave``/``root``/``scale``, else middle
        C."""
        return _native.pitch_resolve(self._pitch_keys(), self.get("scale") or ())[0]

    def freq(self) -> float:
        """The frequency in Hz this event sounds: an explicit ``freq`` if given,
        otherwise `midinote` in equal temperament."""
        return _native.pitch_resolve(self._pitch_keys(), self.get("scale") or ())[1]

    def amp(self) -> float:
        """The linear amplitude this event sounds at: an explicit ``amp``, else
        its ``velocity``, else its ``db``."""
        return _native.level_resolve([self.get(k) for k in _native.LEVEL_KEYS])[0]

    def velocity(self) -> int:
        """The velocity a note-on of this event carries (1..127): an explicit
        ``velocity``, else its amplitude's."""
        return int(_native.level_resolve([self.get(k) for k in _native.LEVEL_KEYS])[1])

    def delta(self) -> float:
        """Beats until the next event: an explicit ``delta`` key if given,
        otherwise ``dur * stretch``. As in SuperCollider, the key overrides the
        calculation when it is present."""
        return _native.event_delta(self["dur"], self["stretch"], self.get("delta"))

    def sustain(self) -> float:
        """Beats the synth sounds: an explicit ``sustain`` key if given,
        otherwise ``dur * legato * stretch``. As in SuperCollider, the key
        overrides the calculation when it is present."""
        return _native.event_sustain(self["dur"], self["legato"], self["stretch"],
                                     self.get("sustain"))

    def _control_args(self) -> list:
        args = ["freq", self.freq(), "amp", self.amp()]
        if self.get("out") is not None:
            args += ["out", float(self["out"])]
        # any extra numeric keys (custom controls) are sent verbatim
        for key, value in self.items():
            if key in _RESERVED or key in ("freq", "amp", "out"):
                continue
            if isinstance(value, (int, float)):
                args += [key, float(value)]
        return args

    # ---- play ----

    def play(self, destination=None):
        """Play this event on ``destination`` (double dispatch): the OSC
        `Server` turns it into `/synth_new` + release,
        a MIDI destination into note on/off -- without the clock or routine
        knowing which.

        Returns **this event, with its keys completed**: the derived
        quantities are written in (``midinote``, ``freq``, ``delta``,
        ``sustain`` -- the values actually used) along with ``node`` (the
        synth node id; ``None`` for a rest or MIDI) and ``server`` (the
        destination), so the note stays actionable after the fact -- `free`
        cuts it, `release` closes it musically. The scheduled self-release
        still arrives regardless.

        ``destination`` is optional: omitted, it resolves to the ambient server
        (the running session's, else the default session's -- booted with
        ``Server().boot()``), so ``Event().play()`` sounds a note with no `Session`
        wiring. Outside a clock the note plays immediately; inside a routine it
        emits at the routine's logical beat."""
        if destination is None:
            from ..base.main import main

            destination = main.resolve_server()
        midinote, freq = self.midinote(), self.freq()
        delta, sustain = self.delta(), self.sustain()
        self.update(midinote=midinote, freq=freq, delta=delta, sustain=sustain)
        self["node"] = destination.play_event(self)
        self["server"] = destination
        return self

    def free(self):
        """Cut the played note **now** (``/node_free``), without waiting for its
        sustain -- for interrupting an extreme duration. A no-op when the event
        has not sounded (a rest, a MIDI play, or never played). The release
        already scheduled at play time still arrives and is harmless."""
        node, server = self.get("node"), self.get("server")
        if node is not None and server is not None:
            Node(node, server).free()

    def release(self):
        """End the played note **musically**, now: the event's own release
        gesture -- ``gate 0`` when it releases by gate (``has_gate``, or the
        built-in ``"default"`` instrument's envelope), a plain ``/node_free``
        otherwise. Same no-op rule as `free`."""
        node, server = self.get("node"), self.get("server")
        if node is None or server is None:
            return
        if self.get("has_gate") or self["instrument"] == "default":
            Node(node, server).set({"gate": 0.0})
        else:
            Node(node, server).free()


def rest(dur: float = 1.0) -> Event:
    """A silent `Event` that sounds nothing but still advances time by ``dur``
    beats -- a rest in the sequence."""
    return Event(type="rest", dur=dur)
