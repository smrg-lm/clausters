#!/usr/bin/env python3
"""A multitrack timeline: tracks of clips placed on one shared time axis.

The DAW-style track editor. A ``track`` is a horizontal lane; a ``clip`` is a
placed rectangle on it spanning ``[offset, offset + dur]`` in timeline sample
units — the model's **graphic unit**, whose *length is its duration*. The
window's tracks share **one time axis** (aligned lanes), so a clip at a given
offset lines up across tracks — the seat the linked-views work
(``gui_linked.py``) designed: a member with a *placement* (offset) on the
shared timeline.

This example lays out three tracks — two audio takes with a decimated waveform
body per clip, and one **piano-roll** lead whose clip carries ``(start, dur,
pitch)`` note events drawn as bars (pitch on the vertical axis). It is plain
GuiDef: new ``track``/``clip`` builders over the unchanged ``/gui_*`` protocol,
no server involved (the bodies are inline here; a real composition would name
mapped files or server buffers).

Dragging a clip (move) or its edge (resize) flows back as a ``"clip"`` event
carrying the new ``offset``/``dur`` — the edit-back pattern — so a driver can
update the composition model and re-realize. This script just prints those
events.

Run it as a script (``python gui_multitrack.py``) or cell by cell (``# %%``).
Needs a display and a GPU adapter; the install bundles the GUI binary (see
``gui_editor.py`` for the setup notes).
"""

# %%
import math
import sys
import time

from clausters import Session
from clausters.gui import clip, label, track, window

# The timeline unit here is "samples"; pick a beat length so the offsets read
# as musical bars for the demo (the ruler unit is cosmetic in this static cut).
BEAT = 12_000  # samples per beat


def blip(cycles: float, n: int = 256, decay: float = 4.0) -> list:
    """A tiny decaying sine, just to give each clip a visible body.

    Kept short on purpose: an inline ``data`` body rides the ``/gui_def`` JSON in
    one OSC datagram, so it must stay well under the ~64 KB UDP limit. The body
    is decimated to the clip's pixel width anyway, so a few hundred samples is
    plenty; a real, long clip would name a mapped file instead of inlining it."""
    return [math.sin(2 * math.pi * cycles * i / n) * math.exp(-decay * i / n)
            for i in range(n)]


# %% [markdown]
# ## Compose the tracks
# Three lanes under one window (a ``col`` layout stacks them). Each clip names
# an ``offset`` (its start on the shared timeline) and a ``dur`` (its length);
# the bodies are inline float lists. The tracks align because the window
# computes one time axis spanning the longest clip end.

# %%
session = Session.live()
gui = session.gui()

DRUMS, BASS, LEAD = 1, 2, 3
win = gui.open(window(
    track(DRUMS,
          clip(10, offset=0 * BEAT, dur=2 * BEAT, data=blip(6), label="kick"),
          clip(11, offset=2 * BEAT, dur=2 * BEAT, data=blip(6), label="kick"),
          clip(12, offset=4 * BEAT, dur=4 * BEAT, data=blip(6), label="fill"),
          label="drums"),
    track(BASS,
          clip(20, offset=0 * BEAT, dur=4 * BEAT, data=blip(2), label="root"),
          clip(21, offset=4 * BEAT, dur=4 * BEAT, data=blip(3), label="turn"),
          label="bass"),
    track(LEAD,
          # A piano-roll clip: (start, dur, pitch) events relative to the clip,
          # pitch mapped over [min, max]. The whole roll moves with the clip.
          clip(30, offset=2 * BEAT, dur=6 * BEAT, min=48, max=72,
               notes=[(0 * BEAT, BEAT, 60), (1 * BEAT, BEAT, 64),
                      (2 * BEAT, BEAT, 67), (3 * BEAT, 2 * BEAT, 72),
                      (5 * BEAT, BEAT, 67)],
               label="theme"),
          label="lead"),
    label(99, "Multitrack: clips placed on one shared time axis"),
    title="Multitrack timeline", w=1000, h=520, layout="col",
))
print(f"opened window {win} — clips of the three tracks line up on one axis")

# %% [markdown]
# ## Move and resize clips from the script
# A clip's placement is live: ``gui.set`` its ``offset`` (start) or ``dur``
# (length). The lane redraws with the clip in its new spot; because the shared
# axis spans the longest clip, pushing one clip out lengthens the whole view.

# %%
gui.set(12, offset=5 * BEAT)          # slide the drum fill a beat later
gui.set(30, dur=8 * BEAT)             # stretch the lead theme


def drain_events(closed=[False]):
    """Print any clip edit-back events (drag/resize) — the ``"clip"`` payload."""
    while (msg := gui.poll(0.0)) is not None:
        addr, args = msg
        if addr == "/gui_closed":
            closed[0] = True
            print("window closed")
        elif addr == "/gui_event" and len(args) >= 4 and args[1] == "clip":
            wid, _, offset, dur = args[:4]
            print(f"clip {wid}: offset {offset:.0f} dur {dur:.0f} samples")
    return closed[0]


# %%
if __name__ == "__main__":
    try:
        deadline = time.monotonic() + 60.0
        while time.monotonic() < deadline and not drain_events():
            time.sleep(0.05)
    finally:
        gui.free(win)
        session.close()
        sys.exit(0)
