#!/usr/bin/env python3
"""A **session**: the multitrack, where its samples are, and reading it back.

A multitrack says what plays when; a *session* is that plus the table saying where
its samples live, and the format lives in the shared crate precisely so that
more than one program can write it. This writes one, reads it back, and opens
it in the multitrack -- the loop a save and an open actually are.

What it shows, in the order the cells run:

- **The multitrack.** Two tracks, one of them muted, and a region on each. A region
  is one placed thing: where it starts, how long it occupies, and what fills it.
  Six regions over one source would be six identities and one source -- nothing
  is copied -- which is the whole of non-destructive editing.
- **A take, which is samples rather than description.** The example writes its
  own stereo WAV, so it needs no material found anywhere, and the session's
  table is what says where those samples are. The multitrack names a source **id**
  and never a path: that is the split that lets one file be opened by a program
  that has no Python in it.
- **What a save can and cannot promise.** The table says three things a save
  has to be able to say without blocking or deciding for the person: a file that
  is there, samples nobody wrote down, and a working copy whose destructive edit
  is still open. The cell prints which sources are in which state.
- **Reading it back.** The session is opened and handed to the multitrack, the
  GUI host's application, which resolves its table: each file it names is read
  **once per source**, however many regions draw it, and the regions that name
  it play. A source the table calls volatile comes back unresolved -- drawn,
  placed, silent -- rather than as a lie.
- **What is the multitrack's and what is the view's.** A muted track reopens muted,
  because mute is the multitrack's. A track's *height* is not: it says nothing
  about what the multitrack is, so no session carries it.

**Handing it to a host with no language attached** is the other half, and it
does not need this script: what it writes is the format, so ::

    clausters-gui --session examples/out/session.json

opens the same multitrack in a window with no Python behind it -- the tracks, the
boxes and their curves, edited and saved by the host itself. It is worth doing
once after a change to the editing path, because it is the configuration where
anything the client fills in for the host stops being filled in.

The two files it writes go to ``examples/out/`` (``session.json`` and
``session-take.wav``), the git-ignored directory every generator in this tree
writes to.

**What it needs:** nothing running -- the example writes its own WAV, and the
GUI host it opens holds its own engine. A display and a GPU adapter for the
reopening cell.

Run it as a script, or step through the cells. Install once, from the repo
root::

    pip install -e clients/python

    python clients/python/examples/editors/session.py
"""

# %%
import math
import os
import struct
import sys
import wave

from clausters.gui import edit
from clausters.multitrack import Content, Multitrack, Session, Source, Span, Tempo, View

SAMPLE_RATE = 48_000

#: Where a run leaves its files: ``examples/out/``, the git-ignored directory
#: every generator in this tree writes to.
OUT = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "out")
os.makedirs(OUT, exist_ok=True)

# %% [markdown]
# ## A take, which is samples rather than description
#
# The multitrack names a source **id** and never a path. Where the samples are is the
# session's table, and keeping the two apart is what lets one file be opened by
# a program that has no Python in it.

# %%
take_path = os.path.join(OUT, "session-take.wav")


def write_take(path: str, seconds: float = 2.0, freq: float = 440.0) -> int:
    """Writes a short **stereo** tone to `path` and returns its frame count.

    Two partials and a slow decay, so the drawn waveform has a shape to
    recognize rather than a rectangle of noise -- and the two channels are
    deliberately unlike (the right one is the third partial alone, quieter), so
    an edit on one is visibly an edit on *one*.
    """
    frames = int(seconds * SAMPLE_RATE)
    with wave.open(path, "w") as f:
        f.setnchannels(2)
        f.setsampwidth(2)
        f.setframerate(SAMPLE_RATE)
        samples = bytearray()
        for i in range(frames):
            t = i / SAMPLE_RATE
            env = math.exp(-3.0 * t / seconds) * (1.0 - math.exp(-t * 400.0))
            left = env * 0.7 * (math.sin(2 * math.pi * freq * t)
                                + 0.3 * math.sin(2 * math.pi * freq * 3 * t))
            right = env * 0.35 * math.sin(2 * math.pi * freq * 3 * t)
            for v in (left, right):
                samples += struct.pack("<h", int(max(-1.0, min(1.0, v)) * 32767))
        f.writeframes(bytes(samples))
    return frames


take_frames = write_take(take_path)
take_seconds = take_frames / SAMPLE_RATE
print(f"wrote {take_path} ({take_frames} frames)")

# %% [markdown]
# ## The multitrack
#
# Two tracks, each with one take lane, each with one region. The region's `position`
# and `length` are in **seconds** -- a multitrack is placed in physical time --
# while what fills it is measured in its own source's units. The tempo map the
# multitrack holds places nothing: it says where the beats and bars of a ruler fall
# over those seconds, and it is saved with the multitrack like everything else it
# holds.

# %%
#: The source id the multitrack names. The table below is what says where it is.
TAKE = 1


def window(start: float = 0.0, duration: float = take_seconds) -> dict:
    """A window onto the take: which source, from where, for how long."""
    return {"source": {"source": TAKE, "lifetime": "session", "generation": 0},
            "start": start, "duration": duration}


multitrack = Multitrack()
multitrack.set_tempo(Tempo(at=0.0, tempo=2.0))     # beats per second: 120 a minute

