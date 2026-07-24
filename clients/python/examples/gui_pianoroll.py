#!/usr/bin/env python3
"""Draw MIDI notes (and OSC events) in the editor-grade ``pianoroll`` and hear
them play.

The dedicated piano-roll view, the editor-grade sibling of the multitrack's
compact `clip` roll (they share the host's note primitives, so a note is drawn
and edited the same way in both). It contemplates the two message families a
sequence carries: **MIDI notes** in the grid (pitch x time, with velocity and
channel) and **OSC events** as flags in a lane below it.

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

Every edit flows back per the **edit-back pattern**: the host emits a ``"notes"``
event (``start dur pitch velocity channel ...``) and an ``"osc"`` event
(``time label ...``). Here the roll is *named*, so the script wires one
``on_event`` that keeps the two lists in sync, and the **play** button turns the
current notes into a `Pbind` -- the melody you draw is the melody you hear.

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention) and
**runs out of the box**. Install once, from the repo root::

    python -m venv .venv
    .venv/bin/pip install -e ./clients/python      # bundles the server + GUI binaries

Run it cell by cell (Shift+Enter) and keep drawing/playing from the live
handles, or as a plain script -- ``python clients/python/examples/gui_pianoroll.py``.
Needs a display and a GPU adapter, plus an audio device.
"""

# %%
import json
import sys
import time

from clausters import Session
from clausters.defs import DoneAction, Env, SynthDef, control, env_gen, out, sine
from clausters.gui import button, label, pianoroll, window
from clausters.seq.pattern import Pbind, Pseq

# %% [markdown]
# ## Launch the server and the GUI
# `Session.live` connects to a running audio server or starts one; `session.gui()`
# starts ``clausters-gui`` wired to it.

# %%
session = Session.live()
server = session.server
gui = session.gui()

# A tempo grid so the roll reads in beats: the notes below are authored in beats
# and converted to the timeline sample units the widget uses.
BPM = 120.0
SR = 48_000.0
SAMPLES_PER_BEAT = SR * 60.0 / BPM


def beats(b: float) -> float:
    return b * SAMPLES_PER_BEAT


# %% [markdown]
# ## The voice the notes play
# A short sine with a percussive envelope that frees its synth -- one note per
# `Pbind` event.

# %%
def voice() -> SynthDef:
    freq = control("freq", 440.0)
    amp = control("amp", 0.2)
    env = env_gen(Env.perc(0.005, 0.35), done_action=DoneAction.FREE_SELF)
    sig = sine(freq) * env * amp
    return SynthDef("gui_pr_voice", out(0.0, sig), out(1.0, sig))


server.add_synthdef(voice())

# %% [markdown]
# ## Open the piano-roll window
# A two-bar melody (beats -> samples) with per-note velocity, plus two OSC event
# markers. The widgets are *named*, so the script drives and listens to the roll
# by name. The pitch window frames an octave around middle C.

# %%
# (start_beat, dur_beat, midi_pitch, velocity)
MELODY = [
    (0.0, 0.5, 60, 100),
    (0.5, 0.5, 64, 90),
    (1.0, 0.5, 67, 105),
    (1.5, 1.0, 72, 120),
    (3.0, 1.0, 65, 80),
]
NOTES = [(beats(s), beats(d), p, v) for (s, d, p, v) in MELODY]
OSC = [(beats(0.0), "/bar"), (beats(2.0), "/bar")]

win = gui.open(window(
    label(name="hint", text="drag notes; Ctrl+click adds/removes; velocity lane; play sends it"),
    pianoroll(name="roll", notes=NOTES, osc=OSC, min=48, max=84, snap=beats(0.25),
              ruler="beats", tempo=BPM / 60.0, sample_rate=SR, label="lead"),
    button(name="play", label="play"),
    title="Piano-roll -> Pbind", w=800, h=520, layout="col"))
print(f"opened window {win} -- draw the notes, then press play")

# The clock has to run for `session.play` to fire the pattern in real time
# (without it the Pbind is scheduled on a stopped clock and never sounds).
session.start()

# %% [markdown]
# ## Hear the drawn notes
# `play()` turns the current note list into a monophonic `Pbind` (notes sorted by
# onset; each event's ``dur`` is the beats to the next onset) and plays it on the
# session clock.

# %%
_notes = list(NOTES)  # kept in sync from the "notes" edit-back
_osc = list(OSC)
_closed = False


