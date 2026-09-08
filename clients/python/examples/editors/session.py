#!/usr/bin/env python3
"""A **session**: the piece, where its samples are, and reading it back.

A piece says what plays when; a *session* is that plus the table saying where
its samples live, and the format lives in the shared crate precisely so that
more than one program can write it. This writes one, reads it back, and reopens
its take onto a running server -- the loop a save and an open actually are.

What it shows, in the order the cells run:

- **The piece.** Two tracks, one of them muted, and a region on each. A region
  is one placed thing: where it starts, how long it occupies, and what fills it.
  Six regions over one source would be six identities and one source -- nothing
  is copied -- which is the whole of non-destructive editing.
- **A take, which is samples rather than description.** The example writes its
  own stereo WAV, so it needs no material found anywhere, and the session's
  table is what says where those samples are. The piece names a source **id**
  and never a path: that is the split that lets one file be opened by a program
  that has no Python in it.
- **What a save can and cannot promise.** The table says three things a save
  has to be able to say without blocking or deciding for the person: a file that
  is there, samples nobody wrote down, and a working copy whose destructive edit
  is still open. The cell prints which sources are in which state.
- **Reading it back.** The session is loaded and its table resolved: each file
  it names is read onto the server **once per source**, however many regions
  draw it, and the region that names it plays. A source the table calls volatile
  comes back frozen -- drawn, placed, silent -- rather than as a lie.
- **What is the piece's and what is the view's.** A muted track reopens muted,
  because mute is the composition's. A track's *height* is not: it says nothing
  about what the piece is, so no session carries it.

**Not yet here: handing it to a host with no language attached.** This example
used to run ``clausters-gui --session`` on what it wrote, and that half went
with the client-side converter it was built on: the standalone host opens the
general tree, and binding the arrangement is the next step in the crate's plan.
It comes back with the host, and this example is where it lands.

The two files it writes go to ``examples/out/`` (``session.json`` and
``session-take.wav``), the git-ignored directory every generator in this tree
writes to.

**What it needs:** nothing running -- the example boots its own server for the
reopening cell, and writes its own WAV.

Run it as a script, or step through the cells. Install once, from the repo
root::

    pip install -e clients/python

    python clients/python/examples/editors/session.py
"""

# %%
import json
import math
import os
import struct
import sys
import time
import wave

from clausters import Session as Server
from clausters.multitrack import (Multitrack, Content, Lane, Region, Session,
                                   Source, Span, Tempo, Track, View)
from clausters.defs.buffer import Buffer
from clausters.document import ARRANGEMENT, domain_edit
from clausters.play import play

SAMPLE_RATE = 48_000

#: Where a run leaves its files: ``examples/out/``, the git-ignored directory
#: every generator in this tree writes to.
OUT = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "out")
os.makedirs(OUT, exist_ok=True)

# %% [markdown]
# ## A take, which is samples rather than description
#
# The piece names a source **id** and never a path. Where the samples are is the
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
# ## The piece
#
# Two tracks, each with one lane, each with one region. The region's `position`
# and `length` are in **beats** -- where a thing sits in a piece is a musical
# decision -- while what fills it is measured in its own source's units. The two
# are not the same axis, and the tempo map is what relates them, which is why
# the map is part of the piece rather than of a track.

# %%
#: The source id the piece names. The table below is what says where it is.
TAKE = 1


def window(start: float = 0.0, duration: float = take_seconds) -> dict:
    """A window onto the take: which source, from where, for how long."""
    return {"source": {"source": TAKE, "lifetime": "session", "generation": 0},
            "start": start, "duration": duration}


piece = Multitrack()
piece.set_tempo(Tempo(at=0.0, bpm=120.0))

#: The take, twice, at two places: two regions, two identities, **one** source.
#: Nothing is copied, and trimming one leaves the other where it was.
tone = Track(id=10, name="tone", lanes=[Lane(id=11)])
tone.active_lane.place(Region(id=12, position=0.0, length=4.0, name="first",
                              content=Content.onto(window())))
tone.active_lane.place(Region(id=13, position=8.0, length=2.0, name="again",
                              content=Content.onto(window(start=0.5))))

#: **Mute is the composition's.** A track left muted here reopens muted, because
#: it says something about the piece. A track's *height* does not, so no session
#: carries one.
echo = Track(id=20, name="echo", muted=True, lanes=[Lane(id=21)])
echo.active_lane.place(Region(id=22, position=4.0, length=4.0,
                              content=Content.onto(window())))

piece.tracks.extend([tone, echo])
print(f"the piece is {piece.end:.0f} beats long, over {len(piece.tracks)} tracks")

# %% [markdown]
# ## Editing it: one edit, one undo
#
# The piece has a vocabulary of its own, reached through the door every other
# structure is reached through. Two things about it are worth seeing rather than
# reading: **where a region is** means track, lane and beat together, so moving
# one to the other track is a single edit -- there is no moment in between where
# it is on no lane at all; and the crate hands back the edit that *puts it back*
# in the same answer, because an inverse has to be read before the edit lands.

