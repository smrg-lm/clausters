"""`edit(x)`: the verb, and what it opens.

**One call opens a window**, the way `clausters.plot` and `clausters.scope` do:
the ambient host is resolved here, the editor is opened on it and its event loop
takes over the draining, so a script that edits writes no loop at all --

    edit(curve)                 # the window is up
    print(curve.to_points())    # what the hand has left there, now

-- and what comes back is the editor, which is the handle the window is closed
and undone through. Reading is done on the structure itself: nothing is handed
back at the end because the object passed in *is* the edited one. A sound file
is the exception, and deliberately: the audio editor
(`clausters.gui.editing.AudioEditor`) is an application the GUI host holds,
which writes the file only when it is saved, as an audio editor does.

One call over the fundamental structures (a sound file, a break-point
curve, a timeline of events, and a **multitrack**), each of which is a
`clausters.gui.editing.Editor` with its own domain and its own view and nothing
else. It dispatches on **what the structure is** rather than on a keyword,
because that is the question a caller has already answered by holding one.

**A multitrack is the multitrack application's**, as a sound file is the
audio editor's: the GUI host opens it, over a saved session's file or over a
`clausters.multitrack.Multitrack` handed to it whole (an untitled session),
and this script is its handler (`clausters.gui.editing.MultitrackEditor`).
The script's multitrack is the window's while it is open.

**A second call over a structure hands back the editor already open on it**
(of the same kind, and -- for a roll -- over the same axis), with its window as
it is: a view names its widgets by what they draw, so a second window of one
role over one structure would ask for the same widgets as the first and draw
on them. Views of different roles over one structure are separate windows and
**one stack** -- a roll in hertz beside one in MIDI notes -- since the editing
context is the data's (`clausters.gui.editing.Editing`), so an undo in either
updates both. That is not a feature of this verb -- it is what asking the data
for its history means, and `edit` inherits it for free.
"""

from typing import overload

from ...defs.node import Node
from ...history import Editing
from ...seq.timeline import Timeline
from .controls import NodeEditor
from .events import NotesEditor, is_events
from .multitrack import MultitrackEditor, is_multitrack
from .points import PointsEditor, is_curve
from .audio import AudioEditor
from ...soundfile import SoundFile
from ...multitrack import Multitrack
from ...multitrack import Session as SavedSession
from .score import ScoreEditor, is_score


@overload
def edit(structure: Node, *, sample_rate: float = 0.0, host=None,
         open: bool = True, **options) -> NodeEditor: ...


@overload
def edit(structure: SoundFile, *, sample_rate: float = 0.0, host=None,
         open: bool = True, **options) -> AudioEditor: ...


@overload
def edit(structure: "SavedSession | Multitrack", *, sample_rate: float = 0.0,
         host=None, open: bool = True, **options) -> MultitrackEditor: ...


@overload
def edit(structure, *, sample_rate: float = 0.0, host=None,
         open: bool = True, **options): ...