#: The take, twice, at two places: two regions, two identities, **one** source.
#: Nothing is copied, and trimming one leaves the other where it was. A track
#: starts with one take lane, and the multitrack names every track, take lane
#: and region: the script holds them, never their numbers.
tone = multitrack.tracks.add("tone")
tone.active_take_lane.regions.add(0.0, 4.0, Content.onto(window()), name="first")
again = tone.active_take_lane.regions.add(8.0, 2.0, Content.onto(window(start=0.5)),
                                          name="again")

#: **Mute is the multitrack's.** A track left muted here reopens muted, because
#: it says something about the multitrack. A track's *height* does not, so no session
#: carries one.
echo = multitrack.tracks.add("echo", muted=True)
echo.active_take_lane.regions.add(4.0, 4.0, Content.onto(window()))

print(f"the multitrack is {multitrack.end:.0f} s long, over {len(multitrack.tracks)} tracks")

# %% [markdown]
# ## Editing it: one edit, one undo
#
# The multitrack has a vocabulary of its own, and every change a script makes
# through its objects is one edit of it. Two things about it are worth seeing
# rather than reading: **where a region is** means track, take lane and second
# together, so moving one to the other track is a single edit -- there is no
# moment in between where it is on no take lane at all; and the multitrack's
# **history** takes it back in one step, because the crate reads the edit that
# puts it back before the edit lands.

# %%
history = multitrack.history         # asking for it is what gives it one
again.place(echo.active_take_lane, position=12.0)
print(f"  moved:  {again.name!r} is on {again.track.name!r}, at {again.position:.0f} s "
      f"-- one edit, {history.undo_label!r}")

history.undo()
print(f"  undone: {again.name!r} is on {again.track.name!r} again, at "
      f"{again.position:.0f} s, and the multitrack is {multitrack.end:.0f} s long")

# %% [markdown]
# ## Written as a session
#
# The table says where each source is. A **relative** path is resolved against
# the session file's own folder, which is what makes the pair of files movable
# together; an absolute one names the user's own file, which a session never
# copies and never rewrites.

# %%
#: **How it was being looked at**, beside what it is. A window's zoom, its
#: grid, what the hand was holding and how tall each track was drawn are not the
#: multitrack -- nothing here can change what plays -- and all of it is state
#: the person loses on a reopen unless the file carries it. A list, because a
#: multitrack drawn in two windows has two views and they disagree on purpose.
window = View(name="arranger", visible=Span(0.0, multitrack.end), quant=1.0)
window.track_view(tone).height = 96.0
window.track_view(tone).take_lanes_shown = True
window.track_view(echo).color = "#4488cc"

session = Session(multitrack=multitrack, views=[window])
session.sources[TAKE] = Source.file(os.path.basename(take_path)).shaped(
    2, take_frames, float(SAMPLE_RATE))

#: A source nobody wrote down, so the cell below has one of each to report. A
#: session may hold one -- saving must not be blocked by it -- but a reader that
#: finds one knows the samples are not there and opens that element unresolved
#: rather than pretending.
session.sources[2] = Source.volatile()

path = session.save(os.path.join(OUT, "session.json"))
print(f"wrote {path} ({os.path.getsize(path)} B)")

# %% [markdown]
# ## What a save can and cannot promise
#
# Three states a table has to be able to hold, because a save that could not say
# them would either block or decide for the person.

# %%
print(f"  volatile:   {session.volatile()}  (samples nobody wrote down)")
print(f"  open edits: {session.open_edits()}  (a working copy still undecided)")
print(f"  dangling:   {session.dangling()}  (named by the multitrack, absent from "
      "the table)")

# %% [markdown]
# ## Read back, in the multitrack
#
# Reopening is two steps: the file gives the multitrack and its table, and the
# multitrack resolves the table as it opens. Each file is read **once per
# source**, however many regions name it -- two regions over one take are two
# windows onto one take, and reading it twice would give them two that drift
# apart on the first edit.

# %%
def run() -> None:
    """Reopen the session, report it, open it in the multitrack and play it."""
    reopened = Session.open(path)
    print(f"reopened {os.path.basename(path)}: "
          f"{len(reopened.multitrack.tracks)} tracks, "
          f"{reopened.volatile()} still volatile")
    #: The window comes back too, and it is *not* the multitrack: the track
    #: heights and the grid are the person's, and a reader that ignored them
    #: would open the same music into a window that had forgotten everything
    #: about how it was left.
    first_track = reopened.multitrack.tracks[0]
    for window in reopened.views:
        print(f"  window {window.name!r}: grid {window.quant:g}, "
              f"{first_track.name!r} at height {window.track(first_track).height}")
    for track in reopened.multitrack.tracks:
        state = " (muted)" if track.muted else ""
        for region in track.active_take_lane.regions:
            print(f"  {region.position:6.2f}  {track.name}{state}: "
                  f"{region.name or 'unnamed'} ({region.length:.0f} s)")
    #: The multitrack reads the take from the file beside the session, relative
    #: to its folder, and plays it: the regions of the first track name one
    #: source, and one take is read behind them.
    editor = edit(reopened, title="session")
    editor.transport.play()
    editor.wait()


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    run()
else:
    print("up -- run() to reopen the session in the multitrack and hear what it names")
