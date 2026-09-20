"""Editing a **buffer's samples**: the one domain whose state is not here.

A curve's points and a timeline's events are values this process holds, so the
crate can be handed one and asked what an edit makes of it. A span of samples is
not: the frames are in a **server buffer**, which is why the crate's own
`Samples` is a borrowed view over memory its caller owns, and why
`clausters._native.domain_edit` answers nothing for this vocabulary. What is
shared is the payload's shape and its coalesce key; where the state lives is the
client's, and this module is that half.

**The inverse rides on the wire.** A ``"draw"`` carries the run it wrote *and*
the run it replaced, and a ``"sample"`` carries the value and the previous one --
the protocol was written that way precisely so an owner can invert a stroke
without having remembered anything. So nothing is read back from the server to
undo: the edit and its inverse arrive together, and what the history records is
the second.

**What the picture measures is the view's.** A waveform is drawn as a stack of
measures over one field -- what the signal reached (``peak``) with what it held
inside that (``rms``) -- and that is a prop of the one widget rather than a pile
of widgets: every view of a signal paints its own field before it draws, so two
of them on one rectangle are not layers, the second hides the first. Measuring
twice into one body is also what makes the rest of it one thing: one axis, one
ruler, one selection, one playhead, one upload of the samples.

**A stroke lands on one channel.** What a write does to the buffer is the
shared crate's (`clausters._native.EditingCore`, ``write``): a mono take
takes the run as flat samples (``/buffer_setRange``), and one channel of a take
with more is written by frames of that channel alone
(``/buffer_setRangeChannel``), so the other channels are never touched. This
client walks those steps against the take's server (`clausters._steps.run_steps`).
"""

import struct
from array import array

from ... import _native
from ..._steps import run_steps
from ...defs._wire import resolve as _resolve
from .domain import Domain
from .editor import Editor
from .view import View


#: The measures a signal view can stack, in the order a reader thinks of them:
#: what the signal reached, and what it held inside that.
MEASURES = ("peak", "rms")


def measures(stack) -> tuple:
    """A measure stack as a tuple, or a `ValueError` naming what is wrong.

    A stack is written by hand, so a silent typo is a layer that quietly does
    not appear, and an empty one is a picture that measures nothing. The check
    is the crate's (`clausters._native.samples_measures`).
    """
    return _native.samples_measures(stack)


def _plain(value):
    """An event's arguments as JSON carries them: a blob is the run a stroke
    wrote or replaced, read into its numbers -- the wire's framing is this
    client's."""
    if isinstance(value, (bytes, bytearray, memoryview, array)):
        return _floats(value)
    if isinstance(value, (list, tuple)):
        return [_plain(v) for v in value]
    return value


