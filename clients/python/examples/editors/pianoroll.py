#!/usr/bin/env python3
"""Draw MIDI notes (and OSC markers) in the editor-grade ``pianoroll`` and hear
them play.

The dedicated piano-roll view, the editor-grade sibling of the multitrack's
compact `clip` roll (they share the host's note primitives, so a note is drawn
and edited the same way in both). It contemplates the two message families a
sequence carries: **MIDI notes** in the grid (pitch x time, with velocity and
channel) and **OSC items** as markers in a lane below it.

Editing gestures, all live and native (the browser keeps display + ``/gui_set``
parity):

- **drag a note** to move it in time and pitch (snapped to the note grid);
- **drag a note's edge** to resize its duration;
- **Ctrl+click** empty grid adds a note there (then drag to set its length);
  **Ctrl+click a note** removes it;
- **drag in the velocity lane** to set a note's velocity;
- **Ctrl+click the OSC lane** adds/removes an event; **drag one** to move it;
- **wheel over the grid** zooms the shared time axis, **Shift+drag** pans it;
- **drag empty grid** also marquee-selects the notes inside the time x pitch
  rectangle; **Alt+click** toggles a note in/out of the selection;
- **drag a selected note** moves the whole selection (rigid, snapped);
  **Delete/Backspace** removes it;
- **q** quantizes the selected notes' onsets (or all) to the snap grid;
- **Ctrl+C / Ctrl+X / Ctrl+V** copy / cut / paste the selection.

The roll opens on one ascending scale, and everything else here is the loop that
sounds it. Every edit flows back per the **edit-back pattern**: the host emits a
``"notes"`` event (``start dur pitch velocity channel ...``) and an ``"osc"``
event (``time label ...``). The roll is *named*, so the script wires one
``on_event`` that keeps its note list current, and the **play/stop** toggle loops
that list as a `Pbind` -- the scale you edit is the scale you hear. Nothing
sounds until you press play.

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention) and
**runs out of the box**. Install once, from the repo root::

    python -m venv .venv
    .venv/bin/pip install -e ./clients/python      # bundles the server + GUI binaries

Run it cell by cell (Shift+Enter) and keep drawing/playing from the live
handles, or as a plain script -- ``python clients/python/examples/editors/pianoroll.py``.
Needs a display and a GPU adapter, plus an audio device.
"""

# %%
import sys

from clausters import Session
from clausters.base import Routine
from clausters.gui import label, pianoroll, toggle, view
from clausters.seq import Pbind, Pseq

# %% [markdown]
# ## Launch the server and the GUI
# `Session.live` connects to a running audio server or starts one; `session.gui()`
# starts ``clausters-gui`` wired to it. The session's clock runs at the roll's own
# tempo (in beats per second), so a note lasts as long as it looks -- a clock left
# at the default 1.0 would sound the roll at half speed.

# %%
BPM = 120.0
SR = 48_000.0
SAMPLES_PER_BEAT = SR * 60.0 / BPM      # the timeline sample units the widget uses
LOOP = 8.0                              # beats between repeats
AMP = 0.2                               # what a full-velocity note reaches

session = Session.live(tempo=BPM / 60.0).activate()
gui = session.gui()


def beats(b: float) -> float:
    return b * SAMPLES_PER_BEAT


# %% [markdown]
# ## Open the piano-roll window
# One ascending major scale, an eighth note each, growing in velocity -- plus two
# OSC markers in the lane below. The widgets are *named*, so the script
# drives and listens to the roll by name; the pitch window frames the octave
# around middle C. The hint and the transport are fixed-height (``h``), so the
# roll -- the only weighted child -- keeps the rest of the window.

# %%
# (start_beat, dur_beat, midi_pitch, velocity)
SCALE = [
    (0.0, 0.5, 60, 90),
    (0.5, 0.5, 62, 95),
    (1.0, 0.5, 64, 100),
    (1.5, 0.5, 65, 105),
    (2.0, 0.5, 67, 110),
    (2.5, 0.5, 69, 115),
    (3.0, 0.5, 71, 120),
    (3.5, 0.5, 72, 125),
]
NOTES = [(beats(s), beats(d), p, v) for (s, d, p, v) in SCALE]
OSC = [(beats(0.0), "/bar"), (beats(2.0), "/bar")]