def edit(structure, *, sample_rate: float = 0.0,
         host=None, open: bool = True, **options):
    """Open ``structure`` in an editor of its own kind.

    Args:
        structure: what to edit: a `clausters.SoundFile` (in the audio
            editor, the GUI host's application, which writes the file only on
            `save`; a WAV, RF64 or W64 file on disk), a saved
            `clausters.multitrack.Session` (in the multitrack, the GUI host's
            application, over the session's file:
            `clausters.gui.editing.MultitrackEditor`), a
            `clausters.multitrack.Multitrack` (in the multitrack, as an
            untitled session over it, whose first save names its file), a curve (a `clausters.defs.Bpf`, an `clausters.defs.Env` or a
            `clausters.multitrack.Automation`), a `clausters.seq.EventSequence`
            (its notes, in place), a `clausters.seq.Timeline` (rendered into a
            sequence first -- ``until=`` bounds one that does not end -- which
            the editor's ``sequence`` then holds; the timeline is not changed)
            or a `clausters.gui.notation.Score` (a symbolic score, on its page, in
            place) -- or a running `clausters.defs.Node`, whose def's controls
            open in a generic editor (`clausters.gui.editing.NodeEditor`).
        sample_rate: the engine's rate, which fixes the data<->view bridge. A
            sound file needs none; anything else takes the
            ambient server's nominal rate -- the current session's, else the
            default session's, as a play resolves its server -- and 48 kHz
            with no server anywhere.
        host: the `clausters.gui.host.GuiHost` to open on; ``None`` -- the
            ordinary case -- resolves the ambient one, the rule
            `clausters.plot`, `clausters.scope` and
            `clausters.gui.guidef.View.open` all follow.
        open: whether to open the window. ``False`` builds the editor without
            one, for a caller composing a window out of several editors or
            inspecting the picture it would draw -- the only case the separate
            `clausters.gui.editing.Editor.open` was ever for.
        options: passed through to the editor -- ``title``, ``width``,
            ``height``, ``base_id``, ``extra`` (widgets of the caller's own,
            appended after the picture and resolved by name on
            `clausters.gui.editing.Editor.window`), and ``context`` for a view
            that joins an editing context the caller already has (which is what
            makes a composed window undo across several structures in one
            order), and ``chrome``: ``False`` opens what is edited with none
            of the application's chrome around it, the keys kept
            (`clausters.gui.editing.Editor`). A curve also takes ``min``/``max`` and ``start``/``end``,
            the ranges its values and its times are kept in
            (`clausters.gui.editing.PointsEditor`). A
            sequence (or a timeline, rendered into one) takes ``y_axis``, the
            roll's vertical axis: ``"midi"`` or ``"hz"``. **``view`` chooses
            the presentation of a structure that has several**: a sequence is
            edited as a ``"roll"``, its default, or as a ``"score"`` -- the
            page it is read into, in the score editor, where an edit changes
            in the sequence only what it changed on the page and is one undo
            order with a roll over the same sequence
            (`clausters.gui.editing.ScoreEditor.over`). There it also takes
            the transcription's keys, which say how the sequence is read
            where its events do not (``meter``, ``key``, ``clef``,
            ``beat_unit``, ``division``, ``tuplets``, ``voices``,
            ``dynamics``), and ``interp``. A node takes ``title`` only, and a
            sound file ``title`` and ``layers``.

    Returns:
        The editor, **open** -- the one already open over ``structure``, when
        there is one of its kind (and, for a roll, over the same axis): then
        nothing is opened, and the options of this call are not applied. It is
        the handle the window is addressed by --
        `clausters.gui.editing.Editor.close`, `Editor.on_closed`, `undo`/`redo`
        -- and a caller who wants none of that may discard it: the host holds an
        open editor until its window closes, so ``edit(curve)`` on its own is a
        complete program. Reading the edited data is done on the structure that
        was passed in, which *is* the edited one, except a sound file, which the
        audio editor writes when it is saved (its ``file`` reads what it is
        now), and a timeline, whose rendered events are the editor's
        ``sequence``.

    Raises:
        TypeError: for something none of the three domains reads, naming what
            they are -- an unopenable structure is a question about the data,
            and answering it with a bare failure teaches nothing.
    """
    if isinstance(structure, Node):
        return _node_editor(structure, host, open, options)
    if open:
        opened = _already_open(structure, host, options)
        if opened is not None:
            return opened
    if isinstance(structure, SoundFile):
        # **The file is the application's**, opened by the host in a window
        # of its own; this is its handler
        editor = AudioEditor(structure, **options)
        if open:
            editor.open(host)
            _open_audio.append(editor)
        return editor
    if _is_saved_session(structure) or is_multitrack(structure):
        # **A multitrack is the multitrack application's**, opened by the
        # host over a session's file or as an untitled session; this is its
        # handler
        editor = MultitrackEditor(structure, **options)
        if open:
            editor.open(host)
            _open_multitracks.append(editor)
        return editor
    if _is_buffer(structure):
        raise TypeError(
            "the audio editor edits sound files, and a buffer is loaded on a server "
            "it does not reach: write it to a file (`buffer.write(path)`) and edit that")
    if is_curve(structure):
        editor = PointsEditor(structure, sample_rate=sample_rate or _ambient_rate(),
                              **options)
    elif is_events(structure):
        # **A timeline is rendered, and the roll edits what it produced**: the
        # events, as concrete data in the timeline's beats with its map. The
        # timeline itself is code and is left as it was; the sequence is the
        # editor's `sequence`.
        if isinstance(structure, Timeline):
            structure = structure.render_events(until=options.pop("until", None))
        # **A sequence has two presentations**, and `view` chooses: the roll,
        # which is its default, or its page -- the score it is read into,
        # where an edit changes in the sequence only what it changed there.
        view = _view(options, ("roll", "score"))
        if view == "score":
            editor = ScoreEditor.over(structure, **options)
        else:
            editor = NotesEditor(structure, sample_rate=sample_rate or _ambient_rate(),
                                 **options)
    elif is_score(structure):
        _view(options, ("score",))
        editor = ScoreEditor(structure, **options)
    else:
        raise TypeError(
            f"nothing edits a {type(structure).__name__}: `edit` opens a SoundFile "
            f"(its samples), a curve (anything with to_points/set_points), "
            f"an EventSequence or a Timeline (its notes), "
            f"a Multitrack or a saved Session (the multitrack) or a Score (its page)."
        )
    if open:
        editor.open(host)
    return editor


