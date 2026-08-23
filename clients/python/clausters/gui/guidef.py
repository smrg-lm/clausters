"""Building GuiDefs the way defs are built.

A GuiDef is the GUI analogue of a ``SynthDef``/``GraphDef``: a tree of
``{id, type, ...props, children}`` nodes serialized to JSON and carried inside
one OSC argument. These helpers compose that tree as plain ``dict``s — they are
**host-agnostic**, just like building a ``SynthDef`` is server-agnostic; only
`clausters.gui.host.GuiHost` knows how to send one. The root node carries no
``id`` (it comes from the ``/gui_def <id>`` argument); every child carries an
integer id on the wire, but a script never has to write one.

**What a `type` names is the model, not a widget.** A GuiDef has three kinds of
node: a **container** owning 0, 1 or 2 axes (`layout`, `plane`, `field`, and
the `window` root), an **element** drawn against those axes (`signal`, `notes`,
`curve`, `score`, `keys`, `nodes`, `meter`, `canvas`, `label`), and a
**control**, which is an element with a value and no axis (`slider`, `knob`,
`number`, `button`, `toggle`, `text`, `menu`). The chrome of an axis — its
ruler, its navigation window, the selection, the playhead, the value range —
belongs to the **container's** ``axes``, not to each element drawn against it.

The builders named after widgets (`panel`, `stack`, `scroll`, `waveform`,
`plot`, `scope`, `track`, `clip`, `timeruler`, ...) are **shortcuts**: each
builds a model node with the props of one common case, and takes its axis
chrome as flat keywords (``ruler=``, ``link=``, ``min=``) which it packs into
the pair for you. `layout`, `plane`, `field` and `signal` are the general
builders beside them, for the cases no shortcut names — a lane that is also a
clip, a plane that is neither a scroll view nor a patcher, a presentation over
a source the shortcuts do not pair.

**Address a widget by name, not by id.** Pass ``name="cutoff"`` to any builder
and `GuiHost.open` hands back a window handle you index by that name —
``win["cutoff"].set(value=…)``, ``win["cutoff"].on_event(fn)``. The name is a
**client-only** key: it labels the widget for the ``name -> handle`` map and is
stripped from the JSON, so it never rides the wire. Unlike the assigned id
(which recycles across redraws), a name is stable, which is what an edit-back or
a live `set` addresses against.

**The id is a keyword argument on every builder, and never the first one.** The
positional slot belongs to what the widget is made of — a container's children
(``panel(knob(), slider())``), a label's text (``label("hello")``), a meter's
bus (``meter(4)``), a menu's options — so an ordinary tree mentions no ids at
all. `GuiHost.open` / `GuiHost.define` then assigns each widget a fresh
host-unique id **in the document it sends**, leaving the tree you wrote as you
wrote it. An id names a live widget, so it belongs to the instance `open` hands
back, not to the definition: the way to a widget is its ``name`` through that
handle (``win["cutoff"]``), and one tree opens as many times as you like.

Passing ``id=`` explicitly stays supported for the cases that need a fixed
number (small ints are fine — the host client allocates from 1000 up, so hand
ids below 1000 never collide with assigned ones). **Widget ids live in one
namespace per host**, across *all* windows, like the server's node ids: two
windows must not reuse an id, which is the bookkeeping the assigned ids and the
names exist to spare you.

The int/float distinction is the user's to make and is preserved end to end:
write ``480`` for an integer property and ``480.0`` for a float — ``json.dumps``
keeps them apart in the JSON text and the host's serde parse keeps them apart on
the wire (ids stay integers, control values stay floats).

**Every widget takes the generic place props**, applied by the container's
layout (all in **logical** pixels, all optional, all live via ``set``):

- ``w``/``h`` — a fixed main-axis size in a ``row``/``col`` (``w`` in a row,
  ``h`` in a col); in ``free``, the widget's size.
- ``weight`` — the share of the leftover a child takes in a ``row``/``col``,
  and the way to stretch a control past the size it asks for.
- ``x``/``y`` — the position inside a ``free`` container; a free child with
  none of these props overlays the whole container area.

A ``row``/``col`` resolves its main axis in **one order**: a fixed ``w``/``h``,
else an explicit ``weight``, else the widget's **natural size** — how big that
kind of widget wants to be, which the host knows — else a share of the
leftover at weight 1. The cross axis always fills. So a control (a ``button``,
a ``knob``, a single-line ``text``, a ``label``) stacked in a ``col`` is one
control-high row rather than half the window, while views (a ``waveform``, a
``pianoroll``) have no natural size and split the rest between them.

A **container** is one of those views — it takes what it is given — unless it
carries ``hug``, and then it wants exactly what it holds: a ``row`` adds its
children up along its axis and takes the tallest of them across it, a ``col``
the other way round, a ``grid`` counts its cells. That is how a strip of
controls under a work surface says how tall it is without anybody computing a
number: ``panel(menu(...), toggle(...), layout="row", hug=True)``. The question
reaches the whole subtree under it, so a plain panel nested inside a hugging one
is measured too; an axis a child leaves elastic (a plane, a lane, a heavy view)
is one the container hands back to the layout.

What a size may read is fixed by **where the value is resolved**: a prop that
settles when you build or ``set`` it (a label's text, a menu's options) may size
a container that hugs, and a *value* — a number being turned, a field being
typed into, a scope's samples — never sizes anything, so no stream of values
ever moves a layout. Outside a ``hug`` nothing reads the content at all.

Those numbers are **logical**, not the screen's: the host multiplies every
declared length (and ``text_size``, a glyph scale) by the display's own scale,
one number per window, resolved when the scale changes and never per frame. So a
``h=28`` strip looks like a 28-pixel strip everywhere, and a script never asks
what it is running on. The one exception is a ``scroll`` workspace's content
plane — its ``content_w``/``content_h``, its ``view_x``/``view_y`` and its
children's place props are content units, physical pixels on the plane, because
the plane carries a zoom of its own.

Containers (``window``/``panel``/``scroll``) additionally take ``margin`` (the
inset before their children, default 6), ``gap`` (between children, default 6)
and ``cols`` (a fixed ``grid`` column count; default near-square);
``window``/``panel``/``stack`` also take ``hug``. A
fixed-height menu bar over a weighted content area over a fixed status bar —
the application shell — is just ``window(bar(h=28), content(), status(h=20),
layout="col")``.

When the content does not fit its container, `scroll` is the container that
pans and zooms: its children live in a **virtual content area** seen through a
2D window, and the constrained forms (a vertical scroll view, a horizontal
strip) are that same widget configured down.

**Every widget also takes the style props**, both live via ``set``:

- ``color`` — one ``"#rrggbb[aa]"`` that re-seeds the roles carrying the
  widget's function: the accent family (a slider's handle and fill, a button
  face, a meter's bar), the trace, the first series color of a multichannel
  view, a clip's body. An empty string clears it.
- ``theme`` — on a container (`window`/`panel`/`scroll`/`track`), a partial
  color-role table (``{"role": "#rrggbb[aa]"}``, the same shape as the host's
  TOML style file) overlaying the parent's theme for the whole subtree — a
  **theme group**, recursive by construction. On a window root it persists
  with a named def. An empty table clears the group.
- ``opacity`` — how opaque the widget draws, ``0.0``–``1.0``. Like ``theme`` it
  is a **group's** property: it multiplies down the whole subtree, so a control
  inside a panel at ``0.5`` that is itself at ``0.5`` draws at ``0.25``. A
  negative number clears it. It fades the flat drawing — the chrome, the
  controls and the text; a heavy view's picture (a waveform's trace, a
  spectrogram's texture, a ``canvas`` shader) is drawn by its own pipeline and
  keeps its own.
- ``radius`` — the corner radius of the boxes this widget draws, in logical
  pixels. Unlike ``opacity`` it applies to the widget alone: a rounded panel
  says nothing about the controls in it. Each box clamps it to half its shorter
  side, so a widget's own frame rounds while the hairlines inside it (a
  divider, a tick, a track edge) keep their shape. A negative number clears it.

**A container also declares its gestures.** Panning, sweeping a selection and
locating the transport belong to the coordinate system a container gives its
contents, not to what is drawn in it — which is why Shift+drag pans the same
way over a ``waveform``, a ``track`` lane, a ``pianoroll`` and a ``timeruler``.
A ``gestures`` prop replaces that mapping, keyed by modifier (``drag``
for the plain drag, ``shift``, ``ctrl``, ``alt``), each value an ordered plan
of steps: ``element`` (hand the press to whatever is under the cursor — a clip,
a note, a box — which may decline), ``pan``, ``select`` (sweep the time span),
``select_box`` (the same sweep restricted to the band of values it covered — a
rectangle, which declines where the picture measures only time), ``locate``,
``none``::

    waveform(data=take, gestures={"drag": "pan", "shift": "select"})
    waveform(data=take, gestures={"drag": "select", "ctrl": "select_box select"})

A plan that consumes nothing falls outward to the container around it. The
defaults are per kind (``{"drag": "element locate", "shift": "pan"}`` on a
lane, ``{"drag": "select", "shift": "pan"}`` on the heavy views), a table names
only what it changes, and a press on a view's vertical strip always pans that
axis. Live via ``set`` (as JSON, the ``theme`` convention).
"""

import array
import atexit
import json
import sys
from ..base.bulk import samples_to_blob as _samples_to_blob
from ..defs.ugens import env_to_points, points_to_env  # re-exported; shared with seq.automation

__all__ = [
    "View",
    "Source",
    "source",
    "node",
    "view",
    "window",
    "layout",
    "plane",
    "field",
    "signal",
    "notes",
    "curve",
    "nodes",
    "keys",
    "panel",
    "scroll",
    "stack",
    "label",
    "knob",
    "slider",
    "number",
    "button",
    "toggle",
    "text",
    "menu",
    "waveform",
    "spectrogram",
    "piano",
    "pianoroll",
    "meter",
    "scope",
    "phasescope",
    "spectrum",
    "nodetree",
    "bpf",
    "env_to_points",
    "points_to_env",
    "plot",
    "score",
    "timeruler",
    "track",
    "clip",
    "patch",
    "canvas",
    "to_json",
    "samples_to_blob",
    "samples_to_file",
    "peaks_cache_file",
    "correlation",
    "lissajous",
]


# ------------------------------------------------------------------ the object
#
# `View`: a GUI node as an object — the AST a client builds and then opens.
#
# A `clausters.gui.guidef` builder used to return a bare ``dict``, which is why
# the *host* had to be the subject of the sentence (``host.open(tree)``) while
# every other resource is its own subject (``synthdef.send(server)``,
# ``clausters.plot(obj)``). A `View` closes that asymmetry: it is the GUI's
# counterpart of a `clausters.defs.SynthDef` — a tree a program composes and
# sends, not a live widget. The live widget is what `View.open` gives back.
#
# `View` **is a ``dict``**, so the document it serializes is byte-identical to the
# one the builders always produced and nothing on the wire, in the host or in
# `to_json` changes. What it adds is behaviour: name lookup over the tree it
# carries, `to_json`, and `open`.
#
# **A name is the client's index, not an id.** The host never reads a widget's
# ``name`` — `clausters.gui.guidef.to_json` strips it before the document goes out
# — so ``view.find("cutoff")`` and ``win["cutoff"]`` are tables this client builds
# by walking the tree. Two rules follow, and both are enforced here:
#
# - **A duplicate name in one view is an error**, raised while the tree is being
#   built rather than silently resolving to whichever widget came last. A shadowed
#   widget still draws, so a silent last-wins leaves something on screen that no
#   name reaches.
# - **A nested view scopes its names.** A ``window``-typed node inside another
#   tree is a sub-view: its names stay its own, so ``osc1`` and ``osc2`` can both
#   hold a ``freq`` and neither reaches out. The sub-view is found by *its* name,
#   and the lookup continues inside it (``v.find("osc1").find("freq")``).


#: The node types that open a scope: a view of this type nested in another tree
#: keeps its names to itself. Today the wire has one such type; `view()` is its
#: client-side spelling.
_SCOPES = ("window",)


class View(dict):
    """One node of a GuiDef tree: a ``dict`` that also knows how to be opened.

    Built by the `clausters.gui.guidef` builders, never directly — ``knob(...)``,
    ``layout(a, b)`` and ``window(...)`` all return one. Composition is nesting,
    exactly as before::

        v = window(layout(knob(name="freq"), slider(name="amp"), flow="col"))
        v.find("freq")["min"] = 110.0     # still a plain dict underneath
        w = v.open()                      # a live window
        w["freq"].set(value=440.0)

    ``v["type"]`` is the dict key, as it has always been; the *name* index is
    `find` / `names`, which keeps the two addressings from colliding on one
    bracket. On the live side the bracket is the name (``w["freq"]``), because a
    `clausters.gui.handle.WindowHandle` has no document to index.
    """

    __slots__ = ("_scope", "_sources")

    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        #: the `Source` objects feeding this node's props, so opening it can
        #: tell each which live widget draws it. Client-side only.
        self._sources: list = []
        #: ``name -> View`` for this view's own scope, built once at
        #: construction from the children's already-built scopes — so composing
        #: a tree costs one pass over each node, not one per lookup.
        self._scope = _scope_of(self)

    # ---- the name index ----

    @property
    def type(self) -> str:
        """The node's widget type (its ``"type"`` key)."""
        return self["type"]

    @property
    def name(self):
        """The node's own name, or ``None`` — the client-side label `find`
        resolves and the host never sees."""
        return self.get("name")

    def names(self) -> list:
        """The names reachable in this view's scope, in tree order. A nested
        view contributes its own name, not the names inside it."""
        return list(self._scope)

    def find(self, name: str) -> "View":
        """The named widget in this view's scope.

        Raises `KeyError` if nothing carries that name here — including when the
        name is inside a nested view, which is a scope of its own::

            v.find("osc1").find("freq")

        """
        try:
            return self._scope[name]
        except KeyError:
            raise KeyError(
                f"no widget named {name!r} in this view "
                f"(names here: {', '.join(self.names()) or 'none'})") from None

    def __contains__(self, key) -> bool:
        # `key in view` keeps its dict meaning; use `name in view.names()` for
        # the index, so a props check never silently answers about a name.
        return super().__contains__(key)

    # ---- the document ----

    def to_json(self) -> str:
        """This view as the GuiDef document ``/gui_def`` takes (names stripped)."""
        from .guidef import to_json

        return to_json(self)

    # ---- opening ----

    def open(self, *blobs: bytes, id: "int | None" = None, host=None):
        """Open this view on a GUI host and return its
        `clausters.gui.handle.WindowHandle`.

        The resource is the subject: ``window(...).open()`` rather than
        ``host.open(window(...))``. ``host`` follows the ambient rule every other
        visual verb follows (`clausters.plot`, `clausters.scope`) — the one
        registered with `clausters.gui.set_ambient_host`, else the current or
        default session's `gui` host, else a standalone host booted and owned by
        the ambient layer. Trailing ``blobs`` and an explicit ``id`` ride through
        to `clausters.gui.host.GuiHost.open` unchanged.
        """
        if host is None:
            from ..plot import _ambient_host

            host = _ambient_host()
        return host.open(self, *blobs, id=id)

    def __repr__(self) -> str:
        name = self.get("name")
        label = f" {name!r}" if name else ""
        kids = self.get("children") or ()
        count = f", {len(kids)} children" if kids else ""
        return f"<View {self.get('type', '?')}{label}{count}>"


