#!/usr/bin/env python3
"""The editor-grade waveform and spectrogram, driven interactively via cells.

The two heavy views at audio-editor depth. A stereo phrase is rendered
**offline** (no audio device needed for the render), written as one interleaved
``f32`` file, and shown twice from that single mapped resource (the bulk path —
the samples never ride OSC):

- a ``waveform`` with ``channels=2`` draws **both** channels as stacked lanes
  sharing the time axis, with an adaptive **time ruler** underneath;
- a ``spectrogram`` with ``channels=2`` analyzes each channel separately (one
  STFT lane per channel) and adds a **Hz ruler** matching its log frequency axis.

Both views navigate identically: **wheel** zooms toward the cursor (the peak
pyramid cross-fades, so zooming never pops), **Shift+drag** pans, **plain drag
selects** — the host emits ``/gui_event <id> "selection" <start> <len>`` (in
samples) as you drag; ``r`` resets. The **playhead** tracks what you hear: the
same render is looped by a ``PlayBuf`` synth and anchored each pass with the
server's sample clock, and the host reads the engine clock from shared memory
with zero per-frame messages.

Both views are *named* (``wave``/``spect``), so the script sets and reads each
by name and never matches a widget id.

Unlike the old three-terminal recipe, this script **launches its own server and
GUI**: `Session.live` starts an audio server if none is already running (picking
a shared-memory segment automatically) and `Session.gui` starts ``clausters-gui``
wired to it — no ``--shm`` path to spell out, and everything the session starts
is torn down when it is closed or the interpreter exits.

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention).
Install once, from the repo root::

    python -m venv .venv
    .venv/bin/pip install -e ./clients/python      # bundles the server + GUI binaries

Then run it either way:

- **Interactively** — open the file in VS Code (Python + Jupyter extensions) or a
  Jupyter notebook and run each ``# %%`` cell (Shift+Enter), inspecting between
  cells and driving the open window from the live ``session``/``gui``/``win``
  handles: ``win["wave"].set(...)``, ``play_pass()``, ``gui.pump()``. The kernel
  stays alive with the window open.
- **As a script** — ``python clients/python/examples/gui_editor.py`` runs the
  whole file: it follows the playhead for a while, then tears everything down.

(The install builds the ``clausters-gui`` binary too; ``CLAUSTERS_SKIP_GUI_BUILD=1``
gives a server-only install, using a ``clients/gui/target`` binary if present.)
Needs a display and a GPU adapter.
"""

# %%
import os
import sys
import tempfile
import time

from clausters import Session
from clausters.render import read_soundfile
from clausters.defs import Buffer, SynthDef, out, play_buf
from clausters.gui import peaks_cache_file, samples_to_file, spectrogram, waveform, window
from clausters.seq import Pbind, Pseq, Pwhite
from clausters.defs import Buffer, Synth

SR = 48_000.0

# %% [markdown]
# ## Render the stereo phrase offline
# Two bars of an arpeggio with amplitude jitter — enough spectral motion for the
# spectrogram to be worth looking at. Rendered through an NRT session (no audio
# device), then written as one interleaved f32 file the views map directly.

# %%
def phrase() -> Pbind:
    return Pbind(degree=Pseq([0, 4, 7, 11, 7, 4], repeats=4), dur=0.25,
                 amp=Pwhite(0.1, 0.25))


def render_stereo(path: str) -> list:
    """Bounces the phrase to `path` and reads it back.

    The server writes the WAV (`render(path=...)` hands the score to the
    ``--nrt`` renderer), so the same file feeds two consumers: `/buffer_allocRead`
    loads it into a buffer for the playhead to sound, and `read_soundfile`
    brings the samples here for the waveform view -- interleaved f32, the
    layout everything downstream already speaks.
    """
    nrt = Session.nrt(tempo=2.0)
    nrt.play(phrase())
    stats = nrt.render(sample_rate=SR, channels=2, path=path)
    print(f"rendered {stats.frames} frames ({stats.duration:.2f} s) -> {path}")
    return list(read_soundfile(path).samples)


_tmp = tempfile.mkdtemp(prefix="clausters_editor_")
raw_path = os.path.join(_tmp, "phrase.f32")
wav_path = os.path.join(_tmp, "phrase.wav")

inter = render_stereo(wav_path)
frames = len(inter) // 2
seconds = frames / SR
samples_to_file(inter, raw_path)
# Not strictly needed (the host builds a sibling cache when it maps the raw
# file), but shows the multichannel cache built client-side through the shared
# core — byte-identical to the host's own.
_cache = peaks_cache_file(inter, os.path.join(_tmp, "phrase.peaks"), channels=2)
print(f"wrote {os.path.getsize(raw_path)} B raw, "
      f"{os.path.getsize(_cache)} B multichannel peak cache")

