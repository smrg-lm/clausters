"""Applications as pages of the editor's window.

Open it in the editor and evaluate it a cell at a time (Ctrl+Enter):

    clausters-editor clients/python/examples/panels/applications.py

The session draws in the editor's own window: each application on a page of
its own, its menu in the editor's bar while its page is shown. Its JavaScript
twin is clients/web/examples/panels/applications.js, for the editor's page.
"""

# %%
from clausters import Session, play, scope
from clausters.gui import edit
from clausters.seq import INF, EventSequence, pbind, pseq

s = Session.live().activate()
g = s.gui()   # the editor's host: nothing is booted

[p["title"] for p in g.dock_call({"verb": "layout"})["panes"]]   # the editor's panels

# %% a sequence in the score editor, on a page of its own, its menu in the editor's bar
a = EventSequence.from_rows([(60, 1), (62, 0.5), (64, 0.5), (67, 1), (72, 1)])
q = edit(a, view="score", title="a")
g.dock_call({"verb": "layout"})["page"]

# %% the multitrack, on another page
mt = g.open_app("multitrack")
[p["name"] for p in g.dock_call({"verb": "layout"})["pages"]]

# %% back to the code: the editor's menu again
g.dock_call({"verb": "page", "name": "Main"})["page"]

# %% a scope of the session's server, beside the code
p = play(pbind(instrument="default", degree=pseq([0, 2, 4, 7], INF), dur=0.25))
w = scope(0)

# %%
p.stop()
w.close()

# %%
s.close()

"""
- The page tabs along the bottom switch between the code, the score and the
  multitrack; the bar at the top shows the menu of the page shown.
- On the score's page, Measures > Insert After says "select a note first"
  under the score: the pick is the score editor's. Click a note and pick it
  again.
- Session > Restart takes the session's windows away with it.
"""