def _scope_of(node: dict) -> dict:
    """``name -> View`` for one node's scope: every named descendant, stopping
    the descent at a nested view (which is registered by its own name and keeps
    the names inside it).

    The node's *own* name is not in its scope — a view is not found inside
    itself; it is found in the scope of whatever contains it.
    """
    scope: dict = {}
    for child in node.get("children") or ():
        name = child.get("name")
        inner = child._scope if isinstance(child, View) else _scope_of(child)
        if isinstance(name, str) and name:
            _claim(scope, name, child)
        if child.get("type") in _SCOPES:
            continue                      # a nested view keeps its names
        for inner_name, inner_node in inner.items():
            _claim(scope, inner_name, inner_node)
    return scope


def _claim(scope: dict, name: str, node):
    """Record ``name -> node``, refusing a name already taken in this scope."""
    if name in scope:
        raise ValueError(
            f"duplicate widget name {name!r} in one view — a name is how this "
            "client addresses a widget, so two widgets cannot share one. Rename "
            "one, or put them in nested views, which scope their names.")
    scope[name] = node


# ------------------------------------------------------------------ the source


#: Inline ``data`` ceiling: at most this many floats ride the GuiDef JSON.
#: Anything longer spills to a temp raw-``f32`` file the host maps — the bulk
#: path, where the samples never touch OSC. The same number `clausters.plot`
#: has always used, now in one place.
INLINE_MAX = 2048

#: Temp sample files this module spilled, removed at interpreter exit.
_tmp_files: list[str] = []


def spill(samples) -> str:
    """Write `samples` to a temp raw little-endian ``f32`` file and return its
    path, remembering it so it is removed when the interpreter exits.

    The bulk path's one implementation: `Source` and `clausters.plot` both take
    it, so "how large data reaches the host" is decided once.
    """
    import os
    import tempfile

    fd, path = tempfile.mkstemp(prefix="clausters_gui_", suffix=".f32")
    os.close(fd)
    samples_to_file(samples, path)
    _tmp_files.append(path)
    return path


@atexit.register
def _remove_spilled():
    import os

    for path in _tmp_files:
        try:
            os.remove(path)
        except OSError:
            pass


class Source:
    """The samples a view draws, as something you hold — and can change.

    A signal view is fed one of several ways, and the wire spells each as its
    own prop: ``data`` (a small list inline in the JSON), ``blob`` (an index
    into the message's trailing binary arguments), ``path`` (a mapped raw-f32
    file), ``cache`` (a prebuilt peak pyramid), ``buffer`` (a server buffer
    number). Which one is right is a question about **size and where the samples
    already are**, not about what you are drawing — and ``blob=0`` in particular
    is a correspondence kept by hand between the widget and the ``open`` call
    that had better pass that blob first.

    A `Source` answers it for you and stays addressable::

        sig = source(samples)
        v = view(waveform(name="wave", data=sig), title="a take")
        win = v.open()

        sig.set(other_samples)     # the definition and every open view follow

    It is the same relation a `clausters.defs.control` has with a knob: the
    entry point named once, and referred to instead of copied. Pass it to the
    prop that names the input you mean (``data``, for samples); the object
    decides how they travel, so the view never mentions a carrier.

    One source in two views is **one payload and two references** — which is
    what makes the blobs interchangeable, said by the program rather than by
    convention.

    **The carrier is decided once**, when the source is made, from what it first
    holds: a short list stays inline, a long one spills to a temp raw-f32 file
    that the source keeps for its life. `set` then writes through that same
    carrier, because a widget already on screen was built around it — inline
    samples are replaced with a ``/gui_set data``, and a file is **rewritten in
    place** and re-read with a ``/gui_set reload``, which is the host's own pair
    of doors for "the samples are now these" and "they are where they were, and
    they moved".
    """

    __slots__ = ("_carrier", "_value", "_props", "_bound", "_live")

    def __init__(self, samples=None, *, buffer: "int | None" = None,
                 path: "str | None" = None, cache: "str | None" = None,
                 channels: "int | None" = None, sample_rate: "float | None" = None,
                 base_bucket: "int | None" = None):
        given = [x for x in (samples, buffer, path, cache) if x is not None]
        if len(given) != 1:
            raise TypeError(
                "a source names its samples exactly one way: an iterable of "
                "floats, or buffer=/path=/cache= for samples that already live "
                "somewhere the host can reach")
        #: the fixed props every carrier carries alongside the samples.
        self._props = _drop_none(channels=channels, sample_rate=sample_rate,
                                 base_bucket=base_bucket)
        #: ``(node, prop)`` for every node this source was placed in — the
        #: definitions it feeds, rewritten by `set` so a later `open` sends the
        #: samples it holds now.
        self._bound: list = []
        #: ``(host, widget_id)`` for every live widget drawing it, so `set`
        #: reaches what is already on screen. An entry leaves when the host
        #: recycles the widget.
        self._live: list = []
        if buffer is not None:
            self._carrier, self._value = "buffer", int(buffer)
        elif path is not None:
            self._carrier, self._value = "path", str(path)
        elif cache is not None:
            self._carrier, self._value = "cache", str(cache)
        else:
            values = [float(x) for x in samples]
            if len(values) <= INLINE_MAX:
                self._carrier, self._value = "data", values
            else:
                self._carrier, self._value = "path", spill(values)

    @property
    def carrier(self) -> str:
        """How these samples travel: ``"data"``, ``"path"``, ``"cache"`` or
        ``"buffer"``. Decided when the source is made and fixed for its life."""
        return self._carrier

    def props(self) -> dict:
        """The carrier props this source expands to — what a builder puts into
        the node in place of the prop it was passed as."""
        return {self._carrier: self._value, **self._props}

    def set(self, samples) -> "Source":
        """Point this source at other samples: the **definitions** that hold it
        are rewritten, and every widget already drawing it is told to redraw.

        A view open twice is updated twice — the samples belong to the
        definition, so both instances follow; the per-instance door stays
        ``win["wave"].set(...)``.

        Only a source that *holds* its samples takes this. One that names a
        server buffer or a cache is a reference to samples somebody else owns:
        change them where they live and call `reload`.
        """
        if self._carrier not in ("data", "path"):
            raise TypeError(
                f"this source names a {self._carrier}, which it does not own — "
                "change the samples where they live and call reload()")
        values = [float(x) for x in samples]
        if self._carrier == "data":
            if len(values) > INLINE_MAX:
                raise ValueError(
                    f"{len(values)} samples do not fit the inline carrier this "
                    f"source was made with (at most {INLINE_MAX}). A source's "
                    "carrier is fixed when it is made, because a widget on "
                    "screen was built around it — make this one from a long "
                    "list, or from a path=, so it spills from the start")
            self._value = values
            for node, prop in self._bound:
                _rewrite_source(node, prop, self.props())
            for host, wid in list(self._live):
                host.set(wid, data=values)
            return self
        # A file: rewritten **where it is**, so every widget re-reads the same
        # path. `reload` is the host's door for exactly this.
        samples_to_file(values, self._value)
        for host, wid in list(self._live):
            host.set(wid, reload=1)
        return self

    def reload(self) -> "Source":
        """Tell every widget drawing this source to read it again — the samples
        are where they were and they moved (a file rewritten from outside, a
        server buffer recorded into, a cache rebuilt)."""
        for host, wid in list(self._live):
            host.set(wid, reload=1)
        return self

    def __repr__(self) -> str:
        held = len(self._value) if self._carrier == "data" else self._value
        return f"<Source {self._carrier}={held!r}, {len(self._live)} live>"


def _samples_arg(data):
    """A ``data=`` argument as it goes into the node: a `Source` passes through
    untouched (`node` expands it into its carrier), anything else is read into a
    list here."""
    if data is None or isinstance(data, Source):
        return data
    return list(data)


#: The prop names a `Source` may stand in for: the logical one (``data``, "the
#: samples") and the carriers themselves, so a tree written the long way can
#: adopt a source without moving the keyword.
SOURCE_PROPS = ("data", "blob", "path", "cache", "buffer")


def _rewrite_source(node: dict, prop: str, props: dict):
    """Put ``props`` into ``node`` in place of whatever carrier it held."""
    for key in SOURCE_PROPS:
        node.pop(key, None)
    node.update(props)


def source(samples=None, *, buffer: "int | None" = None, path: "str | None" = None,
           cache: "str | None" = None, channels: "int | None" = None,
           sample_rate: "float | None" = None,
           base_bucket: "int | None" = None) -> Source:
    """The samples a view draws, as a `Source` — held, referred to, and changed
    in place of being copied into a prop.

    Give it an iterable of floats, or name samples that already live somewhere
    the host can reach: ``buffer=`` (a server buffer number), ``path=`` (a raw
    little-endian f32 file it maps) or ``cache=`` (a prebuilt peak pyramid, see
    `peaks_cache_file`). ``channels`` de-interleaves it, ``sample_rate`` gives
    the time ruler its unit and ``base_bucket`` sizes the peak pyramid.
    """
    return Source(samples, buffer=buffer, path=path, cache=cache, channels=channels,
                  sample_rate=sample_rate, base_bucket=base_bucket)


# ------------------------------------------------------------------- the nodes

def node(type: str, *, children=None, id: int | None = None, **props) -> View:
    """A generic widget node ``{id?, type, ...props, children?}``, as a `View`.

    The building block every other helper wraps. ``children`` is an iterable of
    nodes for a container and any other keyword is a property (kept verbatim, so
    its int/float type is preserved). ``id`` is keyword-only here as it is in
    every builder — normally left out, so the host assigns one.

    The value is a `clausters.gui.guidef.View` — a ``dict`` subclass, so the
    document is exactly what it always was, and the tree can also be looked up by
    name (`View.find`) and opened (`View.open`). Building it is where a
    **duplicate name** is caught: two widgets sharing one name in a single view
    raise here rather than shadowing each other silently.
    """
    out: dict = {"type": type}
    sources = {}
    for key in SOURCE_PROPS:
        held = props.get(key)
        if isinstance(held, Source):
            sources[key] = held
            props.pop(key)
    for key, value in props.items():
        if isinstance(value, Source):
            raise TypeError(
                f"{type}: a source names a view's samples, so it goes in a prop "
                f"that is one of {', '.join(SOURCE_PROPS)} — not {key!r}")
    if id is not None:
        if not isinstance(id, int) or isinstance(id, bool):
            raise TypeError(
                f"widget id must be an int or None, got {id!r} — omit the id "
                "to let GuiHost.open assign one")
        out["id"] = id
    out.update(props)
    if children:
        kids = list(children)
        for child in kids:
            if not isinstance(child, dict):
                raise TypeError(
                    f"{type}: a child must be a widget node, got {child!r} — the "
                    "id is a keyword argument, so children come first: "
                    f"{type}(child, ..., id=…)")
        out["children"] = kids
    built = View(out)
    for key, held in sources.items():
        _rewrite_source(built, key, held.props())
        held._bound.append((built, key))
        built._sources.append(held)
    return built


# --------------------------------------------------------------- the model
#
# A GuiDef names three kinds of thing: a **container** owning 0, 1 or 2 axes,
# an **element** drawn against them, and a **control**, which is an element
# with a value and no axis. The builders below name that model; the ones
# further down (`panel`, `waveform`, `track`, ...) are shortcuts that build
# the same nodes with a familiar name and the props of one common case.


def layout(*children, flow: str | None = None, index: int | None = None,
           margin: float | None = None, gap: float | None = None, cols: int | None = None,
           theme: dict | None = None, color: str | None = None, id: int | None = None,
           **props) -> View:
    """A container with **no axes**, arranging its children by ``flow``.

    ``flow`` is ``"row"``, ``"col"`` (the default), ``"grid"``, ``"free"`` — or
    ``"stack"``, which shows **one child at a time**, the one ``index`` names,
    and lays out and draws none of the others. A stack is not a different
    container: it is this one with a selection instead of an arrangement, which
    is why `stack` is a shortcut rather than a type.

    ``margin`` insets the children, ``gap`` separates them, ``cols`` fixes the
    ``grid`` column count. ``theme`` (a partial ``{"role": "#rrggbb[aa]"}``
    table) makes it a **theme group** over its whole subtree; ``color``
    re-seeds the accent family for itself.
    """
    extra = _drop_none(flow=flow, index=index, margin=margin, gap=gap, cols=cols,
                       theme=theme, color=color)
    return node("layout", id=id, children=children, **extra, **props)


def plane(*children, flow: str | None = None, axis: str | None = None,
          zoom: bool | None = None, content_w: float | None = None,
          content_h: float | None = None, view_x: float | None = None,
          view_y: float | None = None, view_zoom: float | None = None,
          boxes=None, cords=None, margin: float | None = None, gap: float | None = None,
          cols: int | None = None, theme: dict | None = None, color: str | None = None,
          id: int | None = None, **props) -> View:
    """A container with **two axes locked to one scale**: a pannable, zoomable
    plane in content units.

    The children lay out into a content area larger than the widget, seen
    through a window that pans and zooms. ``axis`` (``"both"``/``"x"``/``"y"``)
    and ``zoom`` constrain it — a plain vertical scroll view is
    ``axis="y", zoom=False`` — and ``view_x``/``view_y``/``view_zoom`` are the
    window itself. See `scroll` for the whole of it.

    With ``boxes`` and ``cords`` the plane is a **patcher**: the boxes are what
    it places and the cords are the wires between them, which is all `patch`
    ever added to a plane. See `patch` for their shape.
    """
    extra = _drop_none(flow=flow, axis=axis, content_w=content_w, content_h=content_h,
                       view_x=view_x, view_y=view_y, view_zoom=view_zoom,
                       boxes=list(boxes) if boxes is not None else None,
                       cords=[int(x) for x in cords] if cords is not None else None,
                       margin=margin, gap=gap, cols=cols, theme=theme, color=color)
    if zoom is not None:
        extra["zoom"] = 1 if zoom else 0
    return node("plane", id=id, children=children, **extra, **props)


