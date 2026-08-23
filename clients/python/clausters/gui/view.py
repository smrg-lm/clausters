"""`View`: a GUI node as an object — the AST a client builds and then opens.

A `clausters.gui.guidef` builder used to return a bare ``dict``, which is why
the *host* had to be the subject of the sentence (``host.open(tree)``) while
every other resource is its own subject (``synthdef.send(server)``,
``clausters.plot(obj)``). A `View` closes that asymmetry: it is the GUI's
counterpart of a `clausters.defs.SynthDef` — a tree a program composes and
sends, not a live widget. The live widget is what `View.open` gives back.

`View` **is a ``dict``**, so the document it serializes is byte-identical to the
one the builders always produced and nothing on the wire, in the host or in
`to_json` changes. What it adds is behaviour: name lookup over the tree it
carries, `to_json`, and `open`.

**A name is the client's index, not an id.** The host never reads a widget's
``name`` — `clausters.gui.guidef.to_json` strips it before the document goes out
— so ``view.find("cutoff")`` and ``win["cutoff"]`` are tables this client builds
by walking the tree. Two rules follow, and both are enforced here:

- **A duplicate name in one view is an error**, raised while the tree is being
  built rather than silently resolving to whichever widget came last. A shadowed
  widget still draws, so a silent last-wins leaves something on screen that no
  name reaches.
- **A nested view scopes its names.** A ``window``-typed node inside another
  tree is a sub-view: its names stay its own, so ``osc1`` and ``osc2`` can both
  hold a ``freq`` and neither reaches out. The sub-view is found by *its* name,
  and the lookup continues inside it (``v.find("osc1").find("freq")``).
"""

__all__ = ["View"]

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

    __slots__ = ("_scope",)

    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
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
