#!/usr/bin/env python3
"""Reopening a multitrack: a saved session opened and edited.

`edit_multitrack.py` builds its multitrack in memory and saves it as a **session** --
its takes as files, the join as the parts it is made of, the notes in the file
itself, and the multitrack that names them. This opens that file the way any
program would.

- **Opening** (`Session.open`) gives the multitrack and its source table back, and
  nothing more: every box names a source id, and a source says where it is.
- **Editing** is `edit` over the session: the multitrack, the GUI host's
  application, reads each take from the file beside the session, the join from
  the takes it is made of, and the notes from the file -- the same opening
  ``clausters-gui --session`` does. The join plays the glide's first half and
  then the saw's second, the parts the file states; the box on the keys track
  plays the notes the file holds, as the roll left them if a save came after an
  edit -- double click it to open the roll again. Save writes the session back.

**What it needs:** run ``edit_multitrack.py`` once first, which saves
``clients/python/examples/out/edit_multitrack.json`` and its takes. A display
and a GPU adapter for the window.

Run it from the repository root, where the path below starts -- as a script, or
step through the cells::

    python clients/python/examples/editors/load_multitrack.py
"""

# %%
import sys

from clausters import Session
from clausters.gui import edit
from clausters.multitrack import Session as SavedSession

# %% [markdown]
# ## Open
#
# The multitrack and its table, as the file says them.

# %%
saved = SavedSession.open("clients/python/examples/out/edit_multitrack.json")

for id, source in sorted(saved.sources.items()):
    print(f"  source {id}: {source.location['at']}")
# A sequence is read back as the handle the table holds: the notes it has.
for id, sequence in saved.sequences().items():
    print(f"  source {id} holds {len(sequence)} events")

# %% [markdown]
# ## Edit
#
# The session, in the multitrack: a relative path is read against the folder
# the session was opened from.

# %%
session = Session.live()
session.gui()          # the host, which holds the multitrack
editor = edit(saved, title="reopened")

# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    editor.wait()
    session.close()
else:
    print("open -- edit it in the window; editor.save() writes the session back")
