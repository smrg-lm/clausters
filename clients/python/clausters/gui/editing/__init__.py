"""Editing: the subdomain of the GUI where a picture writes back.

Everything that turns a gesture into a change of the data, and the change back
into a picture. It is a subpackage rather than a module because it is five
collaborators and two editors, and because the boundaries between them are the
whole design:

- `Application` — the window set: the host, the widget-id space, the
  acknowledgement, the socket drain and the walk of the undo order. Everything
  true of a **session on screen** rather than of one structure, so several
  editors can share one — and an editor handed none is an application of one.
- `Editor` — the generic one. It edits **one structure** and imports nothing
  from the arrangement: it opens a window through its `View`, turns a gesture
  into a payload through its `Domain`, answers the host through its
  `Application`, and records in the `Editing` context the data owns.
- `View` — the `GuiDef` of one structure, and the registry from widget id to
  what it shows. The only per-domain thing on the graphic side.
- `Domain` — the data adapter: gesture → payload, payload → the client object,
  the label and the coalesce key. It does not know how an edit inverts (that is
  the crate's `history::Editable`) and it does not draw.
- `Echo` — the acknowledgement protocol: the stamp, the version, the floor, the
  corrections and the reason. Entirely generic, and testable with no structure
  at all.
- `Editing` — the editing context: the history, the version, and the views to
  tell. An editor **asks for it and never builds one**, which is what makes two
  windows over one thing walk one undo order.
- `trace` — the path said out loud, at five points: an event routed, an entry
  recorded, a step of the pile, a publish, an acknowledgement. Silent unless
  asked (`CLAUSTERS_EDIT_LOG=1`, or `watch()`), because what a window in front
  of a person does wrong is otherwise visible to nobody.
- `FormEditor` — the arrangement's editor: `Editor` plus a held document, the
  node index, several views of one composition, the lanes and clips, and the
  transport. `FormEditing` is its context.

The two names are the point of the split: `Editor` is what a person calls to
edit a buffer, a curve or a timeline, and `FormEditor` is what edits a piece.

And `edit(x)` is how a person calls it: one verb over the three fundamental
structures, dispatching on what the structure is — `SamplesEditor` over a
`clausters.defs.Buffer`, `PointsEditor` over a `clausters.seq.Automation`,
`NotesEditor` over a `clausters.seq.Timeline`. Each is `Editor` with its own
domain and view in it and nothing else, which is what the split was for.

`View` here is **not** `clausters.gui.guidef.View`, and only this one is reached
as `clausters.gui.editing.View`: the guidef one is a tree you can open, this one
is the picture of a structure plus the registry that resolves an event back to
it. `clausters.gui` goes on exporting the guidef `View`, so nothing a script
writes changes.
"""

from .application import BASE_ID, Application
from .context import ATTR, FIRST_VERSION, Editing
from .domain import Domain
from .echo import Echo
from .edit import edit
from .editor import NOT_AN_EDIT, Editor
from .events import NotesDomain, NotesEditor, NotesView
from .formeditor import FormEditing, FormEditor
from .points import PointsDomain, PointsEditor, PointsView
from .samples import (MEASURES, SamplesDomain, SamplesEditor, SamplesView,
                      measures)
from .trace import watch
from .view import View

__all__ = [
    "ATTR",
    "Application",
    "BASE_ID",
    "Domain",
    "Echo",
    "Editing",
    "Editor",
    "FIRST_VERSION",
    "FormEditing",
    "FormEditor",
    "MEASURES",
    "NOT_AN_EDIT",
    "NotesDomain",
    "NotesEditor",
    "NotesView",
    "PointsDomain",
    "PointsEditor",
    "PointsView",
    "SamplesDomain",
    "SamplesEditor",
    "SamplesView",
    "View",
    "edit",
    "watch",
    "measures",
]