# %%
moved = domain_edit(
    ARRANGEMENT, piece.write(),
    {"intent": "placeregion", "region": 13, "track": 20, "lane": 21,
     "position": 12.0, "layer": 0},
)
after = Multitrack.read(moved["state"])
where = next((t, l, r) for t in after.tracks for l in t.lanes
             for r in l.regions if r.id == 13)
print(f"  moved:  region 13 is on track {where[0].id}, lane {where[1].id}, "
      f"at beat {where[2].position:.0f}")
print(f"  and to put it back: {moved['current']}")

#: The other direction, through the same door -- and the piece is exactly the
#: one that was built above, which is what "absolute" buys.
piece = Multitrack.read(domain_edit(ARRANGEMENT, moved["state"],
                                     moved["current"])["state"])
print(f"  undone: the piece is {piece.end:.0f} beats long again, "
      f"over {len(piece.tracks)} tracks")

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
#: composition -- nothing here can change what plays -- and all of it is state
#: the person loses on a reopen unless the file carries it. A list, because a
#: piece drawn in two windows has two views and they disagree on purpose.
window = View(name="arranger", visible=Span(0.0, piece.end), quant=1.0)
window.track_view(10).height = 96.0
window.track_view(10).lanes_shown = True
window.track_view(20).color = "#4488cc"
window.selected = [12]

session = Session(multitrack=piece, views=[window])
session.sources[TAKE] = Source.file(os.path.basename(take_path)).shaped(
    2, take_frames, float(SAMPLE_RATE))

#: A source nobody wrote down, so the cell below has one of each to report. A
#: session may hold one -- saving must not be blocked by it -- but a reader that
#: finds one knows the samples are not there and opens that element unresolved
#: rather than pretending.
session.sources[2] = Source.volatile()

path = os.path.join(OUT, "session.json")
with open(path, "w") as f:
    f.write(json.dumps(session.write(), indent=1))
print(f"wrote {path} ({os.path.getsize(path)} B)")

# %% [markdown]
# ## What a save can and cannot promise
#
# Three states a table has to be able to hold, because a save that could not say
# them would either block or decide for the person.

# %%
print(f"  volatile:   {session.volatile()}  (samples nobody wrote down)")
print(f"  open edits: {session.open_edits()}  (a working copy still undecided)")
print(f"  dangling:   {session.dangling()}  (named by the piece, absent from "
      "the table)")

# %% [markdown]
# ## Read back, onto a running server
#
# Reopening is two steps: the file gives the piece and its table, and the table
# is resolved. Each file is read **once per source**, however many regions name
# it -- two regions over one take are two windows onto one buffer, and reading
# it twice would give them two that drift apart on the first edit.

# %%
def reopen(server=None) -> tuple:
    """Reads the session back and resolves its table onto `server`.

    Returns the piece and the buffers by source id. A source the table cannot
    locate is simply absent from the second: half a session is worth opening,
    and the region that names it comes back placed and silent rather than the
    whole file failing.
    """
    with open(path) as f:
        reopened = Session.read(json.load(f))
    buffers = {}
    for id, source in reopened.sources.items():
        if source.path is None or server is None:
            continue
        where = source.path
        if not os.path.isabs(where):
            where = os.path.join(OUT, where)
        buffers[id] = Buffer.read(where, server=server.server)
    return reopened, buffers


def run() -> None:
    """Reopen the session, report it, and play what its first region names."""
    with Server.live(tempo=2.0).activate() as session_server:
        reopened, buffers = reopen(session_server)
        session_server.server.sync()
        print(f"reopened {os.path.basename(path)}: "
              f"{len(reopened.multitrack.tracks)} tracks, "
              f"{len(buffers)} source(s) read, "
              f"{reopened.volatile()} still volatile")
        #: The window comes back too, and it is *not* the piece: the track
        #: heights and the grid are the person's, and a reader that ignored
        #: them would open the same music into a window that had forgotten
        #: everything about how it was left.
        for window in reopened.views:
            print(f"  window {window.name!r}: grid {window.quant:g}, "
                  f"track 10 at height {window.track(10).height}, "
                  f"holding {window.selected}")

        for track in reopened.multitrack.tracks:
            state = " (muted)" if track.muted else ""
            for region in track.active_lane.regions:
                print(f"  {region.position:6.2f}  {track.name}{state}: "
                      f"{region.name or region.id} "
                      f"({region.length:.0f} beats)")

        # The two regions of the first track name one source, and there is one
        # buffer behind them.
        first = reopened.multitrack.track(10).active_lane.regions[0]
        named = first.content.window["source"]["source"]
        buffer = buffers.get(named)
        if buffer is None:
            print("  the take's file was not found, so nothing plays")
            return
        print(f"  playing source {named} from {buffer}")
        play(buffer, server=session_server.server)
        time.sleep(take_seconds + 0.5)


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    run()
else:
    print("up — run() to reopen the session and hear what it names")
