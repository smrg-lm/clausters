"""`edit(x)`: the verb, and what it opens.

**One call opens a window**, the way `clausters.plot` and `clausters.scope` do:
the ambient host is resolved here, the editor is opened on it and its event loop
takes over the draining, so a script that edits writes no loop at all --

    edit(curve)                 # the window is up
    print(curve.to_points())    # what the hand has left there, now

-- and what comes back is the editor, which is the handle the window is closed
and undone through. Reading is done on the structure itself: nothing is handed
back at the end because the object passed in *is* the edited one.

One call over the three fundamental structures — a buffer's samples, a
break-point curve, a timeline of events — each of which is a
`clausters.gui.editing.Editor` with its own domain and its own view and nothing
else. It dispatches on **what the structure is** rather than on a keyword,
because that is the question a caller has already answered by holding one.

What it deliberately does not open is a composition: an arrangement is edited by
`clausters.gui.editing.FormEditor`, which knows a tree from a leaf and holds a
document. `edit` over a piece would be a second door to the same place with a
worse answer.

Two calls over one structure give **two windows and one stack**: the editing
context is the data's (`clausters.gui.editing.Editing`), so an undo in either
updates both. That is not a feature of this verb — it is what asking the data for
its history means, and `edit` inherits it for free.
"""

from .events import NotesEditor, is_events
from .points import PointsEditor, is_curve
from .samples import SamplesEditor, is_samples


def edit(structure, *, sample_rate: float = 0.0, tempo: float = 1.0,
         host=None, open: bool = True, **options):
    """Open ``structure`` in an editor of its own kind.

    Args:
        structure: what to edit — a `clausters.defs.Buffer` (its samples), a
            `clausters.seq.Automation` (its curve) or a
            `clausters.seq.Timeline` (its notes).
        sample_rate: the engine's rate, which fixes the data↔view bridge. A
            take knows its own and needs none.
        tempo: the clock's tempo in beats per second, for the structures placed
            in beats.
        host: the `clausters.gui.host.GuiHost` to open on; ``None`` — the
            ordinary case — resolves the ambient one, the rule
            `clausters.plot`, `clausters.scope` and
            `clausters.gui.guidef.View.open` all follow.
        open: whether to open the window. ``False`` builds the editor without
            one, for a caller composing a window out of several editors or
            inspecting the picture it would draw — the only case the separate
            `clausters.gui.editing.Editor.open` was ever for.
        options: passed through to the editor — ``title``, ``width``,
            ``height``, ``base_id``, and ``context`` for a view that joins an
            editing context the caller already has (which is what makes a
            composed window undo across several structures in one order).

    Returns:
        The editor, **open**. It is the handle the window is addressed by —
        `clausters.gui.editing.Editor.close`, `Editor.on_closed`, `undo`/`redo`
        — and a caller who wants none of that may discard it: the host holds an
        open editor until its window closes, so ``edit(curve)`` on its own is a
        complete program. Reading the edited data is done on the structure that
        was passed in, which *is* the edited one.

    Raises:
        TypeError: for something none of the three domains reads, naming what
            they are — an unopenable structure is a question about the data,
            and answering it with a bare failure teaches nothing.
    """
    if is_samples(structure):
        editor = SamplesEditor(structure, sample_rate=sample_rate, tempo=tempo,
                               **options)
    elif is_curve(structure):
        editor = PointsEditor(structure, sample_rate=sample_rate or 48_000.0,
                              tempo=tempo, **options)
    elif is_events(structure):
        editor = NotesEditor(structure, sample_rate=sample_rate or 48_000.0,
                             tempo=tempo, **options)
    else:
        raise TypeError(
            f"nothing edits a {type(structure).__name__}: `edit` opens a Buffer "
            f"(its samples), an Automation (its curve) or a Timeline (its notes). "
            f"A composition is FormEditor's."
        )
    if open:
        editor.open(host)
    return editor
