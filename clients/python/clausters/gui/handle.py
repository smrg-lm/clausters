"""Widget handles: operate a live widget as an object, never by its integer id.

`clausters.gui.host.GuiHost.open`/`define` hand back a `WindowHandle` — the
window's own id (it *is* an ``int``, so it drops into every place a window id
went before) that additionally indexes the tree's **named** widgets. A lookup
returns a `WidgetHandle`, a thin façade whose ``set``/``bind``/``free``/
``query``/``on_event`` delegate to the host with the resolved id — the same way
`clausters.defs.node.Node.free` delegates to its `Server`. So a script holds the
widget and acts on it (``win["cutoff"].set(value=800.0)``) instead of tracking
integers and matching them in an event loop.

A name is stable; the assigned id is not (it recycles across redraws — see
`clausters.gui.ids`), which is exactly why the handle addresses by name and the
host resolves the current id underneath it.
"""

from typing import TYPE_CHECKING

if TYPE_CHECKING:                       # the host imports this module, not the
    from .host import GuiHost           # other way round: a name for typing only

__all__ = ["WidgetHandle", "WindowHandle"]


class WidgetHandle:
    """A live widget, addressed by its id but operated as an object.

    Every method delegates to the `clausters.gui.host.GuiHost` that produced the
    handle, so the widget's mutation surface (`set`, `bind`/`unbind`, `free`,
    `query`) and its event stream (`on_event`) read as methods on the thing
    itself. The mutating methods return ``self`` for chaining.
    """

    __slots__ = ("_host", "id")

    #: the host every method delegates to. Declared, not just assigned, so the
    #: delegation type-checks: these handles are the one place the host's whole
    #: surface is reached through an attribute rather than a parameter.
    _host: "GuiHost"

    def __init__(self, host, wid: int):
        self._host = host
        #: the widget's current id on the host (an implementation handle; prefer
        #: the name you looked it up by, which survives a redraw).
        self.id = wid

    def set(self, **props) -> "WidgetHandle":
        """``/gui_set`` this widget's properties (see `GuiHost.set`)."""
        self._host.set(self.id, **props)
        return self

    def bind(self, address: str, *prefix) -> "WidgetHandle":
        """Forward this widget's value straight to the audio server (see
        `GuiHost.bind`)."""
        self._host.bind(self.id, address, *prefix)
        return self

    def bind_widget(self, target, prop: str) -> "WidgetHandle":
        """Apply this widget's value to another widget's property (see
        `GuiHost.bind_widget`). ``target`` may be a handle or an id."""
        self._host.bind_widget(self.id, int(target), prop)
        return self

    def unbind(self) -> "WidgetHandle":
        """Remove this widget's binding (see `GuiHost.unbind`)."""
        self._host.unbind(self.id)
        return self

    def free(self):
        """Free this widget and its subtree (see `GuiHost.free`)."""
        self._host.free(self.id)

    def query(self, timeout: float = 1.0):
        """Round-trip this widget's state (see `GuiHost.query`)."""
        return self._host.query(self.id, timeout)

    def on_event(self, func) -> "WidgetHandle":
        """Call ``func(*payload)`` whenever this widget emits a ``/gui_event``,
        when the host's inbound messages are drained through `GuiHost.pump`. The
        payload is the event's arguments after the id (a control's value, or a
        view's ``tag`` followed by its flat values). Passing ``None`` clears the
        handler."""
        self._host._set_event_handler(self.id, func)
        return self

    def __int__(self) -> int:
        return self.id

    def __repr__(self) -> str:
        return f"WidgetHandle(id={self.id})"


class WindowHandle(int):
    """An open window's id that also resolves the tree's **named** widgets.

    It subclasses ``int`` and equals the window id, so it works everywhere a
    window id did (``host.free(win)``, a set of open windows, an
    ``int(args[0]) == win`` check). Indexing it by a widget's ``name`` — the
    unambiguous ``win["cutoff"]`` or the shorthand ``win.cutoff`` — returns a
    `WidgetHandle`. It carries the window's own widget ops too (`set`, `close`,
    `free`, `on_closed`).
    """

    _host: "GuiHost"
    #: widget name -> its current id, refreshed on every redraw.
    _names: "dict[str, int]"

    def __new__(cls, host, wid: int, names: dict):
        obj = super().__new__(cls, wid)
        obj._host = host
        obj._names = names  # name -> current id
        return obj

    def __getitem__(self, name: str) -> WidgetHandle:
        try:
            wid = self._names[name]
        except KeyError:
            raise KeyError(
                f"no widget named {name!r} in this window "
                f"(names: {sorted(self._names)})") from None
        return WidgetHandle(self._host, wid)

    def __getattr__(self, name: str) -> WidgetHandle:
        # Only reached when normal attribute lookup fails, so real attributes
        # (and int's own methods) are untouched; a name that shadows one of
        # those is still reachable through the subscript form.
        names = object.__getattribute__(self, "_names")
        if name in names:
            return WidgetHandle(self._host, names[name])
        raise AttributeError(name)

    def __contains__(self, name: str) -> bool:
        return name in self._names

    def names(self) -> list:
        """The names bound in this window, sorted."""
        return sorted(self._names)

    def widget(self, name: str) -> WidgetHandle:
        """The `WidgetHandle` for ``name`` — the method form of ``self[name]``."""
        return self[name]

    def handle(self) -> WidgetHandle:
        """A `WidgetHandle` for the window root itself (to `set` its props)."""
        return WidgetHandle(self._host, int(self))

    def set(self, **props) -> "WindowHandle":
        """``/gui_set`` the window root's own properties."""
        self._host.set(int(self), **props)
        return self

    def close(self):
        """Close this window (see `GuiHost.close`)."""
        self._host.close(int(self))

    def free(self):
        """Free this window's subtree (see `GuiHost.free`)."""
        self._host.free(int(self))

    def on_closed(self, func) -> "WindowHandle":
        """Call ``func()`` when the user closes this window (a ``/gui_closed``),
        when inbound messages are drained through `GuiHost.pump`. ``None``
        clears it."""
        self._host._set_closed_handler(int(self), func)
        return self

    def __repr__(self) -> str:
        return f"WindowHandle(id={int(self)}, names={sorted(self._names)})"
