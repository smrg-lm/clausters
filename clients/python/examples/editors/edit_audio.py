#!/usr/bin/env python3
"""``edit(buffer)``: a take in the audio editor, edited as a list of parts.

A `clausters.defs.Buffer` opens in `clausters.gui.editing.AudioEditor`, which
edits a private copy of it and writes nothing it was handed until it is saved.
The window draws a **join** -- a buffer made of spans of other buffers -- and
every edit leaves a new list of those spans:

- a **cut** copies the span on the server, then removes it from the list;
- a **paste** writes the block into a take of its own and puts it in;
- a **mix** writes a new take over the frames the block lands on, the block
  added onto them;
- a **pencil stroke** writes a new take the size of the stroke and splices it
  over the frames it was drawn on.

So an undo is the list before, stitched again: it costs the list, not the
samples. The takes a stroke or a paste made are kept while an undo or a redo
can still reach them and freed when neither can -- ``history_bytes`` below caps
what only the history holds, and the oldest entries go first past it.

To check the clipboard across files: copy a selection, add a file with the same
channel count and rate, then paste and mix at its cursor. Close the origin and
paste again; the copy remains available. Copy another span and undo the paste:
clipboard replacement leaves destination history intact. A different channel
count is refused; Paste preserves a different rate, while Paste mixed refuses
it. History-only samples still spill under ``resident_bytes`` independently of
the clipboard's current copy.

The View menu chooses waveform (W), spectrogram (S) or both (B). Call ``wave()``,
``spectrogram()`` and ``both()`` from a cell to make the same changes.

What to do in the window: **drag** to select, then **Ctrl+X** to cut,
**Ctrl+C** to copy, **Ctrl+V** to paste at the cursor and
**Ctrl+Shift+V** to mix there. **Click** to put the position cursor down --
the play cursor goes with it while nothing plays, and a selection puts both
at its start -- and **Home** and **End** to put it at the take's first and
last frame. **Space**
plays from the cursor to the end of the take, or of the selection, with the
play cursor following the sound and going back to the position cursor when
the pass ends; a second press stops it there sooner. **L** switches the loop:
looping, the selection -- or the whole take -- plays over and over, and a
selection drawn again while it loops is where it loops next: the loop follows
the drag, and the sound goes to the new start when the drag is let go. The
**meter** beside the take reads its level while it plays and falls when it
stops, and neither a play nor a stop clicks: the editor sounds the take through
nodes of its own, whose output fades in and out with the transport.
**Wheel** to zoom in until each sample is a disc, then **Alt+drag**
to draw. **Ctrl+Z** and **Ctrl+Shift+Z** walk the history. Every one of
these is also in the **menu bar** -- File, Edit, View, Transport, Help, each
entry with its key -- and the transport and the clipboard are **tools** under
it, which act on the take as the keys do. ``hear()`` plays the take as the edits have left it, and ``parts()``
prints what it is made of. ``editor.save()`` -- or Ctrl+S in the window --
writes the edit back into the file it was opened from, and
``editor.save(path)`` writes it as another file instead. File -> New file...
adds a file not written yet to this editor, as ``new_file()`` does;
its first save asks where it goes. File -> Open file... uses the host's
chooser; Ctrl/Shift selects several files. The independent file list is on
the left and View -> Files hides or shows it. Each file keeps its cursor,
selection and undo/redo history. A playing file continues while another is
shown, with its playhead hidden there. Try File -> New file with two and six
channels, then switch back and forth while the original loops: the meter
keeps the playing file's channel count. Stop from another file: its channel
layout appears with zero levels.

Run it as a script, or step through the cells::

    pip install -e clients/python

    python clients/python/examples/editors/edit_audio.py

It self-launches the audio server and the GUI host, and writes its own take
into ``clients/python/examples/out/`` -- nothing has to be found on disk.
"""

# %%
import math
import pathlib
import sys

from clausters import Session, SoundFile, play
from clausters.gui import edit

SECONDS = 2.0

#: Where the take is written: ``examples/out/``, the git-ignored directory
#: every generator in this tree writes to.
OUT = pathlib.Path(__file__).resolve().parents[1] / "out"
OUT.mkdir(exist_ok=True)

# %% [markdown]
# ## A take, made here
#
# Two seconds of a decaying tone, written to a file with
# `clausters.SoundFile.write`. A shape worth recognizing, so an edit over it is
# visibly an edit over *this*.

# %%
session = Session.live()
server = session.server
rate = server.query_info().nominal_sample_rate
frames = int(SECONDS * rate)
samples = [
    math.exp(-3.0 * i / frames) * 0.7 * math.sin(2 * math.pi * 440.0 * i / rate)
    for i in range(frames)
]
take = SoundFile.write(OUT / "edit_audio.wav", samples, 1, rate, sample_format="float")

# %% [markdown]
# ## One verb
#
# A `SoundFile` opens in the audio editor: a pane holding the join it owns.
# The file is not written while you edit -- every edit is a new list of parts
# over a copy of it and over the takes the edits made -- until you save.

