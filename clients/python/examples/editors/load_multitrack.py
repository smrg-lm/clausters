#!/usr/bin/env python3
"""Reopening a multitrack: a saved session read, loaded and edited.

`edit_multitrack.py` builds its piece in memory and writes it down as a
**session** -- its takes as files, the join as the parts it is made of, and the
piece that names them. This opens that file the way any program would: read it,
load its sources into a server, and hand the piece and its buffers to `edit`.

- **Reading** gives the piece and its source table back, and nothing more:
  every box names a source id and nothing has been loaded.
- **Loading** (`Session.load`) reads each take from the file beside the session
  and stitches the join from the takes it is made of, once they are there. What
  is read and in what order is the shared crate's, so this is the same load
  ``clausters-gui --session`` runs.
- **Editing** is `edit`, with the loaded buffers as its sources. The last box is
  the join: play it and it sounds the glide's first half and then the saw's
  second, the parts the file states.

**What it needs:** run ``edit_multitrack.py`` once first, which writes
``examples/out/edit_multitrack.json`` and its takes. A display and a GPU
adapter for the window.

Run it as a script, or step through the cells. Install once, from the repo
root::

    pip install -e clients/python

    python clients/python/examples/editors/load_multitrack.py
"""

# %%
import json
import os
import sys

from clausters import Session
from clausters.gui import edit
from clausters.multitrack import Session as SavedSession

OUT = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "out")
PATH = os.path.join(OUT, "edit_multitrack.json")

# %% [markdown]
# ## Read
#
# The piece and its table, as the file says them.

# %%
with open(PATH) as f:
    saved = SavedSession.read(json.load(f))

for id, source in sorted(saved.sources.items()):
    print(f"  source {id}: {source.location['at']}")

# %% [markdown]
# ## Load
#
# A buffer per source, keyed by source id. The folder is the session's own,
# which is what a relative path is read against.

# %%
session = Session.live(tempo=1.0, latency=0.1)
server = session.server
buffers = saved.load(server, beside=OUT)

joins = [id for id, source in saved.sources.items()
         if source.location["at"] == "segments"]
for id in joins:
    print(f"  source {id} is a join of {buffers[id].parts()}")

# %% [markdown]
# ## Edit
#
# The piece and the buffers it was loaded into. The rate is the one the table
# states for its takes.

# %%
rate = next(source.sample_rate for source in saved.sources.values()
            if source.sample_rate)
session.gui()
editor = edit(saved.multitrack, sample_rate=rate, server=server, sources=buffers,
              title="reopened", width=1000, height=560)

# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        editor.wait()
    finally:
        session.close()
else:
    print("up - edit in the window, session.close() to end")
