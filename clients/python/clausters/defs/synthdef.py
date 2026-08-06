"""SynthDef: a named UGen graph ready for ``/def_send synth`` (port of the ``SynthDef``
side of ``sc3/synth``, adapted to Clausters' JSON ``SynthDefSpec``).

The UGen-graph counterpart of `FaustDef`: it
wraps one or more output `Ugen` nodes (built with
the lowercase callables in `clausters.defs.ugens`), walks the graph and
serializes the ``{"name", "controls", "ugens"}`` JSON the server compiles.

```python
from clausters.defs import SynthDef, control, sine, out

freq = control("freq", 440.0)
amp = control("amp", 0.2)
sig = sine(freq) * amp
sdef = SynthDef("beep", out(0.0, sig), out(1.0, sig))   # stereo
sdef.send(server)                                # /def_send synth
```

**Instance-based build (no globals).** The walk is a plain post-order traversal
of the output nodes: a UGen is emitted only after its inputs, so the ``ugens``
list is topologically ordered (every ``{"ugen": w}`` reference points at an
earlier node, as the server requires) and shared sub-graphs are emitted once
(dedup by object identity). Controls are gathered in first-seen order; reusing
the same name with a different default is an error. No thread-global build
context is touched, so defs build concurrently.
"""

import json

from ._wire import resolve as _resolve, send_def
from .ugens import ChannelList, Control, Ugen


