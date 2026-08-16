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
- **Reading it back.** `from_session` on what the host wrote gives an
  arrangement again, and the cell prints where each element ended up — which is
  the whole claim: a file passed between two writers means the same thing to
  both.

The two files it writes sit **beside this one** (``gui_session.json``, and
``gui_session-edited.json`` once the host has saved) — handed to another program
and read back from it, so they are worth keeping and looking at rather than
leaving in a temp directory.

**What it needs:** nothing running. This example is about the format and the
owner, not about sound — no server is booted and no material is played, which is
also why the clips in that window are empty rectangles rather than waveforms.
The host binary is `clients/gui/target/*/clausters-gui`; build it with
``cargo build --bin clausters-gui`` from ``clients/gui`` if it is not there.

Run it as a script (it writes the file and prints the command), or step through
the cells. Install once, from the repo root::

    pip install -e clients/python

    python clients/python/examples/gui_session.py
"""

# %%
import json
import os
import shutil
import subprocess
import sys

from clausters.form import Group, Track
from clausters.form.document import from_session, to_session
from clausters.seq import Timeline
from clausters.seq.event import Event as SeqEvent

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
piece = Group([
    (0.0, Group([(0.0, melody)], name="melody")),
    (0.0, Group([(0.0, bass)], name="bass")),
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
    f.write(json.dumps(to_session(piece), indent=1))
print(f"wrote {path} ({os.path.getsize(path)} B)")


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

    Drag a clip, then `Ctrl+Z`, `Ctrl+Shift+Z`, `Ctrl+S`, then close it.
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
    print("  drag a clip, then Ctrl+Z, Ctrl+Shift+Z, Ctrl+S, then close the window")
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
    """Prints where every element sits in the session the host wrote."""
    if not os.path.exists(saved):
        print(f"nothing saved yet: press Ctrl+S in the host to write {saved}")
        return
    with open(saved) as f:
        session = json.load(f)
    element = from_session(session)
    print(f"read {saved} back as {type(element).__name__}:")
    for offset, child in getattr(element, "items", lambda: [])():
        print(f"  {offset:6.2f}  {type(child).__name__}")


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