# %% [markdown]
# ## Launch the server and the GUI
# `Session.live` connects to a running audio server or starts one if none is up
# (choosing a shared-memory segment for us); `session.gui()` starts
# ``clausters-gui`` with its client leg pointed at that server and mapping the
# same segment. Whatever the session started is owned by it — closing it (or
# leaving the interpreter) stops those.

# %%
session = Session.live()
server = session.server
gui = session.gui()
print(f"audio server on segment {server.shm}")


# The playhead's sound source: the very file the render wrote, in a server
# buffer, looped by a synth. Nothing converts it -- /buffer_allocRead reads float
# WAV through the same decoder read_soundfile used above.
bufnum = server.buffers.alloc()
server.send_msg("/buffer_allocRead", bufnum, wav_path)
SynthDef(
    "sampler",
    out(0.0, play_buf(float(bufnum), 0.0)),
    out(1.0, play_buf(float(bufnum), 1.0)),
).send(server)
server.sync()

# %% [markdown]
# ## Open the editor window
# `gui.open` sends a ``window``-rooted GuiDef and returns its handle (edit a
# named widget with ``win["wave"].set(...)``, close it with ``gui.close``). We
# pre-select the second half from here; dragging on either view replaces it and
# reports back as ``selection`` events (drained below).

# %%
def scene(path: str) -> dict:
    return window(
        waveform(name="wave", path=path, channels=2, sample_rate=SR),
        spectrogram(name="spect", path=path, channels=2, sample_rate=SR,
                    window_size=1024, db_floor=-90.0),
        title="Editor: waveform + spectrogram", w=960, h=640, layout="col",
    )


win = gui.open(scene(raw_path))
win["wave"].set(sel_start=float(frames // 2), sel_len=float(frames // 4))
print(f"opened window {win} — drag to select, Shift+drag to pan, wheel to zoom, r to reset")

# %% [markdown]
# ## Follow the playhead and read events
# Re-run `play_pass()` to (re)start a loop pass and re-anchor the orange playhead.
# The ``wave``/``spect`` handles print any selection changes and `win.on_closed`
# notices a window close; `gui.pump()` dispatches them. When evaluating cells,
# call these whenever you like.

# %%
_synth = None
_closed = False


def play_pass():
    """(Re)start a buffer pass and anchor the playhead at the /clock_query sample where
    buffer position 0 starts sounding."""
    global _synth
    if _synth is not None:
        _synth.free()
    _, args = server.request("/clock_query", expect=("/clock_query.reply",))
    clock_samples = float(args[0])
    _synth = Synth.new("sampler", server=server)
    win["wave"].set(playhead_at=clock_samples)
    win["spect"].set(playhead_at=clock_samples)


def on_selection(name):
    """Print this view's ``"selection"`` edit-back, wired by name."""
    def handler(tag, *vals):
        if tag == "selection" and len(vals) >= 2:
            sel_start, sel_len = vals[0], vals[1]
            print(f"{name}: selection {sel_start:.0f} +{sel_len:.0f} samples "
                  f"({sel_start / SR:.3f}s +{sel_len / SR:.3f}s)")
    return handler


win["wave"].on_event(on_selection("wave"))
win["spect"].on_event(on_selection("spect"))
win.on_closed(lambda: (globals().__setitem__("_closed", True), print("window closed")))

play_pass()
gui.pump()

# %% [markdown]
# ## Edit the open window live
# The selection and the spectrogram's contrast/scale are all settable from here,
# without recomputing anything (shader uniforms).

# %%
win["spect"].set(db_floor=-70.0, colormap=1)   # recolor the spectrogram live
win["wave"].set(sel_start=0.0, sel_len=float(frames))  # select the whole phrase

# %% [markdown]
# ## Close
# `gui.close(win)` closes the window; `session.close()` stops the GUI and server
# processes. (Leaving the interpreter would tear them down too, via the launcher's
# exit hooks — nothing is left running.)

# %%
def teardown():
    gui.close(win)
    Buffer(bufnum, server=server).free()
    session.close()
    for name in os.listdir(_tmp):
        os.remove(os.path.join(_tmp, name))
    os.rmdir(_tmp)


# %% [markdown]
# ## Plain-script run
# Run cell by cell in Jupyter / VS Code to keep the window open and drive the
# handles between cells (``play_pass()``, ``win["wave"].set(...)``,
# ``gui.close(win)``). Run as a plain script instead — ``python gui_editor.py`` —
# and this block follows the playhead for a while, honoring a window close, then
# tears everything down.

# %%
if __name__ == "__main__":
    try:
        deadline = time.monotonic() + 40.0
        next_pass = 0.0
        while time.monotonic() < deadline and not _closed:
            now = time.monotonic()
            if now >= next_pass:
                play_pass()
                next_pass = now + seconds + 0.5
            gui.pump(timeout=0.05)
        teardown()
    except (OSError, RuntimeError, ConnectionError) as e:
        sys.exit(str(e))
