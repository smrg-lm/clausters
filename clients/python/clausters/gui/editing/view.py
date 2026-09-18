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

from ... import _native


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

        Takes the editor because the ids are named through it and the unit
        bridge is its own: a view decides what the picture *is*, never what a
        number in it is measured in.

        The registry is rebuilt and the **ids are not**: a widget still in the
        picture comes back with the number it had. What is rebuilt here is only
        the map from id to what it draws, which has to follow the tree.
        """
        self.widgets = {}
        return self.build(editor)

    def build(self, editor) -> dict:
        """The tree itself. Make each widget with `widget`, which names it and
        registers it in one call."""
        raise NotImplementedError

    def widget(self, editor, role: str, showing, key: str = "") -> int:
        """The id of one widget of this picture, **named** rather than leased.

        The door a `build` takes. ``role`` says what the widget is in this
        picture (``"curve"``, ``"waveform"``, ``"roll"``) and ``key`` which one
        it is when a role has several; together with the structure's identity
        they name the widget, and the name gives back the same id on every
        redraw. So an edit-back in flight, a correction on its way out and the
        widget's own screen state all keep pointing at the widget the hand
        touched, which a leased id could not promise across a redraw.
        """
        return self.register(editor._named_id(role, key), showing)

    def register(self, widget_id: int, showing) -> int:
        """Remember that ``widget_id`` draws ``showing``, and hand the id back
        so a builder can use it inline.

        Called by `widget`; called directly only for a widget that is genuinely
        unnamed — a decoration a picture leases an id for.
        """
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

    def catalogue(self, editor, kind: str, role: str, showing, facts: dict,
                  key: str = "") -> dict:
        """One widget of the **catalogue**, named and registered in one call.

        The door a `build` takes for a picture the crate already knows how to
        describe: `clausters._native.view_props` says which widget a waveform,
        a curve or a roll is and what is on it, and this stamps the id — which
        is the one thing the crate cannot know, since ids are a client's.

        It is here rather than in each view because every one of them takes the
        same three steps in the same order, and because the standalone host and
        the web client take them too: what a picture *is* has one answer, and
        the place a client differs is what it wraps that picture in.

        Raises:
            ValueError: the crate draws no view of ``kind``, or cannot read
                ``facts`` as one, with its reason -- rather than a widget with
                nothing on it.
        """
        props = dict(_native.view_props(kind, facts))
        widget = self.widget(editor, role, showing, key)
        return _node(str(props.pop("type", "")) or kind, widget, props)

    def props(self, editor, widget_id: int) -> dict:
        """**Everything the widget should be drawing**, for a resync.

        Not only what a gesture touched: a stale edit is the one case where the
        host's whole picture of a widget is in doubt, so what goes back is the
        widget's whole state. An empty answer means there is nothing to correct,
        which is what a view with no editable props says.
        """
        return {}


def _node(kind: str, widget_id: int, props: dict) -> dict:
    """The widget as a `clausters.gui.guidef` node.

    Imported where it is used rather than at the top: `guidef` reaches back
    into this package, and a module-level import of it would close the loop.
    """
    from ..guidef import node

    return node(kind, id=widget_id, **props)
