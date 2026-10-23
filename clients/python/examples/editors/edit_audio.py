#!/usr/bin/env python3
"""``edit(file)``: a take in the audio editor, edited as a list of parts.

A `clausters.SoundFile` opens in `clausters.gui.editing.AudioEditor`, which
reads it where it is and writes nothing it was handed until it is saved. The
take is a list of spans of files, and every edit leaves a new list:

- a **cut** puts the span's parts on the clipboard, then removes them;
- a **paste** of a copy puts its parts in -- nothing is made;
- a **mix** renders the copy added onto the frames it lands on into a file
  of the editor's working folder, which the list then names;
- a **pencil stroke** renders a file the size of the stroke and splices it
  over the frames it was drawn on; Edit -> Reverse renders the selection
  backwards the same way.

So an undo is the list before: it costs the list, not the samples, and a take
of an hour cuts and pastes as fast as one of a second. The files a render made
are kept while an undo, a redo or the clipboard can still reach them and
deleted when none can.

To check the clipboard across files: copy a selection, add a file with the same
channel count and rate, then paste and mix at its cursor. Close the origin and
paste again; the copy remains available. Copy another span and undo the paste:
clipboard replacement leaves destination history intact. A different channel
count is refused; Paste preserves a different rate, while Paste mixed refuses
it.

The View menu chooses waveform (W), spectrogram (S) or both (B). Call ``wave()``,
``spectrogram()`` and ``both()`` from a cell to make the same changes.

What to do in the window: **drag** to select, then **Ctrl+X** to cut,
**Ctrl+C** to copy, **Ctrl+V** to paste at the cursor and
**Ctrl+Shift+V** to mix there. **Click** to put the position cursor down --
the play cursor goes with it while nothing plays; dragging a selection keeps
the position cursor where it was -- and **Home** and **End** to put it at the take's first and
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
chooser, which lists only audio files and takes the keyboard: Ctrl/Shift
selects several files, and a double click or Enter on a file opens the
selection as OK does. Cancel it and open it again: it is back in the last
directory gone into, with nothing selected. The independent file list is on
the left and View -> Files hides or shows it. Each file keeps its cursor,
selection and undo/redo history; closing the last one leaves the editor open
with no file, and ``no_file()`` opens a second editor that starts that way.
A playing file continues while another is
shown, with its playhead hidden there. Try File -> New file with two and six
channels, then switch back and forth while the original loops: the meter
shows the viewed file's channels, with zero levels when another file plays.
Return to the playing file to see its live levels again.

The menus and the tools read each action's state from the file in focus:
draw in the take, then add a new file -- Edit -> Undo is disabled there, and
Ctrl+Z does nothing, until you switch back. Cut, Copy and Delete are disabled
with nothing selected, Paste mixed with a copy at another rate, and the View
and Edit entries mark the view and the layer in force. With the
File -> New file dialog up, Ctrl+Z reaches neither the take nor anything
behind the dialog; in its name field it undoes the typing. ``editor.actions``
prints the same readings.

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
# A `SoundFile` opens in the audio editor, read where it is. The file is not
# written while you edit -- every edit is a new list of parts over it and over
# the files the edits rendered -- until you save.

# %%
session.gui()          # the host wired to this session's server
editor = edit(take, title="take", server=server)

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
# Edit -> Edit gain envelope selects the curve body, and the vertical ruler
# reads gain while it is in hand. Drag its points or bend a
# segment; double-click adds a point. These edits keep the samples intact.
# `envelope()` shows a shaped fade; `apply_envelope()` renders it into a new
# file and resets the curve to unity. Undo restores the take and the curve.

# %%
def envelope():
    """Show and set a gain envelope, with times in seconds."""
    editor.show_envelope()
    editor.set_envelope([0, 0, 3, 0, 0.5, 1, 1, 0, SECONDS, 0.25, 1, 0])


def apply_envelope():
    """Render the gain, keeping both take and curve available to undo."""
    editor.apply_envelope()


# %% [markdown]
# ## Hear it, and read what it is made of
#
# `editor.buffer` is the join the take sounds through: the take as the edits
# have left it, streamed from its files, which `clausters.play` reads like any
# other buffer.

# %%
def hear():
    """Play the take as the edits have left it."""
    play(editor.buffer, server=server)


def parts():
    """Print the spans of files the take is made of, in the order they play."""
    for part in editor.parts:
        print(f"{part['path']}: frames {part['start']}..{part['start'] + part['frames']}")


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
    """Close the focused file; the others retain their state. The last one
    closed leaves the editor open, with no file."""
    editor.close_file()


# %% [markdown]
# ## The editor on its own
#
# An audio editor needs no file to open: `AudioEditor` with no take is the
# application on its own, its window offering File -> New file and File ->
# Open file. Closing its last file leaves it so, and File -> Close closes it.

# %%
def no_file():
    """Open a second audio editor with no file in it."""
    from clausters.gui.editing import AudioEditor
    other = AudioEditor(server=server, title="no file")
    other.open()
    return other


# %%
def run():
    """Wait for the editor's window to close, then close the session and its
    processes, and every other window with them."""
    print("drag to select, Ctrl+X / Ctrl+C / Ctrl+V / Ctrl+Shift+V, Alt+drag to draw.")
    print("Ctrl+Z walks back. Call hear() to play it, parts() to read it.")
    try:
        editor.wait()
    finally:
        session.close()


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    run()
else:
    print("up -- edit the take, hear() to play it, parts() to read it")
