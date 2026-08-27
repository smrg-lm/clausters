"""Authoring a **component bundle**: the directory a page mounts.

A bundle is the persisted form of an instrument — its defs, its GuiDef, its
presets, its samples — plus the manifest that says what mounting it needs. The
same directory runs on three legs: a browser tab (as a custom element), the
desktop (``clausters-gui --standalone``), and a loopback host against a running
server. Nothing of this module runs at mount time; it only *writes*.

The two kinds of hole
=====================

Mounting the same bundle twice on one page must not collide, so the GuiDef
record on disk is a **template** with placeholders, told apart by sigil:

- ``@name`` — a **symbol**: an id the page allocates (a node, a bus, a buffer).
- ``$name`` — a **parameter**: a value the tag supplies, or a preset's, or the
  declared default.

`Bundle` holds the symbol table, so the author names things instead of
numbering them: `bus`, `node` and `buffer` each return the placeholder string,
which reads naturally where an index goes::

    lfo = b.bus("lfo")            # -> "@lfo"
    meter(bus=lfo, label="lfo")

**Holes live only in the GuiDef record.** The def payloads carry none, which is
what lets two mounted instances share the one def that was sent — and it forces
one authoring rule, which is the right rule anyway:

    A bus, a node or a buffer reaches a def **as a control**, never as a baked
    constant.

So a voice that publishes its envelope writes ``out_ctl(control("env_bus"),
env)``, not ``out_ctl(0.0, env)``, and the mount passes the allocated bus in.
`write` checks this through the core and refuses to emit a bundle that breaks
it — an unmountable bundle is unwritable.

Writing one
===========

::

    from clausters.bundle import Bundle

    b = Bundle("fm-voice")
    b.param("freq", float, default=220.0, min=60.0, max=700.0)
    lfo = b.bus("lfo")
    graph = b.node("graph")

    b.synthdef(voice())                  # named "fm-voice.voice"
    b.gui(scene(lfo, graph))
    b.preset("bright", freq=660.0)
    b.write("./fm-voice")

and the page gets a tag from one import (`write` generates ``index.js``)::

    <script type="module" src="./fm-voice/index.js"></script>
    <fm-voice freq="440"></fm-voice>

The format itself is documented in ``docs/clients.md``.

The bytes are canonical
=======================

There are two writers of this format — this one and the web client's
``Bundle`` — and the same bundle authored in either language must be the *same
directory*, not merely an equivalent one. That is only checkable if the bytes
are, so both emit **canonical JSON**: keys sorted, no space between tokens
(two spaces of indent for the two files a person reads, ``bundle.json`` and a
preset), and numbers written the shortest way that reads back — which means an
integral float is ``220``, not ``220.0``.

The number rule is the one that costs something, and it is not a preference:
JavaScript has a single number type, so a writer there cannot tell ``220.0``
from ``220`` and could never emit Python's spelling. Dropping the trailing
zero is what both languages *can* agree on. Nothing downstream reads a type
out of the spelling — a declared ``"type": "float"`` says what a value is, and
every leg parses these files with a schema in hand.
"""

from __future__ import annotations

import json
import os
from typing import Any

from . import _native

#: Where a page serves the component run time from. Not the bundle's business
#: — an argument of `Bundle.write`, defaulting to the package's own layout.
DEFAULT_RUNTIME = "/dist/runtime.js"

_TYPE_NAMES = {float: "float", int: "int", str: "string", bool: "bool"}


def _minimal(value):
    """`value` with every integral float written as an integer.

    The half of the canonical form Python has to do work for (see the module
    docstring): JavaScript spells ``220.0`` as ``220`` and has no way not to,
    so this side meets it there.
    """
    if isinstance(value, bool):
        return value
    if isinstance(value, float) and value.is_integer() and abs(value) < 2 ** 53:
        return int(value)
    if isinstance(value, dict):
        return {k: _minimal(v) for k, v in value.items()}
    if isinstance(value, (list, tuple)):
        return [_minimal(v) for v in value]
    return value


def _json(value, *, indent: int | None = None) -> str:
    """`value` as canonical JSON — what both writers of this format emit."""
    separators = (",", ": ") if indent is not None else (",", ":")
    return json.dumps(_minimal(value), sort_keys=True, ensure_ascii=False,
                      indent=indent, separators=separators)


