#!/usr/bin/env python3
"""Paint a MIDI keyboard into the ``pianoroll``, from the client.

The recording leg of `pianoroll.py` (that one draws and plays a scale; this
one writes the roll from what you play). A `MidiFunc` pair listens on the
client's virtual MIDI **input** port and paints each key into a named roll with
``/gui_set``: a note-on appends a note at the beat the session clock is on
(counted from the first key down), and the matching note-off writes its real
duration.

The host has the same feature natively -- open the roll with ``midi_in=True``
and route a device into the host's own **"clausters-gui"** port -- and both legs
exist on purpose: the native one records with no client in the loop, this one
puts the notes under the script's control (they are a plain Python list here, so
the take can be quantized, transposed or played before it goes back to the roll).
To *sound* a keyboard instead of drawing it, see `midi_responder.py`.

Needs the live cdylib (the virtual input port)::

    cargo build --release -p clausters-midi --features live

The port ``clausters-in`` is a loose cable until you wire a MIDI source into it:
list ports with ``pw-link -o`` / ``-i`` and wire them with ``pw-link``, or
visually in qpwgraph; with raw ALSA, ``aconnect``.

Cells (``# %%``) as usual, and it runs out of the box::

    python clients/python/examples/editors/pianoroll_midi.py

Needs a display and a GPU adapter.
"""

# %%
import json
import sys

from clausters import Session
from clausters.base import MidiReceiver
from clausters.gui import label, pianoroll, view
from clausters.responders import MidiFunc

# %% [markdown]
# ## The session, the roll and the MIDI port
# The clock runs at the roll's tempo, so a beat counted off the clock lands where
# it looks on the grid. The roll opens empty -- the take fills it.

# %%
BPM = 120.0
SR = 48_000.0
SAMPLES_PER_BEAT = SR * 60.0 / BPM      # the timeline sample units the widget uses

session = Session.live(tempo=BPM / 60.0)
gui = session.gui()


def beats(b: float) -> float:
    return b * SAMPLES_PER_BEAT


win = view(
    label(name="hint", text='play into "clausters-in" -- the keys land in the roll',
          text_size=1.4, h=20),
    pianoroll(name="roll", notes=[], min=48, max=84, snap=beats(0.25),
              ruler="beats", tempo=BPM / 60.0, sample_rate=SR, label="take"),
    title="MIDI -> piano-roll", w=800, h=520, layout="col").open()
print(f"opened window {win} -- wire a keyboard into \"clausters-in\" and play")

session.start()                          # the clock times the take
recv = MidiReceiver(port="clausters-in").start()

# %% [markdown]
# ## Paint what is played
# ``notes`` is the take, in the roll's own units: ``(start, duration, pitch,
# velocity)``. A note-on appends one and a note-off closes it; `paint` sends the
# whole list back as the flat quintuple carrier (a ``/gui_set`` value is a scalar,
# so the array rides as its JSON string). The roll's ``"notes"`` edit-back keeps
# the list current when *you* edit it, so both hands write to the same take.

# %%
notes = []      # the take
held = {}       # (channel, note) -> its index in `notes`, while the key is down
t0 = None       # the beat the take started on


def paint():
    """Send the take to the roll."""
    win["roll"].set(notes=json.dumps(
        [x for n in notes for x in (n[0], n[1], float(n[2]), n[3], 0)]))


def note_on(msg, _src):
    global t0
    if msg["velocity"] == 0:             # running-status note-off
        return note_off(msg, _src)
    if t0 is None:
        t0 = session.clock.beats()       # the take starts at the first key down
    held[(msg["channel"], msg["note"])] = len(notes)
    notes.append((beats(session.clock.beats() - t0), beats(0.25),
                  msg["note"], msg["velocity"]))
    paint()


def note_off(msg, _src):
    i = held.pop((msg["channel"], msg["note"]), None)
    if i is None:
        return
    start, _, pitch, velocity = notes[i]
    dur = max(beats(session.clock.beats() - t0) - start, beats(0.05))
    notes[i] = (start, dur, pitch, velocity)
    paint()


def on_roll(tag, *vals):
    """The roll's edit-back: the flat ``"notes"`` quintuples become the take."""
    global notes
    if tag == "notes":
        notes = [tuple(vals[i:i + 4]) for i in range(0, len(vals), 5)]
        held.clear()                     # the indexes moved with the edit
        print(f"take: {len(notes)} notes")


win["roll"].on_event(on_roll)

on = MidiFunc(note_on, "note_on", recv=recv)
off = MidiFunc(note_off, "note_off", recv=recv)

# %% [markdown]
# ## Drive it
# Play. The roll's edits arrive on the host's event loop and the MIDI responders
# fire on the receiver's own thread, so a script has nothing left to do but hold
# the window open. Everything is freed on the way out.

# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        win.wait()
    finally:
        on.free()
        off.free()
        recv.stop()
        session.close()
else:
    print("roll up - play your keyboard; on.free() / off.free() / session.close() to end")