def play(*_):
    """Play the currently-drawn notes as a monophonic sequence."""
    if not _notes:
        print("no notes to play")
        return
    seq = sorted(_notes, key=lambda n: n[0])
    pitches, durs, amps = [], [], []
    for i, (start, dur, pitch, vel) in enumerate(seq):
        pitches.append(int(pitch))
        nxt = seq[i + 1][0] if i + 1 < len(seq) else start + dur
        durs.append(max((nxt - start) / SAMPLES_PER_BEAT, 0.05))
        amps.append(vel / 127.0)
    session.play(Pbind(instrument="gui_pr_voice", midinote=Pseq(pitches, 1),
                       dur=Pseq(durs, 1), amp=Pseq(amps, 1), legato=0.9))
    print(f"played {len(seq)} notes")


def on_roll(tag, *vals):
    """The roll's edit-backs: ``"notes"`` quintuples or ``"osc"`` pairs, kept in
    the local lists (silently) so `play` hears what is drawn."""
    global _notes, _osc
    flat = list(vals)
    if tag == "notes":
        _notes = [(flat[i], flat[i + 1], int(flat[i + 2]), int(flat[i + 3]))
                  for i in range(0, len(flat) - 4, 5)]
        print(f"notes edited: {len(_notes)} notes")
    elif tag == "osc":
        _osc = [(flat[i], flat[i + 1]) for i in range(0, len(flat) - 1, 2)]
        print(f"osc edited: {len(_osc)} events")


win["roll"].on_event(on_roll)
win["play"].on_event(lambda value: play() if value == 1 else None)  # 1 = press
win.on_closed(lambda: globals().__setitem__("_closed", True))

# %% [markdown]
# ## Hear it now, and set the notes from the script
# Play the seed once, then replace the melody live -- a ``/gui_set`` value is a
# scalar, so the array rides as its JSON string (the carrier ``points`` uses).

# %%
play()
_scale = [(beats(i * 0.5), beats(0.5), 60 + i * 2, 100) for i in range(8)]
win["roll"].set(notes=json.dumps([x for n in _scale for x in (n[0], n[1], float(n[2]), n[3], 0)]))
_notes = _scale

# %% [markdown]
# ## Paint notes from a MIDI keyboard (optional cell)
# The client-side live input: a `MidiFunc` pair catches note-on/off from the
# client's virtual MIDI port (route a keyboard into **"clausters-in"**) and
# paints each note into the roll via ``set``, timed by the session clock from the
# first key down. The host has the same feature natively: open the roll with
# ``midi_in=True`` and route a device into the host's **"clausters-gui"** port.

# %%
def record_midi():
    """Arm the client-side MIDI painting (run this cell interactively)."""
    from clausters.responders import MidiFunc

    held, t0 = {}, None

    def paint():
        win["roll"].set(notes=json.dumps(
            [x for n in _notes for x in (n[0], n[1], float(n[2]), n[3], 0)]))

    def on(msg, _src):
        nonlocal t0
        if t0 is None:
            t0 = session.clock.beats
        at = beats(session.clock.beats - t0)
        held[(msg["channel"], msg["note"])] = len(_notes)
        _notes.append((at, beats(0.25), msg["note"], msg["velocity"]))
        paint()

    def off(msg, _src):
        i = held.pop((msg["channel"], msg["note"]), None)
        if i is not None and i < len(_notes):
            start, _, pitch, vel = _notes[i]
            end = beats(session.clock.beats - t0)
            _notes[i] = (start, max(end - start, beats(0.05)), pitch, vel)
            paint()

    return MidiFunc(on, "note_on"), MidiFunc(off, "note_off")


# funcs = record_midi()   # arm it, play, then: [f.free() for f in funcs]

# %% [markdown]
# ## Drive it
# Cell-run: keep drawing and call `play()` between cells. Script-run: pump events
# for a while -- edits print as they arrive, the **play** button and a timer send
# the sequence -- then everything is torn down.

# %%
def run(seconds: float) -> None:
    """Dispatches roll events for ``seconds``, replaying every 8 s."""
    start = time.monotonic()
    next_play = start + 8.0
    while time.monotonic() - start < seconds and not _closed:
        gui.pump(timeout=0.05)
        if time.monotonic() >= next_play:
            play()
            next_play += 8.0


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        run(45.0)
    finally:
        session.close()
else:
    print("pianoroll up - run(10) to dispatch events, session.close() to end")
