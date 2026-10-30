#!/usr/bin/env python3
"""A take that is a list of segments over a file, drawn by the host.

An application's takes live on disk: a take is a **list of segments**, each a
span of a file with its own fades and channel map, and an edit is a new list
over files that never change. This example renders one file and draws two
lists over it, one above the other, both with ``waveform(segments=...)``,
and the spectrogram of the second with ``spectrogram(segments=...)``:

- **the take as it is**: one part, the whole file -- a four-second tone that
  swells from quiet to loud;
- **the take edited**: the same file with its halves swapped, the loud half
  first, with a 10 ms fade either side of the seam. Nothing was rendered for
  it. It is two parts over the one file, and its picture is composed by the
  host from the file's summary;
- **its spectrogram**, under it: analyzed from the file by the host, only
  for the span on screen, and analyzed again as you zoom and scroll.

The file is rendered offline to ``take.w64`` and named with no peaks, so the
host summarizes its samples itself, once, with no server involved in the
drawing (an application measures a peaks file for a take of hours, and a
part names it as ``peaks``). It draws the levels its window resolves, and
reads **samples only under a zoom**. So zoom in with
the wheel until single samples show, and what you see is read from the file
then, a span at a time. **Check by eye** that the edited take falls from loud
to quiet at its seam and then swells again, that zooming into the seam
shows the 10 ms fades meeting, and that the spectrogram keeps one line at
440 Hz through the seam, sharpening as you zoom in.

The files land in ``clients/python/examples/out/segments/``.

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention).
Install once, from the repo root::

    python -m venv .venv
    .venv/bin/pip install -e ./clients/python      # bundles the server + GUI binaries

Then step through it (``win["edited"].set(segments=...)`` from a cell tries
another list), or run it as a script: the window stays open until you close
it. Needs a display and a GPU adapter.
"""

# %%
import sys
from pathlib import Path

from clausters import Session
from clausters.defs import DoneAction, Synth, SynthDef, out, sine
from clausters.defs.ugens import line
from clausters.gui import spectrogram, view, waveform

SR = 48_000
SECONDS = 4.0
FRAMES = int(SR * SECONDS)
FADE = SR // 100      # 10 ms

OUT = Path(__file__).resolve().parent.parent / "out" / "segments"
OUT.mkdir(parents=True, exist_ok=True)
TAKE = str(OUT / "take.w64")

# %% [markdown]
# ## Render the take
# A tone that swells over four seconds and frees itself at the end. The render
# writes its output to the W64 and lasts exactly as long as the synth.

# %%
def swell() -> SynthDef:
    level = line(0.05, 0.8, SECONDS, done_action=DoneAction.FREE_SELF)
    tone = sine(440.0) * level
    return SynthDef("swell", out(0.0, tone))


offline = Session.nrt().activate()
swell().send()
Synth("swell")
stats = offline.render(sample_rate=SR, channels=1, path=TAKE, tail=SECONDS)
offline.deactivate()
print(f"rendered {stats.frames} frames to {TAKE}")

# %% [markdown]
# ## Two lists over the one file
# A part names the file by its path, the span it reads, its fades and the
# file's own shape.

# %%
def part(start: int, frames: int, fade_in: int = 0, fade_out: int = 0) -> dict:
    return {"path": TAKE, "start": start, "frames": frames,
            "fade_in": fade_in, "fade_out": fade_out,
            "source_channels": 1, "source_frames": stats.frames, "source_rate": SR}


half = stats.frames // 2
whole = {"channels": 1, "sample_rate": SR, "parts": [part(0, stats.frames)]}
swapped = {"channels": 1, "sample_rate": SR, "parts": [
    part(half, stats.frames - half, fade_out=FADE),
    part(0, half, fade_in=FADE),
]}

# %% [markdown]
# ## Draw them
# The host draws both from the file; the session's server is not asked about
# either.

# %%
session = Session.live()
gui = session.gui()
win = view(
    waveform(name="whole", segments=whole, sample_rate=SR, ruler="time"),
    waveform(name="edited", segments=swapped, sample_rate=SR, ruler="time"),
    spectrogram(name="spectrum", segments=swapped, sample_rate=SR, ruler="time"),
    title="a take as a list of segments", layout="col",
).open()

# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        win.wait()
    finally:
        session.close()
else:
    print('up - win["edited"].set(segments=...) to draw another list, session.close() to end')
