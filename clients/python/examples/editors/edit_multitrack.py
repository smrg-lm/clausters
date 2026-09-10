#!/usr/bin/env python3
"""``edit(piece)``: a multitrack of audio, drawn, edited and played.

The fourth structure, and the one that holds the other three: a
`clausters.multitrack.Multitrack` — the document the three applications share —
opened with `clausters.gui.edit` like a buffer or a timeline. **One verb, and
the editor is the application**: the picture, the gestures, the history, the
readers on the server and the transport are all its, so this file writes a piece
and hands it over.

What to do in the window:

- **Move a box**, drag its edge to trim it, drag it onto another lane, sweep a
  block and move it as one. **Click** a box to select it, **e** to split it at
  the position cursor, **j** to join a touching run, **q** to quantize.
- **Click the ruler** — or the slack between boxes — to place the **position
  cursor**, which is where the next play starts. The playhead is never placed:
  stopped, it stands on the mark.
- **play/pause** and **stop** are the window's own row. A pause freezes the
  readers where they stand, so playing again continues rather than starting
  over, and stop goes back to the mark rather than to the top.
- **Double click a box** to enter it: what a box holds is a structure like any
  other, so entering one opens the take in the sample editor — on the **piece's**
  own undo order, so `Ctrl`+`Z` walks a stroke drawn inside a box and a box
  dragged on the stack as one history.

**The takes are rendered here and the piece is written plainly**, which is all
this file is: four buffers, three tracks, four boxes and two curves. Everything
after that is `edit`.

**The curves are drawn and not yet heard.** The row under the first track is a
track automation and the line inside the first box is a clip envelope; both are
edited and kept by the piece, and neither reaches a reader — a curve's ``gain``
and the knob's ``gain`` have to name one parameter of one node first, which is
the synthesis node system's design.

Run it as a script (``python edit_multitrack.py``) or cell by cell (``# %%``).
Needs a display and a GPU adapter.
"""

# %%
from clausters import Buffer, Session, Synth
from clausters.defs import SynthDef, out
from clausters.defs.ugens import line, pink_noise, saw, sine, white_noise
from clausters.gui import edit
from clausters.multitrack import (Automation, Content, Lane, Multitrack, Region,
                                  Track)

SR = 48_000.0
TAKE_DUR = 2.0

# %% [markdown]
# ## The takes, rendered offline
# Four signals, so that one lane is told from the next by ear.

# %%
def gentake(name: str, expr, secs: float = TAKE_DUR) -> list:
    """Render ``expr`` for ``secs`` seconds offline and hand back its samples."""
    session = Session.nrt(tempo=1.0)          # beats == seconds
    server = session.server
    SynthDef(name, out(0.0, expr)).send(server)
    node = Synth(name, server=server)
    server.send_bundle_after(secs, ("/node_free", node.id))
    stats = session.render(sample_rate=SR, channels=1)
    return list(stats.samples)


TAKES = {"white": gentake("t_white", white_noise() * 0.5),
         "glide": gentake("t_glide", sine(line(220.0, 440.0, TAKE_DUR)) * 0.5),
         "saw": gentake("t_saw", saw(110.0) * 0.5),
         "pink": gentake("t_pink", pink_noise() * 0.5)}

# %% [markdown]
# ## The session and the takes on the server

# %%
session = Session.live(tempo=1.0, latency=0.1)
server = session.server
BUFS = {name: Buffer.from_samples(samples, server=server)
        for name, samples in TAKES.items()}
server.sync()

# %% [markdown]
# ## The piece
# Three tracks, one lane each, four boxes. A **source id** is what the document
# names — never a path and never a buffer number — because a piece must open in
# a program that has no Python in it. Which buffer each source was read into is
# the one thing about a piece that is not in the piece, and it travels beside it
# in `clausters.gui.editing.Sources`.

# %%
#: source id -> the take it names, so the piece and the table below are written
#: against one list.
SOURCES = {1: "white", 2: "glide", 3: "saw", 4: "pink"}


def box(id: int, at: float, source: int, name: str) -> Region:
    """A box over the whole of one source, from its start."""
    return Region(id=id, position=at, length=TAKE_DUR, name=name,
                  content=Content.onto({"source": {"source": source,
                                                   "lifetime": "session",
                                                   "generation": 0},
                                        "start": 0.0, "duration": TAKE_DUR}))


#: **A track automation**: a row of its own under the track, as long as the
#: timeline, because a track's gain does not begin and end with a box. ``target``
#: is the client's own word for what it drives, and the range is read out of it —
#: the document says what a curve automates and never reads it.
noise_gain = Automation(id=100, name="gain", visible=True,
                        target={"ctl": "level", "min": 0.0, "max": 1.0},
                        points=[{"at": 0.0, "value": 1.0},
                                {"at": 8.0, "value": 0.2}])

#: **A clip envelope**: a layer drawn inside its box, lasting exactly as long as
#: the box does. Its time is the box's own, from zero.
white_fade = Automation(id=101, name="fade", visible=True,
                        target={"ctl": "amp", "min": 0.0, "max": 1.0},
                        points=[{"at": 0.0, "value": 0.2},
                                {"at": TAKE_DUR, "value": 1.0}])

first = box(20, 0.0, 1, "white")
first.automation.append(white_fade)

piece = Multitrack(tracks=[
    Track(id=10, name="noise", automation=[noise_gain],
          lanes=[Lane(id=11, regions=[first, box(21, 6.0, 4, "pink")])]),
    Track(id=12, name="tone",
          lanes=[Lane(id=13, regions=[box(22, 2.0, 2, "glide")])]),
    Track(id=14, name="bass",
          lanes=[Lane(id=15, regions=[box(23, 4.0, 3, "saw")])]),
])

# %% [markdown]
# ## One verb
#
# A `clausters.multitrack.Multitrack` opens as a
# `clausters.gui.editing.MultitrackEditor` — one `multitrack` widget with its own
# ruler and its own transport row, the crate's picture and the crate's reading of
# every gesture. Given a **server** it also sounds: one resident reader per box,
# in a group the server's transport governs, put where the piece says on every
# edit whoever made it.

# %%
session.gui()          # the host wired to this session's server
editor = edit(piece, sample_rate=SR, server=server,
              #: Which buffer each source was read into — and, given as the
              #: **objects**, what a box opens as when it is entered: the same
              #: table answers both, because a piece names a source and only
              #: whoever loaded it holds the take.
              sources={id: BUFS[take] for id, take in SOURCES.items()},
              title="piece", width=1000, height=560)

# %%
editor.wait()
session.close()