def field(*children, axes: dict | None = None, offset: float | None = None,
          dur: float | None = None, label: str | None = None, height: float | None = None,
          snap: float | None = None, header_w: float | None = None, mute: bool | None = None,
          solo: bool | None = None, level: float | None = None, h: float | None = None,
          theme: dict | None = None, color: str | None = None, id: int | None = None,
          **props) -> View:
    """A container with **two independent axes** — the time/value container.

    One container, told apart by what is placed on it: holding other fields it
    is a **lane** (with the header props ``label``/``height``/``header_w``/
    ``mute``/``solo``/``level``), carrying ``offset``/``dur`` it is a **clip**
    placed on its parent's x axis, and with nothing on it at all it is a
    free-standing **ruler** over its navigation group. The three shortcuts
    `track`, `clip` and `timeruler` are those three cases.

    ``axes`` is the pair the chrome belongs to::

        field(axes={"x": {"unit": "beats", "tempo": 2.0, "link": 1},
                    "y": {"min": -1.0, "max": 1.0}})

    On the x axis: ``unit`` (``"time"``/``"samples"``/``"beats"``/``"off"``),
    ``start``/``len`` (the navigation window), ``tempo`` (beats per second), ``beat_at`` and ``quant`` (**beats per bar**, the grid a ``bar:beat`` label counts on — not a length in samples),
    ``sample_rate``, ``link`` (the navigation group), ``sel_start``/``sel_len``
    and the ``playhead`` family. On the y axis: ``unit``, ``start``/``len``,
    ``min``/``max``, ``bit_depth``.

    A placed field's **bodies** — a take, note events, an automation curve —
    are still described by props (``data``/``buffer``/``path``/``cache``,
    ``notes``, ``points``); `clip` is the shortcut that packs them.
    """
    extra = _drop_none(offset=offset, dur=dur, label=label, height=height, snap=snap,
                       header_w=header_w, mute=mute, solo=solo, level=level, h=h,
                       theme=theme, color=color)
    extra.update(_axes(axes))
    return node("field", id=id, children=children, **extra, **props)


def signal(*, view: str | None = None, data=None, blob: int | None = None,
           buffer: int | None = None, path: str | None = None, cache: str | None = None,
           bus: int | None = None, rate: str | None = None, channels: int | None = None,
           retention: float | None = None,
           base_bucket: int | None = None, navigable: bool | None = None,
           selectable: bool | None = None, editable: bool | None = None,
           overlay: bool | None = None, measure: str | None = None,
           fills: bool | None = None,
           at: float | None = None, dur: float | None = None,
           start: float | None = None, loop: bool | None = None,
           axes: dict | None = None, label: str | None = None,
           color: str | None = None, id: int | None = None, **props) -> View:
    """**Every view of a signal**, as the one element they are: a presentation
    of a source, with the capabilities offered over it.

    - ``view`` — the presentation: ``"trace"`` (the default; value against
      time), ``"spectrum"`` (magnitude against frequency), ``"spectrogram"``
      (the STFT, magnitude against time *and* frequency) or ``"phase"`` (the
      goniometer of a stereo pair).
    - ``measure`` — **what the picture measures**: ``"peak"`` (the default, the
      min/max envelope the signal reached), ``"rms"`` (the symmetric body of the
      level it held, drawn in the body color role), or **both** as one
      space-separated string — ``"peak rms"``, the classic editor picture, the
      level drawn inside the envelope. It is a factor of the view rather than a
      widget of its own: one body, drawn once per measure by the same renderer,
      which is also what keeps the axis, the ruler, the selection and the
      upload single. A source whose peak cache was built before the measure
      existed draws no body rather than a flat line of zeros.

    - the **source** — ``bus`` (with ``rate``) is forward-only, read live;
      ``data``/``blob``/``buffer``/``path``/``cache`` are addressable samples,
      which is what lets a view navigate, slice and select. ``channels``
      de-interleaves it, ``base_bucket`` sizes the peak pyramid.
    ``fills`` says the buffer is **being written into as it is drawn** — a
    take you are recording. The view then draws it up to the buffer's write
    frontier and leaves the axis past it empty, instead of drawing a flat line
    across the buffer's own zeros: past the frontier there is no silence, there
    is no samples yet. The host cannot infer this and does not try, because a
    frontier alone does not distinguish a recording from a loaded take that one
    write touched — you allocated the buffer, so you are what knows. Clear it
    (``set(fills=False)``) when the take is finished and the whole of the
    samples is drawn again.

    - the **capabilities** — ``navigable`` (zooms and pans its axes, and joins
      the navigation group its x axis names), ``selectable``, ``editable``.
      Over ``view="spectrum"`` it means the **frequency** axis instead: that x
      is not time, so it navigates on a window of its own
      (``axes={"x": {"start": ..., "len": ...}}``, normalized over
      ``[0, Nyquist]``, reported as ``"view_x"``) and joins no group. It is the
      one view where ``navigable`` is off unless asked for.
    - ``retention`` — **seconds of history the host keeps of a ``bus``** (0 =
      none, the default). A forward-only source has no addressable past, which
      is what stops it being navigable: there is nothing behind the newest
      window to zoom out to. This is what supplies one, so
      ``signal(view="spectrogram", bus=0, retention=8.0, navigable=True)`` is a
      **waterfall** — eight seconds of live spectrum you can zoom and pan like a
      file. It is a policy of the axis, not of the drawing: the same seconds
      mean the same seconds at any frame rate, FFT size or hop, and a
      `GuiHost.set` of it resizes the history live.

    So ``signal(view="trace", path=take)`` is the heavy waveform and
    ``signal(view="trace", bus=0)`` the oscilloscope: the same element over the
    two kinds of source. The presentation's own parameters (``fft_size`` /
    ``window_size``, ``hop``, ``db_floor``/``db_ceil``, ``freq_scale``,
    ``colormap``, ``window_ms``, ``trigger``, ``hold``, ``averaging``,
    ``peak_hold``) ride through as keywords; the shortcuts `waveform`, `plot`,
    `scope`, `spectrum`, `spectrogram` and `phasescope` name and document the
    six common points of the product.

    ``axes`` is the axis pair the rulers, the navigation window, the selection,
    the playhead and the value range belong to (see `field`).

    **Inside a `clip`** four more mean something, and only there: ``at`` and
    ``dur`` place this body on a **stretch** of the clip's own time (a clip
    holding three segments of three files holds three takes, each over its own
    third), and ``start``/``loop`` are that body's own **window** onto its
    samples — the frame it reads from, and whether it wraps. A body that names
    none of them fills the clip and reads through the clip's own window, which
    is every take written as a clip prop.
    """
    extra = _drop_none(view=view, data=_samples_arg(data),
                       blob=blob, buffer=buffer, path=path, cache=cache,
                       retention=retention,
                       bus=bus, rate=rate, channels=channels, base_bucket=base_bucket,
                       measure=measure, at=at, dur=dur, start=start,
                       label=label, color=color)
    for key, flag in (("navigable", navigable), ("selectable", selectable),
                      ("editable", editable), ("overlay", overlay), ("loop", loop),
                      ("fills", fills)):
        if flag is not None:
            extra[key] = 1 if flag else 0
    extra.update(_axes(axes))
    return node("signal", id=id, **extra, **props)


def view(*children, title: str | None = None, w: int | None = None, h: int | None = None,
         flow: str | None = None, layout: str | None = None, margin: float | None = None,
         gap: float | None = None, cols: int | None = None, hug: bool | None = None,
         theme: dict | None = None, **props) -> View:
    """A view's **root**: a container that becomes an OS window when nothing
    holds it, and an ordinary component when something does. It takes no id.

    There is one node type, not two. A view with no parent is a window, so this
    is the container you reach for when the thing you are building is the whole
    of what a window shows; nested in another view it is a panel that **scopes
    its names** (`View.find`), which is what lets two copies
    of one strip each hold their own ``freq``.

    Any node opens (`clausters.gui.guidef.View.open`): ``knob(...).open()`` is a
    window that is a knob. Use ``view()`` when the window's own properties
    matter — a title, a size, a theme — since a root that is not one is framed
    in a window that hugs whatever it holds.

    ``w``/``h`` size the OS window; ``layout`` (``row``/``col``/``grid``/
    ``free``) places the children, tuned by ``margin``/``gap``/``cols`` (see
    the module docstring for the per-child place props).

    ``hug`` sizes the window to its content instead: the OS window opens as
    big as what it holds on the axes that settle, keeping the declared ``w``/
    ``h`` on the others — a window with one control in it is that control,
    not a pane with a strip at the top. In a page there is no window to size,
    so a mounted GuiDef takes the box the element gives it and only the
    containers inside it hug.

    ``theme`` is a partial color-role table (``{"role": "#rrggbb[aa]"}``, the
    same shape as the host's TOML style file) overlaying the host theme for
    the whole window — a **theme group**. On the root it persists with a named
    def, so a standalone bundle ships its look.
    """
    extra = _drop_none(title=title, w=w, h=h, flow=flow or layout, margin=margin, gap=gap,
                       cols=cols, theme=theme)
    if hug is not None:
        extra["hug"] = 1 if hug else 0
    return node("window", children=children, **extra, **props)


#: The root's older spelling, from before a view and a window were one node. Kept
#: so existing trees go on reading; ``view`` is the name the documentation uses.
window = view


def panel(*children, flow: str | None = None, layout: str | None = None,
          margin: float | None = None, gap: float | None = None, cols: int | None = None,
          hug: bool | None = None, theme: dict | None = None, color: str | None = None,
          id: int | None = None, **props) -> View:
    """A nestable ``panel`` container; ``layout`` is ``row``/``col``/``grid``/``free``.

    ``margin`` insets the children, ``gap`` separates them, ``cols`` fixes the
    ``grid`` column count. As a child, a panel takes the same place props as
    any widget (``w``/``h``/``weight``, or ``x``/``y`` in a ``free`` parent).

    ``hug`` makes it want its content rather than its share of the leftover —
    a strip of controls that says how tall it is instead of being told (see
    the module docstring for what a size may read).

    ``theme`` (a partial ``{"role": "#rrggbb[aa]"}`` table) makes the panel a
    **theme group**: the overlay styles its whole subtree — a transport bar
    dimmed, a recording strip warm — recursively over the parent's theme.
    ``color`` re-seeds just the accent family for the panel itself.
    """
    extra = _drop_none(flow=flow or layout, margin=margin, gap=gap, cols=cols,
                       theme=theme, color=color)
    if hug is not None:
        extra["hug"] = 1 if hug else 0
    return node("layout", id=id, children=children, **extra, **props)


def stack(*children, index: int | None = None, margin: float | None = None,
          hug: bool | None = None, theme: dict | None = None, color: str | None = None,
          id: int | None = None, **props) -> View:
    """A ``stack`` container showing **one child at a time**: the one at ``index``.

    The shown page fills the container (``margin`` insets it); the hidden ones
    are not laid out and not drawn, so a page costs nothing while it is away —
    but they stay in the tree, so a heavy view keeps its GPU slot and its bus
    reads across a switch and comes back without re-uploading anything.

    ``index`` is live via ``set``, and it is the prop a control **binds** to:
    a toggle or a menu bound to it (`GuiHost.bind_widget`, or an inline
    ``bind=["widget", stack_id, "index"]``) flips the page with no round-trip
    through this script — which is what makes tabs, a pager and a
    waveform/spectrogram switch composition rather than widgets::

        pages = stack(waveform(data=take), spectrogram(data=take), name="views")
        picker = menu(["wave", "spectrum"], name="picker")
        ...
        host.bind_widget(win["picker"].id, win["views"].id, "index")

    An ``index`` outside the children shows nothing — a blank page rather than
    a clamped one, so a pager that runs off the end never shows the wrong
    child.

    ``hug`` sizes the stack to its **largest** page rather than to the shown
    one, so flipping a pager does not resize it.
    """
    extra = _drop_none(index=index, margin=margin, theme=theme, color=color)
    if hug is not None:
        extra["hug"] = 1 if hug else 0
    return node("layout", id=id, children=children, flow="stack", **extra, **props)


def scroll(*children, axis: str | None = None, zoom: bool | None = None,
           content_w: float | None = None, content_h: float | None = None,
           view_x: float | None = None, view_y: float | None = None,
           view_zoom: float | None = None, flow: str | None = None, layout: str | None = None,
           margin: float | None = None, gap: float | None = None, cols: int | None = None,
           theme: dict | None = None, color: str | None = None, id: int | None = None,
           **props) -> View:
    """A ``scroll`` container: a 2D workspace onto a virtual content area.

    The children lay out into a content area larger than the widget, seen
    through a window that pans and zooms — dragging the empty plane pans it,
    the wheel zooms anchored at the cursor. The general case is the full 2D
    workspace; the constrained scroll views come from configuration, not from
    a different widget:

    - ``axis="y", zoom=False`` is a plain vertical scroll view (the wheel
      scrolls, x never moves),
    - ``axis="x", zoom=False`` a horizontal strip,
    - the default (``axis="both"``, zoom on) is the free plane.

    ``layout`` arranges the children inside the content area and defaults to
    ``free`` here (the workspace's natural arrangement), so a child's ``x``/
    ``y``/``w``/``h`` place it in content units. The content area sizes from
    those placement extents unless ``content_w``/``content_h`` say otherwise.
    ``view_x``/``view_y`` (content units at the widget's top-left corner) and
    ``view_zoom`` (physical pixels per content unit) are the view state: live via
    ``/gui_set``, and emitted as ``"view" x y zoom`` when a gesture moves them.
    Leaving ``view_zoom`` out is not the same as passing ``1``: a plane with no
    zoom of its own starts at the **display's scale**, so one content unit is one
    logical pixel and the boxes come up the size they are meant to look. Pass a
    number (or turn the wheel) and it is literal from then on; ``set(view_zoom=0)``
    clears it again, which is how a script says "back to the default" for a
    number it cannot name.
    """
    extra = _drop_none(axis=axis, content_w=content_w, content_h=content_h,
                       view_x=view_x, view_y=view_y, view_zoom=view_zoom,
                       flow=flow or layout, margin=margin, gap=gap, cols=cols,
                       theme=theme, color=color)
    if zoom is not None:
        extra["zoom"] = 1 if zoom else 0
    return node("plane", id=id, children=children, **extra, **props)


