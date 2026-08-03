"""Making a window draw when it is displayed, without the core knowing.

`plot` returns a `clausters.plot.PlotWindow`, `scope` a `ScopeWindow`, a GuiDef
opened by hand a `clausters.gui.WindowHandle`. In a notebook, showing one of
those should show the window. The obvious way to arrange that is a
``_repr_mimebundle_`` on each class — and it is the one way this package will
not use, because it would put display logic in `clausters`, which is exactly
what this package exists to avoid.

IPython's own answer is the third-party one: `for_type` registers a formatter
for a class from outside it. The class stays unaware, the registration is
undone by unloading this package, and a `clausters` used from a plain script
carries nothing.

**A widget is made when a window is displayed, not when it is opened.** So a
`plot` whose result is not the cell's last expression draws nothing, exactly as
any other object in a notebook shows nothing until it is shown — and a window
opened in a loop does not fill the output with canvases nobody asked for. The
window is displayable later, from another cell, because the tree is in the
journal until then (`clausters_jupyter.bridge`).
"""

__all__ = ["register", "unregister"]

#: (formatter, type) pairs this module registered, for `unregister`.
_registered: list = []


def _window_id(obj) -> int:
    """The window id behind whatever the verbs return.

    `clausters.gui.WindowHandle` *is* the id (it subclasses ``int``); the verb
    wrappers hold it as ``id``.
    """
    return int(getattr(obj, "id", obj))


def register(bridge):
    """Have the window-returning types display as their cell's canvas.

    Called by `clausters_jupyter.notebook`; safe to call outside IPython, where
    it does nothing.
    """
    try:
        from IPython import get_ipython
    except ImportError:
        return
    shell = get_ipython()
    if shell is None:
        return

    from clausters.gui import WindowHandle
    from clausters.plot import PatchWindow, PlotWindow
    from clausters.scope import ScopeWindow

    formatter = shell.display_formatter.mimebundle_formatter

    def show(obj, include=None, exclude=None):
        """The window formats *as* its widget: the widget's own mimebundle.

        The mimebundle formatter, not ``ipython_display_formatter``. That one
        takes a printer that displays as a side effect, and a printer calling
        `display` from inside a format cycle produces no data for the front
        end — the result arrives with no mimetypes and the cell shows nothing,
        silently. Returning the bundle is both simpler and what a class with a
        ``_repr_mimebundle_`` of its own would do; this is that method, kept
        outside the class.
        """
        widget = bridge.widget_for(_window_id(obj))
        bundle = widget._repr_mimebundle_(include=include, exclude=exclude)
        return bundle

    for kind in (PlotWindow, ScopeWindow, PatchWindow, WindowHandle):
        formatter.for_type(kind, show)
        _registered.append((formatter, kind))


def unregister():
    """Undo `register` — the notebook goes back to plain reprs."""
    while _registered:
        formatter, kind = _registered.pop()
        formatter.pop(kind, None)
