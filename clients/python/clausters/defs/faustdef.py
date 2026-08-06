"""FaustDef: a named Faust definition ready for ``/def_send faust``.

Wraps a graph built with `clausters.defs.signals` (the **signal tree**
form), one built with `clausters.defs.boxes` (the **box tree** form — a `Box`,
or a raw dict for machine-generated trees), or a Faust **source** string: the
three payloads the server's ``/def_send faust`` accepts, on equal footing (it sniffs
which by the first byte; see the server's ``faust`` module). They are three ways
of writing Faust, not a main road and two detours — pick the one that says what
you mean. Sending and instantiating is the
`Server`'s job; this only builds the payload and
exposes the declared control names (UI labels), plus the reserved ``in``/``out``
bus controls the server adds.
"""

import json

from ._wire import resolve as _resolve, send_def
from .boxes import Box, check_wires
from .signals import Signal


class FaustDef:
    """DSP the server JIT-compiles: a named instrument, in the Faust language.

    A def is a recipe; `Synth` plays it. `FaustDef` is one of the two families
    that write that recipe, and it is a **peer** of `SynthDef`, not a fallback:
    a synth of either is the same node in the same tree, driven by the same
    `Node.set`, and neither is faster or more capable by construction. What
    differs is who compiles it. A `SynthDef` is a graph of the server's own
    UGens, wired at run time; a `FaustDef` is a Faust program the server hands
    to libfaust, which compiles it to machine code before the first block. So
    the whole Faust language is available — its libraries, its sample-level
    feedback, its block-diagram algebra — at the cost of a compile when the
    def lands.

    **Three ways to write one, all equal on the wire.** The server sniffs which
    it got; pick the one that says what you mean:

    - `from_signals` — `clausters.defs.signals` as Python callables and
      operators. The most Python-looking, and the one that composes with
      ordinary Python code.
    - `from_box` — `clausters.defs.boxes`, Faust's block-diagram algebra,
      point-free. Terse, and ``boxes.faust`` opens the whole Faust standard
      library (`os.osc`, `fi.lowpass`, `pm.*`) as composable pieces.
    - `from_source` — Faust source as a string, for DSP you already have or
      that reads best in its own language.

    Controls are the UI elements the program declares — an ``hslider``,
    ``nentry``, ``button`` — and their labels are the names `Node.set` uses.
    `control_names` lists them. The server adds two more of its own,
    ``in``/``out`` (see `reserved`), so a Faust def can be aimed at a `Bus`
    without declaring anything for it.

    The same instrument written three ways, and one of them played:

    ```python
    from clausters import FaustDef, Server, Synth
    from clausters.defs import boxes as box, signals as S

    s = Server().boot()

    # signals: the phasor written out, one sample of feedback
    freq = S.hslider("freq", 440.0, 20.0, 20000.0, 0.01)
    phase = S.rec(lambda p: (p + freq / S.sr()) % 1.0)
    FaustDef.from_signals("a", S.sin(phase * S.TAU) * 0.2).send(s)

    # boxes: the same oscillator, borrowed from Faust's library
    d = FaustDef.from_box(
        "b", box.faust("os.osc")(box.hslider("freq", 440.0, 20.0, 20000.0, 0.01)) * 0.2)
    d.send(s)

    # source: the same thing again, in Faust
    FaustDef.from_source("c", '''
        import("stdfaust.lib");
        freq = hslider("freq", 440, 20, 20000, 0.01);
        process = os.osc(freq) * 0.2;
    ''').send(s)

    print(d.control_names())                   # ['freq'] -- what it declares
    n = Synth("b", {"freq": 330.0}, server=s)   # a Synth, like any other
    n.free()
    ```

    Attributes:
        name: the def's name on the server — what `Synth` looks up, and what
            ``/def_send faust`` replies with on success.
        kind: which of the three payloads this def carries, ``"signals"``,
            ``"box"`` or ``"source"``.
    """

    def __init__(self, name: str, payload, kind: str):
        """Wraps an already-built payload. You normally do not call this: the
        three named constructors — `from_signals`, `from_box`, `from_source` —
        each build the payload for their form and pass it here.

        Args:
            name: the def's name on the server.
            payload: the signal or box tree (a dict), or Faust source (a str).
            kind: ``"signals"``, ``"box"`` or ``"source"``, matching it.
        """
        #: the def name (also what `/def_send faust` replies with on success)
        self.name = name
        self._payload = payload  # dict (signal/box tree) or str (source)
        self.kind = kind         # 'signals' | 'box' | 'source'

    # --- constructors ---

    @classmethod
    def from_signals(cls, name: str, *outputs) -> "FaustDef":
        """One output per argument (``Signal`` or number)."""
        if not outputs:
            raise ValueError("a signal def needs at least one output")
        nodes = [o.to_json() if isinstance(o, Signal) else o for o in outputs]
        return cls(name, {"signals": nodes}, "signals")

    @classmethod
    def from_source(cls, name: str, src: str) -> "FaustDef":
        return cls(name, src, "source")

    @classmethod
    def from_box(cls, name: str, box) -> "FaustDef":
        """From a `clausters.defs.boxes.Box` (or a raw box-tree dict, kept
        for machine-generated graphs). A `Box` is checked for the one silent
        mistake the box algebra allows: reusing the same ``wire()``/``cut()``
        object in two positions (each wire is a distinct input)."""
        if isinstance(box, Box):
            check_wires(box.node)
            box = box.to_json()
        return cls(name, box, "box")

    # --- serialization ---

    def dump_def(self) -> str:
        """The def serialized to text -- the ``/def_send faust <name> <payload>`` wire
        payload: a JSON signal/box tree, or the Faust source string verbatim.
        Useful to inspect the built graph before sending it."""
        if self.kind == "source":
            return self._payload
        return json.dumps(self._payload)

    def send(self, server=None, *, wait: bool = True,
             timeout: float = 10.0) -> str:
        """Sends this def to the server via ``/def_send faust`` and returns its
        name.

        ``/def_send faust`` JIT-compiles **asynchronously** on the server's network
        thread, answered later by ``/done``/``/fail``. ``wait=True``
        (the default) blocks in RT until ``/done``/``/fail`` -- raising
        `clausters.errors.CommandError` on the failure, or
        `clausters.errors.ReplyTimeout` if the reply never lands. ``wait=False``
        returns immediately (fire-and-forget), to be sequenced with the
        server's ``sync`` before anything relies on the def (``yield`` it from
        a routine, never block in one). In NRT the send is always *scored* at
        time 0 -- the renderer loads the def before time advances -- so
        ``wait`` does not apply."""
        return send_def(_resolve(server), "faust", (self.name, self.dump_def()),
                        self.name, wait, timeout)

    # --- controls ---

    def control_names(self) -> list[str]:
        """The control names this def declares (UI labels), in tree order.
        The reserved ``in``/``out`` bus controls (added by the server) are not
        included; see `reserved`."""
        names: list[str] = []
        if self.kind in ("signals", "box"):
            _collect_labels(self._payload, names)
        return names

    #: bus-selecting controls every Faust synth also accepts.
    reserved = ("out", "in")

    def plot_def(self, *, label: str | None = None, w: int = 1000, h: int = 700,
                 title: str | None = None, host=None):
        """Open this FaustDef's **structure** as a directed `patch` view in its
        own window on the ambient GUI host — the level-2 patcher drawn from the
        def's signal graph (every signal op a box, every operand a cord, the host
        laying them out as an inverted tree). One window per call, the
        `clausters.plot` posture; this shows the def's *structure*, where
        `clausters.plot(self)` renders its *sound*.

        A **signal-tree** def (`from_signals`) decodes node for node; a
        **box-tree** or **source** def is opaque and draws as a single box (its
        internals are the Faust compiler's, not reconstructable client-side).
        ``label`` captions the patch panel (defaults to ``"faustdef"`` — the
        panel names *what* is drawn, not the def's name); ``host`` is an explicit
        `clausters.gui.GuiHost`, ``None`` resolves the ambient one. Returns a
        `clausters.plot.PatchWindow` (``.close()``)."""
        from ..plot import _open_patch_view
        from .patch import DefPatch

        model = DefPatch.from_faustdef(self)
        return _open_patch_view(model, label=label if label is not None else "faustdef",
                                w=w, h=h, title=title or self.name, host=host)

    def __repr__(self):
        return f"FaustDef({self.name!r}, kind={self.kind!r})"


_CONTROL_OPS = {"hslider", "vslider", "nentry", "button", "checkbox"}


def _collect_labels(node, out: list[str]):
    if isinstance(node, dict):
        if node.get("op") in _CONTROL_OPS:
            label = node.get("label")
            if isinstance(label, str) and label not in out:
                out.append(label)
        for value in node.values():
            _collect_labels(value, out)
    elif isinstance(node, list):
        for item in node:
            _collect_labels(item, out)