#: The audio editors this script opened, which a second `edit` of the same
#: file finds.
_open_audio: list = []

#: The multitracks this script opened, which a second `edit` of the same
#: session file, or of the same multitrack, finds.
_open_multitracks: list = []


def _is_saved_session(structure) -> bool:
    """Whether ``structure`` is a `clausters.multitrack.Session`."""
    from ...multitrack import Session as SavedSession

    return isinstance(structure, SavedSession)


def _is_buffer(structure) -> bool:
    """Whether ``structure`` is a server buffer, which the audio editor no
    longer opens."""
    return hasattr(structure, "bufnum") and hasattr(structure, "set_samples")


#: The node editors open, by server and node id: a running node's controls
#: are the server's and not a structure with an editing context, so the editor
#: already open over one is found here rather than among a context's views.
_open_nodes: dict = {}


def _node_editor(node, host, open: bool, options: dict) -> NodeEditor:
    """The generic editor of ``node``: the one already open over it, on the
    same host when one is named, or a new one."""
    key = (id(node._server()), node.id)
    for k, editor in list(_open_nodes.items()):
        if editor.closed:
            del _open_nodes[k]
    current = _open_nodes.get(key)
    if open and current is not None and (host is None or current.host is host):
        return current
    editor = NodeEditor(node, **options)
    if open:
        editor.open(host)
        _open_nodes[key] = editor
    return editor


def _view(options: dict, views: tuple) -> str:
    """The presentation ``options`` ask for among ``views``, the first of
    which is the structure's default -- taken out of the options."""
    view = options.pop("view", None) or views[0]
    if view not in views:
        raise ValueError(
            f"no view {view!r} of this structure: it is edited as "
            + " or ".join(repr(name) for name in views))
    return view


def _already_open(structure, host, options):
    """**The editor already open over ``structure``** that this call would
    open again -- the same kind, the same structure, the same axis for a roll,
    on the same host when one is named -- or ``None``.

    Found among the views of the structure's editing context, since every
    editor attaches there when it opens. A timeline is rendered into a new
    sequence by each call, so it never finds one."""
    if isinstance(structure, SoundFile):
        _open_audio[:] = [e for e in _open_audio if not e.closed]
        return next((e for e in _open_audio
                     if e._file is not None and e._file.path == structure.path
                     and (host is None or e.host is host)), None)
    if _is_saved_session(structure) or is_multitrack(structure):
        _open_multitracks[:] = [e for e in _open_multitracks if not e.closed]
        return next((e for e in _open_multitracks
                     if (e.multitrack is structure or (
                         _is_saved_session(structure) and structure.path is not None
                         and e.path == str(structure.path)))
                     and (host is None or e.host is host)), None)
    kinds = ((is_curve, PointsEditor), (is_events, NotesEditor),
             (is_score, ScoreEditor))
    kind = next((k for test, k in kinds if test(structure)), None)
    if kind is None or isinstance(structure, Timeline):
        return None
    # a sequence's page is the score editor's, over that sequence
    if kind is NotesEditor and options.get("view") == "score":
        kind = ScoreEditor
    context = options.get("context") or Editing.of(structure)
    axis = str(options.get("y_axis", "midi"))
    for view in context.views():
        if (type(view) is kind and view.structure is structure
                and view.window is not None
                and (host is None or view._host is host)
                and (kind is not NotesEditor or view.y_axis == axis)):
            return view
    return None


def _ambient_rate() -> float:
    """The ambient server's nominal rate -- the current session's, else the
    default session's -- or 48 kHz with none, the rate a window drawn with no
    engine is laid out at."""
    from ...base.main import main

    try:
        server = main.resolve_server()
    except RuntimeError:
        return 48_000.0
    return float(server.query_info().nominal_sample_rate)
