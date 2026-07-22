"""The directed patcher model: boxes with typed inlets/outlets and cords.

This is the **programmatic** patcher — the `patch` GUI widget is only a visual
view of it (`to_widget` renders the model the widget draws). There are two
levels, one directed-cord grammar apart by what a box *is* and what the patch is
*of*:

- **`GraphPatch`** (level 1): a box is a **whole def** (a SynthDef/FaustDef the
  server has) — itself a graph — and the patch compiles to a **`GraphDef`**,
  whole nodes wired by server buses. A cord *is* a bus, but you never number one:
  `compile` runs the shared cord->bus pass (`clausters_core::patch`, via
  `clausters._native`) that names one bus per connected net (its writers
  summing).
- **`DefPatch`** (level 2): a box is a single **UGen** (or a Faust signal op) and
  the patch is the **internal graph of one `SynthDef`/`FaustDef`**. A cord is an
  **internal wire**, never an allocated server bus, so a third cord type joins
  audio/control: **init** (`ir`). Built as a **read-only view** —
  `DefPatch.from_synthdef` / `from_faustdef` decode a def's in-memory graph so it
  draws as its UGen boxes; `to_synthdef` reconstructs the SynthDef (the decode is
  faithful — the round trip reproduces the spec).

A box has typed **inlets** and **outlets**; a **cord** runs an outlet to an
inlet, and cords of different rates never connect.

    from clausters.defs import GraphPatch

    p = GraphPatch()
    tone = p.add(tone_def)               # ports derived from the SynthDef's graph
    dac = p.add(dac_def)                 # a terminal sink: an inlet, no outlet
    p.connect(tone, "out", dac, "in")    # tone -> dac -> speakers
    server.add_graphdef(p.to_graphdef("chain"))
    server.graph("chain")                # sounds

Pass a `clausters.defs.SynthDef` to `add` and its typed ports are read off the
def itself — a control feeding an ``In`` is an inlet, one feeding an ``Out`` an
outlet (the same structural fact the server uses to order a graph). Or pass a def
**name** and list the ports yourself.

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
from .synthdef import SynthDef
from .ugens import Control, Ugen, ugen_input_names

#: A UGen that **reads** a bus (an inlet when its bus is a control), and the port
#: rate it implies.
_READERS = {"In": "audio", "InCtl": "control"}
#: A UGen that **writes** a bus (an outlet when its bus is a control), and its
#: rate. ``ReplaceOut`` overwrites rather than sums, but it is still an outlet.
_WRITERS = {"Out": "audio", "ReplaceOut": "audio", "OutCtl": "control"}


def synthdef_ports(sdef: SynthDef) -> tuple[list, list]:
    """Derive a `SynthDef`'s patcher ports ``(inlets, outlets)`` from its graph,
    the way the directed patcher wants them — **structural, not a guess**: a
    control that feeds an ``In``/``InCtl`` is an inlet, one that feeds an
    ``Out``/``OutCtl``/``ReplaceOut`` is an outlet, and the reading/writing UGen's
    family fixes the rate (audio for ``In``/``Out``, control for the ``Ctl``
    pair). A control that feeds neither is a plain value, not a port.

    Each port is returned in the form `GraphPatch.add` consumes: a bare name for
    an audio port, ``(name, "control")`` for a control one. Names are de-duplicated
    keeping first-seen order (a stereo ``Out`` writing one bus control is one
    outlet)."""
    inlets: dict[str, str] = {}
    outlets: dict[str, str] = {}
    for ugen in _walk(sdef.outputs):
        bus = ugen.inputs[0] if ugen.inputs else None
        if not isinstance(bus, Control):
            continue
        if ugen.kind in _READERS:
            inlets.setdefault(bus.name, _READERS[ugen.kind])
        elif ugen.kind in _WRITERS:
            outlets.setdefault(bus.name, _WRITERS[ugen.kind])
    spec = lambda name, rate: name if rate == "audio" else (name, rate)  # noqa: E731
    return ([spec(n, r) for n, r in inlets.items()],
            [spec(n, r) for n, r in outlets.items()])


def _walk(roots):
    """Every `Ugen` reachable from ``roots``, each once (a DFS over ``inputs``).
    Controls and constants are inputs, not walked as nodes."""
    seen: set[int] = set()
    stack = list(roots)
    while stack:
        node = stack.pop()
        if isinstance(node, Ugen) and id(node) not in seen:
            seen.add(id(node))
            yield node
            stack.extend(node.inputs)


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

    def add(self, defname, inlets=(), outlets=()) -> int:
        """Add a box for a def and return its index. ``defname`` is either a
        `clausters.defs.SynthDef` — whose typed ports are then **derived from its
        graph** (a control feeding an ``In`` is an inlet, one feeding an ``Out`` an
        outlet; see `synthdef_ports`) — or a def **name** (a string), for which you
        list the ``inlets``/``outlets`` yourself (each a name, or ``(name,
        "control")``). Passing explicit ports with a `SynthDef` overrides the
        derived ones. A **terminal** def (a sink that reaches hardware itself) is
        simply one with inlets and no outlets."""
        if isinstance(defname, SynthDef):
            name = defname.name
            if not inlets and not outlets:
                inlets, outlets = synthdef_ports(defname)
        else:
            name = str(defname)
        ports = [_port(p, "in") for p in inlets] + [_port(p, "out") for p in outlets]
        self.boxes.append({"def": name, "ports": ports})
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

    # ---- decoding a stored graph back into a patch (the inverse of to_graphdef) ----

    @classmethod
    def from_graphdef(cls, gdef: GraphDef, defs: dict | None = None) -> "GraphPatch":
        """Decode a `GraphDef` into a directed patch — the inverse of
        `to_graphdef`. Each member becomes a box; a member control valued an
        internal-bus **name** (a string other than the hardware sentinel ``"OUT"``)
        becomes a cord from the writing outlet to every reading inlet on that bus.

        Direction and rate are **not guessed**: a box's typed ports come from its
        def, so ``defs`` maps a member's def name to the `SynthDef` it was built
        from (a control feeding an ``In`` is an inlet, one feeding an ``Out`` an
        outlet; see `synthdef_ports`). A member whose def is not resolvable through
        ``defs`` draws **port-less** — its wiring cannot be typed, so it grows no
        cords. The box order is the member order, so a caller maps a box index
        straight back to the member it came from."""
        defs = defs or {}
        patch = cls()
        members = gdef.members()
        for member in members:
            sdef = defs.get(member["def"])
            patch.add(sdef if isinstance(sdef, SynthDef) else member["def"])
        # A cord is a bus: group each box's bus-valued controls into writers and
        # readers by port direction, then wire every writer to every reader sharing
        # a bus name (fan-in and fan-out fall out of the shared name).
        writers: dict = {}
        readers: dict = {}
        for box, member in enumerate(members):
            ports = patch.boxes[box]["ports"]
            out_names = {p["name"] for p in ports if p["dir"] == "out"}
            in_names = {p["name"] for p in ports if p["dir"] == "in"}
            for ctl, value in (member.get("controls") or {}).items():
                if not isinstance(value, str) or value == "OUT":
                    continue  # a number is a value; "OUT" reaches hardware, not a cord
                if ctl in out_names:
                    writers.setdefault(value, []).append((box, ctl))
                elif ctl in in_names:
                    readers.setdefault(value, []).append((box, ctl))
        for bus, sources in writers.items():
            for src_box, outlet in sources:
                for dst_box, inlet in readers.get(bus, []):
                    patch.connect(src_box, outlet, dst_box, inlet)
        return patch

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
        """The patch as the `patch` widget draws it: boxes with **split**
        inlets/outlets and cords as ``[from_box, outlet, to_box, inlet]``
        quadruples (the indices are within each box's inlet/outlet lists). Pass
        ``geometry`` (``{box_index: (x, y)}``) to place boxes; the rest auto-stack.
        The GUI edits the same model — a ``"wire"`` event names its ports, which
        `connect` resolves, so the round trip needs no index bookkeeping."""
        return _patch_to_widget(self.boxes, self.cords, geometry)

    # ---- helpers ----

    def _port_index(self, box: int, name, direction: str) -> int:
        if isinstance(name, int):
            return name
        for i, p in enumerate(self.boxes[int(box)]["ports"]):
            if p["name"] == name and p["dir"] == direction:
                return i
        raise ValueError(f"box {box} has no {direction}let named {name!r}")


def _widget_port(p: dict):
    """A port for the widget schema: a bare name for audio, ``{name, rate}`` for
    control or init (so the widget draws the cord's weight, dashing init)."""
    return p["name"] if p["rate"] == "audio" else {"name": p["name"], "rate": p["rate"]}


def _split_index(boxes: list, box: int, flat: int, direction: str) -> int:
    """The within-side index of the flat port ``flat`` on ``box``: the widget
    draws inlets and outlets as separate lists, so a cord endpoint (a flat index
    into the box's combined ``ports``) is remapped to its position among the
    ports on its own side."""
    ports = boxes[int(box)]["ports"]
    same = [i for i, p in enumerate(ports) if p["dir"] == direction]
    return same.index(flat)


def _patch_to_widget(boxes: list, cords: list, geometry: dict | None = None) -> dict:
    """Render the shared ``{boxes, cords}`` model into the `patch` widget schema
    — boxes with split inlet/outlet lists and cords as flat ``[from_box, outlet,
    to_box, inlet]`` quadruples. Both patch levels draw through this."""
    geometry = geometry or {}
    drawn = []
    for i, box in enumerate(boxes):
        d = {
            "def": box["def"],
            "inlets": [_widget_port(p) for p in box["ports"] if p["dir"] == "in"],
            "outlets": [_widget_port(p) for p in box["ports"] if p["dir"] == "out"],
        }
        # The layout role (the host's inverted tree pins sources, tucks consts);
        # absent / "object" is the default, so level-1 boxes need not carry it.
        role = box.get("role")
        if role and role != "object":
            d["role"] = role
        if i in geometry:
            d["x"], d["y"] = geometry[i]
        drawn.append(d)
    flat: list[int] = []
    for c in cords:
        flat += [
            c["from_box"], _split_index(boxes, c["from_box"], c["from_port"], "out"),
            c["to_box"], _split_index(boxes, c["to_box"], c["to_port"], "in"),
        ]
    return {"boxes": drawn, "cords": flat}


# ===================================================================
# Level 2: the Def-view — a SynthDef/FaustDef as its internal graph.
# ===================================================================

#: A UGen calculation rate -> the cord type the widget draws. ``ir`` (init /
#: scalar) is the level-2 third weight (dashed); ``dr`` (demand) has no bus
#: weight of its own, so it reads as control. An **unset** UGen rate defaults to
#: audio: most UGens are audio-rate, and the exact per-kind default is the
#: server's, not the client's — an honest headless heuristic for a view.
_UGEN_RATE = {"ar": "audio", "kr": "control", "ir": "init", "dr": "control"}
#: A control **type** -> the cord type. A scalar (``ir``) control is an init cord.
_CONTROL_RATE = {"kr": "control", "control": "control", "tr": "control",
                 "trigger": "control", "ir": "init", "scalar": "init"}
#: Faust signal ops that are controls (a UI label), drawn as source boxes.
_FAUST_CONTROL_OPS = frozenset(
    {"hslider", "vslider", "nentry", "button", "checkbox"}
)


def _rate_of(node) -> str:
    """The cord type of ``node``'s output — ``"audio"``/``"control"``/``"init"``
    — for drawing and typing a cord. A `Ugen` maps its calc rate (unset ->
    audio); a `Control` maps its type (unset -> control); a bare number is a
    constant (init)."""
    if isinstance(node, Ugen):
        return _UGEN_RATE.get(node.rate, "audio")
    if isinstance(node, Control):
        return _CONTROL_RATE.get(node.rate, "control")
    return "init"


def _ugen_label(u: Ugen) -> str:
    """The box caption for a UGen: the operator name for the generic op UGens
    (so ``a * b`` reads ``mul``, not ``BinaryOpUGen``), the kind otherwise."""
    if u.op and u.kind in ("BinaryOpUGen", "UnaryOpUGen"):
        return u.op
    return u.kind


def _fmt_const(value) -> str:
    """A value box's caption: a compact number (an integer-valued float drops its
    trailing ``.0``, others keep a few significant digits)."""
    try:
        f = float(value)
    except (TypeError, ValueError):
        return str(value)
    return str(int(f)) if f == int(f) else f"{f:g}"


def _outlet_flat(box: dict) -> int:
    """The flat index of a box's single outlet in its ``ports`` — the inlets come
    first, so it is the inlet count."""
    return sum(1 for p in box["ports"] if p["dir"] == "in")


def _topo_ugens(outputs) -> list:
    """Every `Ugen` reachable from ``outputs`` in the def's topological order
    (a UGen after its inputs), each once — the same post-order `SynthDef.spec`
    walks, so in the decode a box's input boxes always precede it."""
    ordered: list = []
    seen: set[int] = set()

    def visit(node):
        if not isinstance(node, Ugen) or id(node) in seen:
            return
        seen.add(id(node))
        for inp in node.inputs:
            visit(inp)
        ordered.append(node)

    for o in outputs:
        visit(o)
    return ordered


class DefPatch:
    """A level-2 patch — the internal graph of a single `SynthDef`/`FaustDef`,
    its UGen (or Faust op) boxes wired by internal cords. Built as a **read-only
    view**: `from_synthdef` / `from_faustdef` decode a def's in-memory graph so
    it draws as its boxes; `to_widget` renders it for the `patch` widget exactly
    as level 1, plus the init (`ir`) cord type; `to_synthdef` reconstructs the
    SynthDef (the decode is faithful — the round trip reproduces the spec).

    A cord here is an **internal wire**, never an allocated server bus — that is
    the whole difference from `GraphPatch`."""

    def __init__(self):
        #: Each box a dict with a ``kind`` and a layout ``role``. A **ugen** box:
        #: ``{def, kind:"ugen", role:"object", ugen:{kind, rate, op, label,
        #: static}, ports:[...]}``. A **control** box: ``{def, kind:"control",
        #: role:"source", control:{...}, ports:[outlet]}``. A **const** value box:
        #: ``{def, kind:"const", role:"const", const: value, ports:[outlet]}``. A
        #: **faust** box mirrors ugen without the rebuild fields.
        self.boxes: list[dict] = []
        #: Each cord ``{from_box, from_port, to_box, to_port}`` — flat port
        #: indices into each box's ``ports`` (an outlet -> an inlet).
        self.cords: list[dict] = []
        #: Box indices of the def's output roots (its ``Out``/side-effect UGens
        #: or the Faust output signals), in order — what `to_synthdef` rebuilds.
        self.roots: list[int] = []

    # ---- decoding a SynthDef's UGen graph ----

    @classmethod
    def from_synthdef(cls, sdef: SynthDef) -> "DefPatch":
        """Decode a `SynthDef`'s in-memory UGen graph into a level-2 patch: every
        UGen a box, every referenced control a **source** box, every constant a
        **value** box, and every input a cord. Each box carries a layout role, so
        the host draws it as an inverted tree — controls pinned to the top row,
        value boxes tucked above the box they feed, sinks at the bottom."""
        patch = cls()
        ordered = _topo_ugens(sdef.outputs)
        # Controls first (one box per unique name — the pinned source row), then
        # the UGens in the def's own order (each after the inputs that feed it).
        controls: dict[str, int] = {}
        for u in ordered:
            for inp in u.inputs:
                if isinstance(inp, Control) and inp.name not in controls:
                    controls[inp.name] = len(patch.boxes)
                    patch._add_control(inp)
        ugen_box: dict[int, int] = {}
        for u in ordered:
            ugen_box[id(u)] = len(patch.boxes)
            patch._add_ugen(u)
        for u in ordered:
            bi = ugen_box[id(u)]
            for pos, inp in enumerate(u.inputs):
                if isinstance(inp, Ugen):
                    src = ugen_box[id(inp)]
                elif isinstance(inp, Control):
                    src = controls[inp.name]
                else:
                    src = patch._add_const(inp)   # a literal -> its own value box
                patch._connect(src, _outlet_flat(patch.boxes[src]), bi, pos)
        patch.roots = [ugen_box[id(o)] for o in sdef.outputs]
        return patch

    def _add_ugen(self, u: Ugen):
        names = ugen_input_names(u.kind) or []
        inlets = [
            {"name": names[pos] if pos < len(names) else str(pos),
             "dir": "in", "rate": _rate_of(inp)}
            for pos, inp in enumerate(u.inputs)
        ]
        outlet = {"name": "", "dir": "out", "rate": _rate_of(u)}
        self.boxes.append({
            "def": _ugen_label(u),
            "kind": "ugen",
            "role": "object",
            "ugen": {"kind": u.kind, "rate": u.rate, "op": u.op,
                     "label": u.label, "static": u.static},
            "ports": inlets + [outlet],
        })

    def _add_control(self, c: Control):
        self.boxes.append({
            "def": c.name,
            "kind": "control",
            "role": "source",
            "control": {"name": c.name, "default": c.default, "rate": c.rate,
                        "lag": c.lag, "lag_down": c.lag_down},
            "ports": [{"name": "", "dir": "out", "rate": _rate_of(c)}],
        })

    def _add_const(self, value) -> int:
        """Add a **value** box for a literal input and return its index — a source
        with a single init-rate outlet, captioned with the number."""
        self.boxes.append({
            "def": _fmt_const(value),
            "kind": "const",
            "role": "const",
            "const": value,
            "ports": [{"name": "", "dir": "out", "rate": "init"}],
        })
        return len(self.boxes) - 1

    def _connect(self, fb: int, fp: int, tb: int, tp: int):
        self.cords.append({"from_box": fb, "from_port": fp,
                           "to_box": tb, "to_port": tp})

    # ---- decoding a FaustDef ----

    @classmethod
    def from_faustdef(cls, fdef) -> "DefPatch":
        """Decode a `FaustDef` into a level-2 patch. A **signal-tree** def
        (`FaustDef.from_signals`) decodes node for node — every signal op a box,
        every control (slider/button) a source box, every operand a cord. A
        **box-tree** or **source** def is opaque (its internals are the Faust
        compiler's, not reconstructable client-side), so it draws as a single
        box. Faust cords carry no server-bus rate, so they read as audio; a
        control's is control."""
        patch = cls()
        if fdef.kind == "signals":
            memo: dict = {}
            for node in (fdef._payload.get("signals") or []):
                root = patch._signal_box(node, memo)
                if root is not None:
                    patch.roots.append(root)
        else:
            patch.boxes.append({
                "def": fdef.name, "kind": "faust-opaque", "role": "object",
                "ports": [{"name": "", "dir": "out", "rate": "audio"}],
            })
            patch.roots.append(0)
        return patch

    def _signal_box(self, node, memo: dict):
        """Build the box for one Faust signal node (post-order, so operands
        precede it); returns its index, or ``None`` for a bare number (which the
        caller turns into a value box). Shared nodes dedup by identity."""
        if not isinstance(node, dict):
            return None
        key = id(node)
        if key in memo:
            return memo[key]
        op = node.get("op", "?")
        is_ctl = op in _FAUST_CONTROL_OPS
        operands = [] if is_ctl else list(node.get("in", []))
        children = [self._signal_box(o, memo) for o in operands]
        bi = len(self.boxes)
        memo[key] = bi
        inlets = [{"name": str(i), "dir": "in", "rate": "audio"}
                  for i in range(len(operands))]
        rate = "control" if is_ctl else "audio"
        self.boxes.append({
            "def": node.get("label", op) if is_ctl else op,
            "kind": "faust",
            "role": "source" if is_ctl else "object",
            "ports": inlets + [{"name": "", "dir": "out", "rate": rate}],
        })
        for pos, (operand, child) in enumerate(zip(operands, children)):
            src = child if child is not None else self._add_const(operand)
            self._connect(src, _outlet_flat(self.boxes[src]), bi, pos)
        return bi

    # ---- the GUI view + the SynthDef round trip ----

    def to_widget(self, geometry: dict | None = None) -> dict:
        """The patch as the `patch` widget draws it — boxes with split
        inlets/outlets and flat cord quadruples (see `_patch_to_widget`), the
        same schema level 1 uses, with init cords dashed."""
        return _patch_to_widget(self.boxes, self.cords, geometry)

    def to_synthdef(self, name: str) -> SynthDef:
        """Reconstruct the `SynthDef` this patch represents — the inverse of
        `from_synthdef`. Each box is rebuilt from its cords (following them back to
        the sources, so a shared box rebuilds once and value boxes resolve to their
        numbers). Only a UGen-graph patch rebuilds; a Faust patch has no
        SynthDef."""
        incoming: dict[int, dict[int, int]] = {}
        for c in self.cords:
            incoming.setdefault(c["to_box"], {})[c["to_port"]] = c["from_box"]
        built: dict[int, object] = {}

        def build(bi: int):
            if bi in built:
                return built[bi]
            box = self.boxes[bi]
            kind = box.get("kind")
            if kind == "control":
                cc = box["control"]
                node = Control(cc["name"], cc["default"], rate=cc["rate"],
                               lag=cc["lag"], lag_down=cc["lag_down"])
            elif kind == "const":
                node = box["const"]
            elif kind == "ugen":
                wired = incoming.get(bi, {})
                inputs = [build(wired[pos]) for pos in range(_outlet_flat(box))]
                uu = box["ugen"]
                node = Ugen(uu["kind"], inputs, rate=uu["rate"], op=uu["op"],
                            label=uu["label"], static=uu["static"])
            else:
                raise ValueError(
                    "to_synthdef only rebuilds a UGen-graph patch (from_synthdef)"
                )
            built[bi] = node
            return node

        return SynthDef(name, *[build(bi) for bi in self.roots])