class Bundle:
    """A bundle being written: its symbols, its parameters, its defs and its
    GuiDef.

    ``name`` names the bundle and **prefixes its def names** (a def name is a
    global namespace on the server, so two bundles defining ``voice``
    differently must not collide). It is also the custom element's tag by
    default — HTML wants a hyphen in one, so a one-word name needs an explicit
    `write(tag=...)`.
    """

    def __init__(self, name: str, *, gui_name: str | None = None):
        self.name = str(name)
        #: The GuiDef's file stem under ``defs/guidefs/``.
        self.gui_name = gui_name or self.name
        self._params: dict[str, dict] = {}
        self._nodes: list[str] = []
        self._buses: list[dict] = []
        self._buffers: list[str] = []
        self._buffer_files: dict[str, str] = {}
        self._synthdefs: list[Any] = []
        self._graphdefs: list[Any] = []
        self._presets: dict[str, dict] = {}
        self._gui: dict | None = None
        self._boot: list[list] = []

    # ---- the contract ----

    def param(self, name: str, kind: type = float, *, default=None,
              min: float | None = None, max: float | None = None) -> str:
        """Declares a parameter and returns its placeholder (``"$name"``).

        ``kind`` is `float`, `int`, `str` or `bool`. A parameter with no
        ``default`` is **required**: the tag or a preset must supply it, and
        mounting without it is an error rather than a silent zero. ``min``/
        ``max`` bound the numeric kinds, checked at mount.
        """
        if kind not in _TYPE_NAMES:
            raise ValueError(f"parameter {name!r}: type must be float, int, str or bool")
        spec: dict = {"type": _TYPE_NAMES[kind]}
        if default is not None:
            spec["default"] = default
        if min is not None:
            spec["min"] = float(min)
        if max is not None:
            spec["max"] = float(max)
        self._params[str(name)] = spec
        return f"${name}"

    def node(self, name: str) -> str:
        """Declares a node symbol and returns its placeholder (``"@name"``) —
        the id the page allocates for a synth or graph this bundle boots."""
        self._declare(name)
        self._nodes.append(str(name))
        return f"@{name}"

    def bus(self, name: str, *, rate: str = "control", channels: int = 1) -> str:
        """Declares a bus symbol and returns its placeholder (``"@name"``).

        ``rate`` is ``"control"`` or ``"audio"``. The placeholder reads
        naturally where a bus index goes (``meter(bus=lfo)``); a def that uses
        the bus takes it **as a control**, never baked in.
        """
        if rate not in ("control", "audio"):
            raise ValueError(f"bus {name!r}: rate must be 'control' or 'audio'")
        self._declare(name)
        self._buses.append({"name": str(name), "rate": rate, "channels": int(channels)})
        return f"@{name}"

    def buffer(self, name: str, path: str) -> str:
        """Declares a sample and returns its placeholder (``"@name"``).

        ``path`` is relative to the bundle directory (the file is the author's
        to place there). The mount allocates the buffer index and loads the
        file into it.
        """
        self._declare(name)
        self._buffers.append(str(name))
        self._buffer_files[str(name)] = str(path)
        return f"@{name}"

    def _declare(self, name: str) -> None:
        """Refuses one name in two namespaces — ``@name`` would not say which."""
        taken = set(self._nodes) | {b["name"] for b in self._buses} | set(self._buffers)
        if str(name) in taken:
            raise ValueError(f"symbol {name!r} is already declared in this bundle")

    # ---- the contents ----

    def synthdef(self, sdef) -> str:
        """Adds a SynthDef (or a FaustDef), prefixing its name with the
        bundle's, and returns the prefixed name — what an ``/synth_new`` in the
        boot list spawns."""
        return self._add_def(sdef, self._synthdefs)

    def graphdef(self, gdef) -> str:
        """Adds a GraphDef, prefixing its name with the bundle's, and returns
        the prefixed name."""
        return self._add_def(gdef, self._graphdefs)

    def _add_def(self, d, into: list) -> str:
        prefixed = d.name if d.name.startswith(f"{self.name}.") else f"{self.name}.{d.name}"
        d.name = prefixed
        into.append(d)
        return prefixed

    def gui(self, tree: dict) -> None:
        """Sets the GuiDef tree — the template. Its widgets should be numbered
        ``1..N``; the mount offsets them by an allocated base, so the numbers
        are local to the bundle and never collide between instances."""
        self._gui = tree

    def boot(self, *messages: list) -> None:
        """Adds boot messages — ``[addr, *args]`` each, with placeholders where
        ids and values go::

            b.boot(["/graph_new", "fm-voice.graph", graph, 0, 0],
                   ["/node_set", graph, "freq", freq])

        They run once per instance, after its defs are in. A parameter that
        nothing draws reaches the synthesis this way; one a widget carries
        reaches it through that widget's ``bind``.
        """
        self._boot.extend(list(m) for m in messages)

    def preset(self, name: str, **values) -> None:
        """Declares a named preset — a bundle of parameter values a tag selects
        with ``preset="<name>"``. An attribute overrides it; it overrides the
        declared defaults."""
        unknown = set(values) - set(self._params)
        if unknown:
            raise ValueError(f"preset {name!r} sets undeclared parameter(s): {sorted(unknown)}")
        self._presets[str(name)] = dict(values)

    # ---- writing ----

    def manifest(self) -> dict:
        """The `bundle.json` this bundle would write."""
        out: dict = {"name": self.name, "gui": self.gui_name}
        if self._synthdefs:
            out["synthdefs"] = [d.name for d in self._synthdefs]
        if self._graphdefs:
            out["graphdefs"] = [d.name for d in self._graphdefs]
        if self._gui is not None:
            out["widgets"] = _widget_span(self._gui)
        symbols: dict = {}
        if self._nodes:
            symbols["nodes"] = list(self._nodes)
        if self._buses:
            symbols["buses"] = list(self._buses)
        if self._buffers:
            symbols["buffers"] = list(self._buffers)
        if symbols:
            out["symbols"] = symbols
        if self._params:
            out["params"] = dict(self._params)
        if self._presets:
            out["presets"] = sorted(self._presets)
        if self._buffer_files:
            out["buffers"] = dict(self._buffer_files)
        return out

    def record(self) -> dict:
        """The GuiDef record this bundle would write: ``{"id": 1, "gui":
        <tree>}``, the boot list carried at the tree's root."""
        if self._gui is None:
            raise ValueError(f"bundle {self.name!r} has no GuiDef (call .gui(...))")
        tree = dict(self._gui)
        if self._boot:
            tree["boot"] = self._boot
        return {"id": 1, "gui": tree}

    def validate(self) -> None:
        """Runs the core's pre-flight: the mount dry-run over the declared
        defaults, plus the no-holes check on every def payload. Raises
        `ValueError` with the reason — an unknown symbol, a parameter whose
        default does not type-check, a hole baked into a def.

        `write` calls this first, so a bundle that would fail to mount fails to
        be written.
        """
        defs = [json.loads(d.dump_def()) for d in (*self._synthdefs, *self._graphdefs)]
        _native.bundle_validate(self.manifest(), self.record(), defs)

    def files(self, *, runtime: str = DEFAULT_RUNTIME,
              tag: str | None = None) -> dict[str, str]:
        """The whole bundle as text, by path relative to its directory.

        Validates first, then builds every file `write` would write: the def
        payloads (each its own ``/def_send`` spec), the GuiDef record, the
        presets, the manifest, and the five-line ES module that registers the
        tag. Samples are not here — the audio files are the author's to place
        in the directory, and the manifest only names them.

        This is the writer without the disk, which is what a caller mounting
        a bundle it has just authored wants (a page, a test, a build step that
        serves it from memory). `write` is this plus the directory.
        """
        # The substance first: what the bundle *is* matters more than what it
        # will be called, and its error is the more useful one to see.
        self.validate()
        tag = tag or self.name
        if "-" not in tag or tag != tag.lower() or tag[0].isdigit():
            raise ValueError(
                f"{tag!r} is not a valid custom element name (lowercase, with a "
                f"hyphen, not starting with a digit) — pass write(tag=...)"
            )
        out = {"bundle.json": _json(self.manifest(), indent=2) + "\n",
               f"defs/guidefs/{self.gui_name}.json": _json(self.record()),
               "index.js": _module(tag, runtime)}
        for d in self._synthdefs:
            out[f"defs/synthdefs/{d.name}.json"] = _json(json.loads(d.dump_def()))
        for d in self._graphdefs:
            out[f"defs/graphdefs/{d.name}.json"] = _json(json.loads(d.dump_def()))
        for name, values in self._presets.items():
            out[f"presets/{name}.json"] = _json(values, indent=2) + "\n"
        return out

    def write(self, directory: str, *, runtime: str = DEFAULT_RUNTIME,
              tag: str | None = None) -> str:
        """Writes the bundle to ``directory`` and returns the path.

        `files` is what it writes, and carries what each file is; this adds
        the directories and the disk.

        ``tag`` is the custom element's name, defaulting to the bundle's.
        HTML requires a hyphen in it (that is how a custom element is told from
        a built-in one), so a one-word bundle name — perfectly good on the
        desktop, where the name is a GuiDef's — needs an explicit ``tag``.
        ``runtime`` is where the page serves the component run time from: the
        page's business, not the bundle's.
        """
        for path, text in self.files(runtime=runtime, tag=tag).items():
            full = os.path.join(directory, *path.split("/"))
            os.makedirs(os.path.dirname(full), exist_ok=True)
            with open(full, "w") as f:
                f.write(text)
        return directory

    def __repr__(self) -> str:
        return (f"Bundle({self.name!r}, {len(self._synthdefs)} synthdef(s), "
                f"{len(self._params)} param(s))")


def _module(tag: str, runtime: str) -> str:
    """The generated ES module: one import, one call, a named tag."""
    return (
        f"// {tag}/index.js -- generated by clausters.bundle; do not edit.\n"
        f'import {{ defineComponent }} from "{runtime}";\n'
        f'defineComponent("{tag}", new URL(".", import.meta.url));\n'
    )


def _widget_span(tree: dict) -> int:
    """The width of the id block one instance needs: the highest widget id the
    tree uses, the root's included (the root is id 1)."""
    high = 1
    stack = [tree]
    while stack:
        node = stack.pop()
        if not isinstance(node, dict):
            continue
        wid = node.get("id")
        if isinstance(wid, int):
            high = max(high, wid)
        stack.extend(node.get("children", []))
    return high
