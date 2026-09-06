#!/usr/bin/env python3
"""A tempo envelope you can drag, and the beat ruler it produces.

One figure with three rulers, and the point is what the top one does when you
move the curve:

- **left**, the value axis: beats per minute, the curve's own range. It is the
  plain 1-2-5 value ladder (``ruler_y="value"``) rather than an amplitude one,
  because a break-point function's values belong to whatever parameter it
  drives;
- **bottom**, seconds: the axis itself, evenly spaced, because time is what the
  axis measures;
- **top**, beats: *not* an axis. A beat is a **logical coordinate**, so those
  marks say where the beats fall — they crowd where the piece is fast and
  spread where it is slow, over an axis that never stopped measuring samples.
  It counts in ones (``quant=1``), so every mark is a beat and its label reads
  ``4:1`` — the fourth of them, on its first beat.

**Drag a break-point and the top ruler re-rules under your hand.** That is the
whole example. The curve reports its new shape (``/gui_event id "points" …``),
this script rebuilds the piece's `TempoMap` from it — one `env` call, extents in
seconds — and sets it back on the ruler, which redraws. Nothing else moves: the
seconds do not change, because seconds are not what a tempo edit changes.

Drag a corner **up** and watch the bars on top narrow; drag it **down** and they
open out. Bend a segment (drag it vertically between two corners) and the
crowding follows the bend. The piece gets longer or shorter *in beats* as you
work, which is what changing a tempo means, so the top ruler renumbers itself.

The starting shape is the plainest one that shows the whole apparatus: two
break-points over sixty seconds, the first at the bottom of the value axis and
the last at the top — 30 BPM accelerating in a straight line to 90. Everything
is in reach from there: drag either corner, bend the segment between them, or
Ctrl+click to add a third.

One thing the picture only approximates: the curve interpolates each segment in
**seconds**, the axis it is drawn on, and a `TempoMap` interpolates its segments
in **beats**. They agree exactly at every corner and part between them — on this
envelope by at most 1.2 BPM out of a 25 BPM excursion — so the corners, and the
ruler above them, are right, and the line between two corners is close.

Two corners cannot be dragged onto one instant (an envelope needs every extent
positive), and the first corner stays at zero: the map is written from there.
Both are refused quietly — the ruler simply does not move.

Run it like the other GUI examples (see ``editor.py`` for the install):
interactively cell by cell, or as a plain script. Nothing sounds. Needs a
display and a GPU adapter.
"""

# %%
import sys

from clausters import Session
from clausters.base import TempoMap
from clausters.gui import bpf, label, timeruler, view

SR = 48_000.0
QUANT = 1.0                  # what the top ruler counts on: one mark, one beat

# The envelope, in beats per minute against seconds: the corners and the
# stretches between them, so there is one more tempo than extent. A finite
# shape, which is what a piece's tempo is — no sustain, no loop, a tempo has no
# gate.
BPM = [30.0, 90.0]
EXTENTS = [60.0]                                       # seconds
# One shape per stretch. A **negative** curvature rises fast and flattens, a
# **positive** one holds low and then climbs.
SHAPES = ["lin"]
SECONDS = sum(EXTENTS)
SPAN = SECONDS * SR                                    # the axis, in samples
CORNERS = [sum(EXTENTS[:i]) for i in range(len(BPM))]  # 0, 60
BPM_LO, BPM_HI = 30.0, 90.0


def tempo_of(corners, bpm, shapes) -> TempoMap:
    """The piece's tempo, from an envelope written in **seconds**.

    ``env`` takes the tempos and the stretches between them, and
    ``unit="seconds"`` reads those stretches as wall clock — so each segment's
    width in beats is solved exactly rather than searched for, which is what
    lets a shape drawn against seconds be a tempo at all.
    """
    extents = [b - a for a, b in zip(corners, corners[1:])]
    tempo = TempoMap(bpm[0] / 60.0)
    tempo.env(0.0, [b / 60.0 for b in bpm], extents, shapes, unit="seconds")
    return tempo


