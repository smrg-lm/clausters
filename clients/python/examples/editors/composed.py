#!/usr/bin/env python3
"""A multitrack and a take's own editor, over one piece and **one** undo order.

The multitrack shows a take as a clip's body, at a clip's size. Open the same
take by itself — `clausters.gui.FormEditor.open_signal` — and you get the
editor-grade view of its samples, big enough to draw on. What this file is for
is what happens *between* the two windows: they are not two programs sharing a
picture, they are two editors over one composition.

**They are not the same kind of editor, and that is the point.** The multitrack
edits a **tree**, and its edits are intents on a document. The signal window is a
`clausters.gui.editing.SamplesEditor` — the same one `clausters.gui.edit` opens
over a bare buffer — and its edits are writes in the crate's ``samples``
vocabulary, whose state the crate deliberately does not hold. Neither knows what
the other did. What they share is the composition's **editing context**, so the
two are one order: draw on the take, drag the clip, and Ctrl+Z walks back
through both, in the order your hand made them, from whichever window has focus.

What to do, and what to watch:

1. **Draw on the take** in the signal window (the button puts the drag into draw
   mode; a plain drag sweeps a selection). The waveform changes where you drew
   it, and the clip in the multitrack draws the same buffer.
2. **Bend the curve** over the take in the multitrack — drag a break-point of
   the filter sweep.
3. **Ctrl+Z over the multitrack.** The *stroke* comes back first, undone by a
   window that cannot read a samples edit at all: it hands that leg to the
   editor that can. Again, and the curve goes back.
4. **Ctrl+Shift+Z over the signal window** replays both, in order.
5. **press play** in either: one transport, because there is one piece.

The stroke's inverse is not read back from the server — the host sends the run
it wrote *and* the run it replaced in the same event, and what the history
records is the second.

Needs an audio device, a display and a GPU adapter. From the repo root::

    python -m venv .venv
    .venv/bin/pip install -e ./clients/python
    .venv/bin/python clients/python/examples/editors/composed.py

Organized as ``# %%`` cells: step through it with Shift+Enter, or run it as a
plain script.
"""

# %%
import sys
import tempfile
from pathlib import Path

from clausters import Session
from clausters.defs import (
    Buffer as ServerBuffer,
    SynthDef,
    control,
    out,
    play_buf,
)
from clausters.form import Aggregate, Element, Vector
from clausters.gui import FormEditor, button, panel
from clausters.seq import Automation
from clausters.seq.pattern import Pbind, Pseq

TEMPO = 2.0          # beats per second (120 bpm)

# %% [markdown]
# ## A server, and the instrument a take sounds through
# A buffer is *data*, so it sounds through the def **named to play it** — the
# arrangement's own rule, and the reason a `Vector` element carries an
# ``instrument``.

# %%
def sampler(name: str = "take") -> SynthDef:
    """Plays a buffer once, at the length its event gives it."""
    buf = control("buf", 0.0, "ir")
    amp = control("amp", 0.8, "ir")
    sig = play_buf(buf, 0.0, 1.0, 0.0) * amp
    return SynthDef(name, out(0.0, sig), out(1.0, sig))


session = Session.live(tempo=TEMPO, latency=0.1)
server = session.server
sampler().send(server)
SR = server.query_info().nominal_sample_rate
folder = Path(tempfile.mkdtemp(prefix="clausters-composed-"))

# %% [markdown]
# ## The piece: one take, and a curve over it
# The take is a phrase bounced offline and loaded from the file (a buffer is
# loaded or generated on the server, never push-filled). The curve is an
# `Automation` — a break-point envelope placed in time, which is what the
# arrangement draws as a `bpf` body on its clip.

# %%
PHRASE = [60, 67, 64, 72, 69, 76]
BEATS = float(len(PHRASE))
SECONDS = BEATS / TEMPO
wav = folder / "phrase.wav"
offline = Session.nrt(tempo=TEMPO)
offline.play(Pbind(midinote=Pseq(PHRASE, 1), dur=1.0, legato=0.9, amp=0.35))
offline.render(sample_rate=SR, channels=1, path=str(wav))
buf = ServerBuffer.read(str(wav), server=server)
take = Vector(buf, duration=SECONDS, instrument="take", name="phrase")

sweep = Automation.from_points(
    [(0.0, 400.0, 1, 0.0), (SECONDS / 2, 3000.0, 2, 0.0), (SECONDS, 600.0, 1, 0.0)],
    target=None, name="cutoff")
sweep.prepare(server)    # the control buffer + bus, off the clock thread

piece = Aggregate([
    (0.0, Aggregate([(0.0, take)], name="audio")),
    (0.0, Aggregate([(0.0, Element(sweep, duration=SECONDS))], name="filter")),
], name="composed")

# %% [markdown]
# ## The two windows
# One `FormEditor`. `open` draws the multitrack; `open_signal` **composes** a
# `SamplesEditor` over the take and opens its window beside it. The composed
# editor is reachable as `FormEditor.composed` — it has its own selection, its
# own undo and the buffer it holds — and it is joined to this piece's editing
# context, which is the whole of why the two step one history.

# %%
gui = session.gui()
bar = panel(button(name="play", label="play"),
            button(name="stop", label="stop"),
            button(name="draw", label="drag: select"),
            layout="row", h=34.0)

editor = FormEditor(piece, sample_rate=SR, tempo=TEMPO, quant=0.25,
                    extra=[bar], title="Composed", width=900, height=420)
multitrack = editor.open(gui)
signal = editor.open_signal(gui, take)
session.start()          # the clock runs the routines -- play schedules onto it

#: What the drag does in the signal window. A stroke is a gesture the widget has
#: to be put into: the same drag that sweeps a selection cannot also draw, so the
#: mode is a prop and the button flips it.
_mode = "select"


def toggle_mode() -> None:
    """Flip the signal view's drag between sweeping and drawing."""
    global _mode
    _mode = "draw" if _mode == "select" else "select"
    for wid in editor.composed[-1].view.widgets:
        gui.set(wid, gestures={"drag": _mode})
    signal["draw"].set(label=f"drag: {_mode}")


signal["draw"].on_click(toggle_mode)
signal["play"].on_click(lambda: editor.play(server, session.clock))
signal["stop"].on_click(editor.stop)
multitrack["play"].on_click(lambda: editor.play(server, session.clock))
multitrack["stop"].on_click(editor.stop)

_closed = False
multitrack.on_closed(lambda: globals().__setitem__("_closed", True))
signal.on_closed(lambda: globals().__setitem__("_closed", True))

# %% [markdown]
# ## The loop
# `FormEditor.poll` drains the host **and the editors it composed** — one socket,
# so one loop. Each editor answers only for the widgets it drew, and the read-out
# prints what the history says after every event: one label, whichever window is
# about to move it.

# %%
def run():
    """Hold until a window is closed."""
    shown = None
    while not _closed:
        editor.transport.update()
        editor.poll(0.05)
        state = (editor.can_undo, editor.undo_label,
                 editor.can_redo, editor.redo_label)
        if state != shown:
            shown = state
            print(f"undo={state[0]!s:5} {state[1]!r:20} "
                  f"redo={state[2]!s:5} {state[3]!r}")


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    print("draw on the take, bend the curve, then Ctrl+Z over either window")
    try:
        run()
    finally:
        session.close()
    sys.exit(0)
else:
    print("two windows up - run() to hold them, session.close() to end")