class SynthDef:
    """A graph of the server's own UGens: a named instrument, wired in Python.

    A def is a recipe; `Synth` plays it. `SynthDef` is one of the two families
    that write that recipe, and it is a **peer** of `FaustDef`, not the default
    one: a synth of either is the same node in the same tree, driven by the
    same `Node.set`. What differs is who computes the sound. A `SynthDef` names
    the DSP units the server already ships — `clausters.defs.ugens`, the
    lowercase callables — and the server wires them at run time, so there is
    nothing to compile and the vocabulary is the server's. A `FaustDef` ships a
    program instead, and gets the Faust language in exchange for a JIT compile.

    **You build it by writing the signal, not by wiring it.** A UGen callable
    returns a value, and values compose with ordinary Python operators, so the
    graph is the expression:

    ```python
    from clausters import Server, Synth, SynthDef
    from clausters.defs import Env, DoneAction, control, env_gen, out, sine

    s = Server().boot()

    freq = control("freq", 440.0)               # a named control: /node_set reaches it
    env = env_gen(Env.perc(), done_action=DoneAction.FREE_SELF)
    d = SynthDef("beep", out(0, sine(freq) * 0.2 * env))
    d.send(s)                                   # waits for the server's /done

    print(d.control_names())                    # ['freq']
    Synth("beep", {"freq": 660.0}, server=s)    # sounds, then frees itself
    ```

    **The arguments are the graph's roots**, not its output. Usually those are
    the outputs — ``out``, ``replace_out``, and any ``local_out`` that closes a
    feedback path inside the graph — but a root can equally be a UGen with no
    audio output at all: ``send_trig``, ``send_reply`` and ``poll`` are roots
    because nothing reads them. A def may be nothing but those, which is how
    you write an analyzer that reports and makes no sound. What a def cannot be
    is empty: the server rejects a graph with no roots.

    Everything reachable from a root is walked in post-order, so the wire's
    UGen list is topologically sorted and a sub-graph used twice is emitted
    once. Nothing global is touched during the build, so defs can be built
    concurrently — the graph is only the expression you passed.

    Sending is asynchronous on the server, and `send` waits for the ``/done``
    by default; offline it is scored at time 0 instead. Either way the def is
    installed when the call returns, which is what makes the `Synth` on the
    next line safe.

    Attributes:
        name: the def's name on the server — what `Synth` looks up.
    """

    def __init__(self, name: str, *roots: Ugen):
        """Builds the graph reachable from ``roots``.

        Args:
            name: the name the server files it under, and `Synth` names.
            *roots: the graph's root UGens — the outputs, plus any
                side-effect UGen nothing reads. A multichannel root (an
                ``out`` over a `clausters.defs.ChannelList`) counts as one
                root per channel.

        Raises:
            ValueError: no roots at all.
            TypeError: a root that is not a `clausters.defs.Ugen`.
        """
        flat: list[Ugen] = []
        for o in roots:
            # A multichannel root (out(bus, dup(sig)) returns a ChannelList of
            # Outs) contributes one root per channel.
            if isinstance(o, ChannelList):
                flat.extend(o.items)
            else:
                flat.append(o)
        if not flat:
            raise ValueError(
                "a SynthDef needs at least one root UGen (an output like "
                "out(bus, signal), or a side-effect UGen like send_trig(...))"
            )
        for o in flat:
            if not isinstance(o, Ugen):
                raise TypeError(f"SynthDef roots must be UGens, got {o!r}")
        self.name = str(name)
        self.outputs = flat

    def spec(self) -> dict:
        """The ``SynthDefSpec`` dict the server's ``/def_send synth`` compiles."""
        ordered: list[Ugen] = []      # UGens in topological order
        wire: dict[int, int] = {}     # id(ugen) -> its index in `ordered`
        controls: list[Control] = []  # controls in first-seen order
        ctl_index: dict[str, int] = {}

        def visit(node):
            if isinstance(node, Ugen):
                if id(node) in wire:
                    return
                for inp in node.inputs:
                    visit(inp)
                wire[id(node)] = len(ordered)
                ordered.append(node)
            elif isinstance(node, Control):
                seen = ctl_index.get(node.name)
                if seen is None:
                    ctl_index[node.name] = len(controls)
                    controls.append(node)
                elif controls[seen]._signature() != node._signature():
                    raise ValueError(
                        f"control {node.name!r} used with conflicting definitions "
                        f"(default/type/lag differ)"
                    )
            elif isinstance(node, ChannelList):
                raise TypeError(
                    "a channel list cannot feed a single-channel input -- "
                    "index it (chans[0]) or mix() it down; per-argument "
                    "multichannel expansion is not implemented"
                )
            elif isinstance(node, bool) or not isinstance(node, (int, float)):
                raise TypeError(f"not a UGen graph node: {node!r}")
            # a plain number is a constant: nothing to gather here

        for o in self.outputs:
            visit(o)

        def ser(inp):
            if isinstance(inp, Ugen):
                return {"ugen": wire[id(inp)]}
            if isinstance(inp, Control):
                return {"control": ctl_index[inp.name]}
            return {"const": float(inp)}

        def ser_control(c):
            d = {"name": c.name, "default": c.default}
            if c.rate is not None:
                d["rate"] = c.rate
            if c.lag is not None:
                d["lag"] = c.lag
            if c.lag_down is not None:
                d["lag_down"] = c.lag_down
            return d

        def ser_ugen(u):
            d = {"kind": u.kind, "inputs": [ser(i) for i in u.inputs]}
            if u.rate is not None:
                d["rate"] = u.rate
            if getattr(u, "op", None) is not None:
                d["op"] = u.op
            if getattr(u, "label", None) is not None:
                d["label"] = u.label
            static = getattr(u, "static", None)
            if static:
                d.update(static)
            return d

        return {
            "name": self.name,
            "controls": [ser_control(c) for c in controls],
            "ugens": [ser_ugen(u) for u in ordered],
        }

    def dump_def(self) -> str:
        """The def serialized to text -- the ``/def_send synth`` wire payload, the JSON
        ``SynthDefSpec`` (see `spec`). Useful to inspect the built graph before
        sending it."""
        return json.dumps(self.spec())

    def send(self, server=None, *, wait: bool = True,
             timeout: float = 10.0) -> str:
        """Sends this def to the server via ``/def_send synth`` and returns its
        name.

        ``wait=True``
        (the default) blocks in RT until ``/done``/``/fail`` -- raising
        `clausters.errors.CommandError` on the failure, or
        `clausters.errors.ReplyTimeout` if the reply never lands. ``wait=False``
        returns immediately (fire-and-forget), to be sequenced with the
        server's ``sync`` before anything relies on the def (``yield`` it from
        a routine, never block in one). In NRT the send is always *scored* at
        time 0 -- the renderer loads the def before time advances -- so
        ``wait`` does not apply."""
        return send_def(_resolve(server), "synth", (self.dump_def(),),
                        self.name, wait, timeout)

    def control_names(self) -> list[str]:
        """The control names this def declares, in spec order (parallels
        `FaustDef.control_names`)."""
        return [c["name"] for c in self.spec()["controls"]]

    def plot_def(self, *, label: str | None = None, w: int = 1000, h: int = 700,
                 title: str | None = None, host=None):
        """Open this SynthDef's **structure** as a directed `patch` view in its
        own window on the ambient GUI host — the level-2 patcher drawn from the
        def's internal UGen graph (every UGen a box, every input a cord, the host
        laying them out as an inverted tree). One window per call, the
        `clausters.plot` posture; this shows the def's *structure*, where
        `clausters.plot(self)` renders its *sound*.

        ``label`` captions the patch panel (defaults to ``"synthdef"`` — the
        panel names *what* is drawn, not the def's name); ``host`` is an explicit
        `clausters.gui.GuiHost`, ``None`` resolves the ambient one. Returns a
        `clausters.plot.PatchWindow` (``.close()``)."""
        from ..plot import _open_patch_view
        from .patch import DefPatch

        model = DefPatch.from_synthdef(self)
        return _open_patch_view(model, label=label if label is not None else "synthdef",
                                w=w, h=h, title=title or self.name, host=host)

    def __repr__(self):
        return f"SynthDef({self.name!r}, {len(self.outputs)} outputs)"
