#!/usr/bin/env python3
"""The whole loop: compose, edit by hand, hear it, undo it, save it, open it again.

`gui_composer.py` is about the **material** — what the five primitives are, how
a group places them, how a curve drives a voice. This one is about the **loop**,
and deliberately builds the smallest composition that can show it: two lanes, a
take and a melody.

What the loop is, and why each step needs the one before it:

1. **Edit.** Drag a clip. The gesture leaves the host as an *intent* — where the
   hand put it, absolute — and the **shared crate** decides what it becomes:
   the musical grid snaps it there, not here. What comes back is the value that
   actually holds, and the window adopts it. So the clip lands on the grid even
   though nothing on this side snapped anything.
2. **Hear it.** With ``follow=True`` the composition is re-scheduled from the
   playhead on every edit, so you hear the clip where you dropped it.
3. **Undo it** — **Ctrl+Z**, or the button. The history is **not this editor's**:
   it lives with the document, in the same crate. A log a view keeps sees only
   the gestures *that view* made, so a script editing the arrangement, or a
   second window on the same piece, would leave it describing a composition that
   has moved on — and undoing would then write a state nobody was ever in.
4. **Save it.** A *session* is the document plus the one half a document
   deliberately lacks: the table saying where its material lives. Written here
   beside the WAV it references, with the **provenance** of the script that made
   it — carried opaquely, which is what makes re-generating possible without the
   format knowing how.
5. **Open it again.** The file rebuilds the arrangement, and the node ids survive
   it — so the reopened piece is the same composition by *identity*, not merely
   by shape.

**One thing to try that is not a button**, because it is the rule the whole
placement model rests on: shorten a clip over its own notes. You hear fewer
notes and the element keeps all of them — lengthen it again and they come back.
A placement is a **window onto** an element, never a rewrite of it, which is why
this is reversible and why a resize is not the same act as *rendering* the
element down to what it produced.

Needs an audio device, a display and a GPU adapter. With the client importable
(``pip install ./clients/python`` or ``PYTHONPATH=clients/python``)::

    python clients/python/examples/gui_daw.py

Organized as ``# %%`` cells: step through it with Shift+Enter and the window
stays up between cells, or run it as a plain script.
"""

# %%
import json
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
from clausters.form import Buffer, Group, Track
from clausters.form.document import from_session, to_session
from clausters.gui import Editor, button, panel
from clausters.seq import Timeline
from clausters.seq.event import Event as SeqEvent
from clausters.seq.pattern import Pbind, Pseq

TEMPO = 2.0          # beats per second (120 bpm)
QUANT = 0.5          # the drag grid: half a beat


# %% [markdown]
# ## A server, and the instrument a take sounds through
# A buffer is *data*, so it sounds through the def **named to play it** — the
# arrangement's own rule, and the reason a `Buffer` element carries an
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

SR = float(server.options.sample_rate)
folder = Path(tempfile.mkdtemp(prefix="clausters-daw-"))

# %% [markdown]
# ## The material
# A take bounced offline and **loaded from the file** (a buffer is loaded or
# generated on the server, never push-filled), and a melody of four notes. The
# folder holding the WAV is where the session will be saved too, which is what
# makes the saved file's source table point at something that is still there.

# %%
wav = folder / "take.wav"
offline = Session.nrt(tempo=TEMPO)
offline.play(Pbind(midinote=Pseq([36], 1), dur=2.0, legato=1.0, amp=0.3))
offline.render(sample_rate=SR, channels=1, path=str(wav))
buf = ServerBuffer.read(str(wav), server=server)
print(f"bounced and loaded {wav.name}: buffer {buf.bufnum}")

# Two **elements** over one server buffer, not one element placed twice: the
# material is shared, the placements are not. An element is what a placement
# names, so the same object in two places would be one name for two positions --
# and an edit could not say which one it meant.
take_a = Buffer(buf, duration=2.0, instrument="take")
take_b = Buffer(buf, duration=2.0, instrument="take")
melody = Track(Timeline([
    (0.0, SeqEvent(midinote=72, dur=1.0)),
    (1.0, SeqEvent(midinote=76, dur=1.0)),
    (2.0, SeqEvent(midinote=79, dur=1.0)),
    (3.0, SeqEvent(midinote=84, dur=1.0)),
]))

