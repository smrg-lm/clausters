"""Editing: the subdomain of the GUI where a picture writes back.

Everything that turns a gesture into a change of the data, and the change back
into a picture. It is a subpackage rather than a module because it is four
collaborators and two editors, and because the boundaries between them are the
whole design:

- `Editor` — the generic one. It edits **one structure** and imports nothing
  from the arrangement: it opens a window through its `View`, turns a gesture
  into a payload through its `Domain`, answers the host through its `Echo`, and
  records in the `Editing` context the data owns.
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
- `FormEditor` — the arrangement's editor: `Editor` plus a held document, the
  node index, several views of one composition, the lanes and clips, and the
  transport. `FormEditing` is its context.

The two names are the point of the split: `Editor` is what a person calls to
edit a buffer, a curve or a timeline, and `FormEditor` is what edits a piece.

`View` here is **not** `clausters.gui.guidef.View`, and only this one is reached
as `clausters.gui.editing.View`: the guidef one is a tree you can open, this one
is the picture of a structure plus the registry that resolves an event back to
it. `clausters.gui` goes on exporting the guidef `View`, so nothing a script
writes changes.
"""

from .context import ATTR, FIRST_VERSION, Editing
from .domain import Domain
from .echo import Echo
from .editor import NOT_AN_EDIT, Editor
from .formeditor import FormEditing, FormEditor
from .view import View

__all__ = [
    "ATTR",
    "Domain",
    "Echo",
    "Editing",
    "Editor",
    "FIRST_VERSION",
    "FormEditing",
    "FormEditor",
    "NOT_AN_EDIT",
    "View",
]