def label(text: str = "", *, text_size: float | None = None, wrap: bool | None = None,
          align: str | None = None, color: str | None = None, id: int | None = None, **props
          ) -> View:
    """Static ``label`` text, passed positionally: ``label("hello")``.

    ``text_size`` is the glyph scale over the host's font (default 2.0 —
    every text-bearing widget takes it). A host drawing with its embedded
    bitmap face quantizes it to half-steps, so a glyph's own pixels stay
    equal; one built with a rasterizer takes it as sent. ``wrap=True``
    word-wraps the text to the label's width; off, a single line that
    overflows clips with an ellipsis. ``align`` places each line in the rect:
    ``"start"`` (the default left edge), ``"center"`` or ``"end"``.
    """
    extra = _drop_none(text_size=text_size, align=align, color=color)
    if wrap is not None:
        extra["wrap"] = 1 if wrap else 0
    return node("label", id=id, text=text, **extra, **props)


def knob(*, label: str | None = None, min: float | None = None, max: float | None = None,
         value: float | None = None, text_size: float | None = None, color: str | None = None,
         id: int | None = None, **props) -> View:
    """A rotary ``knob`` over a continuous range. ``text_size`` scales its
    label and value read-out."""
    extra = _drop_none(label=label, min=min, max=max, value=value, text_size=text_size, color=color)
    return node("knob", id=id, **extra, **props)


def slider(*, label: str | None = None, min: float | None = None, max: float | None = None,
           value: float | None = None, vertical: bool = False, text_size: float | None = None,
           color: str | None = None, id: int | None = None, **props) -> View:
    """A continuous ``slider`` over a range. ``vertical=True`` lays it out along
    the y axis (min at the bottom, max at the top) instead of horizontally.
    ``text_size`` scales its label and value read-out."""
    extra = _drop_none(label=label, min=min, max=max, value=value, text_size=text_size, color=color)
    if vertical:
        extra["vertical"] = True
    return node("slider", id=id, **extra, **props)


def number(*, label: str | None = None, min: float | None = None, max: float | None = None,
           value: float | None = None, text_size: float | None = None, color: str | None = None,
           id: int | None = None, **props) -> View:
    """A draggable numeric read-out over a range. ``text_size`` scales its
    label and value."""
    extra = _drop_none(label=label, min=min, max=max, value=value, text_size=text_size, color=color)
    return node("number", id=id, **extra, **props)


def button(*, label: str | None = None, text_size: float | None = None, color: str | None = None,
           id: int | None = None, **props) -> View:
    """A momentary push ``button`` (emits ``1`` on press, ``0`` on release).
    ``text_size`` scales its face label."""
    extra = _drop_none(label=label, text_size=text_size, color=color)
    return node("button", id=id, **extra, **props)


def toggle(*, label: str | None = None, value: bool | None = None, text_size: float | None = None,
           color: str | None = None, id: int | None = None, **props) -> View:
    """A boolean ``toggle``. ``value`` is sent as ``1``/``0`` (OSC has no bool).
    ``text_size`` scales its label."""
    extra = _drop_none(label=label, text_size=text_size, color=color)
    if value is not None:
        extra["value"] = 1 if value else 0
    return node("toggle", id=id, **extra, **props)


def text(*, value: str | None = None, label: str | None = None, text_size: float | None = None,
         multiline: bool | None = None, color: str | None = None, id: int | None = None, **props
         ) -> View:
    """An editable ``text`` field. The user types into it and the entered string
    is emitted as a ``/gui_event`` (or forwarded to the server when bound) on
    **every** edit — like a slider's value, never gated on Enter. ``multiline``
    allows embedded newlines (Enter inserts one) and a growing field; ``value``
    seeds the initial contents (and ``/gui_set value`` sets it live). ``text_size``
    scales the field text and its label."""
    extra = _drop_none(value=value, label=label, text_size=text_size, color=color)
    if multiline is not None:
        extra["multiline"] = bool(multiline)
    return node("text", id=id, **extra, **props)


def menu(options=(), *, index: int | None = None, label: str | None = None,
         text_size: float | None = None, color: str | None = None, id: int | None = None, **props
         ) -> View:
    """A ``menu`` over ``options`` (a list of strings), emitting the chosen
    ``index``.

    A press **opens the list** over the window — the field grown downward by a
    row per option, flipped above it near the bottom edge — and a press on a row
    picks it; a press anywhere else dismisses it and picks nothing. The list is
    the host's, so a bound menu (`GuiHost.bind`/`bind_widget`) drives its target
    with no round trip through this script. ``text_size`` scales the shown
    choice and the label."""
    extra = _drop_none(index=index, label=label, text_size=text_size, color=color)
    return node("menu", id=id, options=list(options), **extra, **props)


def waveform(*, data=None, blob: int | None = None, buffer: int | None = None,
             path: str | None = None, cache: str | None = None, channels: int | None = None,
             base_bucket: int | None = None, overlay: bool | None = None,
             measure: str | None = None, fills: bool | None = None,
             ruler: str | None = None,
             ruler_y: str | None = None, bit_depth: int | None = None,
             min: float | None = None, max: float | None = None,
             sample_rate: float | None = None, tempo: float | None = None,
             beat_at: float | None = None, quant: float | None = None,
             sel_start: float | None = None, sel_len: float | None = None,
             sel_min: float | None = None, sel_max: float | None = None,
             playhead_at: float | None = None, playhead: float | None = None,
             playhead_loop_start: float | None = None, playhead_loop_len: float | None = None,
             y_start: float | None = None,
             y_len: float | None = None, link: int | None = None, axes: dict | None = None,
             color: str | None = None, id: int | None = None, **props) -> View:
    """The heavy ``waveform`` view, fed its samples one of several ways (in the
    host's precedence order):

    - ``cache`` — a path to a prebuilt peak-pyramid file (see `peaks_cache_file`)
      the host memory-maps and renders directly; the raw samples are never
      loaded. The most compact **bulk path**: nothing rides OSC. A cache built
      with ``channels > 1`` holds every channel in the one file.
    - ``path`` — a path to a file of raw little-endian ``f32`` samples (see
      `samples_to_file`, or the server's ``/buffer_export``) the host memory-maps; a
      **multi-megabyte buffer renders with no OSC and no re-send**.
    - ``buffer`` — a server buffer number; the host fetches its samples from the
      audio server over OSC (it must be started with ``--server``). The async
      fallback when a shared file is not available.
    - ``data`` — a small list of floats embedded inline in the JSON;
    - ``blob`` — the index of a binary blob carried beside the JSON in the same
      ``/gui_def`` message (see `samples_to_blob` and `GuiHost.define`).

    ``channels`` is the interleaved channel count of ``path``/``data``/``blob``
    (default 1): **every** channel is kept and drawn — stacked lanes sharing the
    time axis by default, or per-color overlaid traces with ``overlay=True``.

    ``measure`` chooses what the columns measure — ``"peak"`` (the default
    envelope) or ``"rms"`` (the level body). Two `waveform` views of one source
    on the same axes, one of each, are the classic editor picture as a **stack**
    (see `signal`).
    ``base_bucket`` sets the peak-pyramid bucket size (default 256); for ``path``
    it also keys the sibling cache the host writes beside the file.

    The rulers (each in its own strip beside the view, each independently
    switchable off, all live via ``GuiHost.set`` — so a menu or button in the
    same GUI can retune them): ``ruler`` labels the time axis — ``"time"``
    (the default; clock time, using ``sample_rate`` or the rate the source
    brings), ``"samples"``, ``"beats"`` (musical time: ``tempo`` in beats per
    second — pass ``clock.tempo`` — ``beat_at`` the beat position of sample 0,
    ``quant`` the beats per bar, labels ``bar:beat``), or ``"off"``.
    ``ruler_y`` labels the amplitude axis — ``"norm"`` (the default;
    normalized [-1, 1]), ``"db"`` (dBFS), ``"bits"`` (integer sample values at
    the ``bit_depth`` resolution, default 16), ``"percent"`` (0-100% of full
    scale), or ``"off"``. It **labels** the axis and does not map it: the
    picture is linear in amplitude whichever unit is named, and ``"db"`` is a
    ladder of rungs drawn at the amplitudes those decibels are. So what a
    reading names and what an edit writes at a height are one value, and editing
    is in linear amplitude and only there.

    ``min``/``max`` are the **value domain** the trace is drawn over, ``[-1,
    1]`` (full-scale audio) when omitted. A named domain is **ruled with its
    own numbers** whatever ``ruler_y`` says, since ``"db"``, ``"bits"`` and
    ``"percent"`` are units of full scale.

    A column is the **min/max of what the signal did in that pixel**, never
    extended to the zero line: the solid body of a zoomed-out waveform is the
    data filling it, not a fill the drawing adds — which is why a subsonic
    signal (a 1 Hz curve, whose samples do not fit the screen either) draws as
    the curve it is. Zoom in far enough that consecutive samples stand apart
    and each one is **marked with a dot**: the line between them is
    interpolation, the dots are the data.

    The rest of the editor chrome: ``sel_start``/``sel_len`` set the selection
    in samples (dragging on the view updates it and emits
    ``/gui_event id "selection" start len``; Shift+drag pans, the wheel zooms).
    A selection is a **count of samples**: ``sel_len`` is how many it holds and
    ``sel_start`` is the first, snapped whether set from here or swept with the
    pointer — never a band of pixels standing between two samples. A sweep takes
    the samples it passed over, so one joins when the cursor reaches it.
    ``sel_min``/``sel_max`` restrict it on the **value axis**, in the domain's
    own units: a sweep with height reports them as two further arguments
    (``"selection" start len min max``) and draws the band as the rectangle it
    is, while a sweep along one height reports the two numbers it always did.
    An empty or inverted pair is no restriction, which is the default.
    ``playhead_at`` draws a playhead tracking the engine sample clock: pass the
    ``/clock_query`` sample value that corresponds to buffer position 0 (negative or
    omitted = no playhead). ``playhead`` is the **static** counterpart — a
    position in samples where a located, stopped transport parks the line
    (negative = none); it stands still while ``playhead_at`` is off, so a
    paused cursor does not drift with the clock.
    ``playhead_loop_start``/``playhead_loop_len`` (in
    samples) make that sweep **wrap** inside the region instead of running
    straight past it — what a looping playback does, so playing a selection on
    a loop can be followed on the same one anchor and still costs no message
    per frame; a non-positive length is the straight pass. ``y_start``/``y_len`` set the **vertical view
    window** — the visible slice of the amplitude axis, in normalized display
    units where ``0, 1`` (the default) is the full axis: the wheel over the
    y-ruler strip zooms it, dragging the strip pans it, and every change is
    reported as ``/gui_event id "view_y" y_start y_len`` (a non-positive
    ``y_len`` resets to the full axis). The zoom is **symmetric about zero** —
    it keeps the window's centre, so on a multichannel file every channel's
    zero line stays at the centre of its own lane and the traces grow and
    shrink in place; drag the strip to reach an off-centre region.

    ``link`` puts the view in a shared **navigation group**: every timeline
    view (waveform or spectrogram, in any window) declaring the same ``link``
    id shares one horizontal view, selection and playhead — a zoom, pan or
    drag-selection on any member moves all of them, and setting
    ``view_start``/``view_len`` (samples; a non-positive ``view_len`` resets
    to the whole timeline), ``sel_start``/``sel_len`` or ``playhead_at`` via
    ``GuiHost.set`` on any member applies group-wide. Events still emit once,
    with the interacted member's id. Membership is live: set ``link`` to
    another group id to move the view, or to a negative value to unlink it
    (it keeps the view it had). Only the vertical window ``y_start``/``y_len``
    stays per-view."""
    extra = _drop_none(data=_samples_arg(data),
                       blob=blob, buffer=buffer, path=path, cache=cache,
                       channels=channels, base_bucket=base_bucket, measure=measure,
                       color=color)
    extra.update(_axes(axes, ruler=ruler, ruler_y=ruler_y, bit_depth=bit_depth,
                       min=min, max=max,
                       sample_rate=sample_rate, tempo=tempo, beat_at=beat_at,
                       quant=quant, sel_start=sel_start, sel_len=sel_len,
                       sel_min=sel_min, sel_max=sel_max,
                       playhead_at=playhead_at, playhead=playhead,
                       playhead_loop_start=playhead_loop_start,
                       playhead_loop_len=playhead_loop_len,
                       y_start=y_start, y_len=y_len, link=link))
    if overlay is not None:
        extra["overlay"] = 1 if overlay else 0
    if fills is not None:
        extra["fills"] = 1 if fills else 0
    return node("signal", id=id, view="trace", **extra, **props)


