"""A node's generic editor: the window of its def's controls, which any node
gets without anybody writing one.

``edit(node)`` reads the node's def (``/node_query`` names it, ``/def_query``
describes its controls, the same record in every family) and hands the
description to the shared core (``clausters_core::node_editor``), which
decides what each control becomes -- a slider, one with detents, a toggle, a
list -- what range it is drawn over and how a large def is grouped and paged.
What comes back is an ordinary GuiDef, opened like any other.

**The host keeps it, not this script.** Every widget is bound to its control
(``/node_set``), so a hand writes the node with no round trip through here,
and the host reads each binding back from the server, so the window follows
the node whoever changes it -- another client, the plugin's own window, a
loaded state. All this object does after opening is answer the search field
and the page chooser, with the core's next page.

**It is not a document editor.** A running node's controls are the server's
state and not a structure the client holds, so nothing here enters an undo
order (`clausters.gui.editing.Editor`); the plugin's own window is a
different thing again, `clausters.defs.Node.open_plugin_editor`.
"""

from dataclasses import asdict

from ... import _native
from .application import _resolve_host


class NodeEditor:
    """**A running node's controls in a window**, generated from its def and
    bound to the node by the host.

    Args:
        node: the `clausters.defs.Node` to edit: a synth of a SynthDef, a
            FaustDef or a plugin.
        title: the window's title; by default the def's name and the node's
            id.

    The window has a search field over the controls' names, labels and groups,
    and a page chooser; `search` and `show_page` do the same from code.
    """

    def __init__(self, node, *, title: "str | None" = None):
        #: The node it edits.
        self.node = node
        #: Its def's controls, as `clausters.defs.info.ControlInfo` -- read when
        #: the editor opens.
        self.controls: list = []
        #: What the search field holds.
        self.query = ""
        #: The page shown, from zero.
        self.page = 0
        #: The open window's `clausters.gui.handle.WindowHandle`, or ``None``.
        self.window = None
        #: The `clausters.gui.host.GuiHost` the window is open on.
        self.host = None
        self._title = title
        self._def = ""

    def _generate(self) -> dict:
        controls = [asdict(c) for c in self.controls]
        return _native.node_editor(self.node.id, self._def, controls, self.query, self.page)

    def open(self, host=None) -> "NodeEditor":
        """Open the window on ``host`` (the ambient one by default), reading
        the node's def first. An open editor is left as it is."""
        if not self.closed:
            return self
        server = self.node._server()
        info = self.node.info()
        if not info.exists or info.is_group:
            raise ValueError(f"node {self.node.id} is not a synth: it has no controls to edit")
        self._def = info.defname
        self.controls = server.query_defs(info.defname)[0].controls
        tree = self._generate()["window"]
        if self._title is not None:
            tree["title"] = self._title
        self.host = _resolve_host(host, server)
        self.window = self.host.open(tree)
        self.window["search"].on_event(self.search)
        self.window["page"].on_event(lambda index: self.show_page(int(index)))
        return self

    def search(self, query: str) -> "NodeEditor":
        """Show the controls whose name, label or group holds ``query``
        (ignoring case), from their first page."""
        self.query = str(query)
        self.page = 0
        return self._show()

    def show_page(self, page: int) -> "NodeEditor":
        """Show page ``page`` (from zero; past the last is the last)."""
        self.page = int(page)
        return self._show()

    def _show(self) -> "NodeEditor":
        if self.closed:
            return self
        answer = self._generate()
        self.page = answer["page"]
        self.host.redefine(int(self.window["controls"]), answer["contents"], window=int(self.window))
        self.window["page"].set(options=answer["pages"], index=answer["page"])
        return self

    @property
    def closed(self) -> bool:
        """Whether the window is closed, or was never opened."""
        return self.window is None or self.window.closed

    def close(self):
        """Close the window. The node is left as it is."""
        if self.window is not None:
            self.window.close()

    def wait(self, timeout: "float | None" = None) -> bool:
        """Wait until the window closes (see
        `clausters.gui.handle.WindowHandle.wait`)."""
        return self.window is None or self.window.wait(timeout)

    def on_closed(self, func) -> "NodeEditor":
        """Call ``func()`` when the window closes."""
        if self.window is None:
            raise RuntimeError("the node editor is not open")
        self.window.on_closed(func)
        return self

    def capture(self, path, timeout: float = 5.0) -> str:
        """A picture of the window, as a PNG at ``path`` (see
        `clausters.gui.host.GuiHost.capture`)."""
        if self.window is None:
            raise RuntimeError("the node editor is not open")
        return self.host.capture(int(self.window), path, timeout)

    def __repr__(self) -> str:
        state = "closed" if self.closed else "open"
        return f"<NodeEditor node {self.node.id} {self._def or '?'} ({state})>"