win = view(
    label(name="hint", text="drag notes; Ctrl+click adds/removes; velocity lane; play loops it",
          text_size=1.4, h=20),
    pianoroll(name="roll", notes=NOTES, osc=OSC, min=48, max=84, snap=beats(0.25),
              ruler="beats", tempo=BPM / 60.0, sample_rate=SR, label="lead"),
    toggle(name="play", label="play", text_size=1.6, h=30),
    title="Piano-roll -> Pbind", w=800, h=520, layout="col").open()
print(f"opened window {win} -- edit the scale, then press play")

# The clock has to run for `session.play` to fire the pattern in real time
# (without it the Pbind is scheduled on a stopped clock and never sounds).
session.start()

# %% [markdown]
# ## Hear the drawn notes
# `play` turns the current note list into a monophonic `Pbind` -- notes by onset,
# each event's ``dur`` the beats to the next one -- on the built-in ``"default"``
# instrument, whose envelope is closed by its gate (a voice freed at its sustain
# instead would cut its own tail and click). `start` / `stop` are the transport
# behind the toggle: `start` schedules `repeat`, a routine that plays the roll
# every `LOOP` beats on the session clock.

# %%
notes = list(NOTES)     # kept current from the "notes" edit-back
player = None           # the sequence playing, if any
loop = None             # the repeat routine, while the transport is on


def play():
    """Play the drawn notes once, replacing whatever is playing."""
    global player
    if player is not None:
        player.stop()   # the roll is one voice: never two sequences at once
    seq = sorted(notes)
    pitches, durs, amps = [], [], []
    for i, (start, dur, pitch, vel) in enumerate(seq):
        nxt = seq[i + 1][0] if i + 1 < len(seq) else start + dur   # the next onset
        pitches.append(pitch)
        durs.append(max((nxt - start) / SAMPLES_PER_BEAT, 0.05))
        amps.append(vel / 127.0 * AMP)
    player = session.play(Pbind(instrument="default", midinote=Pseq(pitches, 1),
                                dur=Pseq(durs, 1), amp=Pseq(amps, 1), legato=0.9))


def repeat():
    """Play the roll every `LOOP` beats, until stopped."""
    while not win.closed:
        play()
        yield LOOP


def start(*_):
    """Start the loop, from this beat."""
    global loop
    stop()
    loop = Routine(repeat).play()
    win["play"].set(label="stop")


def stop(*_):
    """Stop the loop and cut the sequence still playing, if any."""
    global loop, player
    if loop is not None:
        session.clock.unsched(loop)
        loop = None
    if player is not None:
        player.stop()
        player = None
    win["play"].set(label="play")


def on_roll(tag, *vals):
    """The roll's edit-backs: flat ``"notes"`` quintuples (kept, so the loop
    plays what is drawn) or ``"osc"`` pairs."""
    global notes
    if tag == "notes":
        notes = [tuple(vals[i:i + 4]) for i in range(0, len(vals), 5)]
        print(f"notes edited: {len(notes)}")
    elif tag == "osc":
        print(f"osc edited: {len(vals) // 2}")


win["roll"].on_event(on_roll)
win["play"].on_event(lambda value: start() if value == 1 else stop())

# %% [markdown]
# ## Drive it
# Cell-run: keep editing, and `start()` / `stop()` (or the toggle) between cells.
# Script-run: hold the window open. Neither dispatches anything -- the roll's
# edits arrive on the host's event loop, so they print as they are made and the
# toggle drives the transport. Timing is nobody's business here either; it
# belongs to the routine on the session clock, in the same beats the roll is
# drawn in.

# %%
def run(seconds: float | None = None) -> None:
    """Hold the window open for ``seconds``; the toggle is the transport."""
    win.wait(seconds)
    stop()


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        run()
    finally:
        session.close()
else:
    print("pianoroll up - press play, or call start() / stop(); session.close() to end")