def spectrogram(*, data=None, blob: int | None = None, buffer: int | None = None,
                path: str | None = None, cache: str | None = None, channels: int | None = None,
                window_size: int | None = None, hop: int | None = None,
                sample_rate: float | None = None, db_floor: float | None = None,
                db_ceil: float | None = None, freq_scale: str | None = None,
                log_freq: bool | None = None, colormap: int | None = None,
                ruler: str | None = None, ruler_y: str | None = None, tempo: float | None = None,
                beat_at: float | None = None, quant: float | None = None,
                sel_start: float | None = None, sel_len: float | None = None,
                playhead_at: float | None = None, playhead: float | None = None,
                playhead_loop_start: float | None = None,
                playhead_loop_len: float | None = None, y_start: float | None = None,
                y_len: float | None = None, link: int | None = None, axes: dict | None = None,
                color: str | None = None, id: int | None = None, **props) -> View:
    """The heavy ``spectrogram`` (STFT time-frequency) view, fed like the
    `waveform`: a mapped ``path`` of raw little-endian ``f32``, a server
    ``buffer``, inline ``data``/``blob``, or a prebuilt single-channel STFT
    ``cache`` file. ``channels`` de-interleaves the source (default 1); each
    channel gets its own analysis, drawn as stacked lanes sharing the time axis.

    The analysis: ``window_size`` is the FFT size (a power of two, default
    1024) and ``hop`` the frame advance (default ``window_size // 2``; the host
    raises it as needed so a long file fits the GPU texture). ``sample_rate``
    places the frequency axis for ``path``/inline sources (a fetched ``buffer``
    brings its own rate). The display is live (``GuiHost.set``): the dB window
    ``[db_floor, db_ceil]`` (default ``-90``/``0``) controls contrast,
    ``freq_scale`` picks the frequency axis — ``"log"`` (the default),
    ``"linear"``, ``"mel"`` or ``"bark"`` (``log_freq`` is the legacy boolean
    alias for the first two) — and ``colormap`` picks 0 viridis / 1 magma /
    2 grayscale.

    The rulers ride their own strips beside the view: ``ruler_y`` (``"hz"``,
    the default, or ``"off"``) draws the frequency ruler, its tick positions
    following ``freq_scale``; ``ruler`` labels the time axis exactly as on the
    `waveform` (``"time"``/``"samples"``/``"beats"`` with
    ``tempo`` (beats per second), ``beat_at`` and ``quant`` (**beats per bar**, the grid a ``bar:beat`` label counts on — not a length in samples), or ``"off"``). The rest of the editor
    chrome (``sel_start``/``sel_len``, ``playhead_at``/``playhead`` and their
    ``playhead_loop_start``/``playhead_loop_len`` region, drag-to-select /
    Shift+drag pan / wheel zoom) also works exactly as on the `waveform` —
    including the vertical view window ``y_start``/``y_len``, which here
    slices the **frequency display axis** (normalized, ``0, 1`` = the full
    axis, whatever the ``freq_scale``): wheel over the Hz-ruler strip zooms,
    dragging it pans, changes emit ``/gui_event id "view_y" y_start y_len``.

    ``link`` joins a shared navigation group exactly as on the `waveform` —
    the classic composition is a waveform lane and a spectrogram lane of the
    same render under one ``link``, scrolling and selecting in lockstep."""
    extra = _drop_none(data=_samples_arg(data),
                       blob=blob, buffer=buffer, path=path, cache=cache,
                       channels=channels, window_size=window_size, hop=hop,
                       db_floor=db_floor, db_ceil=db_ceil, freq_scale=freq_scale,
                       colormap=colormap, color=color)
    extra.update(_axes(axes, ruler=ruler, ruler_y=ruler_y, sample_rate=sample_rate,
                       tempo=tempo, beat_at=beat_at, quant=quant,
                       sel_start=sel_start, sel_len=sel_len,
                       playhead_at=playhead_at, playhead=playhead,
                       playhead_loop_start=playhead_loop_start,
                       playhead_loop_len=playhead_loop_len,
                       y_start=y_start, y_len=y_len, link=link))
    if log_freq is not None:
        extra["log_freq"] = 1 if log_freq else 0
    return node("signal", id=id, view="spectrogram", **extra, **props)


def meter(bus: int = 0, *, rate: str = "audio", min: float | None = None,
          max: float | None = None, label: str | None = None, color: str | None = None,
          id: int | None = None, **props) -> View:
    """A level ``meter`` on ``bus``, read from the audio server's shared-memory
    segment each frame (zero OSC messages; the host must be started with
    ``--shm`` pointing at the server's segment).

    At ``rate="audio"`` (the default) it meters an **audio** bus — bus 0 is the
    first hardware output, so ``meter()`` is the console meter on the left out
    — reading the level the server publishes per block: a peak held with a
    decay, so a transient is caught even though the display refreshes far
    slower than the engine. Metering costs the server nothing to set up, so a
    mixer's worth of meters is fine. At ``rate="control"`` it reads a control
    bus's current value instead. ``min``/``max`` scale the bar (default
    ``0``/``1``).
    """
    extra = _drop_none(min=min, max=max, label=label, color=color)
    return node("meter", bus=bus, rate=rate, **extra, **props, id=id)


def scope(bus: int = 0, *, rate: str = "audio", channels: int | None = None,
          overlay: bool | None = None, measure: str | None = None,
          window_ms: float | None = None,
          trigger: float | None = None, hold: bool | None = None, min: float | None = None,
          max: float | None = None, ruler: "bool | str | None" = None,
          ruler_y: "bool | str | None" = None, label: str | None = None, color: str | None = None,
          axes: dict | None = None, id: int | None = None, **props) -> View:
    """A time-domain ``scope`` over ``channels`` **adjacent** buses starting at
    ``bus`` (bus 0 is the first hardware output), in one of two rates.

    At ``rate="audio"`` (the default) it is a real **oscilloscope**: a
    ``window_ms`` display window (default 20 ms) of each bus's samples, re-read
    every frame and aligned on a rising crossing of ``trigger`` found in the
    **first** channel (default level ``0.0``, with hysteresis; free-running
    when the signal never crosses), so a periodic signal draws a stable trace
    and the channels keep their true relative phase — a lock/free read-out
    names which mode it is in. Asking to see an audio bus is all a script does:
    the GUI host has the server record it and stops when nothing draws it.

    At ``rate="control"`` it plots the recent history of the control buses
    instead, one sample per frame tick.

    Channels draw as stacked lanes, or as color-coded traces in one field with
    ``overlay``. ``hold`` freezes the trace. The audio-rate form carries axis
    rulers: ``ruler`` (x, in milliseconds of the window) and ``ruler_y`` (value
    over ``[min, max]``), both shown by default and hidden with ``False`` (or
    ``"off"``). Natively the host reads the samples out of the ``--shm``
    segment with zero messages; in the browser it subscribes ``/bus_tapStream``
    over the server leg. ``min``/``max`` set the vertical range (default the
    bipolar ``-1``/``1``).
    """
    extra = _drop_none(channels=channels, window_ms=window_ms,
                       trigger=trigger, measure=measure, label=label, color=color)
    for key, flag in (("hold", hold), ("overlay", overlay)):
        if flag is not None:
            extra[key] = 1 if flag else 0
    extra.update(_axes(axes, min=min, max=max, **_strips(ruler=ruler, ruler_y=ruler_y)))
    return node("signal", bus=bus, rate=rate, view="trace", **extra, **props, id=id)


def phasescope(bus: int = 0, *, window_ms: float | None = None, hold: bool | None = None,
               label: str | None = None, color: str | None = None,
               id: int | None = None, **props) -> View:
    """A ``phasescope`` (goniometer) of the stereo pair ``bus`` (left) and
    ``bus + 1`` (right) — the adjacent-channel layout the whole family uses —
    drawn as the 45°-rotated Lissajous figure: vertical is the mid
    ``(L + R)/√2``, horizontal the side ``(L - R)/√2``, the audio-engineering
    convention where mono reads as a vertical line, anti-phase as horizontal
    and a wide field fills the lozenge. An age-faded persistence trail spans
    the last ``window_ms`` of pairs (default 30 ms) and a **correlation**
    read-out (Pearson's r over the window) sits under the field. ``hold``
    freezes the trace. Audio rate only. Reads the segment natively (zero
    messages) and ``/bus_tapStream`` in the browser, like the oscilloscope."""
    extra = _drop_none(window_ms=window_ms, label=label, color=color)
    if hold is not None:
        extra["hold"] = 1 if hold else 0
    return node("signal", bus=bus, view="phase", **extra, **props, id=id)


def spectrum(bus: int = 0, *, channels: int | None = None, fft_size: int | None = None,
             db_floor: float | None = None, db_ceil: float | None = None,
             freq_scale: str | None = None, log_freq: bool | None = None,
             averaging: float | None = None, peak_hold: bool | None = None,
             navigable: bool | None = None,
             view_start: float | None = None, view_len: float | None = None,
             ruler: "bool | str | None" = None, ruler_y: "bool | str | None" = None,
             label: str | None = None, color: str | None = None, axes: dict | None = None, id: int | None = None, **props
             ) -> View:
    """A live ``spectrum`` (spectroscope) over ``channels`` **adjacent**
    audio buses starting at ``bus``: one forward FFT per channel per frame of
    the newest ``fft_size`` window (default 2048), magnitudes in dB over
    ``[db_floor, db_ceil]`` (default ``-100``/``0``), the frequency axis on
    ``freq_scale`` — ``"log"`` (the default), ``"linear"``, ``"mel"`` or
    ``"bark"`` (``log_freq`` is the legacy boolean alias). The channels overlay
    as color-coded curves in one field. ``averaging`` (0..1, default 0.5)
    exponentially smooths each bin so the curve does not flicker; ``peak_hold``
    overlays a slowly decaying peak trace per channel. ``ruler`` (x, in hertz
    on the active scale) and ``ruler_y`` (y, in dB) are shown by default and
    hidden with ``False`` (or ``"off"``). Audio rate only; asking for the bus
    is all a script does, the host has the server record it. The analysis
    reuses the shared-core FFT and Hann window, so it agrees with the
    spectrogram exactly.

    ``navigable`` turns the **frequency axis** into one you can move: drag it
    to pan, wheel over it to zoom under the cursor, ``R`` to see all of it
    again. It needs no history behind it — unlike a live time axis, every bin
    is there every frame — so it is one window the view carries alone, in
    normalized units over ``[0, Nyquist]``: ``view_start``/``view_len``
    (``0, 1`` = the whole axis), live via `GuiHost.set` and reported as a
    ``"view_x"`` event. It is off by default; without it this is the watching
    spectroscope it has always been.
    """
    extra = _drop_none(channels=channels, fft_size=fft_size,
                       db_floor=db_floor, db_ceil=db_ceil,
                       freq_scale=freq_scale, averaging=averaging, label=label, color=color)
    if log_freq is not None:
        extra["log_freq"] = 1 if log_freq else 0
    if peak_hold is not None:
        extra["peak_hold"] = 1 if peak_hold else 0
    if navigable is not None:
        extra["navigable"] = 1 if navigable else 0
    extra.update(_axes(axes, view_start=view_start, view_len=view_len,
                       **_strips(ruler=ruler, ruler_y=ruler_y)))
    return node("signal", bus=bus, view="spectrum", **extra, **props, id=id)


def nodetree(*, group: int = 0, controls: bool | None = None, label: str | None = None,
             color: str | None = None, id: int | None = None, **props) -> View:
    """A live ``nodetree`` view of the audio server's node tree rooted at ``group``
    (default the root group ``0``). The host mirrors the server's tree over its
    client leg (it must be started with ``--server``), refreshing on node
    creation/removal and a low-rate poll, so group/synth changes and ``/node_set``
    edits show live. ``controls`` (default true) shows each synth's control
    name/value pairs. A read-only view."""
    extra = _drop_none(label=label, color=color)
    if controls is not None:
        extra["controls"] = 1 if controls else 0
    return node("nodes", id=id, group=group, **extra, **props)


def bpf(*, points=None, min: float | None = None, max: float | None = None,
        duration: float | None = None, exp: bool | None = None, label: str | None = None,
        color: str | None = None, axes: dict | None = None, id: int | None = None, **props) -> View:
    """A drawable ``bpf`` break-point function — the envelope editor.

    Breakpoints ``(time, value)`` plus a per-segment shape using the server's
    own envelope shape numbers, evaluated host-side through the same shared
    math the server's ``EnvGen`` plays — what you draw is what you hear.
    ``points`` accepts either the flat quad list ``[t, v, shape, curve, ...]``
    (the wire form: shapes int, everything else float) or a list of tuples
    ``(time, value)`` / ``(time, value, shape)`` where ``shape`` is an
    `Env`-style curve spec — a name (``"lin"``, ``"exp"``, ``"sin"``,
    ``"step"``, ``"hold"``, ...) or a numeric curvature. Omitting ``points``
    draws a flat, immediately editable line. See `env_to_points` /
    `points_to_env` for the round trip with `clausters.defs.Env`.

    The widget is general on purpose (the automation-lane shape): values live in
    ``[min, max]`` — unipolar (the ``0``/``1`` default), bipolar, or any
    parameter span; an on/off lane is the ``"hold"`` shape over ``0``/``1``
    (each point's value held until the next point — ``"step"``, per the
    SuperCollider semantics, instead jumps to the *target* level at segment
    start, so a step segment shows the next point's value);
    ``exp=True`` gives frequency-like ranges a geometric display scale
    (requires ``0 < min < max``). Times span ``[0, duration]`` (omitting
    ``duration`` fits the last point).

    Editing (drag a point — times stay monotonic; drag a segment vertically to
    bend its curvature; Ctrl+click adds a point, Ctrl+click on one removes it)
    flows back per the **edit-back pattern**:
    ``/gui_event <id> "points" <t v shape curve ...>`` to the script — or, when
    the widget is bound (`GuiHost.bind` or an inline ``bind``), the flat list
    is forwarded straight to the audio server after the binding's prefix.
    Setting is live too: ``GuiHost.set(id, points=json.dumps(flat))`` replaces
    the whole list (a ``/gui_set`` value is a scalar, so the array rides as its
    JSON string)."""
    extra = _drop_none(points=_flat_points(points) if points is not None else None,
                       duration=duration, label=label, color=color)
    extra.update(_axes(axes, min=min, max=max))
    if exp is not None:
        extra["exp"] = 1 if exp else 0
    return node("curve", id=id, **extra, **props)


def _flat_points(points) -> list:
    """Normalizes a `bpf` ``points`` argument to the flat quad list: a flat
    number list is kept (validated to whole quads, shapes coerced int), tuples
    become ``t, v, shape, curve`` with the shape resolved like an `Env` curve
    spec (default linear)."""
    points = list(points)
    if not points:
        return []
    if not isinstance(points[0], (tuple, list)):
        if len(points) % 4 != 0:
            raise ValueError("a flat points list must be [t, v, shape, curve, ...] quads")
        return [int(x) if i % 4 == 2 else float(x) for i, x in enumerate(points)]
    from ..defs.ugens import _resolve_curve  # lazy: keep guidef import-light

    out: list = []
    for p in points:
        t, v = float(p[0]), float(p[1])
        shape, curve = _resolve_curve(p[2]) if len(p) > 2 else (1, 0.0)
        out += [t, v, int(shape), float(curve)]
    return out


def _flat_notes(notes) -> list:
    """Normalizes a ``notes`` argument to the flat **quintuple** wire form
    ``start dur pitch velocity channel`` (the canonical form the host reads for
    both the ``pianoroll`` and the ``clip`` roll). Each note is a
    ``(start, dur, pitch[, velocity[, channel]])`` tuple; a missing velocity
    defaults to 100, a missing channel to 0. ``velocity``/``channel`` stay ints
    so the JSON keeps them integral (the host distinguishes them from the float
    time/pitch)."""
    out: list = []
    for n in notes:
        n = list(n)
        start, dur, pitch = float(n[0]), float(n[1]), float(n[2])
        velocity = int(n[3]) if len(n) > 3 else 100
        channel = int(n[4]) if len(n) > 4 else 0
        out += [start, dur, pitch, velocity, channel]
    return out


