"""Events (port of ``sc3/seq/event.py``, adapted to Clausters).

An :class:`Event` is a dict of parameters with sensible defaults that knows how
to **play itself** against a :class:`~clausters.defs.server.Server`. The default
``'note'`` event creates a synth and schedules its release. Timing is the
clock's job: an event emits at the running routine's exact logical beat (via
``server.send_bundle``), and the player advances by the event's :meth:`delta`.

Difference from scsynth: Clausters synths have no ``doneAction`` envelopes yet,
so a note **frees** its synth after ``sustain`` (``/n_free``) rather than
closing a gate — unless ``has_gate`` is set, in which case it sends
``gate 0`` (for defs that expose a ``gate`` control).
"""

from ..base.builtins import midicps

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
    """A parameter dict with note-event defaults and a :meth:`play`."""

    def __init__(self, *args, **kwargs):
        merged = dict(DEFAULTS)
        merged.update(dict(*args, **kwargs))
        super().__init__(merged)

    # ---- derived quantities ----

    def freq(self) -> float:
        if self.get("freq") is not None:
            return float(self["freq"])
        midinote = self.get("midinote")
        if midinote is None:
            degree = self.get("degree")
            if degree is None:
                midinote = 60.0
            else:
                scale = self["scale"]
                n = len(scale)
                d = int(degree)
                midinote = 12.0 * self["octave"] + self["root"] + scale[d % n] + 12 * (d // n)
        return float(midicps(midinote))

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

    def play(self, server):
        """Emit this event to ``server`` at the current logical beat. Returns
        the synth node id (or None for a rest)."""
        if self.get("type") == "rest":
            return None
        node_id = server.nodes.alloc()
        server.send_bundle(
            ("/s_new", self["instrument"], node_id, int(self["add_action"]),
             int(self["target"]), *self._control_args())
        )
        sustain = self.sustain()
        if self.get("has_gate"):
            server.send_bundle(("/n_set", node_id, "gate", 0.0), delay_beats=sustain)
        else:
            server.send_bundle(("/n_free", node_id), delay_beats=sustain)
        return node_id


def rest(dur: float = 1.0) -> Event:
    """A silent event that still advances time by ``dur``."""
    return Event(type="rest", dur=dur)
