"""Events (port of ``sc3/seq/event.py``, adapted to Clausters).

An `Event` is a dict of parameters with sensible defaults that knows how
to **play itself** against a `Server`. The default
``'note'`` event creates a synth and schedules its release. Timing is the
clock's job: an event emits at the running routine's exact logical beat (via
``server.send_bundle``), and the player advances by the event's `delta`.

Difference from scsynth: Clausters synths have no ``doneAction`` envelopes yet,
so a note **frees** its synth after ``sustain`` (``/n_free``) rather than
closing a gate — unless ``has_gate`` is set, in which case it sends
``gate 0`` (for defs that expose a ``gate`` control).
"""

from ..base.builtins import cpsmidi, midicps

#: Keys that drive timing/structure and are never sent as synth controls.
_RESERVED = {
    "type", "instrument", "dur", "legato", "stretch", "sustain", "delta",
    "add_action", "target", "group", "server", "has_gate",
    "midinote", "degree", "octave", "root", "scale",
}

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
    """A parameter dict with note-event defaults and a `play`."""

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
            scale = self["scale"]
            n = len(scale)
            d = int(degree)
            return 12.0 * self["octave"] + self["root"] + scale[d % n] + 12 * (d // n)
        return float(midinote)

    def freq(self) -> float:
        if self.get("freq") is not None:
            return float(self["freq"])
        return float(midicps(self.midinote()))

    def delta(self) -> float:
        """Beats until the next event (``dur * stretch``)."""
        return float(self["dur"]) * float(self["stretch"])

    def sustain(self) -> float:
        """Beats the synth sounds (``dur * legato * stretch``)."""
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

    def play(self, destination):
        """Realize this event on ``destination`` (double dispatch): the OSC
        `Server` turns it into `/s_new` + release,
        a MIDI destination into note on/off — without the clock or routine
        knowing which. Returns whatever the destination's ``play_event`` does
        (the synth node id for OSC, ``None`` for a rest or MIDI)."""
        return destination.play_event(self)


def rest(dur: float = 1.0) -> Event:
    """A silent event that still advances time by ``dur``."""
    return Event(type="rest", dur=dur)
