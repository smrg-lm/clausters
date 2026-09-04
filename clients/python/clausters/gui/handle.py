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

    def focus(self, on: bool = True) -> "WidgetHandle":
        """Point the keyboard at this widget (see `GuiHost.focus`)."""
        self._host.focus(self.id, on)
        return self

    def free(self):
        """Free this widget and its subtree (see `GuiHost.free`)."""
        self._host.free(self.id)

    def query(self, timeout: float = 1.0):
        """Round-trip this widget's state (see `GuiHost.query`)."""
        return self._host.query(self.id, timeout)

    def on_event(self, func) -> "WidgetHandle":
        """Call ``func(*payload)`` whenever this widget emits a ``/gui_event``.
        The payload is the event's arguments after the id (a control's value, or
        a view's ``tag`` followed by its flat values). Passing ``None`` clears
        the handler.

        It fires on the host's own event-loop thread, which an open window
        starts, so nothing in the script has to drive it.

        This is the **raw** stream and sees everything, the interface events of
        `on_press`/`on_release`/`on_click` included -- those arrive here as the
        one-string payload they are. Registering both is legal and useful: a
        button's value is what it drives, its click is what it commands."""
        self._host._set_event_handler(self.id, func)
        return self

    def on_press(self, func) -> "WidgetHandle":
        """Call ``func()`` when the pointer goes **down** on this widget.

        The first of the two primitives, and the one an instrument wants: the
        note is at the down-stroke. ``None`` clears the handler."""
        self._host._set_interface_handler(self.id, "press", func)
        return self

    def on_release(self, func) -> "WidgetHandle":
        """Call ``func()`` when the pointer comes **up** after a press on this
        widget -- wherever it came up, on the widget or off it.

        The other primitive. It fires for an abandoned press too, which is what
        makes it the release rather than the click. ``None`` clears the
        handler."""
        self._host._set_interface_handler(self.id, "release", func)
        return self

    def on_click(self, func) -> "WidgetHandle":
        """Call ``func()`` when a press on this widget is **completed**: the
        pointer came up while still on it.

        The composed gesture, and what a command button wants -- press, slide
        off, release, and nothing happens, which is the cancellation every
        desktop convention gives an "Accept" and a piano key must not have.
        ``None`` clears the handler.

        It reaches the script whether or not the widget is **bound**: a binding
        forwards a widget's *value* to the audio server, and a command is not a
        value. So one button can drive a synth's gate and run a script's action
        at once."""
        self._host._set_interface_handler(self.id, "click", func)
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
    #: widget id -> the def control that widget was built from, for `bind`.
    _controls: "dict[int, str]"

    def __new__(cls, host, wid: int, names: dict, controls: "dict | None" = None):
        obj = super().__new__(cls, wid)
        obj._host = host
        obj._names = names  # name -> current id
        obj._controls = dict(controls or {})
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

    def bind(self, node, *, address: str = "/node_set") -> "WindowHandle":
        """Wire every widget built from a def control straight to ``node``.

        The counterpart of `clausters.gui.guidef.knob` taking a control: the
        widget already knows which control it draws, so the whole surface is one
        verb instead of one hand-typed string per widget::

            w = view(knob(freq), slider(amp)).open()
            w.bind(synth)

        Each becomes a ``/gui_bind`` forwarding ``address <node> <control>
        <value>`` — the host talks to the audio server itself, with no
        round-trip through this script (see `clausters.gui.host.GuiHost.bind`,
        which is still there for anything that is not a def control: a bus, an
        arbitrary address, another widget).

        **Two widgets on one control both bind**, both set the node, and neither
        is told when the other moves; the host fires an apply rather than a
        second binding, so they settle rather than cascade. That drift is yours
        to make and is not detected.

        ``node`` is a `clausters.defs.node.Node` (a `Synth`, a `Group`, a
        GraphDef instance) or a bare node id.

        Raises `ValueError` when no widget in this window was built from a
        control, which can only be a mistake.
        """
        if not self._controls:
            raise ValueError(
                "no widget in this window was built from a def control, so "
                "there is nothing to bind — build them from controls "
                "(knob(freq), slider(sd['amp'])), or bind one at a time with "
                "win['freq'].bind('/node_set', node, 'freq')")
        target = int(getattr(node, "id", node))
        for wid, control in self._controls.items():
            self._host.bind(wid, address, target, control)
        return self

    def unbind(self) -> "WindowHandle":
        """Undo `bind`: every control widget emits ``/gui_event`` to this script
        again."""
        for wid in self._controls:
            self._host.unbind(wid)
        return self

    @property
    def controls(self) -> dict:
        """``widget name -> def control name`` for every widget in this window
        built from a control — what `bind` wires."""
        by_id = {wid: name for name, wid in self._names.items()}
        return {by_id.get(wid, control): control
                for wid, control in self._controls.items()}

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

    @property
    def closed(self) -> bool:
        """Whether this window is gone — closed by a hand or by `close`.

        It reads the host's set of open windows, which a ``/gui_closed``
        updates, so it answers as soon as the loop has delivered the close."""
        return int(self) not in self._host._open

    def wait(self, timeout: "float | None" = None) -> bool:
        """Hold the calling thread until this window is closed.

        `clausters.gui.host.GuiHost.wait` for one window rather than all of
        them — what a script that opened exactly one ends with. ``True`` when it
        closed, ``False`` when ``timeout`` ran out first."""
        return self._host._wait_while(lambda: not self.closed, timeout)

    def on_closed(self, func) -> "WindowHandle":
        """Call ``func()`` when the user closes this window (a ``/gui_closed``).
        It fires on the host's event-loop thread, like `WidgetHandle.on_event`;
        ``None`` clears it. To simply hold a script open until then, `wait`."""
        self._host._set_closed_handler(int(self), func)
        return self

    def __repr__(self) -> str:
        return f"WindowHandle(id={int(self)}, names={sorted(self._names)})"
