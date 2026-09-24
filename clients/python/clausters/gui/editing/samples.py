"""The picture a take is drawn in: one `waveform`, and the measures it stacks.

**What the picture measures is the view's.** A waveform is drawn as a stack of
measures over one field -- what the signal reached (``peak``) with what it held
inside that (``rms``) -- and that is a prop of the one widget rather than a pile
of widgets: every view of a signal paints its own field before it draws, so two
of them on one rectangle are not layers, the second hides the first. Measuring
twice into one body is also what makes the rest of it one thing: one axis, one
ruler, one selection, one playhead, one upload of the samples.

The editor that draws a take in it is `clausters.gui.editing.AudioEditor`.
"""

import struct
from array import array

from ... import _native
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
        meter = self.widget(editor, "meter", editor.structure)
        editor._sync_core()
        tree = editor._call("window", widget=wid, meter=meter)
        # **A script's own widgets are its objects**, so they are appended here
        # rather than composed in the crate.
        tree["children"] = [*tree.get("children", ()), *editor.extra]
        return tree

    def props(self, editor, widget_id: int) -> dict:
        return editor._call("props", widget=int(widget_id))
