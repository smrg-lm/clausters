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
  rectangle (it is the same time-selection gesture, restricted in pitch);
  **Alt+click** toggles a note in/out of the selection;
- **drag a selected note** moves the whole selection (rigid, snapped);
  **Delete/Backspace** removes it; **drag the velocity lane over a selected
  note** nudges all the selected velocities relatively;
- **q** quantizes the selected notes' onsets (or all of them) to the snap
  grid (model-side, ``seq.Timeline.quantize(grid)`` does the same in beats);
- **Ctrl+C / Ctrl+X / Ctrl+V** copy / cut / paste the selection -- the paste
  lands with its first note at the cursor's time (snapped), selected and
  ready to drag; the clipboard travels between rolls and windows.

Every edit flows back per the **edit-back pattern**: the host emits
``/gui_event <id> "notes" <start dur pitch velocity channel ...>`` (the note
list as flat OSC primitives — times/pitch floats, velocity/channel ints) and
``/gui_event <id> "osc" <time label ...>``. This script keeps the two lists in
sync from those events, and the **play** button turns the current notes into a
`Pbind` and plays them on the session clock, so the melody you draw is the
melody you hear.

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention).
Install once, from the repo root::

    python -m venv .venv
    .venv/bin/pip install -e ./clients/python      # bundles the server + GUI binaries

Run it cell by cell (Shift+Enter) and keep drawing/playing from the live
handles, or as a plain script -- ``python clients/python/examples/gui_pianoroll.py``
-- which plays the drawn notes a few times, then tears everything down. Needs a
display and a GPU adapter, plus an audio device.
"""

# %%
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
# A short sine with a percussive envelope that frees its synth — one note per
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
# markers. The pitch window frames an octave around middle C; the beats ruler
# reads the same tempo grid.

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


def scene() -> dict:
    return window(
        label(1, "drag notes; Ctrl+click adds/removes; velocity lane; play sends it"),
        pianoroll(10, notes=NOTES, osc=OSC, min=48, max=84, snap=beats(0.25),
                  ruler="beats", tempo=BPM / 60.0, sample_rate=SR, label="lead"),
        button(20, label="play"),
        title="Piano-roll -> Pbind", w=800, h=520, layout="col",
    )


win = gui.open(scene())
print(f"opened window {win} -- draw the notes, then press play")

# %% [markdown]
# ## Hear the drawn notes
# `play()` turns the current note list into a monophonic `Pbind` (notes sorted
# by onset; each event's ``dur`` is the beats to the next onset) and plays it on
# the session clock. The pitches are the MIDI note numbers straight off the roll.

# %%
_notes = list(NOTES)  # kept in sync from the "notes" edit-back
_osc = list(OSC)
_closed = False


def play():
    """Play the currently-drawn notes as a monophonic sequence."""
    if not _notes:
        print("no notes to play")
        return
    seq = sorted(_notes, key=lambda n: n[0])
    pitches, durs, amps = [], [], []
    for i, (start, dur, pitch, vel) in enumerate(seq):
        pitches.append(int(pitch))
        # The beats to the next onset (the last note holds its own duration).
        nxt = seq[i + 1][0] if i + 1 < len(seq) else start + dur
        durs.append(max((nxt - start) / SAMPLES_PER_BEAT, 0.05))
        amps.append(vel / 127.0)
    session.play(Pbind(
        instrument="gui_pr_voice",
        midinote=Pseq(pitches, 1),
        dur=Pseq(durs, 1),
        amp=Pseq(amps, 1),
        legato=0.9,
    ))
    print(f"played {len(seq)} notes")


def drain_events():
    """Reads pending events: note/OSC edits update the local lists (silently, so
    they print as they arrive), the play button triggers the sequence."""
    global _notes, _osc, _closed
    while (msg := gui.poll(0.0)) is not None:
        addr, args = msg
        if addr == "/gui_closed":
            _closed = True
        elif addr == "/gui_event" and len(args) >= 2 and args[1] == "notes":
            # id, "notes", then start dur pitch velocity channel quintuples.
            flat = list(args[2:])
            _notes = [
                (flat[i], flat[i + 1], int(flat[i + 2]), int(flat[i + 3]))
                for i in range(0, len(flat) - 4, 5)
            ]
            print(f"notes edited: {len(_notes)} notes")
        elif addr == "/gui_event" and len(args) >= 2 and args[1] == "osc":
            flat = list(args[2:])
            _osc = [(flat[i], flat[i + 1]) for i in range(0, len(flat) - 1, 2)]
            print(f"osc edited: {len(_osc)} events")
        elif addr == "/gui_event" and args[0] == 20 and args[1] == 1:
            play()


play()

# %% [markdown]
# ## Set the notes from the script
# The note list is settable live -- a ``/gui_set`` value is a scalar, so the
# array rides as its JSON string (the same carrier ``points`` uses). Here: a
# rising scale replaces the melody, and the window redraws it.

# %%
import json

_scale = [(beats(i * 0.5), beats(0.5), 60 + i * 2, 100) for i in range(8)]
_flat = [x for n in _scale for x in (n[0], n[1], float(n[2]), n[3], 0)]
gui.set(10, notes=json.dumps(_flat))
_notes = _scale

# %% [markdown]
# ## Paint notes from a MIDI keyboard (optional cell)
# The client-side live input: a `MidiFunc` pair catches note-on/off from the
# client's virtual MIDI port (route a keyboard into **"clausters-in"**) and
# paints each note into the roll via ``/gui_set``, timed by the session clock
# from the first key down. The host has the same feature natively: open the
# roll with ``midi_in=True`` (a `pianoroll` prop) and route a device into the
# host's **"clausters-gui"** port -- notes paint at the running playhead, or
# step-enter on the snap grid when the transport is stopped. That native path
# is the standalone story: no language client required.

# %%
def record_midi():
    """Arm the client-side MIDI painting (run this cell interactively)."""
    from clausters.responders import MidiFunc

    held, t0 = {}, None

    def paint():
        flat = [x for n in _notes for x in (n[0], n[1], float(n[2]), n[3], 0)]
        gui.set(10, notes=json.dumps(flat))

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
# ## Plain-script run
# Cell-run: keep drawing and call `play()` / `drain_events()` between cells.
# Script-run: draw for a while -- edits print as they arrive, the **play**
# button sends the sequence -- then everything is torn down.

# %%
if __name__ == "__main__":
    try:
        deadline = time.monotonic() + 45.0
        next_play = time.monotonic() + 8.0
        while time.monotonic() < deadline and not _closed:
            drain_events()
            if time.monotonic() >= next_play:
                play()
                next_play += 8.0
            time.sleep(0.05)
        gui.close(win)
        session.close()
    except (OSError, RuntimeError, ConnectionError) as e:
        sys.exit(str(e))