def _flat_osc(osc) -> list:
    """Normalizes an ``osc`` argument to the flat ``time, label`` pairs the host
    reads. Each event is a ``(time, label)`` tuple or a bare ``time`` (no
    label); the label is coerced to a string (``""`` = none)."""
    out: list = []
    for e in osc:
        if isinstance(e, (tuple, list)):
            time = float(e[0])
            label = str(e[1]) if len(e) > 1 and e[1] is not None else ""
        else:
            time, label = float(e), ""
        out += [time, label]
    return out


def plot(*, data=None, blob: int | None = None,
                 path: str | None = None, cache: str | None = None,
                 buffer: int | None = None, channels: int | None = None, view: str | None = None,
                 overlay: bool | None = None, measure: str | None = None,
                 sample_rate: float | None = None,
                 min: float | None = None, max: float | None = None, ruler: str | None = None,
                 ruler_y: str | None = None, fft_size: int | None = None,
                 db_floor: float | None = None, db_ceil: float | None = None,
                 freq_scale: str | None = None, label: str | None = None, color: str | None = None,
                 axes: dict | None = None, id: int | None = None, **props) -> View:
    """A static ``plot`` of a signal — measurement without navigation. Unlike
    the heavy `waveform`, it does not zoom, pan or edit; it is the catalog's
    "plot of an NRT-generated signal/file", grown x/y rulers, multichannel
    lanes and a hover readout. Its samples come from:

    - ``path`` — a file of raw little-endian ``f32`` (see `samples_to_file`, or
      an NRT render written out) the host memory-maps; the **bulk path**, no
      OSC. ``channels`` (default 1) de-interleaves it — **every** channel is
      drawn, as stacked lanes or as ``overlay=True`` per-color traces.
    - ``cache`` — a peaks cache written beside the take, and ``buffer`` — a
      server buffer the host fetches over its own leg. The same two sources the
      `waveform` reads: a plot and a waveform are one element seen with and
      without navigation, so they take the same sources.
    - ``data`` — a small list of floats inline in the JSON;
    - ``blob`` — the index of a binary blob carried beside the JSON (see
      `samples_to_blob` and `GuiHost.define`).

    ``view`` picks the presentation (the set is host-extensible; live via
    ``GuiHost.set``):

    - ``"signal"`` (default) — value against time/index. The whole sequence is
      always drawn (a polyline when it fits the width, a min/max envelope per
      pixel column when it does not — no visual aliasing). The value axis is
      ``[min, max]``; **omit either side and it auto-fits to the data** (the
      arbitrary-range sequence case, e.g. the values of a ``Pwhite``); set a
      side back to auto live with ``GuiHost.set(id, min="auto")``.
    - ``"spectrum"`` — the averaged magnitude spectrum of the (short) signal,
      one curve per channel: dB over ``[db_floor, db_ceil]`` (default
      ``-100``/``0``) against frequency on ``freq_scale`` — ``"log"`` (the
      default), ``"linear"``, ``"mel"`` or ``"bark"`` — analyzed host-side at
      ``fft_size`` (a power of two, default 2048) with the same shared-core
      FFT the spectrogram uses, so the two agree bin for bin.

    The rulers sit in their own strips and are live: ``ruler`` labels the x
    axis — ``"samples"`` (index counts; what an unknown rate falls back to),
    ``"time"`` (the default; clock time when ``sample_rate`` is given) or
    ``"off"`` — and ``ruler_y`` (``"off"`` to hide) labels the value axis (dB
    on the spectrum view). ``sample_rate`` also places the spectral frequency
    axis. Hovering the body shows a hairline plus the exact value under the
    cursor: sample index/time and the **sample's value** on the signal view,
    the bin's frequency (per the scale) and level in dB on the spectrum view.
    """
    extra = _drop_none(data=_samples_arg(data),
                       blob=blob, path=path, cache=cache, buffer=buffer,
                       channels=channels, fft_size=fft_size,
                       db_floor=db_floor, db_ceil=db_ceil, freq_scale=freq_scale,
                       measure=measure, label=label, color=color)
    extra.update(_axes(axes, ruler=ruler, ruler_y=ruler_y, sample_rate=sample_rate,
                       min=min, max=max))
    if overlay is not None:
        extra["overlay"] = 1 if overlay else 0
    # A plot is the trace (or the spectrum) of a signal that does **not**
    # navigate — the capability, not a different element.
    return node("signal", id=id, view=_PLOT_VIEW.get(view or "signal", view),
                navigable=0, **extra, **props)


def score(*, display_list: dict | None = None, playhead: float | None = None,
          playhead_at: float | None = None, playhead_loop_start: float | None = None,
          playhead_loop_len: float | None = None, sample_rate: float | None = None,
          selected: str | None = None, editable: bool | None = None, color: str | None = None,
          id: int | None = None, **props) -> View:
    """An engraved music-notation ``score`` page.

    The host is only the renderer: it fits the engraved page into the widget
    and tessellates every primitive into the same triangle mesh the rest of the
    chrome uses (glyph outlines and engraving fills through a fill tessellator,
    staff lines and stems as thick-line quads), so notation draws through one
    pipeline natively and in the browser.

    ``display_list`` is the semantic engraving the host consumes, a dict with
    ``vb`` (the ``[width, height]`` page-unit viewBox), ``glyphs`` (a SMuFL
    codepoint-to-outline table) and ``prims`` (the placed glyphs, lines and
    fills). Build it from a score with `clausters.gui.notation.engrave`, which
    drives verovio — an optional dependency the host never needs.

    Every primitive carries the MEI ``xml:id`` it was engraved from, and that id
    is what a **click** reports: pressing the page emits an ``"element"`` event
    carrying the id under the cursor (the smallest one, so a notehead wins over
    the staff line it sits on), and an empty id when the press lands on blank
    paper. The clicked element is highlighted; ``selected`` sets or clears that
    highlight from the script (``GuiHost.set(score_id, selected="")`` clears
    it). Since the id is the client's own, a driver resolves it straight back to
    the note in its score — the seam the editing round trip is built on.

    **Editing is opt-in** with ``editable=True``. On an editable score a drag on
    an element moves it up or down the staff in whole diatonic steps, drawn as it
    goes, and the release emits ``"transpose" <xml:id> <position>`` — the staff
    position the note **reaches**, in whole steps from its staff's top line,
    positive upward. It is absolute rather than a displacement, so a resend
    cannot move the note twice and one arriving after the page was re-engraved
    still lands where it says. The host owns
    no score, so that event is a request, not a result — the driver applies it
    (`clausters.gui.notation.Score.transpose_to` takes exactly those two
    arguments)
    and sends the re-engraved page back with
    ``GuiHost.set(score_id, display_list=notation.page_json(dl))``, which
    replaces the drawing in place. The displacement stays drawn until that page
    arrives, so the note never flicks back to its old pitch; the playhead and
    the selection survive it, so the edited note stays selected. **Default (a
    plain view): a drag does nothing** — the host cannot fulfil an edit the
    driver will not apply, so a read-only page must not offer the gesture.
    Selection and the ``"element"`` click are *not* gated by ``editable``:
    inspecting a page (clicking a note to hear it) is not editing it. Toggle it
    live with ``GuiHost.set(score_id, editable=True)``.

    The **playback cursor** rides the display list's ``cursors`` track (the
    engraved timemap: musical time in ms to the placed x of the event sounding
    then), and it is driven exactly like the timeline views':

    - ``playhead_at`` — the engine sample-clock value at score time 0. Set it
      once when a pass starts (``server.request("/clock_query", …)``) and the cursor
      *sweeps* on its own, since the host reads the clock every frame; a
      negative value stops it. ``sample_rate`` converts clock to musical time
      (omitted / ``0`` = the server's own rate).
    - ``playhead`` — a **static** time in ms, for a stopped transport located on
      a note (negative = no cursor). It stands still while ``playhead_at`` is
      off, so a paused cursor does not drift with the clock.
    - ``playhead_loop_start``/``playhead_loop_len`` — a **loop region** in ms:
      the sweep wraps inside it instead of running off the page, so a repeated
      passage keeps the cursor on it. A non-positive length is the straight
      pass.

    All are settable live with ``GuiHost.set(score_id, playhead_at=…)``.
    """
    dl = dict(display_list or {})
    extra = _drop_none(color=color, playhead=playhead, playhead_at=playhead_at,
                       playhead_loop_start=playhead_loop_start,
                       playhead_loop_len=playhead_loop_len,
                       sample_rate=sample_rate, selected=selected,
                       editable=editable, vb=dl.get("vb"), glyphs=dl.get("glyphs"),
                       prims=dl.get("prims"), cursors=dl.get("cursors"),
                       step=dl.get("step"))
    return node("score", id=id, **extra, **props)


def track(*clips, label: str | None = None, height: float | None = None, snap: float | None = None,
          header_w: float | None = None, mute: bool | None = None, solo: bool | None = None,
          level: float | None = None,
          ruler: str | None = None, sample_rate: float | None = None, tempo: float | None = None,
          beat_at: float | None = None, quant: float | None = None,
          playhead_at: float | None = None, playhead: float | None = None,
          playhead_loop_start: float | None = None, playhead_loop_len: float | None = None,
          link: int | None = None, theme: dict | None = None, color: str | None = None,
          axes: dict | None = None, id: int | None = None, **props) -> View:
    """A multitrack ``track`` lane holding `clip` children placed on a shared
    time axis — the DAW-style track editor's lane. ``label`` names it in a left
    header; ``height`` is its lane weight when several tracks stack under one
    window (a ``col`` layout). The window's tracks share one time axis, so a
    clip at a given offset lines up across lanes. ``snap`` is the drag grid in
    timeline samples a clip's move/resize rounds to (omitted / ``0`` = snap to
    whole samples).

    **Ctrl+wheel over a lane resizes it**: the host sets that lane's ``h`` and
    emits ``"height" h`` (logical pixels), which a driver mirrors onto its
    sibling lanes when it wants one thickness for the stack. It is a gesture of
    its own because the bare wheel is the time axis and a scrolled workspace's
    zoom is uniform over both — thickness is neither.

    The lanes of a window **navigate as one**: they share a time axis you can
    zoom (wheel) and pan (Shift+drag), spanning the composition (the longest clip
    end over every lane, so dragging a clip past the end lengthens it). That is
    the same navigation group the heavy views use, so ``link`` joins or splits it
    — pass a shared id to align lanes across *windows*, or a distinct one to give
    a lane an axis of its own. Scripted navigation is ``GuiHost.set(track_id,
    view_start=…, view_len=…)``, and it applies group-wide.

    A lane carries the same time chrome as the heavy editor views:

    - ``ruler`` — a time ruler under the lane (``"time"``, ``"samples"``,
      ``"beats"``, or the default ``"off"``: a lane reserves no ruler strip
      unless asked). ``sample_rate`` labels real time, and ``tempo``/``beat_at``/
      ``quant`` label beats. It is reserved out of *this lane's* height, so a
      stack of lanes is usually ruled by a free-standing `timeruler` under
      them, which costs no lane a pixel.
    - ``playhead_at`` — the engine sample-clock value at timeline position 0, so
      the playhead sweeps the clips as the composition plays (the same anchor the
      `waveform` uses; read the clock with ``Server.request("/clock_query")``). Set it
      live with ``GuiHost.set(track_id, playhead_at=clock)``; a negative value
      (the default) draws no playhead. ``playhead`` parks a static cursor where
      a located, stopped transport sits, and
      ``playhead_loop_start``/``playhead_loop_len`` wrap the sweep inside a
      looped region — all exactly as on the `waveform`.

    **The header** is the band left of the axis, and it is sizeable: it holds
    the ``label`` and, when asked for, the lane's controls. ``mute`` and
    ``solo`` each add a toggle (pass their initial state); ``level`` adds a
    fader over ``[0, 1]``. Working one sends a ``/gui_event`` naming the prop it
    changed — ``"mute" 0|1``, ``"solo" 0|1``, ``"level" f`` — so a driver
    mirrors the edit by echoing it back with ``GuiHost.set``. ``header_w``
    overrides the width outright (logical pixels); without it the header sizes
    itself to what it carries.

    The header width is the **axis'**, not the lane's: every member of a
    navigation group starts its body at the widest gutter any of them asks for,
    so a wide lane header moves the piano-roll and the ruler stacked with it.

    Pass the clips positionally; ``name`` is what a script addresses a lane or a
    clip by::

        track(clip(offset=0, dur=4, data=take_a, name="a"),
              clip(offset=4, dur=2, data=take_b, name="b"),
              label="drums", name="drums")
    """
    extra = _drop_none(label=label, height=height, snap=snap, header_w=header_w,
                       mute=mute, solo=solo, level=level, theme=theme, color=color)
    extra.update(_axes(axes, ruler=ruler, sample_rate=sample_rate, tempo=tempo,
                       beat_at=beat_at, quant=quant, playhead_at=playhead_at,
                       playhead=playhead, playhead_loop_start=playhead_loop_start,
                       playhead_loop_len=playhead_loop_len, link=link))
    return node("field", id=id, children=clips, **extra, **props)


def timeruler(*, h: float = 20.0, ruler: str | None = None, sample_rate: float | None = None,
              tempo: float | None = None, beat_at: float | None = None, quant: float | None = None,
              link: int | None = None, theme: dict | None = None, color: str | None = None,
              axes: dict | None = None, id: int | None = None, **props) -> View:
    """A free-standing **time ruler**: the shared axis drawn as a strip the
    document places — a DAW's ruler above its tracks.

    A `track`'s own ``ruler`` is a strip reserved out of *that lane's* height, so
    ruling a stack of lanes means choosing one to carry it and to pay for it,
    and the strip then sits wherever that lane sits — between two lanes, unless
    it is the last. This widget has a box of its own instead: put it above the
    lanes and no lane loses a pixel.

    It reads the axis of the navigation group named by ``link``, so it labels
    exactly what those lanes show and moves with them. With **no** ``link`` it
    joins the window's lanes on its own — a free-standing ruler exists to rule
    them — so a ruler dropped under a stack needs nothing said; pass a ``link``
    id only to follow a group that is not this window's lanes. ``ruler`` is the unit (``"time"`` the default, ``"samples"``,
    ``"beats"``), with ``sample_rate`` labelling real time and
    ``tempo`` (beats per second), ``beat_at`` and ``quant`` (**beats per bar**, the grid a ``bar:beat`` label counts on — not a length in samples) labelling beats, exactly as on a lane. Its
    ticks are indented by the **group's** gutter — the widest any member asks
    for — so they stand over the samples they label when it is stacked with the
    lanes. (``link`` names a navigation
    group, not a widget: it is its own small namespace, unrelated to the ids the
    host assigns.)

    A press on it **locates** the transport (emitting ``"locate"``, as a lane's
    ruler does), Shift+drag pans the axis and the wheel zooms it — you scrub on
    the ruler. ``h`` is its thickness in logical pixels::

        panel(timeruler(link=1, ruler="beats", tempo=2.0),
              track(clip(offset=0, dur=4, data=take), link=1),
              layout="col")
    """
    extra = _drop_none(theme=theme, color=color)
    extra.update(_axes(axes, ruler=ruler, sample_rate=sample_rate, tempo=tempo,
                       beat_at=beat_at, quant=quant, link=link))
    return node("field", id=id, h=h, **extra, **props)


