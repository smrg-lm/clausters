#!/usr/bin/env python3
"""One take, opened on its own: the editor's dedicated signal view.

In the multitrack a take is a **clip's body** — drawn at a clip's size, next to
everything else in the piece. This opens the same element by itself, at the size
of a window: `clausters.gui.FormEditor.open_signal`, the sibling of the dedicated
piano roll.

**It is an editor of the take, not a picture of it.** `open_signal` composes a
`clausters.gui.editing.SamplesEditor` — the same one `clausters.gui.edit` opens
over a bare buffer — joined to this piece's editing context, so a stroke drawn
here writes the server's buffer and undoes in the piece's own order. Drawing is
a gesture the widget has to be put into (`examples/editors/composed.py` shows
the toggle); this file stays on the picture and the measures.

What it shows:

- **The stack.** ``layers=("peak", "rms")`` draws two measures of one take: what
  the signal *reached* (the min/max envelope) and what it *held* (the level
  body, drawn inside it). Both are one heavy view measuring twice — one axis,
  one ruler, one selection, one playhead, one upload of the samples — because
  every view of a signal paints its own field before it draws, so two of them on
  one rectangle would not layer: the second would hide the first. The button
  toggle shows and hides the body over the peaks, and that is the point: the
  measure is a live prop, so it costs one message and the view does not move.
- **The measure costs no second read.** Both pictures come off the same peak
  pyramid the host already built: the mean square rides in it beside the min and
  max, at every resolution level, so zooming cross-fades both pictures together.
- **A selection is of the element.** Sweep a range and the editor keeps it in
  **beats**, naming the element it was swept on — the value an operation is
  handed (`clausters.gui.FormEditor.resolve_selection`), not screen state.
- **What a signal view will not open.** A generator has no samples until it is
  rendered, so the last cell asks for one and prints the refusal rather than
  opening a window over nothing.

Zoom with the **wheel**, pan with **Shift+drag**, sweep a selection with a plain
drag, ``r`` resets the view. **play** sounds the take through the def named to
play it, and the playhead tracks what you hear.

Needs an audio device, a display and a GPU adapter. With the client importable
(``pip install ./clients/python`` or ``PYTHONPATH=clients/python``)::

    python clients/python/examples/views/take.py

Organized as ``# %%`` cells: step through it with Shift+Enter and the window
stays up between cells, or run it as a plain script.
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
from clausters.form import Sequence, Vector
from clausters.gui import FormEditor, button, panel, toggle
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

# `query_info` rather than the launch options: it is the one spelling both
# clients have, so this file and its page twin ask the same question.
SR = server.query_info().nominal_sample_rate
folder = Path(tempfile.mkdtemp(prefix="clausters-take-"))

# %% [markdown]
# ## The take
# A phrase bounced offline and loaded from the file (a buffer is loaded or
# generated on the server, never push-filled). Its shape is what makes the two
# measures worth showing together: a legato line, so where two notes overlap
# the peaks jump (0.35 to 0.5 in the bounce) while the level barely moves —
# what the signal *reached* against what it *held*, which is the whole reason an
# editor draws both.
#
# **Its length is part of the point.** A level is an average over a duration,
# and the duration is the signal's, not the view's: the body averages a fixed
# 50 ms of source (2400 samples at 48 kHz) whatever the zoom, so its values
# stand still while you navigate. What ends it is the envelope, which does
# narrow as you zoom in — once it has come down onto the level the two are
# saying the same thing and the layer goes, in one step. Six seconds across a
# window is a few hundred samples a column, which is where the two measures
# have something different to say.

# %%
PHRASE = [48, 55, 60, 67, 64, 60, 55, 52, 60, 67, 72, 67]
BEATS = float(len(PHRASE))          # one note per beat
SECONDS = BEATS / TEMPO             # ...and how long that is, which is what a
                                    # take's length is measured in
wav = folder / "phrase.wav"
offline = Session.nrt(tempo=TEMPO)
offline.play(Pbind(midinote=Pseq(PHRASE, 1), dur=1.0, legato=0.9, amp=0.35))
offline.render(sample_rate=SR, channels=1, path=str(wav))
buf = ServerBuffer.read(str(wav), server=server)
take = Vector(buf, duration=SECONDS, instrument="take")
print(f"bounced and loaded {wav.name}: buffer {buf.bufnum}, {buf.frames} frames")

# %% [markdown]
# ## The view
# One element, one editor, one window. ``layers`` is the stack, back to front —
# and the whole difference between the editor's picture and a bare envelope.

# %%
#: What the toggle turns on and off — the level body, always drawn over the
#: peaks. The peaks are the picture; the body is a reading laid inside it.
WITH_BODY = ("peak", "rms")
BARE = ("peak",)

bar = panel(button(name="play", label="play"),
            button(name="stop", label="stop"),
            toggle(name="body", label="rms", value=True),
            layout="row", h=34.0)

gui = session.gui()
editor = FormEditor(take, sample_rate=SR, tempo=TEMPO, extra=[bar],
                title="Clausters take")
win = editor.open_signal(gui, layers=WITH_BODY)
session.start()


def show_body(on):
    """Show or hide the level body over the peaks — **one message, no redraw**.

    The measure is a live prop, so assigning `clausters.gui.FormEditor.layers` on an
    open view sends a single `/gui_set` and the picture changes where it stands:
    the zoom, the selection and the playhead do not move, and the buttons keep
    working. Redrawing would be the wrong tool twice over — a redefine rebuilds
    every widget (so a handler bound by name is left holding an id nobody
    answers to) and the window it redefines is reopened.
    """
    editor.layers = WITH_BODY if on else BARE


win["play"].on_click(lambda: editor.play(server, session.clock))
win["stop"].on_click(editor.stop)
win["body"].on_event(lambda value: show_body(bool(value)))
editor.locate(0.0)

print("wheel zooms, Shift+drag pans, a plain drag sweeps a selection")
print("press play to hear it — the playhead is where the audio is")


# %% [markdown]
# ## The loop
# `FormEditor.poll` drains the window's events into the editor. A selection is not an
# edit — nothing in the composition changes — so it is read off the editor rather
# than waited for, and printed as it moves.

# %%
def run():
    """Hold until the window is closed — a by-eye and by-ear test ends when the
    person looking at it says so, not on a timer."""
    last = None
    # `windows` rather than `window`: this editor is on screen through the view
    # it composed, and never opened one of its own.
    while editor.windows:
        editor.transport.update()
        editor.poll(0.05)
        if editor.selection and editor.selection != last:
            last = editor.selection
            span = (last["start"], last["start"] + last["len"])
            print(f"selection: {span[0]:.3f} .. {span[1]:.3f} beats "
                  f"of {'the take' if last.get('nodes') else 'the axis'}")


# %% [markdown]
# ## What a signal view will not open
# The generated/generator line, asked at the door: a rendered element has
# samples a view can address, a generator has none until it is rendered. The
# piano roll answers this by showing a bounced generator read-only; a signal view
# cannot, because notes can be bounced for a picture and samples cannot be
# invented — so it refuses, and says what to do.

# %%
def refusal() -> str:
    """The message a generator gets from `FormEditor.open_signal` — raised by the
    call itself, before any window exists, which is why asking for one here
    leaves nothing open."""
    generator = Sequence(Pbind(midinote=Pseq([60, 62], 1), dur=1.0))
    try:
        FormEditor(generator, sample_rate=SR, tempo=TEMPO).open_signal(gui)
    except ValueError as err:
        return str(err)
    return "no refusal — which would be the bug"


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    print(f"a generator asked for a signal view: {refusal()}")
    try:
        run()
    finally:
        session.close()
else:
    print("up — run() to hold the window, session.close() to end")
