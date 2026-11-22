"""Applications as pages of the editor's window, and views beside the code.

Open it in the editor and evaluate it a cell at a time (Ctrl+Enter):

    clausters-editor clients/python/examples/panels/applications.py

`open_app(x)` opens the application that edits ``x`` on a page of its own, its
menu in the editor's bar while its page is shown; `open_app(app=...)` opens one
over nothing, as the buttons at the bar's right end do. `edit(x)` opens the
view alone, a panel beside the code. Its JavaScript twin is
clients/web/examples/panels/applications.js, for the editor's page.
"""

# %%
from clausters import Session, play, scope, edit, open_app
from clausters.seq import INF, EventSequence, pbind, pseq

s = Session.live().activate()
a = EventSequence.from_rows([(60, 1), (62, 0.5), (64, 0.5), (67, 1), (72, 1)])

# %% the score editor over the sequence, on a page of its own
q = open_app(a, view="score", title="a")
print(q.title)

# %% an empty multitrack, on another page
mt = open_app(app="multitrack")
print(mt.title)

# %% the roll over the same sequence, beside the code: no menu, no toolbar
r = edit(a, title="a")

# %% what the roll and the page left in the sequence: drag a note first
a.to_rows("midinote", "dur")

# %% a scope of the session's server, beside the code
p = play(pbind(instrument="default", degree=pseq([0, 2, 4, 7], INF), dur=0.25))
w = scope(0)

# %%
p.stop()
w.close()
r.close()

# %%
s.close()

"""
- A page tab's cross closes its application, asking first if it has changes.
- Measures > Insert After on the score's page says "select a note first" with
  nothing selected: the pick is the score editor's.
- Session > Restart takes the session's windows with it; what the bar's
  buttons opened stays.
"""
