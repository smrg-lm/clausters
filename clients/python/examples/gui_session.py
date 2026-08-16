#!/usr/bin/env python3
"""The **third writer**: a session this client writes, edited by a host with no
language attached, and read back here unchanged.

A document says what plays when; a *session* is that plus the table saying where
its material lives, and it lives in the shared crate precisely so that more than
one program can write it. Until now two of the three writers existed — this
client, and this client again. The third is `clausters-gui --session`, a host
that opens the file, draws it as a multitrack, applies its own gestures through
the crate's own log and saves it back. No Python anywhere in that loop.

What it shows, in the order the cells run:

- **Writing one.** An arrangement built the ordinary way (`Group`, `Track`,
  `Timeline`) becomes a session with `to_session` — the same call `gui_daw.py`
  makes when it saves.
- **Handing it over.** The command to open it in the standalone host is printed
  for you to run. Drag a clip, `Ctrl+Z` to take it back, `Ctrl+Shift+Z` to put
  it back, `Ctrl+S` to save. The host is the owner while that window is open:
  the intent your drag emits is applied *there*, by the crate's `apply`, and the
  inverse comes out of the document rather than being remembered.
- **Editing the material, not only the description.** The take opens twice: as
  a clip in its lane, and as an editor under the ruler on an axis of its own.
  Zoom that one in (wheel) until each sample is a disc, then **Alt+drag** to
  draw over them — the picture changes over the span you drew and nowhere else,
  and `Ctrl+Z` puts the samples back. The take is **stereo** and the channels
  are drawn as stacked lanes: a stroke lands in the lane it was made in and the
  other keeps its shape, because one channel of interleaved material is written
  as the strided span it is. **Click** on the waveform to place the
  playhead, **space** to play from it and to pause where it stands (a pause
  freezes the server's own transport, so playing again continues rather than
  starting over), and **drag** a span to loop it. All of it goes through the
  embedded server: the clip and the editor draw the one buffer a stroke writes,
  and the line you see is the position that server is playing — the host reads
  it, and never computes it.
- **Reading it back.** `from_session` on what the host wrote gives an
  arrangement again, and the cell prints where each element ended up — which is
  the whole claim: a file passed between two writers means the same thing to
  both.

The two files it writes sit **beside this one** (``gui_session.json``, and
``gui_session-edited.json`` once the host has saved) — handed to another program
and read back from it, so they are worth keeping and looking at rather than
leaving in a temp directory.

**What it needs:** nothing running — the host boots its own embedded server
(a `--features standalone` build; without one the take still draws as a named
rectangle and the space bar does nothing). It writes its own WAV, so no material
has to be found. The host binary is `clients/gui/target/*/clausters-gui`; build it with
``cargo build --bin clausters-gui`` from ``clients/gui`` if it is not there.

Run it as a script (it writes the file and prints the command), or step through
the cells. Install once, from the repo root::

    pip install -e clients/python

    python clients/python/examples/gui_session.py
"""

# %%
import json
import math
import os
import shutil
import struct
import subprocess
import sys
import wave

from clausters.form import Buffer, Group, Track
from clausters.form.document import FrozenSource, from_session, to_session
from clausters.seq import Timeline
from clausters.seq.event import Event as SeqEvent

SAMPLE_RATE = 48_000

# %% [markdown]
# ## An arrangement, built the ordinary way
#
# Nothing here knows about sessions or hosts: it is the same `Group` of `Track`s
# any other example builds. Two lanes, so the window has two to draw.

# %%
melody = Track(Timeline([
    (0.0, SeqEvent(midinote=72, dur=1.0)),
    (2.0, SeqEvent(midinote=76, dur=1.0)),
]))
bass = Track(Timeline([
    (0.0, SeqEvent(midinote=48, dur=2.0)),
    (4.0, SeqEvent(midinote=52, dur=2.0)),
]))

# %% [markdown]
# ## And a take, which is material rather than description
#
# A document says *what plays when* and never where the samples are — so a take
# is a source **id**, and the session's table is what says where that source
# lives. The two halves are written together below; the host resolves the table
# and reads each file into a server buffer of its own, which is what lets a clip
# draw its waveform instead of an empty rectangle.

#: A file beside this one, written here so the example needs nothing but itself
#: — an ordinary WAV, the kind a person drags in, decoded by the server the way
#: any other would be.
take_path = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                         "gui_session-take.wav")


