#!/usr/bin/env python3
"""The audio editor's lifecycle, walked: two windows, several files, one
shared undo order, the clipboard between them, and saves that fail, that are
edited after and that close the window.

Each step below is a function to call from a cell, and each says what to look
at and what to listen for; ``status()`` prints what every window holds and
``activity()`` what every window is costing. The order is the one to take
them in, and every step leaves the windows ready for the next.

**Two windows.** ``first`` opens ``a.wav`` (mono, a decaying 440 Hz tone) and
then ``b.wav`` (stereo, 660 Hz) and ``c.wav`` (six channels, 880 Hz);
``second`` opens ``d.wav`` (mono, 550 Hz). Each window keeps its own
selection, cursor, transport and meter.

**Playback across a change of focus.** In ``first``, put the cursor on
``a.wav``, press **L** and **Space**: it loops. Click ``b.wav`` in the list on
the left: the loop goes on, no play cursor is drawn over ``b.wav``, and the
meter shows two channels at zero. Click ``c.wav``: six channels at zero. Click
``a.wav`` again: one channel, moving, and its play cursor is back. Space stops.
``second`` played meanwhile moves nothing in ``first``.

**Independent and shared history.** ``a.wav`` and ``d.wav`` share one undo
order (``shared_history``): ``cut_second()`` cuts ``d.wav``, and
``undo_in_first()`` -- Ctrl+Z in ``first`` over ``a.wav`` -- puts it back without
moving ``first``'s focus. ``b.wav`` and ``c.wav`` each have an order of their
own: an edit there is undone only there.

**The clipboard's lifetime, and a change of type.** ``copy_first()`` copies a
span of ``a.wav``; ``paste_second()`` pastes it into ``d.wav`` (mono into
mono). ``close_origin()`` closes ``a.wav`` in ``first``, and
``paste_second()`` again still pastes: the copy outlives the file it came
from. ``paste_stereo()`` pastes the mono copy into ``b.wav`` and is refused --
a paste never converts channels -- and ``b.wav`` is unchanged. Copy text in
any other program and press Ctrl+V over a take: text stays text, the paste is
refused, and the window's status line says why.

**Saves.** ``failed_save()`` saves ``d.wav`` into a directory that does not
exist: the render cannot write there, the error is printed, and ``status()``
shows the file still dirty and still called ``d.wav``. ``save_then_edit()``
saves it, then cuts it: dirty again, since the cut is newer than what was
written; ``undo_in_second()`` brings back the written take, and it is clean.
``save_and_close()`` saves ``d.wav`` once more and closes ``second``: the
window goes once the write is done, and the file holds the take.

A write in flight is too short for a hand to edit through -- five minutes of
stereo save in under half a second -- so an edit, a focus change or a close
landing while the reply is still out is checked by the client suites (their
shared fault trace), not here.

**What it costs.** Leave both windows alone for two seconds and call
``activity()``: a stopped window ticks for nothing. Play in one and call it
again: that window ticks for its meter and its play cursor, and the other
still for nothing.

Step through the cells, or run it as a script, which leaves a Python prompt
over the live windows with every step in scope (Ctrl+D ends it)::

    pip install -e clients/python

    python clients/python/examples/editors/audio_lifecycle.py

It self-launches the audio server and the GUI host, and writes its takes into
``clients/python/examples/out/``.
"""

# %%
import math
import pathlib
import sys

from clausters import Session, SoundFile
from clausters.errors import CommandError
from clausters.gui import edit
from clausters.gui.editing import Editing
from clausters.segments import Segment

SECONDS = 2.0

#: Where the takes are written: ``examples/out/``.
OUT = pathlib.Path(__file__).resolve().parents[1] / "out"
OUT.mkdir(exist_ok=True)

# %% [markdown]
# ## Four takes
#
# A decaying tone each, at a pitch of its own, so a span pasted from one into
# another is heard as the other's.

# %%
session = Session.live()
server = session.server
rate = server.query_info().nominal_sample_rate
frames = int(SECONDS * rate)


def tone(name, freq, channels):
    """``channels`` channels of a decaying tone, written as ``name``."""
    samples = [math.exp(-3.0 * i / frames) * 0.5 * math.sin(2 * math.pi * freq * (1 + c / 8) * i / rate)
               for i in range(frames) for c in range(channels)]
    return SoundFile.write(OUT / name, samples, channels, rate, sample_format="float")


