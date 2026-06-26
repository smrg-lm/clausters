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

import array
import json
import sys

__all__ = [
    "node",
    "window",
    "panel",
    "label",
    "knob",
    "slider",
    "number",
    "button",
    "toggle",
    "text",
    "menu",
    "waveform",
    "to_json",
    "samples_to_blob",
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


def number(id: int, *, label: str | None = None, min: float | None = None,
           max: float | None = None, value: float | None = None, **props) -> dict:
    """A draggable numeric read-out over a range."""
    extra = _drop_none(label=label, min=min, max=max, value=value)
    return node("number", id=id, **extra, **props)


def button(id: int, *, label: str | None = None, **props) -> dict:
    """A momentary push ``button`` (emits ``1`` on press, ``0`` on release)."""
    extra = _drop_none(label=label)
    return node("button", id=id, **extra, **props)


def toggle(id: int, *, label: str | None = None, value: bool | None = None, **props) -> dict:
    """A boolean ``toggle``. ``value`` is sent as ``1``/``0`` (OSC has no bool)."""
    extra = _drop_none(label=label)
    if value is not None:
        extra["value"] = 1 if value else 0
    return node("toggle", id=id, **extra, **props)


def text(id: int, *, value: str | None = None, label: str | None = None, **props) -> dict:
    """A ``text`` field showing ``value`` (script-driven via ``/gui_set``)."""
    extra = _drop_none(value=value, label=label)
    return node("text", id=id, **extra, **props)


def menu(id: int, options, *, index: int | None = None, label: str | None = None, **props) -> dict:
    """A ``menu`` selector over ``options`` (a list of strings); a click cycles
    to the next and emits the chosen ``index``."""
    extra = _drop_none(index=index, label=label)
    return node("menu", id=id, options=list(options), **extra, **props)


def waveform(id: int, *, data=None, blob: int | None = None, buffer: int | None = None,
             base_bucket: int | None = None, **props) -> dict:
    """The heavy ``waveform`` view, fed its samples one of three ways:

    - ``data`` — a list of floats embedded inline in the JSON (simplest; keep it
      small enough to fit a datagram);
    - ``blob`` — the index of a binary blob carried beside the JSON in the same
      ``/gui_def`` message (the bulk path; see `samples_to_blob` and
      `GuiHost.define`);
    - ``buffer`` — a server buffer number (resolved once the host attaches to the
      audio server, a later milestone).

    ``base_bucket`` sets the peak-pyramid bucket size (default 256).
    """
    extra = _drop_none(data=list(data) if data is not None else None,
                       blob=blob, buffer=buffer, base_bucket=base_bucket)
    return node("waveform", id=id, **extra, **props)


def to_json(tree: dict) -> str:
    """Serializes a GuiDef tree to the JSON string carried in ``/gui_def``."""
    return json.dumps(tree)


def samples_to_blob(samples) -> bytes:
    """Packs an iterable of floats into a little-endian ``f32`` blob, the bulk
    form a ``waveform`` reads via ``blob``. Flat bytes at the boundary — the same
    rule the rest of the client follows."""
    buf = array.array("f", samples)
    if sys.byteorder != "little":
        buf.byteswap()
    return buf.tobytes()


def _drop_none(**kwargs) -> dict:
    """Keeps only the keyword arguments that were actually given."""
    return {k: v for k, v in kwargs.items() if v is not None}
