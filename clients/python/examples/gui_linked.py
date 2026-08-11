#!/usr/bin/env python3
"""Linked editor views: one navigation group, several lanes.

The classic editor layout is **one item with parts**: a waveform lane and a
spectrogram lane of the same sound under one time axis, with one selection.
This example renders a stereo phrase offline, writes it once as a raw ``f32``
file, and composes the two heavy views over that single mapped resource with
``link=1`` — the shared **navigation group**:

- **zoom, pan and drag-selection on either lane move both**: the group owns
  the horizontal view, the selection and the playhead; each member keeps only
  its own vertical (amplitude / frequency) window;
- the script sees **one** event stream — a gesture emits a single
  ``"view"``/``"selection"`` event carrying the interacted lane's id, not one
  per member;
- ``set`` of ``view_start``/``view_len`` (samples; non-positive
  ``view_len`` = the whole timeline), ``sel_start``/``sel_len`` or
  ``playhead_at`` on **any** member applies group-wide;
- membership is **live**: setting ``link`` moves a lane between groups, a
  negative ``link`` unlinks it (it keeps the view it had and diverges).

Both lanes are *named* (``wave``/``spect``), so the script drives the group and
reads its one event stream by name — never by widget id.

The composition is plain GuiDef — existing containers plus the ``link`` prop,
no new widget kind. A stack of linked lanes needs only one time-ruler strip:
the top lane keeps ``ruler="time"``, the bottom one switches its own off.

Run it as a script (``python gui_linked.py``) or cell by cell (``# %%``).
Needs a display and a GPU adapter; the install bundles the GUI binary (see
``gui_editor.py`` for the setup notes).
"""

# %%
import os
import sys
import tempfile
import time

from clausters import Session
from clausters.gui import samples_to_file, spectrogram, waveform, window
from clausters.seq import Pbind, Pseq, Pwhite

SR = 48_000.0

# %% [markdown]
# ## Render the phrase offline and write the shared resource
# One NRT render, one raw f32 file — both lanes map the same bytes.

# %%
nrt = Session.nrt(tempo=2.0)
nrt.play(Pbind(degree=Pseq([0, 4, 7, 11, 7, 4], repeats=4), dur=0.25,
               amp=Pwhite(0.1, 0.25)))
take = nrt.render(sample_rate=SR, channels=2)
inter, frames = take.samples, take.frames
print(f"rendered {frames} frames ({frames / SR:.2f} s) offline")

_tmp = tempfile.mkdtemp(prefix="clausters_linked_")
raw_path = os.path.join(_tmp, "phrase.f32")
samples_to_file(list(inter), raw_path)

# %% [markdown]
# ## Compose the linked item
# Both lanes name ``link=1``. The spectrogram turns its own time ruler off —
# with the navigation shared, the waveform's strip rules for the whole stack.
# Both are *named*, so the script drives them by name.

# %%
session = Session.live()
gui = session.gui()

win = gui.open(window(
    waveform(name="wave", path=raw_path, channels=2, sample_rate=SR, link=1),
    spectrogram(name="spect", path=raw_path, channels=2, sample_rate=SR,
                window_size=1024, db_floor=-90.0, link=1, ruler="off"),
    title="Linked lanes: one timeline, one selection", w=960, h=640,
))
print(f"opened window {win} — wheel/drag on either lane drives both")

# %% [markdown]
# ## Drive the group from the script
# Any member's handle addresses the group: select on the spectrogram, zoom via
# the waveform — both lanes follow either way.

# %%
win["spect"].set(sel_start=float(frames // 2), sel_len=float(frames // 4))
win["wave"].set(view_start=float(frames // 4), view_len=float(frames // 2))

_closed = False


def on_group(name):
    """Print the single per-gesture event stream (no per-member duplicates),
    wired by name."""
    def handler(tag, *vals):
        if tag in ("view", "selection") and len(vals) >= 2:
            print(f"{name}: {tag} {vals[0]:.0f} +{vals[1]:.0f} samples")
    return handler


win["wave"].on_event(on_group("wave"))
win["spect"].on_event(on_group("spect"))
win.on_closed(lambda: (globals().__setitem__("_closed", True), print("window closed")))


# %% [markdown]
# ## Live membership
# Call this from a cell (or let the script run it near the end): it unlinks
# the spectrogram (which keeps its view and diverges), navigates the waveform
# alone, then re-links — the spectrogram snaps back to the group.

# %%
def demo_membership():
    win["spect"].set(link=-1)                        # unlink: keeps its view
    win["wave"].set(view_len=float(frames // 8))     # only the waveform zooms
    time.sleep(1.5)
    win["spect"].set(link=1)                         # rejoin: adopts the group
    win["wave"].set(view_len=0.0)                    # reset both to the whole file


# %%
if __name__ == "__main__":
    try:
        # Interactive first: both lanes are linked — wheel/drag on either one
        # drives both. The membership demo runs only near the end, so early
        # gestures are not confused by a temporarily unlinked lane.
        # The membership demo still fires on a timer; the run itself ends
        # when you close the window.
        demo_at = time.monotonic() + 75.0
        while not _closed:
            gui.pump(timeout=0.05)
            if demo_at is not None and time.monotonic() >= demo_at:
                demo_at = None
                print("membership demo: unlink -> diverge -> relink")
                demo_membership()
        gui.close(win)
        session.close()
        # The host writes a sibling peaks cache beside the mapped raw file.
        for name in os.listdir(_tmp):
            os.remove(os.path.join(_tmp, name))
        os.rmdir(_tmp)
    except (OSError, RuntimeError, ConnectionError) as e:
        sys.exit(str(e))