a = tone("a.wav", 440.0, 1)
b = tone("b.wav", 660.0, 2)
c = tone("c.wav", 880.0, 6)
d = tone("d.wav", 550.0, 1)

# %% [markdown]
# ## Two windows, one shared undo order
#
# Both editors take one editing context, and each joins its first file to the
# context's shared order. Files opened later keep orders of their own.

# %%
session.gui()
shared = Editing()
first = edit(a, title="first", context=shared, shared_history=True, server=server)
first.open_file([b.path, c.path])
first.focus_file(0)
second = edit(d, title="second", context=shared, shared_history=True, server=server)


def status():
    """What each window holds: its files, and the one in focus."""
    for editor in (first, second):
        if editor.closed:
            print(f"{editor.title}: closed; {editor.file.path} holds {editor.file.frames} frames, "
                  f"dirty {editor.dirty}")
            continue
        names = [pathlib.Path(f.path).name for f in editor.files if f is not None]
        file = editor.file
        if file is None:
            print(f"{editor.title}: no file; open {names}")
            continue
        print(f"{editor.title}: {pathlib.Path(file.path).name}, {file.frames} frames, "
              f"{file.channels} ch, dirty {editor.dirty}, undo {editor.undo_label!r}; "
              f"open {names}")


def activity():
    """What each window is doing now, and why."""
    for editor in (first, second):
        if editor.closed:
            continue
        reading = editor.activity()
        print(f"{editor.title}: {reading['ticks']} ticks, {reading['whole']} whole and "
              f"{reading['live']} live frames since it opened; ticking for "
              f"{', '.join(reading['demand']) or 'nothing'}")


# %% [markdown]
# ## Independent and shared history
#
# A tenth of a second marked from 0.5 s and cut, in `second`; undone from
# `first`, whose file in focus shares the order.

# %%
def cut_second():
    """Cut a span of ``d.wav``: a tenth of a second from 0.5 s."""
    second.select(Segment(second.buffer, round(0.5 * second.sample_rate), 0.1))
    second.cut()


def undo_in_first():
    """Ctrl+Z in ``first``: the shared order puts ``d.wav``'s cut back."""
    first.undo()


def undo_in_second():
    """Ctrl+Z in ``second``."""
    second.undo()


# %% [markdown]
# ## The clipboard
#
# A copy is the clipboard's, not the file's: it outlives the file it came
# from, and a paste never converts it.

# %%
def copy_first():
    """Copy a span of ``a.wav``."""
    first.focus_file(0)
    first.select(Segment(first.buffer, round(0.2 * first.sample_rate), 0.3))
    first.copy()


def paste_second():
    """Paste the copy into ``d.wav``, at its cursor."""
    second.paste()


def close_origin():
    """Close ``a.wav`` in ``first``: the copy stays on the clipboard."""
    first.close_file(0)


def paste_stereo():
    """Paste the mono copy into ``b.wav``: refused, and ``b.wav`` unchanged."""
    first.focus_file(0)
    first.paste()


# %% [markdown]
# ## Saves
#
# A refused write keeps the file dirty and where it was; a save cleans only
# the take it wrote; a close after a save waits for it.

# %%
def failed_save():
    """Save ``d.wav`` into a directory that does not exist."""
    try:
        second.save(str(OUT / "no such directory" / "d.wav"))
    except CommandError as error:
        print("refused:", error)


def save_then_edit():
    """Save ``d.wav``, then cut it: dirty again."""
    second.save(str(OUT / "d_saved.wav"))
    second.select(Segment(second.buffer, round(0.5 * second.sample_rate), 0.1))
    second.cut()


def save_and_close():
    """Save ``d.wav`` and close its window."""
    second.save(str(OUT / "d_closed.wav"))
    second.close()


# %%
def run():
    """**A prompt over the live windows**, since every step is a call: the
    functions above are in scope, and leaving the prompt (Ctrl+D) closes the
    session and its processes."""
    import code
    try:
        code.interact(banner="status(), activity(), then the steps in the docstring; "
                             "Ctrl+D ends", local=globals(), exitmsg="")
    finally:
        session.close()


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    run()
else:
    print("up -- status() and activity() read the windows; the steps are in the docstring")
