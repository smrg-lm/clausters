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
    "meter",
    "scope",
    "nodetree",
    "plot",
    "to_json",
    "samples_to_blob",
    "samples_to_file",
    "peaks_cache_file",
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
             path: str | None = None, cache: str | None = None, channels: int | None = None,
             base_bucket: int | None = None, **props) -> dict:
    """The heavy ``waveform`` view, fed its samples one of several ways (in the
    host's precedence order):

    - ``cache`` — a path to a prebuilt peak-pyramid file (see `peaks_cache_file`)
      the host memory-maps and renders directly; the raw samples are never
      loaded. The most compact **bulk path**: nothing rides OSC.
    - ``path`` — a path to a file of raw little-endian ``f32`` samples (see
      `samples_to_file`, or the server's ``/b_export``) the host memory-maps; a
      **multi-megabyte buffer renders with no OSC and no re-send**. ``channels``
      de-interleaves channel 0 (default 1).
    - ``buffer`` — a server buffer number; the host fetches its samples from the
      audio server over OSC (it must be started with ``--server``). The async
      fallback when a shared file is not available.
    - ``data`` — a small list of floats embedded inline in the JSON;
    - ``blob`` — the index of a binary blob carried beside the JSON in the same
      ``/gui_def`` message (see `samples_to_blob` and `GuiHost.define`).

    ``base_bucket`` sets the peak-pyramid bucket size (default 256); for ``path``
    it also keys the sibling cache the host writes beside the file.
    """
    extra = _drop_none(data=list(data) if data is not None else None,
                       blob=blob, buffer=buffer, path=path, cache=cache,
                       channels=channels, base_bucket=base_bucket)
    return node("waveform", id=id, **extra, **props)


def meter(id: int, bus: int, *, min: float | None = None, max: float | None = None,
          label: str | None = None, **props) -> dict:
    """A level ``meter`` reading control ``bus`` straight from the audio server's
    shared-memory segment each frame (zero OSC messages). The host must be started
    with ``--shm`` pointing at the server's segment. ``min``/``max`` scale the bar
    (default ``0``/``1``)."""
    extra = _drop_none(min=min, max=max, label=label)
    return node("meter", id=id, bus=bus, **extra, **props)


def scope(id: int, bus: int, *, min: float | None = None, max: float | None = None,
          label: str | None = None, **props) -> dict:
    """A time-domain ``scope`` plotting the recent history of control ``bus`` (read
    from shared memory each frame; needs ``--shm`` like `meter`). ``min``/``max``
    set the vertical range (default the bipolar ``-1``/``1``)."""
    extra = _drop_none(min=min, max=max, label=label)
    return node("scope", id=id, bus=bus, **extra, **props)


def nodetree(id: int, *, group: int = 0, controls: bool | None = None,
             label: str | None = None, **props) -> dict:
    """A live ``nodetree`` view of the audio server's node tree rooted at ``group``
    (default the root group ``0``). The host mirrors the server's tree over its
    client leg (it must be started with ``--server``), refreshing on node
    creation/removal and a low-rate poll, so group/synth changes and ``/n_set``
    edits show live. ``controls`` (default true) shows each synth's control
    name/value pairs. A read-only view."""
    extra = _drop_none(label=label)
    if controls is not None:
        extra["controls"] = 1 if controls else 0
    return node("nodetree", id=id, group=group, **extra, **props)


def plot(id: int, *, data=None, blob: int | None = None, path: str | None = None,
         channels: int | None = None, min: float | None = None, max: float | None = None,
         label: str | None = None, **props) -> dict:
    """A simple static ``plot`` of a signal over ``[min, max]`` (default the
    bipolar ``-1``/``1``) — a line when the data fits the width, a min/max envelope
    when it does not. Unlike the heavy `waveform`, it does not zoom or pan; it is
    the catalog's "plot of an NRT-generated signal/file". Its samples come from:

    - ``path`` — a file of raw little-endian ``f32`` (see `samples_to_file`, or an
      NRT render written out) the host memory-maps; the **bulk path**, no OSC.
      ``channels`` de-interleaves channel 0 (default 1).
    - ``data`` — a small list of floats inline in the JSON;
    - ``blob`` — the index of a binary blob carried beside the JSON (see
      `samples_to_blob` and `GuiHost.define`).
    """
    extra = _drop_none(data=list(data) if data is not None else None,
                       blob=blob, path=path, channels=channels, min=min, max=max,
                       label=label)
    return node("plot", id=id, **extra, **props)


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


def samples_to_file(samples, path: str) -> str:
    """Writes `samples` to `path` as raw little-endian ``f32`` — the **local
    shared resource** a ``waveform(path=...)`` maps. Unlike `samples_to_blob`
    (which rides the ``/gui_def`` message and so must fit a datagram), a file has
    no size limit: this is how a multi-megabyte buffer reaches the host without
    OSC. Returns `path`."""
    buf = array.array("f", samples)
    if sys.byteorder != "little":
        buf.byteswap()
    with open(path, "wb") as f:
        f.write(buf.tobytes())
    return path


def peaks_cache_file(samples, path: str, base_bucket: int = 256) -> str:
    """Builds the peak-pyramid cache for `samples` (via the shared native core,
    so it is byte-identical to the host's own) and writes it to `path` — the most
    compact bulk path, mapped by a ``waveform(cache=...)``. The host renders the
    overview without ever loading the raw samples. Returns `path`."""
    from .._native import peaks_cache  # lazy: only needs the cdylib if used

    with open(path, "wb") as f:
        f.write(peaks_cache(samples, base_bucket))
    return path


def _drop_none(**kwargs) -> dict:
    """Keeps only the keyword arguments that were actually given."""
    return {k: v for k, v in kwargs.items() if v is not None}