def clip(*, offset: float = 0.0, dur: float, data=None, blob: int | None = None,
         buffer: int | None = None, path: str | None = None, cache: str | None = None,
         channels: int | None = None, base_bucket: int | None = None, view: str | None = None,
         window_size: int | None = None, hop: int | None = None, db_floor: float | None = None,
         db_ceil: float | None = None, freq_scale: str | None = None,
         colormap: int | None = None, notes=None, points=None,
         exp: bool | None = None, min: float | None = None, max: float | None = None,
         editable: bool | None = None, notes_editable: bool | None = None,
         points_editable: bool | None = None,
         start: float | None = None, loop: bool | None = None,
         fit: bool | None = None, layer: str | None = None, hidden: str | None = None,
         label: str | None = None, color: str | None = None, id: int | None = None, **props
         ) -> View:
    """One ``clip`` on a `track`: a placed rectangle spanning ``[offset, offset +
    dur]`` in timeline sample units (the graphic unit — length = duration). Its
    body is one of three:

    - a **take** — the recorded signal, drawn in the presentation ``view``
      names: ``"trace"`` (the default) decimates it to the clip's pixel width
      through the peak pyramid, ``"spectrogram"`` draws its STFT as the
      time-frequency texture. It is the same element a `signal` view is, so the
      analysis (``window_size``, ``hop``) and the display (``db_floor``,
      ``db_ceil``, ``freq_scale``, ``colormap``) are named the same way — and a
      spectral clip is a clip: placed at ``offset``, ending at ``dur``,
      draggable, resizable, on the lane's own axis. The presentation is read
      when the clip is built (the texture is computed then); the display props
      are live via ``set``;
    - a **piano-roll** — ``notes``, an iterable of ``(start, dur, pitch)`` (or
      ``(start, dur, pitch, velocity, channel)``) events (times relative to the
      clip, in samples; pitch mapped over ``[min, max]``), drawn as note bars —
      the events-track view. The dedicated editor-grade `pianoroll` widget draws
      the same notes with a keyboard and editing; or
    - an **automation curve** — ``points``, break-points over the clip's span
      (the `bpf` editor's break-points and shape math, placed on a lane): times relative
      to the clip in samples, values over ``[min, max]`` (``exp=True`` gives a
      frequency-like range a geometric display scale). It is **editable in
      place** — drag a point, Ctrl+click to add one or remove the one under the
      cursor — and an edit flows back as the same flat ``"points"`` event the
      `bpf` view sends, so an `clausters.seq.Automation` consumes it either way.

    A real take is minutes long, so it never rides the wire as JSON. The
    waveform body reaches the clip exactly the ways the heavy `waveform` view's
    samples do, in the same precedence order:

    - ``cache`` — a prebuilt peak-pyramid file the host maps (see
      `peaks_cache_file`); the most compact bulk path, raw samples never loaded.
    - ``path`` — a file of raw little-endian ``f32`` the host maps (see
      `samples_to_file`); ``channels`` de-interleaves it, ``base_bucket`` sizes
      the pyramid built (and cached) on load. No OSC.
    - ``buffer`` — a server buffer, fetched over the host's client leg.
    - ``data``/``blob`` — a short body inline (a float list, or the index of a
      blob carried beside the JSON — see `samples_to_blob`); it must fit the
      datagram, so keep it to a sketch.

    Whichever the source, the body is summarized to fit the clip rectangle
    through the take's peak pyramid — the same "never resolve finer than the
    screen" rule the editor views follow.

    Other keywords:

    - ``offset`` — the clip's start on the shared timeline (samples; ``>= 0``).
    - ``dur`` — its duration (samples); a clip with no duration draws nothing.
      For an audio take placed 1:1, that is the take's frame count.
    - ``min``/``max`` — the waveform value range, or the low/high pitch of a
      piano-roll (default the bipolar ``-1``/``1``).
    - ``editable`` — whether a hand may edit this clip's **bodies** (default
      true), *all* of them: it is a statement about the clip. Set it false where
      the body draws a *rendering* rather than the thing itself — the notes of a
      pattern, a curve this editor cannot write — and the roll or the curve
      refuses the press instead of offering a drag it will unwind. The refusal
      is visible (a ``"refused"`` event carrying the reason) and it consumes the
      press, so nothing behind it turns a refused edit into a selection. It says
      nothing about the clip's own drag: a read-only body still moves and
      resizes as a placement.
    - ``notes_editable`` / ``points_editable`` — the same answer for **one**
      body, overriding ``editable`` where they are given. A clip whose bodies
      layer needs them: an envelope over a pattern's notes is the ordinary case,
      and there the roll is a rendering that cannot be written while the curve
      over it is the thing itself. It is the split ``min``/``max`` already has
      from ``points_min``/``points_max``, for the same reason — two bodies, one
      props map.

    A clip is a **window onto a segment of its samples**, not a rectangle the
    samples are stretched into:

    - ``start`` — the source frame the clip's own time zero reads (default 0).
      One timeline sample is one source frame, so trimming a clip hides frames
      rather than compressing them, and opening the window again brings them
      back.
    - ``loop`` — whether that window wraps: past the last frame the samples
      begins again, and before its first frame comes its own tail. It is what
      lets an edge be pulled past the samples at all.
    - ``fit`` — draw the samples *fitted* to the clip's span instead, which is
      the picture a time stretch would make and is not one. Off by default.

    **Layers.** A clip draws its bodies over each other and one of them is what
    a hand is editing:

    - ``layer`` — which one: ``"clip"`` (the placement — where it sits, how long
      it is), ``"take"``, ``"notes"`` or ``"points"``. Only the active layer
      acts or shows an affordance, so a clip whose curve is being edited shows
      no grips. A press picks the layer under it and reports the change as a
      ``"layer"`` event.
    - ``hidden`` — the layers that are **not drawn**, space-separated
      (``"notes points"``); an empty string draws them all. What is hidden is
      not edited either.

    Dragging a clip (move) or its edge (trim) flows back as a ``"clip"``
    event carrying the new ``offset``/``dur``/``start`` — the edit-back path —
    so a driver can update the arrangement and re-render. The clip's own edit
    verbs report the same way: ``"split" t`` (cut it at that clip-local time)
    and ``"join" id…`` (read these clips as one)."""
    extra = _drop_none(offset=offset, view=view, window_size=window_size, hop=hop,
                       db_floor=db_floor, db_ceil=db_ceil, freq_scale=freq_scale,
                       colormap=colormap,
                       data=_samples_arg(data),
                       blob=blob, buffer=buffer, path=path, cache=cache,
                       channels=channels, base_bucket=base_bucket,
                       notes=_flat_notes(notes) if notes is not None else None,
                       points=_flat_points(points) if points is not None else None,
                       min=min, max=max, start=start, layer=layer, hidden=hidden,
                       label=label, color=color)
    for key, flag in (("exp", exp), ("editable", editable),
                      ("notes_editable", notes_editable),
                      ("points_editable", points_editable),
                      ("loop", loop), ("fit", fit)):
        if flag is not None:
            extra[key] = 1 if flag else 0
    return node("field", id=id, dur=dur, **extra, **props)


def pianoroll(*, notes=None, osc=None, min: float | None = None, max: float | None = None,
              snap: float | None = None, velocity: bool | None = None,
              osc_lane: bool | None = None, midi_in: bool | None = None, link: int | None = None,
              ruler: str | None = None, sample_rate: float | None = None,
              tempo: float | None = None, beat_at: float | None = None, quant: float | None = None,
              sel_start: float | None = None, sel_len: float | None = None,
              sel_min: float | None = None, sel_max: float | None = None,
              playhead_at: float | None = None, playhead: float | None = None,
              playhead_loop_start: float | None = None, playhead_loop_len: float | None = None,
              y_start: float | None = None, y_len: float | None = None, label: str | None = None,
              color: str | None = None, axes: dict | None = None, id: int | None = None, **props) -> View:
    """The dedicated editor-grade ``pianoroll`` view: a piano keyboard gutter, a
    note grid, an optional velocity lane and an OSC-event lane — the timeline
    sibling of the compact `clip` piano-roll body, drawing the **same notes** with
    the same geometry (they share the host's ``pianoroll`` primitives), plus
    editing, rulers and navigation.

    Content:

    - ``notes`` — an iterable of ``(start, dur, pitch)`` or ``(start, dur, pitch,
      velocity, channel)`` MIDI notes: times in timeline samples, ``pitch`` a MIDI
      note number drawn over the ``[min, max]`` window (default the 88-key range
      21–108), ``velocity`` ``0..127`` (default 100), ``channel`` ``0..15``. The
      notes are the MIDI messages the roll represents.
    - ``osc`` — an iterable of ``(time, label)`` (or bare ``time``) OSC events,
      drawn as flags in a lane below the grid — the OSC messages the roll carries
      alongside the notes.

    Editing (native gestures; the browser keeps display + ``/gui_set`` parity):
    drag a note to move it in time/pitch, drag an edge to resize it, Ctrl+click to
    add a note or remove the one under the cursor; drag in the velocity lane to
    set a note's velocity; Ctrl+click the OSC lane to add/remove an event, drag
    one to move it. ``snap`` is the drag grid in timeline samples (``0`` = whole
    samples). An edit flows back as a flat ``"notes"`` event (``start dur pitch
    velocity channel …``) or ``"osc"`` event (``time label …``) — the edit-back
    pattern — so a driver updates the arrangement and re-renders.

    Navigation and chrome mirror the heavy editor views: it is a timeline widget,
    so ``link`` joins/splits its navigation group (zoom with the wheel over the
    grid, pan with Shift+drag, all group-wide); ``ruler`` places a time ruler
    (``"time"``/``"samples"``/``"beats"``, default ``"time"``) with
    ``sample_rate``/``tempo`` (beats per second), ``beat_at`` and ``quant`` (**beats per bar**, the grid a ``bar:beat`` label counts on — not a length in samples) labelling it; ``sel_start``/
    ``sel_len`` mark a time selection and ``sel_min``/``sel_max`` restrict it to
    a band of **pitches** — the roll's own y axis, so a marquee reports the
    whole semitones it swept over at both ends; ``playhead_at`` sweeps a playhead from the
    engine clock (``playhead`` sets a static cursor, and
    ``playhead_loop_start``/``playhead_loop_len`` wrap the sweep inside a
    region); ``y_start``/``y_len`` are the
    vertical pitch window (normalized ``0..1`` over ``[min, max]``) for pitch
    zoom/pan. ``velocity=False`` hides the velocity lane; ``osc_lane=True`` opens
    the OSC lane even with no events (to author them). ``midi_in=True`` arms
    **live MIDI painting** in the native host: it opens a virtual MIDI input
    port ("clausters-gui") and paints incoming notes into this roll — at the
    running playhead, or step-entering on the ``snap`` grid when the transport
    is stopped — flowing back as the usual ``"notes"`` events (the standalone
    host's live input; a script can equally paint via a `clausters.responders.
    MidiFunc` and ``/gui_set``)."""
    extra = _drop_none(
        notes=_flat_notes(notes) if notes is not None else None,
        osc=_flat_osc(osc) if osc is not None else None,
        snap=snap, label=label, color=color)
    extra.update(_axes(
        axes, min=min, max=max, link=link, ruler=ruler,
        sample_rate=sample_rate, tempo=tempo, beat_at=beat_at, quant=quant,
        sel_start=sel_start, sel_len=sel_len,
        sel_min=sel_min, sel_max=sel_max, playhead_at=playhead_at,
        playhead=playhead, playhead_loop_start=playhead_loop_start,
        playhead_loop_len=playhead_loop_len,
        y_start=y_start, y_len=y_len))
    if velocity is not None:
        extra["velocity"] = 1 if velocity else 0
    if osc_lane is not None:
        extra["osc_lane"] = 1 if osc_lane else 0
    if midi_in is not None:
        extra["midi_in"] = 1 if midi_in else 0
    return node("notes", id=id, **extra, **props)


def piano(*, min: int | None = None, max: int | None = None, active_min: int | None = None,
          active_max: int | None = None, pan: bool | None = None, overview: bool | None = None,
          velocity: int | None = None, channel: int | None = None, voice: str | None = None,
          voice_args=None, label: str | None = None, color: str | None = None,
          id: int | None = None, **props) -> View:
    """The playable ``piano`` virtual keyboard: keys laid out with real piano
    proportions (equal white keys, narrower/shorter black keys distributed as on
    the physical instrument), resizing freely with the widget.

    Range and navigation:

    - ``min``/``max`` — the visible MIDI range (default 36–96; ``min`` snaps
      down to a white key so the keyboard starts on a full key).
    - ``overview`` — the strip above the keys showing the full 0–127 range with
      the visible window marked: drag it to pan, wheel over it to zoom (wheel
      over the keys pans by white keys). On by default.
    - ``pan`` — set ``False`` to disable all range navigation (the keyboard
      stays fixed on ``min``/``max``). Range changes flow back as a
      ``"range" min max`` event and are settable via ``set`` (the browser's
      path).
    - ``active_min``/``active_max`` — the mapped range: keys outside it draw
      grayed and are inert, visualizing what the instrument answers to.

    Playing emits **MIDI-shaped** note events — ``"note" pitch velocity state
    channel`` (ints; ``state`` 1 on press, 0 on release), so a consumer can
    translate them 1:1 to MIDI note-on/note-off. Dragging across keys
    glissandos (off + on). ``velocity`` fixes the press velocity; unset, it
    maps from the press height (striking nearer the front edge plays louder).
    ``channel`` (0–15, default 0) rides in every event.

    Mapping to server instruments, two ways:

    - **Programmed (events)** — leave the widget unbound and map ``"note"``
      events to voices in the script: ``state 1`` spawns a synth (`clausters.defs.Synth`
      with ``freq``/``amp`` from pitch/velocity), ``state 0`` sends its
      ``gate=0``. Fully programmable, like driving any GuiDef.
    - **Host voices** — set ``voice`` to a SynthDef name and the *host* manages
      one server voice per held key: ``/synth_new <voice> … freq <hz> amp <vel/127>
      gate 1`` on press, ``gate 0`` on release. The def must have
      ``freq``/``amp``/``gate`` controls and free itself on release (an
      ``Env.adsr`` with ``FREE_SELF``). ``voice_args`` is an iterable of extra
      ``(name, value)`` control pairs for the ``/synth_new``. This path needs no
      script in the loop — a saved GuiDef bundle plays standalone."""
    extra = _drop_none(min=min, max=max, active_min=active_min,
                       active_max=active_max, velocity=velocity,
                       channel=channel, voice=voice,
                       voice_args=[x for kv in voice_args
                                   for x in (str(kv[0]), _value(kv[1]))]
                       if voice_args is not None else None,
                       label=label, color=color)
    if pan is not None:
        extra["pan"] = 1 if pan else 0
    if overview is not None:
        extra["overview"] = 1 if overview else 0
    return node("keys", id=id, **extra, **props)


