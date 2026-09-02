"""The picture of one structure, and the registry from widget id to what it
shows.

A view is the **only per-domain thing on the graphic side**: it builds the
`GuiDef` for one structure and remembers which widget draws what, so an event
naming a widget resolves to something an editor can act on. Everything else
about drawing — the window, the ids, the acknowledgement — is the editor's and
is the same for every structure.

It is separate from `clausters.gui.editing.Domain` because one structure is
drawn several ways while its vocabulary is one: a curve is a `bpf` on its own
axis and a body inside a clip, and both send the same `points` payload.
"""


class View:
    """One structure on screen.

    Subclass it per picture: `build` is the tree, and `showing` is what the
    widgets in it draw. The registry is kept here rather than in the editor
    because it is rebuilt with the tree, and the two going out of step is how a
    gesture reaches the wrong object.
    """

    def __init__(self):
        #: widget id -> what that widget draws. Rebuilt by every `draw`.
        self.widgets: dict = {}

    def draw(self, editor) -> dict:
        """The `GuiDef` this view is, with the registry rebuilt.

        Takes the editor because ids come from its pool and the unit bridge is
        its own: a view decides what the picture *is*, never what a number in
        it is measured in.
        """
        self.widgets = {}
        return self.build(editor)

    def build(self, editor) -> dict:
        """The tree itself. Register each widget as it is made (`register`)."""
        raise NotImplementedError

    def register(self, widget_id: int, showing) -> int:
        """Remember that ``widget_id`` draws ``showing``, and hand the id back
        so a builder can use it inline."""
        self.widgets[int(widget_id)] = showing
        return int(widget_id)

    def owns(self, widget_id: int) -> bool:
        """Whether this view drew the widget an event names.

        Asked before anything else, because a poll loop may be shared: answering
        for another view's window retires a pending edit nobody applied, and the
        host adopts a picture its real owner never saw.
        """
        return int(widget_id) in self.widgets

    def showing(self, widget_id: int):
        """What that widget draws, or ``None``."""
        return self.widgets.get(int(widget_id))

    def props(self, editor, widget_id: int) -> dict:
        """**Everything the widget should be drawing**, for a resync.

        Not only what a gesture touched: a stale edit is the one case where the
        host's whole picture of a widget is in doubt, so what goes back is the
        widget's whole state. An empty answer means there is nothing to correct,
        which is what a view with no editable props says.
        """
        return {}
