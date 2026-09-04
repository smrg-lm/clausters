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
samples) as you drag; ``r`` resets.

**Ctrl+drag** asks for the other selection: the span restricted to the band of
values the sweep covered, drawn as the rectangle it is and reported as two
further arguments. Both views carry the same plan (``"select_box select"``), and
the step declines where the picture has one measured axis — so today the
rectangle appears on the waveform, whose y is amplitude, while the spectrogram
falls through to the plain span, because its y is frequency and a selection of
frequencies is a range of *bins*: a different field of a selection, and a
gesture this host does not have yet. A plain drag stays a time span on both, on
purpose.

The two views are deliberately **not linked** here, so each one's selection is
its own (see ``linked.py`` for the shared-axis case) — which is what makes
the clipboard's addressee visible: a block operation goes to the view under the
pointer, and to whichever view holds the window's most recent selection when the
pointer is over neither (a sweep out to the first or last sample leaves it in
the window's margin).

**Ctrl+C then Ctrl+V** is the clipboard, and it shows where the host's authority
stops. The copy is a *read*, so the host makes it alone: the selected span
leaves the samples it has mapped and lands on its clipboard, typed and carrying
its sample rate — nothing reaches this script. The paste *changes data*, which
the host does not own, so it arrives here as a request with the clipboard beside
it, and what this script does is the smallest honest thing: the block goes into
a buffer of its own (`Buffer.set_samples`, which chunks it as blobs — a
half-second of stereo is 200 kB and would not fit one datagram as arguments) and
plays once. Copy a range, paste it, hear that range — with nothing written over
the take, because a destructive edit belongs to whoever owns those samples. The
**playhead** tracks what you hear: `play_pass` starts one pass of the render
through a ``PlayBuf`` voice (``loop`` is off — the take plays once and the sound
never repeats under whatever else you are checking) and anchors the line with
the server's sample clock, which the host reads from shared memory with zero
per-frame messages.

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
  handles: ``win["wave"].set(...)``, ``play_pass()``. The kernel stays alive
  with the window open, and the host's event loop keeps delivering to it
  between cells.
- **As a script** — ``python clients/python/examples/views/editor.py`` runs the
  whole file: one playback pass with the playhead following it, then the window
  stays open until you close it (``play_pass()`` from a cell replays it). The
  sound does not repeat on its own, so what you hear after that pass is what you
  asked for.

(The install builds the ``clausters-gui`` binary too; ``CLAUSTERS_SKIP_GUI_BUILD=1``
gives a server-only install, using a ``clients/gui/target`` binary if present.)
Needs a display and a GPU adapter.
"""

# %%
import array
import json
import os
import sys
import tempfile

from clausters import Session
from clausters.render import read_soundfile
from clausters.defs import Buffer, Synth, SynthDef, control, out, play_buf
from clausters.gui import peaks_cache_file, samples_to_file, spectrogram, view, waveform
from clausters.seq import Pbind, Pseq, Pwhite

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
# buffer, played by a synth. Nothing converts it -- `Buffer.read` is
# /buffer_allocRead, which decodes float WAV through the same decoder
# read_soundfile used above, and hands back the shape it found.
take = Buffer.read(wav_path, server=server)
SynthDef(
    "sampler",
    out(0.0, play_buf(float(take.bufnum), 0.0)),
    out(1.0, play_buf(float(take.bufnum), 1.0)),
).send(server)

# The clipboard's own voice: a one-shot over whatever buffer the paste filled.
# A buffer of its own rather than a write into the take -- the host copied a
# range, and hearing that range needs no edit to the samples it came from. The
# buffer is a *control* because a paste allocates one sized to what came over,
# so the def outlives every block it plays.
_clip_voice = None
_clip_buf = None
SynthDef(
    "clipboard-player",
    out(0.0, play_buf(control("buf", 0.0, "ir"), 0.0)),
    out(1.0, play_buf(control("buf", 0.0, "ir"), 1.0)),
).send(server)
server.sync()

# %% [markdown]
# ## Open the editor window
# `gui.open` sends a ``window``-rooted GuiDef and returns its handle (edit a
# named widget with ``win["wave"].set(...)``, close it with ``gui.close``). We
# pre-select the second half from here; dragging on either view replaces it and
# reports back as ``selection`` events (drained below).

# %%
#: The gesture plan both views carry: a plain drag sweeps the **time span**, and
#: Ctrl+drag asks for the rectangle -- the span restricted to the band of values
#: the sweep covered. `select_box` **declines** where the picture has one
#: measured axis and the plan falls through to `select`, so the same chord draws
#: a rectangle on the waveform (whose y is amplitude) and a plain span on the
#: spectrogram (whose y is frequency, a range of *bins* rather than of values --
#: its own gesture, and not one this host has yet). One binding, and the day the
#: spectral sweep exists it answers here with nothing to rewire.
SELECT_PLAN = {"drag": "select", "ctrl": "select_box select"}


def scene(path: str) -> dict:
    return view(
        waveform(name="wave", path=path, channels=2, sample_rate=SR,
                 gestures=SELECT_PLAN),
        spectrogram(name="spect", path=path, channels=2, sample_rate=SR,
                    window_size=1024, db_floor=-90.0, gestures=SELECT_PLAN),
        title="Editor: waveform + spectrogram", w=960, h=640, layout="col",
    )


win = scene(raw_path).open()
win["wave"].set(sel_start=float(frames // 2), sel_len=float(frames // 4))
print(f"opened window {win} — drag to select, Shift+drag to pan, wheel to zoom, r to reset")

# %% [markdown]
# ## Follow the playhead and read events
# Re-run `play_pass()` to play the take again and re-anchor the orange playhead.
# The ``wave``/``spect`` handles print any selection changes and `win.on_closed`
# notices a window close; the host's event loop delivers both, so nothing here
# dispatches them. When evaluating cells, call these whenever you like.

# %%
_synth = None


def play_pass():
    """Play the take once and anchor the playhead at the /clock_query sample where
    buffer position 0 starts sounding. Any previous pass is freed first, so
    calling it again restarts the take rather than layering a second voice."""
    global _synth
    if _synth is not None:
        _synth.free()
    _, args = server.request("/clock_query", expect=("/clock_query.reply",))
    clock_samples = float(args[0])
    _synth = Synth("sampler", server=server)
    win["wave"].set(playhead_at=clock_samples)
    win["spect"].set(playhead_at=clock_samples)


def on_clipboard(tag, *vals):
    """Ctrl+C on the waveform, then Ctrl+V: hear exactly what was copied.

    The split this shows is the host's whole posture. **Copy is a read**, so the
    host does it alone and nothing arrives here — the block is on its clipboard,
    typed, carrying the rate it was taken at. **Paste changes data**, which the
    host does not own, so it arrives as a request with the clipboard travelling
    beside it: the kind, the document, and the samples as one little-endian
    ``f32`` blob.

    What this script does with it is the smallest honest thing: it puts the
    block in a server buffer of its own and plays it once. Nothing is written
    back over the take — a destructive edit belongs to whoever owns that
    samples — so the round trip is *copy a range and hear that range*, which is
    the whole of what the clipboard promises.
    """
    if tag == "refused":
        print(f"host refused the {vals[0]}: {vals[1]}")
        return
    if tag != "paste" or len(vals) < 4:
        return
    kind, doc, blob = vals[1], json.loads(vals[2]), vals[3]
    if kind != "samples":
        print(f"nothing to audition: the clipboard holds {kind}")
        return
    block = doc["content"]
    values = array.array("f")
    values.frombytes(bytes(blob))
    channels = int(block["channels"])
    block_frames = int(block["frames"])
    print(f"pasted {block_frames} frames x {channels} ch at {block['sample_rate']:.0f} Hz "
          f"({block_frames / block['sample_rate']:.3f} s) — auditioning it")
    # A buffer of its own, sized to what came over and filled with it. The rate
    # travels with the block and nothing here resamples it: that would be an
    # edit, and an edit is the owner's. `set_samples` is why this is one line:
    # it sends the block as little-endian ``f32`` blobs, chunked to the
    # transport's bound — half a second of stereo is 200 kB, which as OSC
    # arguments would not fit a datagram at all.
    global _clip_voice, _clip_buf
    # The previous audition stops first -- one voice at a time -- but its buffer
    # is freed *last*: a write is parsed against the server's buffer state as of
    # the last completed command, so the barrier below is what the new buffer
    # needs, and a free thrown in ahead of it only adds a reply to wait past.
    if _clip_voice is not None:
        _clip_voice.free()
    previous, _clip_buf = _clip_buf, Buffer.alloc(block_frames, channels, server=server)
    server.sync()          # the allocation has to be *done*, not merely sent,
    _clip_buf.set_samples(values)   # before a write can name the buffer
    # It does not free itself (a one-shot done-action is the def's business, not
    # the clipboard's), so the next paste and the teardown are what end it.
    _clip_voice = Synth("clipboard-player", {"buf": _clip_buf.bufnum}, server=server)
    if previous is not None:
        previous.free()


def on_selection(name):
    """Print this view's ``"selection"`` edit-back, wired by name.

    A sweep with **height** over the waveform restricts the selection to a band
    of amplitudes as well as a span of time, and the two extra arguments are
    that band, in the view's own domain (here ``[-1, 1]``, full scale). The
    spectrogram sends no band: its vertical measures frequency, and a selection
    of frequencies is a range of *bins* rather than of values -- a different
    field of a selection, and a gesture this host does not have yet.
    """
    def handler(tag, *vals):
        if tag == "selection" and len(vals) >= 2:
            sel_start, sel_len = vals[0], vals[1]
            band = (f"  in [{vals[2]:.3f}, {vals[3]:.3f}]" if len(vals) >= 4
                    else "  (whole amplitude range)")
            print(f"{name}: selection {sel_start:.0f} +{sel_len:.0f} samples "
                  f"({sel_start / SR:.3f}s +{sel_len / SR:.3f}s){band}")
    return handler


def on_wave(tag, *vals):
    """The waveform's whole event stream: its selection, and the clipboard
    verbs the same view answers (one handler per widget, so they share one)."""
    on_selection("wave")(tag, *vals)
    on_clipboard(tag, *vals)


win["wave"].on_event(on_wave)
win["spect"].on_event(on_selection("spect"))
win.on_closed(lambda: print("window closed"))

play_pass()

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
    if _clip_voice is not None:
        _clip_voice.free()
    if _clip_buf is not None:
        _clip_buf.free()
    take.free()
    session.close()
    for name in os.listdir(_tmp):
        os.remove(os.path.join(_tmp, name))
    os.rmdir(_tmp)


# %% [markdown]
# ## Plain-script run
# Run cell by cell in Jupyter / VS Code to keep the window open and drive the
# handles between cells (``play_pass()``, ``win["wave"].set(...)``,
# ``gui.close(win)``). Run as a plain script instead — ``python editor.py`` —
# and this block holds the window open until it is closed, then tears
# everything down.

# %%
if __name__ == "__main__":
    try:
        # No deadline: the window is the manual test surface, so it ends
        # when you close it.
        # **One pass, not a loop.** The window is the manual test surface and
        # it stays open until it is closed; what must not stay running is the
        # sound. A take restarting every few seconds plays over anything else
        # the window is being used to check -- an auditioned clipboard block,
        # most of all -- and a reader who wants it again has `play_pass()`.
        play_pass()
        win.wait()
        teardown()
    except (OSError, RuntimeError, ConnectionError) as e:
        sys.exit(str(e))
