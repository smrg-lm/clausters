#!/usr/bin/env python3
"""The gesture table: what a drag does is the container's, not the widget's.

Panning, sweeping a selection and locating the transport belong to the
**coordinate system** a container gives its contents, not to whatever is drawn
inside it. That is why one Shift+drag pans a ``waveform``, a ``track`` lane, a
``pianoroll`` and a free-standing ``timeruler`` alike, why a plain drag on a
``scroll``'s background pans the plane, and why a press a container does not
want falls *outward* to the container around it.

Each container declares that mapping in a ``gestures`` table, keyed by modifier
chord (``drag`` for the plain drag, plus ``shift``, ``ctrl``, ``alt``), whose
value is an ordered plan of steps:

- ``element`` — hand the press to whatever is under the cursor (a clip, a note,
  a box, a control). It may decline, and the plan goes on;
- ``pan`` — pan the container's own axis (time here, the plane in a ``scroll``);
- ``select`` — sweep the shared time selection (a rectangle in time x pitch on a
  roll, which also picks its notes);
- ``locate`` — put the transport's cursor under the pointer;
- ``none`` — nothing.

This window shows the same views twice. The **left** column keeps the defaults
(``"element locate"`` / ``"pan"`` on a lane, ``"select"`` / ``"pan"`` on a
waveform); the **right** one is told to pan on a plain drag and select with
Shift — the reversal, with no element's code involved. A menu switches the
right column live through ``set(gestures=...)``, which starts again from the
kind's defaults each time, so a table names only the chords it changes.

**Two kinds of axis, and they do not share a navigation group.** A ``waveform``
is bounded by its own content — its axis *is* the take — while a ``track`` lane
and a ``pianoroll`` are open-ended surfaces you place things on and zoom past
the end of. So the lanes, the rolls and the rulers share one group here, and
each waveform navigates alone. Audio joins a multitrack the way it does in
``gui_multitrack.py``: inside a ``clip``, which is what gives it a placement on
the open axis. The lane below each ruler carries the very same take that way.

Two gestures are deliberately *not* in the table, because they are not
ambiguous: a press on a view's vertical strip (the waveform's ``ruler_y``, the
roll's keyboard gutter) always pans that axis, and the wheel always zooms.

Run it as a script (``python gui_gestures.py``) or cell by cell (``# %%``).
Needs a display and a GPU adapter; the install bundles the GUI binary (see
``gui_editor.py`` for the setup notes).
"""

# %%
import json
import os
import sys
import tempfile
import time

from clausters import Session
from clausters.gui import (
    clip,
    label,
    menu,
    panel,
    pianoroll,
    samples_to_file,
    timeruler,
    track,
    waveform,
    window,
)
from clausters.seq import Pbind, Pseq, Pwhite

SR = 48_000.0
TEMPO = 2.0  # beats per second (120 BPM)

# %% [markdown]
# ## A take to look at
# Rendered offline and mapped from a file, the way a real take arrives: the
# gestures are the point here, not the audio.

# %%
nrt = Session.nrt(tempo=TEMPO)
nrt.play(Pbind(degree=Pseq([0, 2, 4, 7, 5, 4, 2, 0], repeats=4), dur=0.25,
               amp=Pwhite(0.3, 0.7)))
take = nrt.render(sample_rate=SR, channels=1)
frames = take.frames
_tmp = tempfile.mkdtemp(prefix="clausters_gestures_")
raw_path = os.path.join(_tmp, "take.f32")
samples_to_file(list(take.samples), raw_path)
print(f"rendered {frames} frames ({frames / SR:.2f} s)")

# Notes for the roll, on the same axis as the take.
beat = SR / TEMPO
NOTES = [(i * beat / 2, beat / 3, 60 + (i % 5) * 2, 100, 0) for i in range(16)]