# %%
session.gui()          # the host wired to this session's server
editor = edit(take, title="take", history_bytes=64 * 1024 * 1024, server=server)

# %% [markdown]
# ## Two views of the same take
#
# View -> Waveform (W), Spectrogram (S), or Waveform and spectrogram (B) calls
# the same verbs as these functions. The first two replace each other in the
# take's lane; the third stacks them, the spectrogram in a lane under the
# waveform, with one selection and play cursor and a vertical each.

# %%
def select_take():
    """Put the take body in hand; its pane keeps the ruler and selection."""
    editor.layer = "take"


def wave():
    """Show the peak and RMS waveform."""
    editor.view_wave()


def spectrogram():
    """Show the take's time-frequency picture."""
    editor.view_spectrogram()


def both():
    """Show the spectrogram under the waveform."""
    editor.view_both()


# %% [markdown]
# ## A gain envelope over the take
#
# View -> Edit gain envelope selects the curve body, and the vertical ruler
# reads gain while it is in hand. Drag its points or bend a
# segment; double-click adds a point. These edits keep the samples intact.
# `envelope()` shows a shaped fade; `apply_envelope()` renders it into a new
# take and resets the curve to unity. Undo restores the take and the curve.
# `preview_on()` hears and draws it; `preview_off()` ramps back to dry.
# `remove_effect()` frees the insert and retains the unapplied curve.

# %%
def envelope():
    """Show and set a gain envelope, with times in seconds."""
    editor.show_envelope()
    editor.set_envelope([0, 0, 3, 0, 0.5, 1, 1, 0, SECONDS, 0.25, 1, 0])


def preview_on():
    """Hear and draw the gain without changing the take."""
    editor.preview_on()


def preview_off():
    """Ramp to the unprocessed take and pause the insert."""
    editor.preview_off()


def remove_effect():
    """Free the insert and retain the curve for later use."""
    editor.remove_effect()


def apply_envelope():
    """Render the gain, keeping both take and curve available to undo."""
    editor.apply_envelope()


# %% [markdown]
# ## Hear it, and read what it is made of
#
# `editor.buffer` is the join the window draws: the take as the edits have left
# it, which `clausters.play` reads like any other buffer.

# %%
def hear():
    """Play the take as the edits have left it."""
    play(editor.buffer, server=server)


def parts():
    """Print the spans the take is made of, in the order they play."""
    for part in editor.parts:
        span = part["source"]["range"]
        print(f"buffer {part['source']['source']}: frames {span['start']}..{span['end']}")


# %% [markdown]
# ## Many channels
#
# A take of eight channels stacks a row each. View -> Fold channel folds one to
# a strip with its name, View -> Solo channel shows one alone, and View -> Show
# all channels opens them again; a press on a strip's name unfolds it. A stack
# too tall for the window keeps its rows readable and scrolls under the wheel
# over the vertical ruler (Ctrl+wheel there zooms the amplitude).

# %%
def many_channels():
    """Open eight channels, each its own tone, with the second and fifth folded."""
    tones = [math.exp(-3.0 * i / frames) * 0.5 * math.sin(2 * math.pi * 440.0 * (1 + c / 4) * i / rate)
             for i in range(frames) for c in range(8)]
    wide = edit(SoundFile.write(OUT / "edit_audio_8ch.wav", tones, 8, rate, sample_format="float"),
                title="eight channels", server=server)
    wide.fold_channel(1)
    wide.fold_channel(4)
    return wide


# %% [markdown]
# ## A file not written yet
#
# `SoundFile.empty` is a shape with no path: silence that opens like any take.
# Its first save -- Ctrl+S, or File -> Save as... -- asks where it goes.

# %%
def new_file():
    """Add two seconds of stereo silence to this editor."""
    editor.new_file(2, frames, rate)


def open_files():
    """Add two files at once, then use the list to switch and undo per file."""
    first = SoundFile.write(OUT / "edit_audio_b.wav", samples, 1, rate, sample_format="float")
    second = SoundFile.write(OUT / "edit_audio_c.wav", samples, 1, rate, sample_format="float")
    editor.open_file([first.path, second.path])


def focus_first():
    """Restore the first file's cursor, selection and edits."""
    editor.focus_file(0)


def hide_files():
    """Give the editing view the space occupied by the file list."""
    editor.hide_files()


def show_files():
    """Show the independent file list again."""
    editor.show_files()


def close_file():
    """Close the focused file; the others retain their state."""
    editor.close_file()


# %%
def run():
    """Keep the window open until it is closed."""
    print("drag to select, Ctrl+X / Ctrl+C / Ctrl+V / Ctrl+Shift+V, Alt+drag to draw.")
    print("Ctrl+Z walks back. Call hear() to play it, parts() to read it.")
    editor.wait()


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    run()
else:
    print("up -- edit the take, hear() to play it, parts() to read it")