# %% [markdown]
# ## The map, before there is a window
# `TempoMap` is a pure function of a beat: it answers about a piece nobody is
# playing. Ask it where each beat falls and the drawing is already decided.

# %%
tempo = tempo_of(CORNERS, BPM, SHAPES)
beats = tempo.beats_at(SECONDS)
print(f"{SECONDS:.0f} s of music is {beats:.2f} beats")
for beat in range(0, int(beats), 8):
    print(f"  beat {beat + 1:3d} falls at {tempo.secs_at(beat):6.2f} s and lasts "
          f"{tempo.span_secs(beat, beat + 1):.2f} s "
          f"({tempo.tempo_at(beat) * 60:.1f} BPM there)")

# %% [markdown]
# ## The figure: one curve, three rulers
# The curve and the ruler share a navigation group (`link`), so the group's
# gutter is the value axis' width and the beat ticks start at the same pixel the
# curve does. The curve is what gives the group its extent — without it the
# ruler above would have nothing to rule. What the curve's values *are* is said
# once, above the beat ruler, rather than inside the field it labels.

# %%
session = Session.live()
gui = session.gui()

points = [(secs * SR, value, shape)
          for secs, value, shape in zip(CORNERS, BPM, SHAPES + ["lin"])]

win = view(
label("tempo (BPM)"),
timeruler(name="beats", ruler="beats", h=24.0, link=1,
          sample_rate=SR, tempo=BPM[0] / 60.0, tempo_map=tempo, quant=QUANT),
bpf(name="tempo", points=points, min=BPM_LO, max=BPM_HI, duration=SPAN,
    weight=1.0,
    axes={"x": {"unit": "time", "sample_rate": SR, "link": 1},
          "y": {"unit": "value"}}),
title="A tempo envelope, and the beat ruler it makes", w=1000, h=460,
).open()
print(f"opened window {win} — drag a corner and watch the top ruler")


# %% [markdown]
# ## The edit, and what it re-rules
# The curve owns nothing: it reports the whole break-point list in its own units
# and this script decides what that means. Here it means the piece's tempo, so
# the map is rebuilt and handed back to the ruler — a `/gui_set` value is a
# scalar, hence the JSON the map writes with `dump`.

# %%
def _shape_of(shape: int, curvature: float):
    """A wire shape number as a tempo curve. A tempo segment holds a step, a
    straight line, an exponential or a curvature; anything else the envelope
    editor can draw is read as a straight line rather than refused, since the
    shape is the drawing's and the tempo is what it is being read *as*."""
    return {0: "step", 1: "lin", 2: "exp"}.get(int(shape), float(curvature)
                                               if int(shape) == 5 else "lin")


def on_points(tag, *payload):
    """The envelope was edited: rebuild the piece's tempo from what the hand
    left, and re-rule the beats above it."""
    if tag != "points":
        return
    quads = [payload[i:i + 4] for i in range(0, len(payload), 4)]
    corners = [q[0] / SR for q in quads]
    if corners[0] != 0.0 or any(b <= a for a, b in zip(corners, corners[1:])):
        return                       # the map is written from zero, forward
    try:
        moved = tempo_of(corners, [q[1] for q in quads],
                         [_shape_of(q[2], q[3]) for q in quads[:-1]])
    except ValueError:
        return                       # a tempo the envelope cannot be read as
    win["beats"].set(tempo_map=moved.dump())
    print(f"re-ruled: {moved.beats_at(corners[-1]):.2f} beats "
          f"in {corners[-1]:.1f} s")


win["tempo"].on_event(on_points)

# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        win.wait()
        gui.close(win)
        session.close()
    except (OSError, RuntimeError, ConnectionError) as e:
        sys.exit(str(e))
else:
    print("up — drag the curve; session.close() to end")