# The reversal: a plain drag pans, Shift sweeps the selection. The chords the
# table does not name (`ctrl`, `alt`) keep the kind's defaults.
REVERSED = {"drag": "pan", "shift": "select"}
# ...and on a lane, `element` still comes first, so a clip is grabbed before
# the container gets to pan: a plan is an *order*.
LANE_PANS = {"drag": "element pan", "shift": "locate"}

# %% [markdown]
# ## The window: the same views, two tables
# The two open axes join one navigation group (``link=1``), so whichever column
# you drive, the other follows — which makes the difference between them exactly
# the gesture and nothing else. The two waveforms navigate on their own, each
# bounded by the take it holds.

# %%
PRESETS = ["default", "drag pans", "lane: drag pans, shift locates"]


def column(tag: str, gestures: dict | None):
    """One column: the open axis (ruler over lane over roll) and, under it, the
    same take as a standalone `waveform` navigating alone."""
    extra = {"gestures": gestures} if gestures else {}
    return panel(
        label(text=tag),
        timeruler(name=f"{tag}-ruler", link=1, h=18.0, sample_rate=SR, **extra),
        track(clip(name=f"{tag}-clip", path=raw_path, channels=1,
                   offset=0.0, dur=float(frames), label="take"),
              name=f"{tag}-lane", label="audio", link=1, snap=beat, height=90.0, **extra),
        pianoroll(name=f"{tag}-roll", notes=NOTES, min=48, max=84, snap=beat / 2,
                  link=1, sample_rate=SR, **extra),
        # No `link`: a waveform's axis is its own content, so it navigates by
        # itself instead of sharing the open axis above.
        waveform(name=f"{tag}-wave", path=raw_path, channels=1, sample_rate=SR, **extra),
        layout="col",
    )


scene = window(
    panel(column("default", None), column("reversed", REVERSED), layout="row"),
    panel(label(text="right column:"),
          menu(name="preset", options=PRESETS, label="gestures"),
          layout="row", h=44.0),
    title="Gestures: the container decides", w=1100, h=760, layout="col",
)

session = Session.live()
gui = session.gui()
win = gui.open(scene)
print(f"opened window {win}")
print("left column:  drag a lane locates, drag its clip moves it, "
      "drag the waveform selects, Shift+drag pans anywhere")
print("right column: drag pans everywhere, Shift+drag selects")

# %% [markdown]
# ## Switching the table live
# ``set(gestures=...)`` re-reads the kind's defaults and overlays the chords the
# table names, so switching back is just an empty table. The views are the same
# widgets throughout — nothing about the waveform, the lane or the roll changed.

# %%
_closed = False


def on_preset(index):
    table = [{}, REVERSED, LANE_PANS][int(index)]
    for tag in ("ruler", "lane", "roll", "wave"):
        win[f"reversed-{tag}"].set(gestures=json.dumps(table))
    print(f"right column -> {PRESETS[int(index)]}: {table or 'the defaults'}")


def report(tag, *vals):
    """Every navigation and edit-back the two columns emit, named."""
    print(f"  {tag} {' '.join(f'{v:g}' if isinstance(v, float) else str(v) for v in vals)}")


win["preset"].on_event(on_preset)
for side in ("default", "reversed"):
    for tag in ("ruler", "lane", "roll", "wave", "clip"):
        win[f"{side}-{tag}"].on_event(report)
win.on_closed(lambda: globals().__setitem__("_closed", True))

# %% [markdown]
# ## Plain-script run
# Cell by cell the window stays live under your hands; as a script this block
# services the events for a while, then tears everything down.

# %%
def teardown():
    gui.close(win)
    session.close()
    for name in os.listdir(_tmp):
        os.remove(os.path.join(_tmp, name))
    os.rmdir(_tmp)


if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        deadline = time.monotonic() + 120.0
        while time.monotonic() < deadline and not _closed:
            gui.pump(timeout=0.05)
        teardown()
    except (OSError, RuntimeError, ConnectionError) as e:
        sys.exit(str(e))
else:
    print("up - drag in both columns, gui.close(win) and session.close() to end")
