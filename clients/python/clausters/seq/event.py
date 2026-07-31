"""Events (port of ``sc3/seq/event.py``, adapted to Clausters).

An `Event` is a dict of parameters with sensible defaults that knows how
to **play itself** against a `Server`. The default
``'note'`` event creates a synth and schedules its release. Timing is the
clock's job: an event emits at the running routine's exact logical beat (via
``server.send_bundle``), and the player advances by the event's `delta`.

By default a note **frees** its synth after ``sustain`` (``/n_free``) rather
than closing a gate — unless ``has_gate`` is set, in which case it sends
``gate 0`` (for defs whose `env_gen` envelope has a release node and a
``doneAction`` that frees the synth once the release finishes). The built-in
``"default"`` instrument is the exception: it carries such an envelope and is
always released by its gate, so it ramps out without a click even with the
global ``has_gate`` default left False.
"""

from .. import _native
from ..base.builtins import cpsmidi, midicps
from ..defs.node import Node

#: Keys that drive timing/structure and are never sent as synth controls.
#: ``node`` and ``server`` are the play-completed keys (see `Event.play`).
_RESERVED = {
    "type", "instrument", "dur", "legato", "stretch", "sustain", "delta",
    "add_action", "target", "group", "server", "has_gate",
    "midinote", "degree", "octave", "root", "scale", "node",
}

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
    ``degree`` within ``octave``/``root``/``scale``), `delta` is the beats to
    the next event and `sustain` the beats the synth sounds. `play` renders the
    event on a destination -- a `Server` or a MIDI destination.
    """

    def __init__(self, *args, **kwargs):
        merged = dict(DEFAULTS)
        merged.update(dict(*args, **kwargs))
        super().__init__(merged)

    # ---- derived quantities ----

    def midinote(self) -> float:
        """The MIDI note number this event sounds (the value `freq` derives
        from). An explicit `freq` (Hz) is inverted via `cpsmidi`; otherwise it
        comes from `midinote`, or `degree`/`octave`/`root`/`scale`."""
        if self.get("freq") is not None:
            return float(cpsmidi(float(self["freq"])))
        midinote = self.get("midinote")
        if midinote is None:
            degree = self.get("degree")
            if degree is None:
                return 60.0
            # Pitch-space resolution is the core's shared rule (floored octave
            # wrapping), so every client's Event resolves degrees identically.
            return _native.degree_to_midinote(
                float(degree), float(self["octave"]), float(self["root"]), self["scale"]
            )
        return float(midinote)

    def freq(self) -> float:
        """The frequency in Hz this event sounds: an explicit ``freq`` if given,
        otherwise `midinote` converted through the native ``midicps``."""
        if self.get("freq") is not None:
            return float(self["freq"])
        return float(midicps(self.midinote()))

    def delta(self) -> float:
        """Beats until the next event: an explicit ``delta`` key if given,
        otherwise ``dur * stretch``. As in SuperCollider, the key overrides the
        calculation when it is present."""
        d = self.get("delta")
        return float(d) if d is not None else float(self["dur"]) * float(self["stretch"])

    def sustain(self) -> float:
        """Beats the synth sounds: an explicit ``sustain`` key if given,
        otherwise ``dur * legato * stretch``. As in SuperCollider, the key
        overrides the calculation when it is present."""
        s = self.get("sustain")
        if s is not None:
            return float(s)
        return float(self["dur"]) * float(self["legato"]) * float(self["stretch"])

    def _control_args(self) -> list:
        args = ["freq", self.freq(), "amp", float(self["amp"])]
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
        `Server` turns it into `/s_new` + release,
        a MIDI destination into note on/off — without the clock or routine
        knowing which.

        Returns **this event, with its keys completed**: the derived
        quantities are written in (``midinote``, ``freq``, ``delta``,
        ``sustain`` — the values actually used) along with ``node`` (the
        synth node id; ``None`` for a rest or MIDI) and ``server`` (the
        destination), so the note stays actionable after the fact — `free`
        cuts it, `release` closes it musically. The scheduled self-release
        still arrives regardless.

        ``destination`` is optional: omitted, it resolves to the ambient server
        (the running session's, else the default session's — booted with
        ``Server.boot()``), so ``Event().play()`` sounds a note with no `Session`
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
        """Cut the played note **now** (``/n_free``), without waiting for its
        sustain — for interrupting an extreme duration. A no-op when the event
        has not sounded (a rest, a MIDI play, or never played). The release
        already scheduled at play time still arrives and is harmless."""
        node, server = self.get("node"), self.get("server")
        if node is not None and server is not None:
            Node(node, server).free()

    def release(self):
        """End the played note **musically**, now: the event's own release
        gesture — ``gate 0`` when it releases by gate (``has_gate``, or the
        built-in ``"default"`` instrument's envelope), a plain ``/n_free``
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