def patch(*, boxes=None, cords=None, label: str | None = None, color: str | None = None,
          id: int | None = None, **props) -> View:
    """A ``patch`` **patcher**: a directed, typed signal graph (a level-1
    `clausters.defs.GraphPatch`, compiling to a `clausters.defs.GraphDef`), drawn
    as boxes with **inlets on top and outlets on the bottom** and a **cord** per
    ``outlet -> inlet`` connection. The buses are not drawn — a cord *is* a bus.

    ``boxes`` and ``cords`` are the widget's split schema, exactly what
    `GraphPatch.to_widget` produces — pass it straight through (the model is
    conventionally ``p`` so it does not shadow this ``patch`` builder):

        patch(**p.to_widget(geometry), name="patch")

    - ``boxes`` — each ``{"def": name, "inlets": [...], "outlets": [...],
      "x"?, "y"?}``; a port is a bare name (audio) or ``{"name", "rate"}``
      (control), and ``x``/``y`` place the box (absent, it auto-stacks).
    - ``cords`` — a flat ``[from_box, outlet, to_box, inlet, ...]`` list, the
      indices within each box's inlet/outlet lists.

    A box is drawn as three bands: a top strip of **inlet cells** and a bottom
    strip of **outlet cells** (both green, ``port_strip``), the def name in the
    wider middle (blue, ``object_fill``). Each port is a labelled square holding
    its name — the square a cord connects to — so a box reads like its signal flow
    (an edge with no ports keeps its strip, empty). The band, port, and cord colors
    are theme roles (``port_strip``, ``port``, ``object_fill``, ``cord``),
    configurable per widget through the ``theme`` prop like any other color.

    The patch is a **pan/zoom canvas**, so put it in a `scroll` workspace: a plain
    drag on empty canvas sweeps the marquee box-selection, and **Shift+drag pans**
    (wheel zooms, anchored at the cursor) — the heavy-view convention. A box drags
    freely (moving a selected box moves the whole selection), each move flowing
    back as ``/gui_event <id> "move" <index> <x> <y>`` (canvas units) so the
    driver owns the geometry.

    Dragging an outlet onto an inlet (either grab order) **draws a cord**,
    refusing a rate mismatch; the edit flows back as ``/gui_event <id> "wire"
    <src_box> <outlet> <dst_box> <inlet>`` (the ports by name), so a driver adds
    the cord to its `GraphPatch` and re-renders — the clips' edit-back pattern.
    """
    extra = _drop_none(
        boxes=list(boxes) if boxes is not None else None,
        cords=[int(x) for x in cords] if cords is not None else None,
        label=label, color=color)
    return node("plane", id=id, **extra, **props)


def canvas(shader: str | None = None, *, params=None, buses=None, label: str | None = None,
           color: str | None = None, id: int | None = None, **props) -> View:
    """A ``canvas`` running a script-supplied WGSL shader over the widget area --
    custom visuals (ShaderToy-style).

    ``shader`` is the body of a ``shade`` function the host wraps and runs::

        fn shade(uv: vec2<f32>, frag: vec4<f32>) -> vec4<f32> { ... }

    Inside it, the host exposes ``u.resolution`` (the viewport size in px),
    ``u.time`` (seconds), and ``u.params`` (a ``vec4<f32>`` of four values). The
    four params are driven two ways, which is the point of the widget:

    - from the **script** -- ``GuiHost.set(id, param0=...)`` sends an OSC value
      that lands in ``u.params.x`` (``param0``..``param3`` -> ``.x``..``.w``);
    - from a **control bus**, read straight from the audio server's shared memory
      each frame (zero messages) -- ``buses=[busA, busB, ...]`` maps each control
      bus onto the param of the same index; a ``-1`` (or absent) slot stays
      script-driven. Needs the host started with ``--shm`` (like `meter`).

    So a shader can animate from OSC parameters and from live server audio at
    once. Omitting ``shader`` uses a default moving color field. ``params`` is an
    optional initial list of floats."""
    extra = _drop_none(shader=shader, label=label,
                       params=[_value(x) for x in params] if params is not None else None,
                       buses=[_value(b, int) for b in buses] if buses is not None else None,
                       color=color)
    return node("canvas", id=id, **extra, **props)


#: The model's names for the four elements the catalog named after the thing
#: they show rather than for what they are. The same builder under both names,
#: since the rename is the whole of the difference: a piano-roll is the
#: **notes** element, a break-point envelope is a **curve**, the server's graph
#: is **nodes** and a keyboard is **keys**.
notes = pianoroll
curve = bpf
nodes = nodetree
keys = piano


def to_json(tree: dict) -> str:
    """Serializes a GuiDef tree to the JSON string carried in ``/gui_def``.

    The client-only ``name`` key (a stable handle name — see
    `clausters.gui.host.GuiHost.open`) is stripped from every node: it labels
    the widget for the host client's ``name -> handle`` map and never rides the
    wire."""
    return json.dumps(_strip_names(tree))


def _strip_names(node: dict) -> dict:
    """A shallow copy of ``node`` (and its subtree) without the client-only
    ``name`` key — so serialization never leaks it to the host, whether or not
    the tree went through `clausters.gui.host.GuiHost`'s id/name walk."""
    out = {k: v for k, v in node.items() if k != "name"}
    children = node.get("children")
    if children:
        out["children"] = [_strip_names(c) for c in children]
    return out


#: Packs an iterable of floats into a little-endian ``f32`` blob, the bulk form
#: a ``waveform`` reads via ``blob``. Re-exported from `clausters.base.bulk`,
#: which is where the convention lives: every path carrying samples uses the
#: same pack, so a ``waveform``'s blob and a ``/buffer_setRange`` run cannot
#: disagree about byte order.
samples_to_blob = _samples_to_blob


def samples_to_file(samples, path: str) -> str:
    """Writes `samples` to `path` as raw little-endian ``f32`` — the **local
    shared resource** a ``waveform(path=...)`` maps. Unlike `samples_to_blob`
    (which rides the ``/gui_def`` message and so must fit a datagram), a file has
    no size limit: this is how a multi-megabyte buffer reaches the host without
    OSC. Returns `path`."""
    with open(path, "wb") as f:
        f.write(samples_to_blob(samples))
    return path


def peaks_cache_file(samples, path: str, base_bucket: int = 256, channels: int = 1) -> str:
    """Builds the peak-pyramid cache for `samples` (via the shared native core,
    so it is byte-identical to the host's own) and writes it to `path` — the most
    compact bulk path, mapped by a ``waveform(cache=...)``. The host renders the
    overview without ever loading the raw samples. With ``channels > 1`` the
    samples are interleaved frames and the file is the **multichannel** cache
    (one resource, a pyramid per channel — the editor-grade stacked lanes).
    Returns `path`."""
    from .._native import peaks_cache  # lazy: only needs the cdylib if used

    with open(path, "wb") as f:
        f.write(peaks_cache(samples, base_bucket, channels))
    return path


def peaks_cache_update_file(path: str, samples, start: int, frames: int) -> str:
    """Rewrites the part of the cache at `path` that a **frame span** touched,
    from `samples` (the whole interleaved buffer as it now stands) — the owner's
    half of an edit, so the picture that maps this file follows without the
    take being re-summarized.

    Returns `path`. Raises `ValueError` when the samples are not the buffer the
    cache describes, which is a length change and therefore a rebuild
    (`peaks_cache_file`)."""
    from .._native import peaks_cache_update  # lazy: only needs the cdylib if used

    with open(path, "rb") as f:
        cache = f.read()
    updated = peaks_cache_update(cache, samples, start, frames)
    with open(path, "wb") as f:
        f.write(updated)
    return path


def peaks_cache_empty_file(path: str, frames: int, channels: int = 1,
                           base_bucket: int = 256) -> str:
    """Writes the cache of a take **allocated and not yet recorded into** to
    `path` — `frames` frames of `channels` channels, every bucket a measured
    zero. The file a ``waveform(cache=...)`` maps before the recording starts,
    grown from there by `peaks_cache_stream_file`.

    Returns `path`."""
    from .._native import peaks_cache_empty  # lazy: only needs the cdylib if used

    with open(path, "wb") as f:
        f.write(peaks_cache_empty(frames, channels, base_bucket))
    return path


def peaks_cache_stream_file(path: str, start_frame: int, bucket: int, stats) -> str:
    """Folds a ``/buffer_stream.reply`` report into the cache at `path` — the
    listener's half of a recording being drawn, for a client that hears about
    the samples rather than mapping it.

    `stats` is the reply's blob read as floats, **bucket-major and
    channel-minor**: for each bucket of `bucket` frames in order, for each
    channel, ``min``, ``max`` and mean square. `start_frame` is where the report
    begins on the buffer's own sample axis. Nothing is measured here — the
    writer measured — so a picture mapping this file grows as the take does,
    at about 2 kB/s per channel against the 190 the audio would cost.

    Returns `path`. Raises `ValueError` when the report is on another grid than
    the cache (a different bucket size, a start off a bucket boundary, or a run
    that does not fit)."""
    from .._native import peaks_cache_write_buckets  # lazy: only needs the cdylib if used

    with open(path, "rb") as f:
        cache = f.read()
    updated = peaks_cache_write_buckets(cache, start_frame, bucket, stats)
    with open(path, "wb") as f:
        f.write(updated)
    return path


def correlation(left, right) -> float | None:
    """The stereo **correlation** (Pearson's r) of two equal-length channels,
    in ``[-1, 1]`` — ``+1`` mono/in-phase, ``0`` decorrelated, ``-1`` anti-phase
    — via the shared native core, so a headless capture reads the identical
    number the GUI phasescope draws. ``None`` when it is undefined (empty input
    or a constant channel: silence/DC). Pair it with ``Server.stream_taps`` to
    measure a live stereo signal without the GUI."""
    from .._native import correlation as _correlation  # lazy: needs the cdylib

    return _correlation(left, right)


def lissajous(left, right) -> list:
    """The **Lissajous / goniometer** coordinates of stereo pairs ``(left,
    right)``: each maps to ``(x, y)`` with ``x`` the side ``(L - R)/√2`` and
    ``y`` the mid ``(L + R)/√2`` — the rotated stereo plane a goniometer draws.
    The geometry lives once in the shared native core (the phasescope draws the
    same points); useful for plotting or driving a stereo image in
    electroacoustic work. Returns a list of ``(x, y)`` tuples."""
    from .._native import lissajous as _lissajous  # lazy: needs the cdylib

    return _lissajous(left, right)


def _value(x, cast=float):
    """``x`` coerced with ``cast``, unless it is a **bundle placeholder** —
    ``"@symbol"`` (an id the mount allocates) or ``"$param"`` (a value the tag
    supplies), which passes through untouched for the mount to fill.

    A hole is a legal value wherever a value goes; see `clausters.bundle`.
    """
    if isinstance(x, str) and x[:1] in ("@", "$"):
        return x
    return cast(x)


#: A `plot`'s ``view`` keyword as the model's presentation name. The static
#: plot spelled the same choice its own way, which is the clearest single sign
#: that the six view names were points of one product all along.
_PLOT_VIEW = {"signal": "trace", "spectrum": "spectrum"}


def _strips(**strips) -> dict:
    """The live views' ``ruler``/``ruler_y`` keywords, which accept a bool as
    well as a unit name (their x unit was never selectable, so only on/off was
    ever meaningful there), as the values an axis takes."""
    return {k: (v if isinstance(v, str) else (1 if v else "off"))
            for k, v in strips.items() if v is not None}


def _drop_none(**kwargs) -> dict:
    """Keeps only the keyword arguments that were actually given."""
    return {k: v for k, v in kwargs.items() if v is not None}


#: The axis each chrome keyword belongs to, and what it is called **on** that
#: axis: a property drops the axis marker once it is inside one, so the
#: keyword ``view_start`` is ``axes["x"]["start"]`` and ``ruler_y`` is
#: ``axes["y"]["unit"]``. Read by `_axes`, which is what every builder with an
#: axis pair packs its flat keywords through.
_X_AXIS = {
    "ruler": "unit", "view_start": "start", "view_len": "len",
    "tempo": "tempo", "beat_at": "beat_at", "quant": "quant",
    "sample_rate": "sample_rate", "link": "link",
    "sel_start": "sel_start", "sel_len": "sel_len",
    "playhead": "playhead", "playhead_at": "playhead_at",
    "playhead_loop_start": "playhead_loop_start",
    "playhead_loop_len": "playhead_loop_len",
}
_Y_AXIS = {
    "ruler_y": "unit", "y_start": "start", "y_len": "len",
    "min": "min", "max": "max", "bit_depth": "bit_depth",
    "sel_min": "sel_min", "sel_max": "sel_max",
}


def _axes(_axes_given: dict | None = None, **flat) -> dict:
    """The ``axes`` prop the flat chrome keywords describe, or ``{}`` for none.

    The ruler, the navigation window, the selection, the playhead and the value
    range describe the **container's axes** rather than each element drawn
    against them, so they ride nested. The builders still take them flat —
    that is the shorthand a script types — and this is where the two meet.
    ``_axes_given`` is an explicit ``axes`` argument, which wins over the flat
    keywords for any axis it names.
    """
    out = {}
    for axis, table in (("x", _X_AXIS), ("y", _Y_AXIS)):
        side = {name: flat[key] for key, name in table.items()
                if flat.get(key) is not None}
        if side:
            out[axis] = side
    for axis, side in (_axes_given or {}).items():
        out[axis] = {**out.get(axis, {}), **side}
    return {"axes": out} if out else {}