def _floats(blob) -> list:
    """A run of samples as floats, from the little-endian ``f32`` blob the wire
    carries (or from a list, which is what a hand-written test sends)."""
    if isinstance(blob, (bytes, bytearray, memoryview)):
        raw = bytes(blob)
        return list(struct.unpack(f"<{len(raw) // 4}f", raw[:len(raw) // 4 * 4]))
    if isinstance(blob, array):
        return list(blob)
    if isinstance(blob, (list, tuple)):
        return [float(v) for v in blob]
    return []


class SamplesDomain(Domain):
    """A span of samples' vocabulary: the crate's ``samples``, over frames the
    server holds.

    **What a gesture means is the application's** (`clausters._native.EditingCore`): the
    run a stroke wrote and the run it replaced are read there in one reading,
    so nothing waits here between two calls. What is left is the write itself,
    onto the buffer this client holds -- a stroke's, and a step of the
    history's.
    """

    name = _native.SAMPLES
    ingested = True

    #: The editor whose take this is: what turns a write into steps, and the
    #: runner they are walked through.
    editor = None

    def project(self, structure, payload: dict) -> bool:
        """Write ``payload`` onto the take's buffer: the steps the core answers
        for it, walked against the take's server. A stroke's write and a step of
        the history's are this one call."""
        editor = self.editor
        if editor is None or not (payload or {}).get("values"):
            return False
        server = _resolve(getattr(structure, "server", None))
        steps = editor._call("write", edit=payload,
                                  chunk=int(server._bulk_chunk())).get("steps") or []
        if not steps:
            return False
        run_steps(server, editor._runner, steps)
        return True


class SamplesView(View):
    """One `clausters.gui.guidef.waveform`: the take on its own axis, drawn by
    the host straight from the server buffer, with the measures it stacks as a
    prop of that one widget (see the module docstring).

    **The window is the application's**, composed in the shared crate
    (`clausters._native.EditingCore`): the waveform, the gesture plan a
    take is edited with (a drag selects, Alt draws, Ctrl grabs one sample), the
    label and the correction a write answers with. What is left here is the id a
    hand's gestures come back on.
    """

    def __init__(self, layers=MEASURES):
        super().__init__()
        #: What the picture measures, innermost last.
        self.layers = measures(layers)

    def build(self, editor) -> dict:
        wid = self.widget(editor, "waveform", editor.structure)
        editor._sync_core()
        tree = editor._call("window", widget=wid)
        # **A script's own widgets are its objects**, so they are appended here
        # rather than composed in the crate.
        tree["children"] = [*tree.get("children", ()), *editor.extra]
        return tree

    def props(self, editor, widget_id: int) -> dict:
        return editor._call("props", widget=int(widget_id))


class SamplesEditor(Editor):
    """A buffer's samples on screen, editable back into the server's buffer.

    The picture and the sound are the **same** buffer: the host draws what the
    server holds, and a stroke writes there -- so what is heard after an edit is
    what is seen, with no copy in between.
    """

    def __init__(self, take, *, sample_rate: float = 0.0,
                 title: str = "Samples", layers=MEASURES, **options):
        rate = float(sample_rate or getattr(take, "sample_rate", 0.0) or 48_000.0)
        view = SamplesView(layers)
        domain = SamplesDomain()
        super().__init__(take, sample_rate=rate,
                         domain=domain, view=view,
                         title=title, **options)
        domain.editor = self
        #: The runner a write's steps are walked through.
        self._runner = _native.StepRunner()
        #: **The editor, in the shared crate**: a member of this take's editing
        #: context, holding the take, the measures and the chrome the window is
        #: composed from, and reading every message. A take is named by its
        #: buffer, so two windows over one are one structure in the order.
        self._member, self._structure_id = self._editing.open(
            "openSamples", f"buffer:{int(getattr(take, 'bufnum', 0) or 0)}",
            {**self._facts(), "layers": list(view.layers)}, take, domain)

    def _facts(self) -> dict:
        take = self.structure
        name = getattr(take, "name", None)
        return {"buffer": int(getattr(take, "bufnum", 0) or 0),
                "channels": max(1, int(getattr(take, "channels", 1) or 1)),
                "name": name if isinstance(name, str) and name else None,
                "rate": self.sample_rate,
                "title": self.title, "w": int(self.size[0]), "h": int(self.size[1]),
                "window": self._window}

    def _call(self, verb: str, **args) -> dict:
        """One verb of this editor's member, through the context."""
        return self._editing.member(self._member, verb, **args)

    def _sync_core(self) -> None:
        """Hand the core what this client holds: the take a script may have
        resized, the axis, the window's chrome and the window it is open in."""
        self._call("sync", **self._facts())

    # ---- the crate's turns ----

    def _deliver(self, addr: str, args) -> bool:
        self._sync_core()
        turned = self._editing.event(self._member, str(addr), _plain(list(args)))
        outcome = turned.get("outcome") or {}
        if outcome.get("turn") == "closed":
            return self._closed()
        if outcome.get("turn") == "step":
            # **The step is the context's, already taken**; what is left is
            # carrying it out, and the acknowledgement the crate wrote.
            stepped = self.app.stepped(turned.get("stepped") or {}, self)
            self.echo.send(outcome.get("answer"))
            return stepped
        return self._take(outcome)

    def _route(self, args) -> bool:
        """One ``/gui_event`` payload, with the stamp already taken off: the
        same turn as a message, unstamped."""
        self._sync_core()
        wid, tag, values = args[0], args[1], list(args[2:])
        turned = self._editing.event(self._member, "/gui_event",
                                     _plain([wid, 0, 0, tag, *values]))
        return self._take(turned.get("outcome") or {})

    def _take(self, outcome: dict) -> bool:
        """Carry out what a turn came to, and answer the host. Returns whether
        the take changed."""
        if outcome.get("turn") in (None, "nothing"):
            return False
        changed = bool(outcome.get("changed"))
        if changed:
            # **The write is this client's to carry out**: the samples are in
            # the server's buffer, and the crate answered what to write there.
            # The entry is already recorded and the version moved: both are the
            # context's.
            self.domain.project(self.structure, outcome["edit"])
            self.dirty = True
            self._editing.changed()
        if outcome.get("locate") is not None:
            self.cursor = float(outcome["locate"])
            self.locate(self.cursor)
            if self.composed_in is not None:
                self.composed_in.locate(self.cursor)
            if callable(self.on_locate):
                self.on_locate(self.cursor)
        if outcome.get("selection") is not None:
            self.selection = outcome["selection"]
            self.selected()
        self.echo.send(outcome.get("answer"))
        return changed

    @property
    def layers(self) -> tuple:
        """What the picture measures -- `("peak", "rms")` for the editor's
        view, `("peak",)` for the bare envelope.

        **Assigning it on an open view sends one message.** The measure is a
        live `/gui_set` prop, so the body appears and disappears over the peaks
        with the picture, the axis, the zoom, the selection and the playhead all
        exactly where they were. Redrawing for this would be the wrong tool
        twice over: a redefine rebuilds every widget (so a handler bound to one
        by name is left holding an id nobody answers to) and the window it
        redefines is reopened.
        """
        return self.view.layers

    @layers.setter
    def layers(self, stack) -> None:
        answer = self._call("layers", stack=[str(name) for name in stack])
        if "error" in answer:
            raise ValueError(answer["error"])
        self.view.layers = tuple(answer["layers"])
        if self._host is not None and self._window is not None:
            for wid in self.view.widgets:
                self._host.set(wid, measure=answer["measure"])


def is_samples(structure) -> bool:
    """Whether `edit` should open this as a take: anything with a buffer
    number, which is what a `clausters.defs.Buffer` answers with."""
    return hasattr(structure, "bufnum") and hasattr(structure, "set_samples")
