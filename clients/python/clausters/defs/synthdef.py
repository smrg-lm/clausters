"""SynthDef: a named UGen graph ready for ``/d_recv`` (port of the ``SynthDef``
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
server.add_synthdef(sdef)                                # /d_recv
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

from .ugens import ChannelList, Control, Ugen


class SynthDef:
    """A named UGen graph. Pass the graph's **root** UGens — normally the
    outputs (``out(...)`` / ``replace_out(...)``, and any ``local_out(...)`` to
    keep feedback writes in the graph), but a root can equally be a side-effect
    UGen with no audio output (``send_trig(...)`` / ``send_reply(...)`` /
    ``poll(...)``): a def may consist only of those and no ``out`` at all. Every
    root must be a UGen; a def needs at least one (the server rejects an empty
    graph). A def with no output UGen is simply silent on the server."""

    def __init__(self, name: str, *roots: Ugen):
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
        """The ``SynthDefSpec`` dict the server's ``/d_recv`` compiles."""
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
        """The def serialized to text -- the ``/d_recv`` wire payload, the JSON
        ``SynthDefSpec`` (see `spec`). Useful to inspect the built graph before
        sending it."""
        return json.dumps(self.spec())

    def control_names(self) -> list[str]:
        """The control names this def declares, in spec order (parallels
        `FaustDef.control_names`)."""
        return [c["name"] for c in self.spec()["controls"]]

    def __repr__(self):
        return f"SynthDef({self.name!r}, {len(self.outputs)} outputs)"
