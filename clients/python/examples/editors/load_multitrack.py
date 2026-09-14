#!/usr/bin/env python3
"""Reopening a multitrack: a saved session opened, loaded and edited.

`edit_multitrack.py` builds its piece in memory and saves it as a **session** --
its takes as files, the join as the parts it is made of, and the piece that
names them. This opens that file the way any program would.

- **Opening** (`Session.open`) gives the piece and its source table back, and
  nothing more: every box names a source id and nothing has been loaded.
- **Loading** (`Session.load`) reads each take from the file beside the session
  and stitches the join from the takes it is made of, once they are there. What
  is read and in what order is the shared crate's, so this is the same load
  ``clausters-gui --session`` runs.
- **Editing** is `edit`, with the loaded buffers as its sources. The last box is
  the join: play it and it sounds the glide's first half and then the saw's
  second, the parts the file states.

**What it needs:** run ``edit_multitrack.py`` once first, which saves
``clients/python/examples/out/edit_multitrack.json`` and its takes. A display
and a GPU adapter for the window.

Run it from the repository root, where the path below starts -- as a script, or
step through the cells::

    python clients/python/examples/editors/load_multitrack.py
"""

# %%
from clausters import Session
from clausters.gui import edit
from clausters.multitrack import Session as SavedSession

# %% [markdown]
# ## Open
#
# The piece and its table, as the file says them.

# %%
saved = SavedSession.open("clients/python/examples/out/edit_multitrack.json")

for id, source in sorted(saved.sources.items()):
    print(f"  source {id}: {source.location['at']}")

# %% [markdown]
# ## Load
#
# A buffer per source, keyed by source id. A relative path is read against the
# folder the session was opened from.

# %%
session = Session.live(tempo=1.0, latency=0.1)
server = session.server
buffers = saved.load(server)

for id, source in saved.sources.items():
    if source.location["at"] == "segments":
        print(f"  source {id} is a join of {buffers[id].parts()}")

# %% [markdown]
# ## Edit
#
# The piece and the buffers it was loaded into, at the rate the table states for
# its takes.

# %%
rate = next(source.sample_rate for source in saved.sources.values()
            if source.sample_rate)
session.gui()
editor = edit(saved.multitrack, sample_rate=rate, server=server, sources=buffers,
              title="reopened", width=1000, height=560)

# %%
editor.wait()
session.close()
