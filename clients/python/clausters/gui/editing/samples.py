"""Editing a **buffer's samples**: the one domain whose state is not here.

A curve's points and a timeline's events are values this process holds, so the
crate can be handed one and asked what an edit makes of it. A span of samples is
not: the frames are in a **server buffer**, which is why the crate's own
`Samples` is a borrowed view over memory its caller owns, and why
`clausters._native.domain_edit` answers nothing for this vocabulary. What is
shared is the payload's shape and its coalesce key; where the state lives is the
client's, and this module is that half.

**The inverse rides on the wire.** A ``"draw"`` carries the run it wrote *and*
the run it replaced, and a ``"sample"`` carries the value and the previous one —
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

**A stroke lands on one channel.** The samples are interleaved, so writing one
channel of a stereo take is a strided write and ``/buffer_setRange`` is
contiguous: the span is read, the channel's frames are spliced into it and the
run goes back whole. Mono needs neither, which is the ordinary take.
"""

import struct
from array import array

from ... import _native
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
    server holds."""

    name = _native.SAMPLES
    ingested = True

    def request(self, structure, tag: str, values) -> dict:
        """The report, with the two runs decoded.

        **The wire's own framing is this client's.** A `draw` carries its run as
        a little-endian ``f32`` blob, which is a `memoryview` here and an
        `ArrayBuffer` in the page and cannot be either in a JSON request — so
        the blob is read into numbers and the crate reads the numbers.
        """
        values = list(values)
        if tag == "draw" and len(values) >= 4:
            values[2], values[3] = _floats(values[2]), _floats(values[3])
        return {"values": values}

    def current(self, structure, payload: dict) -> "dict | None":
        """What the stroke replaced, as the write that puts it back.

        **The one vocabulary whose inverse arrives with the gesture**: the
        payload states the run written and has no field for the run it
        replaced, and the host sends both in the same event. So there is nothing
        to read off the structure — by the time this is asked, the answer is
        already in the reading, and it is ``None`` when the two runs did not
        cover the same span, which is an entry the pile cannot invert and is
        better recorded as one than pretended.
        """
        return self._taken.get("inverse")

    def project(self, structure, payload: dict) -> bool:
        channels = max(1, int(getattr(structure, "channels", 1) or 1))
        channel = min(int(payload.get("channel", 0)), channels - 1)
        start = int(payload.get("start", 0))
        values = [float(v) for v in payload.get("values") or ()]
        if not values:
            return False
        if channels == 1:
            structure.set_samples(values, start=start)
            return True
        # Interleaved: read the frames the stroke covers, splice this channel's
        # into them and write the run back whole. One extra round trip on a
        # multi-channel take, and none on a mono one.
        first = start * channels
        span = list(structure.get_samples(first, len(values) * channels))
        if len(span) < len(values) * channels:
            span += [0.0] * (len(values) * channels - len(span))
        for i, value in enumerate(values):
            span[i * channels + channel] = value
        structure.set_samples(span, start=first)
        return True


class SamplesView(View):
    """One `clausters.gui.guidef.waveform`: the take on its own axis, drawn by
    the host straight from the server buffer, with the measures it stacks as a
    prop of that one widget (see the module docstring).

    **The window is the application's**, composed in the shared crate
    (`clausters._native.SamplesEditorCore`): the waveform, the gesture plan a
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
        tree = editor._core.call("window", widget=wid)
        # **A script's own widgets are its objects**, so they are appended here
        # rather than composed in the crate.
        tree["children"] = [*tree.get("children", ()), *editor.extra]
        return tree

    def props(self, editor, widget_id: int) -> dict:
        return editor._core.call("props", widget=int(widget_id))


class SamplesEditor(Editor):
    """A buffer's samples on screen, editable back into the server's buffer.

    The picture and the sound are the **same** buffer: the host draws what the
    server holds, and a stroke writes there — so what is heard after an edit is
    what is seen, with no copy in between.
    """

    def __init__(self, take, *, sample_rate: float = 0.0, tempo: float = 1.0,
                 title: str = "Samples", layers=MEASURES, **options):
        rate = float(sample_rate or getattr(take, "sample_rate", 0.0) or 48_000.0)
        view = SamplesView(layers)
        super().__init__(take, sample_rate=rate, tempo=tempo,
                         domain=SamplesDomain(), view=view,
                         title=title, **options)
        #: **The window, in the shared crate**: the take, the measures and the
        #: chrome it is composed from.
        self._core = _native.SamplesEditorCore(
            {**self._facts(), "layers": list(view.layers)})

    def _facts(self) -> dict:
        take = self.structure
        name = getattr(take, "name", None)
        return {"buffer": int(getattr(take, "bufnum", 0) or 0),
                "channels": max(1, int(getattr(take, "channels", 1) or 1)),
                "name": name if isinstance(name, str) and name else None,
                "rate": self.sample_rate, "tempo": self.tempo,
                "title": self.title, "w": int(self.size[0]), "h": int(self.size[1])}

    def _sync_core(self) -> None:
        """Hand the core what this client holds: the take a script may have
        resized, the axis and the window's chrome."""
        self._core.call("sync", **self._facts())

    @property
    def layers(self) -> tuple:
        """What the picture measures — `("peak", "rms")` for the editor's
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
        answer = self._core.call("layers", stack=[str(name) for name in stack])
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
