"""Building GuiDefs the way defs are built.

A GuiDef is the GUI analogue of a ``SynthDef``/``GraphDef``: a tree of
``{id, type, ...props, children}`` nodes serialized to JSON and carried inside
one OSC argument. These helpers compose that tree as plain ``dict``s — they are
**host-agnostic**, just like building a ``SynthDef`` is server-agnostic; only
`clausters.gui.host.GuiHost` knows how to send one. The root node carries no
``id`` (it comes from the ``/gui_def <id>`` argument); every child carries its
own client-allocated integer id.

The int/float distinction is the user's to make and is preserved end to end:
write ``480`` for an integer property and ``480.0`` for a float — ``json.dumps``
keeps them apart in the JSON text and the host's serde parse keeps them apart on
the wire (ids stay integers, control values stay floats).
"""

import json

__all__ = [
    "node",
    "window",
    "panel",
    "label",
    "knob",
    "slider",
    "waveform",
    "to_json",
]


def node(type: str, *, id: int | None = None, children=None, **props) -> dict:
    """A generic widget node ``{id?, type, ...props, children?}``.

    The building block every other helper wraps. Pass ``id`` for any non-root
    widget, ``children`` as an iterable of nodes for a container, and any other
    keyword as a property (kept verbatim, so its int/float type is preserved).
    """
    out: dict = {"type": type}
    if id is not None:
        out["id"] = id
    out.update(props)
    if children:
        out["children"] = list(children)
    return out


def window(*children, title: str | None = None, w: int | None = None, h: int | None = None,
           layout: str | None = None, **props) -> dict:
    """A top-level ``window`` container (a GuiDef root). It takes no id."""
    extra = _drop_none(title=title, w=w, h=h, layout=layout)
    return node("window", children=children, **extra, **props)


def panel(id: int, *children, layout: str | None = None, **props) -> dict:
    """A nestable ``panel`` container; ``layout`` is ``row``/``col``/``grid``/``free``."""
    extra = _drop_none(layout=layout)
    return node("panel", id=id, children=children, **extra, **props)


def label(id: int, text: str, **props) -> dict:
    """Static ``label`` text."""
    return node("label", id=id, text=text, **props)


def knob(id: int, *, label: str | None = None, min: float | None = None,
         max: float | None = None, value: float | None = None, **props) -> dict:
    """A rotary ``knob`` over a continuous range."""
    extra = _drop_none(label=label, min=min, max=max, value=value)
    return node("knob", id=id, **extra, **props)


def slider(id: int, *, label: str | None = None, min: float | None = None,
           max: float | None = None, value: float | None = None, **props) -> dict:
    """A continuous ``slider`` over a range."""
    extra = _drop_none(label=label, min=min, max=max, value=value)
    return node("slider", id=id, **extra, **props)


def waveform(id: int, *, buffer: int | None = None, **props) -> dict:
    """The heavy ``waveform`` view, fed a server ``buffer`` number (or, later, a
    blob). The renderer arrives in a later milestone; the node is valid now."""
    extra = _drop_none(buffer=buffer)
    return node("waveform", id=id, **extra, **props)


def to_json(tree: dict) -> str:
    """Serializes a GuiDef tree to the JSON string carried in ``/gui_def``."""
    return json.dumps(tree)


def _drop_none(**kwargs) -> dict:
    """Keeps only the keyword arguments that were actually given."""
    return {k: v for k, v in kwargs.items() if v is not None}
