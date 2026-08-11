#!/usr/bin/env python3
"""Configurable editor rulers, switched live from widgets in the same GUI.

Every axis of the two heavy views is a ruler with selectable units, each drawn
in its **own strip** beside the view (no overlap with the traces or the cursor
readout), each optional per axis, and each retunable at runtime through
``GuiHost.set`` — which is what the menus in this window do:

- the **time axis** (both views) reads ``"time"`` (``h:mm:ss.mmm``),
  ``"samples"``, or ``"beats"`` — musical time labeled ``bar:beat`` on the
  client's grid: ``tempo`` (beats/second, the `Clock` convention), ``beat_at``
  (the beat at sample 0) and ``quant`` (beats per bar);
- the waveform's **amplitude axis** reads ``"norm"`` ([-1, 1]), ``"db"``
  (dBFS), ``"bits"`` (integer sample values at ``bit_depth``), ``"percent"``,
  or ``"off"``;
- the spectrogram's **frequency axis** follows ``freq_scale`` — ``"log"``,
  ``"linear"``, ``"mel"`` or ``"bark"`` (the perceptual scales; the shader's
  display mapping and the ruler share the closed forms in the native core).

Every axis also **navigates vertically**: the mouse wheel over a y-ruler strip
zooms that axis around the cursor (amplitude on the waveform, frequency on the
spectrogram), dragging the strip pans it, and ``R`` resets. The visible window
is the ``y_start``/``y_len`` prop pair (normalized display units, ``0, 1`` =
the full axis) — settable from the script, reported back as
``/gui_event id "view_y" y_start y_len`` — and the tick layout is adaptive:
it measures its actual labels, so zooming any axis keeps revealing finer,
non-colliding rungs in whatever unit is active.

A stereo phrase is rendered offline at a known tempo, mapped as one raw file,
and shown in both views; three menus and a toggle drive the units. The script
drains ``/gui_event`` and translates each menu pick into the matching
``gui.set`` — the "wire a button to the display" path, no recompute anywhere
(rulers are painter chrome; the frequency scale is a shader uniform).

Run it like the other GUI examples (see ``gui_editor.py`` for the install):
interactively cell by cell, or as a plain script. Needs a display and a GPU
adapter.
"""

# %%
import os
import sys
import tempfile
import time

from clausters import Session
from clausters.gui import menu, panel, samples_to_file, spectrogram, toggle, waveform, window
from clausters.seq import Pbind, Pseq, Pwhite

SR = 48_000.0
TEMPO = 2.0  # beats per second (120 BPM), the grid the beats ruler shows
QUANT = 4.0  # beats per bar

# %% [markdown]
# ## Render a stereo phrase offline at a known tempo
# The beats ruler is only meaningful against a grid, so the phrase is rendered
# at ``TEMPO`` and the same value is handed to the views below.

# %%
nrt = Session.nrt(tempo=TEMPO)
nrt.play(Pbind(degree=Pseq([0, 2, 4, 7], repeats=8), dur=0.5,
               amp=Pwhite(0.3, 0.7)))
take = nrt.render(sample_rate=SR, channels=2)
inter, frames = take.samples, take.frames
print(f"rendered {frames} frames ({frames / SR:.2f} s, "
      f"{frames / SR * TEMPO:.1f} beats) offline")

_tmp = tempfile.mkdtemp(prefix="clausters_rulers_")
raw_path = os.path.join(_tmp, "phrase.f32")
samples_to_file(list(inter), raw_path)

# %% [markdown]
# ## The window: both views plus the unit controls
# The views start with the defaults — time ruler in clock time, amplitude in
# normalized units, frequency in log Hz — and carry the beat grid so switching
# to ``"beats"`` is just a unit change. Each menu's options are ordered so its
# reported index maps straight to the prop value.

# %%
TIME_UNITS = ["time", "samples", "beats"]
AMP_UNITS = ["norm", "db", "bits", "percent"]
FREQ_SCALES = ["log", "linear", "mel", "bark"]

def scene(path: str) -> dict:
    # The two heavy views take a full row each; the unit controls share one
    # compact row underneath (a nested `row` panel), so the views keep most of
    # the window. Every widget is *named*, so the script drives it by name.
    return window(
        waveform(name="wave", path=path, channels=2, sample_rate=SR,
                 tempo=TEMPO, quant=QUANT, bit_depth=16),
        spectrogram(name="spect", path=path, channels=2, sample_rate=SR,
                    window_size=1024, tempo=TEMPO, quant=QUANT),
        panel(menu(name="time", options=TIME_UNITS, label="time axis"),
              menu(name="amp", options=AMP_UNITS, label="amplitude axis"),
              menu(name="freq", options=FREQ_SCALES, label="frequency scale"),
              toggle(name="yaxis", label="vertical rulers", value=True),
              # `hug` is the strip saying "I am as tall as what I hold": the
              # menus know their own height and the panel composes them, so no
              # number here has to be kept in step with the controls.
              layout="row", hug=True),
        title="Rulers: units per axis", w=960, h=720, layout="col",
    )