song = Group([
    (0.0, Group([(0.0, take_a), (4.0, take_b)], name="take")),
    (0.0, Group([(0.0, melody)], name="melody")),
], name="piece")


# %% [markdown]
# ## Saving, and opening again
# A **session** is the document plus the table saying where its material lives.
# The document says *what plays when* and deliberately not where a source is —
# inside a running system a source is a server buffer, a mapped file or a
# rendered result, and the tree has no business knowing which — so the table is
# the half that lets the thing be closed and opened.
#
# `Editor.load` points the open window at the reopened tree. The node ids survive
# the file, so it is the same composition by identity; the **history is dropped**,
# because its inverses describe a session that is over.

# %%
SESSION_FILE = folder / "piece.claust"


def save():
    """Write the composition and where its material is."""
    sources = {
        buf.bufnum: {
            "location": str(wav),
            "lifetime": "session",
            "generation": 0,
            "channels": 1,
            "frames": int(buf.frames),
            "sample_rate": SR,
        }
    }
    document = to_session(editor.element, sources=sources,
                          provenance={"script": "gui_daw.py"})
    SESSION_FILE.write_text(json.dumps(document, indent=2))
    print(f"saved {SESSION_FILE} ({len(SESSION_FILE.read_text())} bytes)")


def reopen():
    """Read it back and show it — the same piece, from the file this time.

    The **source table is resolved by the caller**, and that is the point of it
    being a table rather than something the document knows: what a source *is*
    depends on what is running — a buffer to allocate here, a file to map in a
    viewer, nothing at all in a process that only wants the structure. So the
    table is read first and a resolver closes over it, loading each WAV back
    onto the server; without one, every clip comes back as a reference the
    editor draws with no waveform (which is what it does, rather than
    refusing).
    """
    if not SESSION_FILE.exists():
        return print("nothing saved yet — press save first")
    raw = json.loads(SESSION_FILE.read_text())
    table = {int(k): v for k, v in (raw.get("sources") or {}).items()}

    def resolve(kind, config):
        if kind != "buffer":
            return None
        entry = table.get(int((config or {}).get("source", -1)))
        location = (entry or {}).get("location")
        return None if location is None else ServerBuffer.read(location, server=server)

    element, sources = from_session(raw, resolve=resolve)
    editor.load(element)
    print(f"opened {SESSION_FILE.name}: {len(element)} lanes, "
          f"{len(sources)} source(s) resolved — history cleared, node ids kept")


# %% [markdown]
# ## The window
# The transport, the history and the file, as named buttons the script resolves
# through the window handle. ``follow=True`` re-schedules on every edit, which is
# what makes step 2 of the loop audible.

# %%
bar = panel(button(name="play", label="play"),
            button(name="stop", label="stop"),
            button(name="undo", label="undo"),
            button(name="redo", label="redo"),
            button(name="save", label="save"),
            button(name="open", label="open"),
            layout="row", h=34.0)

gui = session.gui()
editor = Editor(song, sample_rate=SR, tempo=TEMPO, quant=QUANT,
                follow=True, extra=[bar], title="Clausters DAW")
win = editor.open(gui)
session.start()

press = lambda fn: (lambda value: fn() if value == 1 else None)  # noqa: E731
win["play"].on_event(press(lambda: editor.play(server, session.clock)))
win["stop"].on_event(press(editor.stop))
win["undo"].on_event(press(editor.undo))
win["redo"].on_event(press(editor.redo))
win["save"].on_event(press(save))
win["open"].on_event(press(reopen))
editor.locate(0.0)

print("drag a clip, or an edge to trim it — press play to hear it")
print("undo/redo: the buttons, or Ctrl+Z / Ctrl+Shift+Z over the window")


# %% [markdown]
# ## The loop
# `Editor.apply` takes the host's events into the composition and answers each
# one; `poll` drains the window's whole stream into it. The buttons above are the
# script's, so `apply` leaves them alone and their own handlers run.

# %%
def run():
    """Hold until the window is closed — a by-eye and by-ear test ends when the
    person looking at it says so, not on a timer."""
    while editor.window is not None:
        editor.transport.update()
        editor.poll(0.05)


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        run()
    finally:
        session.close()
else:
    print("up — run() to hold the window, session.close() to end")
