"""The directed patcher model: boxes with typed inlets/outlets and cords.

This is the **programmatic** patcher — build a signal graph in code, compile it,
send it — of which the GUI `graph` widget is only a visual view (it edits the
same model). There are two levels, one directed-cord grammar apart by what a box
*is* and what the patch *compiles to*:

- **`GraphPatch`** (this module, level 1): a box is a **whole def** (a
  SynthDef/FaustDef the server has) — itself a graph — and the patch compiles to
  a **`GraphDef`**, whole nodes wired by server buses. A cord *is* a bus, but you
  never number one: `compile` runs the shared cord->bus pass
  (`clausters_core::patch`, via `clausters._native`) that names one bus per
  connected net (its writers summing).
- **`DefPatch`** (level 2, the P5 milestone — not built yet): a box is a single
  **UGen** and the patch compiles to a **`SynthDef`/`FaustDef`** (a def) through
  the existing builders (its buses are internal wires, implicit).

A box has typed **inlets** and **outlets**; a **cord** runs an outlet to an
inlet, and audio and control cords never connect.

    from clausters.defs import GraphPatch

    p = GraphPatch()
    tone = p.add("tone", outlets=["out"])
    dac = p.add("dac", inlets=["in"])    # a terminal sink: reaches hardware itself
    p.connect(tone, "out", dac, "in")    # tone -> dac -> speakers
    server.add_graphdef(p.to_graphdef("chain"))
    server.graph("chain")                # sounds

The buses are never drawn or named by you, so **the hardware output is not one
either**: a signal reaches the speakers through a **terminal def** — a ``dac``
with an inlet and no outlet, its ``Out.ar(0, …)`` baked in — a box like any other,
not a special ``OUT`` node.

The **rate** of a port is its cord type: an audio port is a plain name, a control
port the pair ``(name, "control")``. Audio and control cords never connect, and
`compile` refuses a reversed or rate-mismatched cord (naming the offender).
"""

from .. import _native
from .graphdef import GraphDef


def _port(spec, direction: str) -> dict:
    """Normalize a port spec — a bare name (audio) or ``(name, "control")`` — into
    the flat ``{name, dir, rate}`` the cord->bus pass consumes."""
    name, rate = spec if isinstance(spec, tuple) else (spec, "audio")
    if rate not in ("audio", "control"):
        raise ValueError(f"port rate must be 'audio' or 'control', got {rate!r}")
    return {"name": str(name), "dir": direction, "rate": rate}


class GraphPatch:
    """A directed level-1 patch — whole defs wired by buses — that compiles to a
    `GraphDef`. Its boxes and the cords between their ports."""

    def __init__(self):
        #: Each box a flat ``{def, ports: [{name, dir, rate}, ...]}`` — the schema
        #: the cord->bus pass reads.
        self.boxes: list[dict] = []
        #: Each cord a ``{from_box, from_port, to_box, to_port}`` (ports are flat
        #: indices into the box's ``ports``).
        self.cords: list[dict] = []

    # ---- building ----

    def add(self, defname: str, inlets=(), outlets=()) -> int:
        """Add a box for def ``defname`` with its typed ``inlets``/``outlets``
        (each a name, or ``(name, "control")``). A **terminal** def (a sink that
        reaches hardware itself) is simply one with inlets and no outlets. Returns
        the box index."""
        ports = [_port(p, "in") for p in inlets] + [_port(p, "out") for p in outlets]
        self.boxes.append({"def": str(defname), "ports": ports})
        return len(self.boxes) - 1

    def connect(self, src: int, outlet, dst: int, inlet) -> "GraphPatch":
        """Draw a directed cord: box ``src``'s ``outlet`` -> box ``dst``'s
        ``inlet`` (each port by name or flat index). A no-op if it already
        exists, so applying an edit twice is safe."""
        cord = {
            "from_box": int(src),
            "from_port": self._port_index(src, outlet, "out"),
            "to_box": int(dst),
            "to_port": self._port_index(dst, inlet, "in"),
        }
        if cord not in self.cords:
            self.cords.append(cord)
        return self

    def disconnect(self, src: int, outlet, dst: int, inlet) -> "GraphPatch":
        """Remove the cord ``src.outlet -> dst.inlet`` if present."""
        cord = {
            "from_box": int(src),
            "from_port": self._port_index(src, outlet, "out"),
            "to_box": int(dst),
            "to_port": self._port_index(dst, inlet, "in"),
        }
        self.cords = [c for c in self.cords if c != cord]
        return self

    # ---- compiling ----

    def to_json(self) -> dict:
        """The patch as the cord->bus pass reads it (``{boxes, cords}``)."""
        return {"boxes": self.boxes, "cords": self.cords}

    def compile(self) -> dict:
        """Run the shared cord->bus pass. Returns ``{buses, members}`` — one
        private bus per connected net (writers summing), each member its def and
        its wired controls. Raises `ValueError` on a bad cord (reversed,
        rate-mismatched, out of range)."""
        return _native.compile_patch(self.to_json())

    def to_graphdef(self, name: str) -> GraphDef:
        """Compile to a ready-to-send `GraphDef`: the private buses declared and
        each member wired to them."""
        compiled = self.compile()
        gdef = GraphDef(name)
        refs = {b["name"]: gdef.bus(b["name"], rate=b["rate"]) for b in compiled["buses"]}
        for member in compiled["members"]:
            controls = {w["control"]: refs[w["bus"]] for w in member["controls"]}
            gdef.add(member["def"], controls)
        return gdef

    # ---- the GUI view (the `graph` widget's split schema) ----

    def to_widget(self, geometry: dict | None = None) -> dict:
        """The patch as the `graph` widget draws it: boxes with **split**
        inlets/outlets and cords as ``[from_box, outlet, to_box, inlet]``
        quadruples (the indices are within each box's inlet/outlet lists). Pass
        ``geometry`` (``{box_index: (x, y)}``) to place boxes; the rest auto-stack.
        The GUI edits the same model — a ``"wire"`` event names its ports, which
        `connect` resolves, so the round trip needs no index bookkeeping."""
        geometry = geometry or {}
        boxes = []
        for i, box in enumerate(self.boxes):
            drawn = {
                "def": box["def"],
                "inlets": [_widget_port(p) for p in box["ports"] if p["dir"] == "in"],
                "outlets": [_widget_port(p) for p in box["ports"] if p["dir"] == "out"],
            }
            if i in geometry:
                drawn["x"], drawn["y"] = geometry[i]
            boxes.append(drawn)
        cords: list[int] = []
        for c in self.cords:
            cords += [
                c["from_box"],
                self._split_index(c["from_box"], c["from_port"], "out"),
                c["to_box"],
                self._split_index(c["to_box"], c["to_port"], "in"),
            ]
        return {"boxes": boxes, "cords": cords}

    # ---- helpers ----

    def _port_index(self, box: int, name, direction: str) -> int:
        if isinstance(name, int):
            return name
        for i, p in enumerate(self.boxes[int(box)]["ports"]):
            if p["name"] == name and p["dir"] == direction:
                return i
        raise ValueError(f"box {box} has no {direction}let named {name!r}")

    def _split_index(self, box: int, flat: int, direction: str) -> int:
        ports = self.boxes[int(box)]["ports"]
        same = [i for i, p in enumerate(ports) if p["dir"] == direction]
        return same.index(flat)


def _widget_port(p: dict):
    """A port for the widget schema: a bare name for audio, ``{name, rate}`` for
    control (so the widget draws the cord's weight)."""
    return p["name"] if p["rate"] == "audio" else {"name": p["name"], "rate": p["rate"]}