session = Session.live()
gui = session.gui()
win = gui.open(scene(raw_path))
print(f"opened window {win} — click the menus to cycle each axis' unit")

# %% [markdown]
# ## Wire the widgets to the rulers, by name
# The host reports every menu pick as an index (and the toggle as 0|1); each
# handle callback answers with the ``set`` that retunes the matching axis. This
# is script glue by design: the same events could equally drive a synth, and the
# same ``set`` calls could come from anywhere. The views themselves report their
# vertical window as a ``"view_y"`` edit-back when the wheel/strip zooms them.

# %%
_amp_unit = "norm"  # remembered so the toggle can restore it
_closed = False


def on_time(index):
    unit = TIME_UNITS[int(index)]
    win["wave"].set(ruler=unit)
    win["spect"].set(ruler=unit)
    print(f"time axis -> {unit}")


def on_amp(index):
    global _amp_unit
    _amp_unit = AMP_UNITS[int(index)]
    win["wave"].set(ruler_y=_amp_unit)
    print(f"amplitude axis -> {_amp_unit}")


def on_freq(index):
    scale = FREQ_SCALES[int(index)]
    win["spect"].set(freq_scale=scale)
    print(f"frequency scale -> {scale}")


def on_yaxis(value):
    on = bool(int(value))
    win["wave"].set(ruler_y=_amp_unit if on else "off")
    win["spect"].set(ruler_y="hz" if on else "off")
    print(f"vertical rulers -> {'on' if on else 'off'}")


def on_view_y(tag, *vals):
    if tag == "view_y" and len(vals) >= 2:
        print(f"vertical window: start={vals[0]:.3f} len={vals[1]:.3f}")


win["time"].on_event(on_time)
win["amp"].on_event(on_amp)
win["freq"].on_event(on_freq)
win["yaxis"].on_event(on_yaxis)
win["wave"].on_event(on_view_y)
win["spect"].on_event(on_view_y)
win.on_closed(lambda: globals().__setitem__("_closed", True))

# %% [markdown]
# ## The same math from the client
# The grid the beats ruler draws and the perceptual scales the frequency ruler
# uses are the same native-core functions the client exposes — a headless
# script reads the identical numbers the GUI shows.

# %%
from clausters import _native  # noqa: E402

pos = 9.5  # beats
print(f"beat {pos} on a {QUANT:.0f}-beat grid is bar "
      f"{int(_native.bar(pos, QUANT)) + 1}, beat {_native.beat_in_bar(pos, QUANT) + 1:g}")
print(f"1 kHz is {_native.hz_to_mel(1000.0):.0f} mel, "
      f"{_native.hz_to_bark(1000.0):.2f} bark")

# %% [markdown]
# ## Everything is also settable directly
# The menus are a convenience; any client can retune an axis at any time.
# (As a plain script this cell runs right after the window opens, so it
# announces itself and restores the defaults — the menus still read index 0.)

# %%
print("demo: spectrogram -> mel, waveform -> beats + dBFS (3 s) ...")
win["spect"].set(freq_scale="mel")
win["wave"].set(ruler="beats", ruler_y="db")
time.sleep(3.0)
win["spect"].set(freq_scale="log")
win["wave"].set(ruler="time", ruler_y="norm")

# %% [markdown]
# ## Vertical navigation from the script
# The same ``y_start``/``y_len`` window the wheel and the strip-drag drive is
# a plain live prop: zoom the waveform into its top half (watch the ticks
# refine) and the spectrogram into the low mids, then reset both.

# %%
print("demo: vertical zoom on both views (3 s), then back to the full axes ...")
win["wave"].set(y_start=0.5, y_len=0.5)     # amplitude axis: the upper half
win["spect"].set(y_start=0.2, y_len=0.35)   # frequency axis: a low-mid band
time.sleep(3.0)
win["wave"].set(y_len=0.0)                  # <= 0 resets to the full axis
win["spect"].set(y_len=0.0)

# %% [markdown]
# ## Plain-script run
# Cell-by-cell keeps the window open under your hands; as a script this block
# services the menu events for a while, then tears everything down.

# %%
def teardown():
    gui.close(win)
    session.close()
    # The host writes a sibling peaks cache next to the mapped file; sweep the
    # whole temp dir.
    for name in os.listdir(_tmp):
        os.remove(os.path.join(_tmp, name))
    os.rmdir(_tmp)


if __name__ == "__main__":
    try:
        # No deadline: the axes are here to be navigated, so the run ends
        # when you close the window.
        while not _closed:
            gui.pump(timeout=0.05)
        teardown()
    except (OSError, RuntimeError, ConnectionError) as e:
        sys.exit(str(e))