def write_take(path: str, seconds: float = 2.0, freq: float = 440.0) -> int:
    """Writes a short **stereo** tone to `path` and returns its frame count.

    Two partials and a slow decay, so the drawn waveform has a shape to
    recognize rather than a rectangle of noise -- and the two channels are
    deliberately unlike (the right one is the third partial alone, quieter), so
    an edit on one is visibly an edit on *one*: a channel of interleaved
    material is a strided write, and that it lands where it was aimed is the
    thing worth seeing.
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

#: The element names **source 1** and nothing else: `FrozenSource` is what a
#: document reader hands back for a source it has not resolved to a live server
#: buffer, and it is exactly what writing one from a file needs — the id, and
#: the table beside it.
take = Buffer(FrozenSource({"source": 1, "lifetime": "session"}),
              duration=take_frames / SAMPLE_RATE)

#: Where source 1 is. A **relative** path, resolved against the session file's
#: own folder, which is what makes the pair of files movable together.
sources = {
    1: {
        "location": {"at": "file", "path": os.path.basename(take_path)},
        "lifetime": "session",
        "generation": 0,
        "channels": 2,
        "frames": take_frames,
        "sample_rate": float(SAMPLE_RATE),
    },
}

piece = Group([
    (0.0, Group([(0.0, melody)], name="melody")),
    (0.0, Group([(0.0, bass)], name="bass")),
    (1.0, Group([(0.0, take)], name="take")),
], name="piece")

# %% [markdown]
# ## Written as a session
#
# `to_session` is the format's one writer on this side — the crate defines it,
# and this client and the standalone host are two readers of the same
# definition rather than two implementations of one idea.

#: The two artifacts, **beside this file** — the shape the server's examples
#: already use for a companion (`config.toml`, `ws_ping.html` live next to the
#: scripts that name them). Not a temp directory, because these are not scratch:
#: one is handed to another program and the other comes back from it, and both
#: are worth opening, diffing and re-running the host on. Named after the
#: example so they group with it; git ignores them.
HERE = os.path.dirname(os.path.abspath(__file__))
path = os.path.join(HERE, "gui_session.json")
saved = os.path.join(HERE, "gui_session-edited.json")

# %%
with open(path, "w") as f:
    f.write(json.dumps(to_session(piece, sources=sources), indent=1))
print(f"wrote {path} ({os.path.getsize(path)} B)")
print(f"  and {take_path} ({take_frames} frames), which its source table names")


# %% [markdown]
# ## Handed to a host with no language attached
#
# The host opens it, draws it, and **owns** it: the gesture you make there is
# applied by the crate's `apply` and logged by the crate's log, so the undo you
# press reads its inverse out of the document. Nothing round-trips through
# Python while that window is open — which is the point of the milestone.

# %%
def host_binary() -> "str | None":
    """The standalone host, wherever this checkout built it."""
    here = os.path.dirname(os.path.abspath(__file__))
    root = os.path.abspath(os.path.join(here, "..", "..", ".."))
    for profile in ("release", "debug"):
        candidate = os.path.join(root, "clients", "gui", "target", profile, "clausters-gui")
        if os.path.exists(candidate):
            return candidate
    return shutil.which("clausters-gui")


def open_in_host(wait: bool = True) -> None:
    """Runs the host on the session, saving to a second file.

    Drag a clip, then `Ctrl+Z`, `Ctrl+Shift+Z`, `Ctrl+S`, then close it. The
    take's editor pane under the ruler is the other half: zoom in to the
    samples, Alt+drag to draw over them, click to place the playhead, Space to
    play and pause, and drag a span to loop it.
    Saving writes to ``--save-to`` and never over what was opened: overwriting
    the file you were given is a decision, not a default.
    """
    binary = host_binary()
    if binary is None:
        print("clausters-gui not found — build it:\n"
              "  cd clients/gui && cargo build --bin clausters-gui")
        return
    cmd = [binary, "--session", path, "--save-to", saved]
    print("running: " + " ".join(cmd))
    print("  drag a clip, then Ctrl+Z, Ctrl+Shift+Z, Ctrl+S")
    print("  and in the take's editor pane: wheel to zoom to the samples, "
          "Alt+drag to draw, click to place the playhead, Space to play/pause, "
          "drag a span to loop it, then close the window")
    if wait:
        subprocess.run(cmd, check=False)


# %% [markdown]
# ## Read back here
#
# `from_session` turns what the host saved into an arrangement again. The offsets
# printed are where the elements ended up — move a clip in the window and the
# number moves with it, which is the claim the whole milestone rests on.

# %%
def read_back() -> None:
    """Prints where every element sits in the session the host wrote.

    `from_session` hands back **the arrangement and its source table** — what a
    source *is* (a buffer to allocate, a file to map) being the caller's to
    decide — so the element is the first of the pair.
    """
    if not os.path.exists(saved):
        print(f"nothing saved yet: press Ctrl+S in the host to write {saved}")
        return
    with open(saved) as f:
        session = json.load(f)
    element, sources = from_session(session)
    print(f"read {os.path.basename(saved)} back as {type(element).__name__} "
          f"({len(sources)} source(s)):")
    for offset, child, depth in _walk(element):
        print(f"  {offset:7.3f}  {'  ' * depth}{type(child).__name__}")


def _walk(element, base: float = 0.0, depth: int = 0):
    """Every placed element and where it sits, absolute in beats.

    A composition is a tree, so reading one back is a walk: what the host moved
    is a placement somewhere inside it, and printing only the top would show
    nothing changing.
    """
    for offset, _dur, child in getattr(element, "members", []):
        here = base + offset
        yield here, child, depth
        yield from _walk(child, here, depth + 1)


# %%
def run() -> None:
    """Open the session in the host, then read back whatever it saved."""
    open_in_host()
    read_back()


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    run()
else:
    print("up — open_in_host() to hand it over, read_back() to read what it saved")
