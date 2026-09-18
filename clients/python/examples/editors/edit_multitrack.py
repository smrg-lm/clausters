#!/usr/bin/env python3
"""``edit(multitrack)``: a multitrack of audio, drawn, edited and played.

The fourth structure, and the one that holds the other three: a
`clausters.multitrack.Multitrack` — the document the three applications share —
opened with `clausters.gui.edit` like a buffer or a timeline. **One verb, and
the editor is the application**: the picture, the gestures, the history, the
readers on the server and the transport are all its, so this file writes a multitrack
and hands it over.

What to do in the window:

- **Move a box**, drag its edge to trim it, drag it onto another lane, sweep a
  block and move it as one. **Click** a box to select it, **e** to split it at
  the position cursor, **j** to join a touching run, **q** to quantize. A
  **run** is two boxes or more on one lane that touch *and read on from each
  other*, so two halves put back in the other order do not join — and the
  **status bar** along the bottom says which of those it was, for every verb
  that finds nothing to do.
- **Click the ruler** — or the slack between boxes — to place the **position
  cursor**, which is where the next play starts. The playhead is never placed:
  stopped, it stands on the mark.
- **Reach a tall multitrack**: `Shift`+wheel **scrolls the stack**, so a track that
  fell off the bottom comes back; `Ctrl`+wheel **zooms the row under the
  pointer** — a track or one of the automation rows, each on its own. A track
  is also zoomed by dragging its header's **bottom edge**; a curve row has no
  edge to pull, which is what the wheel is for. A plain wheel is still the time
  axis'.
- **`A` in a track's header** shows and hides that track's **automation rows** —
  and on a track that has none, the first press **makes** one: a `gain` curve,
  flat at unity across the multitrack, heard like any other. The same shape the
  double click that adds a track has, the verb making the thing rather than
  asking about it. Try it on **bass**, which this file gives no automation.
- **rewind**, **play/pause** and **stop** are the window's own row. A pause
  freezes the readers where they stand, so playing again continues rather than
  starting over; stop goes back to the mark rather than to the top; and rewind
  puts the **mark** back at the top, which is the cursor's verb and not the
  transport's.
- **Double click a box** to enter it: what a box holds is a structure like any
  other, so entering one opens the take in the sample editor — on the **multitrack's**
  own undo order, so `Ctrl`+`Z` walks a stroke drawn inside a box and a box
  dragged on the stack as one history.
- **Watch the meters** while it plays: the strip in each track's header is one
  column per channel, in decibels, over what that track produces *after* its
  clips, its curves and its fader — with the peak it reached held beside it.

**The takes are rendered here and the multitrack is written plainly**, which is all
this file is: six buffers, three tracks, six boxes and two curves. Everything
after that is `edit`.

**The last box is loud on purpose**, so the meter has something to fill: it
reaches full scale and the top of the column is red, where the boxes before it
sit in the green and the amber.

**The curves are heard.** The row under the first track is a track automation
and the line inside the first box is a clip envelope; each names the ``gain``
port of the node it sits on — the same port the header's knob writes — so a
point dragged while the multitrack plays is heard where it is drawn.

**The last box is a join**: one buffer whose samples are spans of two takes,
crossfaded at the seam, which is what a comping pass cut by hand is. It plays as
**one** reader like any other take. Put the cursor inside it and play: the reader
finds the span it lands in once and reads it like a plain buffer from there.

**The multitrack is also written down as a session** (``examples/out/``): its takes as
files, the join as its parts, and the multitrack that names them. So
``clausters-gui --session clients/python/examples/out/edit_multitrack.json``
opens the standalone editor on exactly this material, with no Python behind it.

Run it as a script (``python edit_multitrack.py``) or cell by cell (``# %%``).
Needs a display and a GPU adapter.
"""

# %%
import os

from clausters import Buffer, Session, Synth
from clausters.defs import Part, SynthDef, out
from clausters.defs.ugens import line, pink_noise, saw, sine, white_noise
from clausters.gui import edit
from clausters.multitrack import (Automation, Content, Lane, Multitrack, Region,
                                  Source, Track)
from clausters.multitrack import Session as SavedSession

SR = 48_000.0
TAKE_DUR = 2.0

# %% [markdown]
# ## The takes, rendered offline
# Four signals, so that one lane is told from the next by ear.

# %%
def gentake(name: str, expr, secs: float = TAKE_DUR) -> list:
    """Render ``expr`` for ``secs`` seconds offline and hand back its samples."""
    session = Session.nrt()          # beats == seconds
    server = session.server
    SynthDef(name, out(0.0, expr)).send(server)
    node = Synth(name, server=server)
    server.send_bundle_after(secs, ("/node_free", node.id))
    stats = session.render(sample_rate=SR, channels=1)
    return list(stats.samples)


TAKES = {"white": gentake("t_white", white_noise() * 0.5),
         "glide": gentake("t_glide", sine(line(220.0, 440.0, TAKE_DUR)) * 0.5),
         "saw": gentake("t_saw", saw(110.0) * 0.5),
         "pink": gentake("t_pink", pink_noise() * 0.5),
         #: Deliberately hot, to have something that fills a meter: centred, a
         #: mono take comes out 3 dB down a side (the pan law), so 1.4 reads as
         #: full scale on the track's meter and the last box is the red one.
         "loud": gentake("t_loud", sine(330.0) * 1.4)}

# %% [markdown]
# ## The session and the takes on the server

# %%
session = Session.live(latency=0.1)
server = session.server
BUFS = {name: Buffer.from_samples(samples, sample_rate=SR, server=server)
        for name, samples in TAKES.items()}

# %% [markdown]
# ## A fifth take that is a cut
# The **join**: half of the glide and the second half of the saw, read as one
# buffer with a ten-millisecond crossfade at the seam. Two spans that do not
# continue each other make a step, and a step is a click however well the frames
# are read. A join owns no samples — it is spans of the buffers above — so it
# refuses every write, and `clausters.Buffer.parts` is how a program asks what it
# is made of before it offers to draw one.

# %%
HALF = int(TAKE_DUR * SR) // 2
FADE = int(0.010 * SR)
BUFS["comp"] = Buffer.stitch(
    [Part(BUFS["glide"], start=0, frames=HALF, fade_out=FADE),
     Part(BUFS["saw"], start=HALF, frames=HALF, fade_in=FADE)],
    sample_rate=SR, server=server)
server.sync()

# %% [markdown]
# ## The multitrack
# Three tracks, one lane each, five boxes. A **source id** is what the document
# names — never a path and never a buffer number — because a multitrack must open in
# a program that has no Python in it. Which buffer each source was read into is
# the one thing about a multitrack that is not in the multitrack, and it travels beside it
# in `clausters.gui.editing.Sources`.

# %%
#: source id -> the take it names, so the multitrack and the table below are written
#: against one list.
SOURCES = {1: "white", 2: "glide", 3: "saw", 4: "pink", 5: "comp",
           6: "loud"}


def box(id: int, at: float, source: int, name: str) -> Region:
    """A box over the whole of one source, from its start."""
    return Region(id=id, position=at, length=TAKE_DUR, name=name,
                  content=Content.onto({"source": {"source": source,
                                                   "lifetime": "session",
                                                   "generation": 0},
                                        "start": 0.0, "duration": TAKE_DUR}))


#: **A track automation**: a row of its own under the track, as long as the
#: timeline, because a track's gain does not begin and end with a box. ``target``
#: names the **port** it drives on whatever it is on — the one shape the document
#: crate reads there, because a curve that named nothing could only be guessed at.
noise_gain = Automation(id=100, name="gain", visible=True,
                        target={"port": "gain"},
                        points=[{"at": 0.0, "value": 1.0},
                                {"at": 8.0, "value": 0.2}])

#: **A clip envelope**: a layer drawn inside its box, lasting exactly as long as
#: the box does. Its time is the box's own, from zero, and its port is the clip's.
white_fade = Automation(id=101, name="fade", visible=True,
                        target={"port": "gain"},
                        points=[{"at": 0.0, "value": 0.2},
                                {"at": TAKE_DUR, "value": 1.0}])

first = box(20, 0.0, 1, "white")
first.automation.append(white_fade)

multitrack = Multitrack(tracks=[
    Track(id=10, name="noise", automation=[noise_gain],
          lanes=[Lane(id=11, regions=[first, box(21, 6.0, 4, "pink")])]),
    Track(id=12, name="tone",
          lanes=[Lane(id=13, regions=[box(22, 2.0, 2, "glide"),
                                      box(24, 8.0, 5, "comp"),
                                      box(25, 10.0, 6, "loud")])]),
    #: **The fader is the track's own field**, like its width: what a multitrack
    #: sounds like is the multitrack's, so it is saved with it and reopens as it was.
    Track(id=14, name="bass", level=0.7,
          lanes=[Lane(id=15, regions=[box(23, 4.0, 3, "saw")])]),
])

# %% [markdown]
# ## The same multitrack, for a host with no Python behind it
# The takes above exist only on this server, so a program that cannot run this
# file cannot open this multitrack. Written down as a **session** -- every take as a
# file, the join as the parts it is made of, and the multitrack that names them -- it
# opens with nothing else:
#
#     clausters-gui --session clients/python/examples/out/edit_multitrack.json
#
# which is the standalone editor on exactly this material, and the way to put the
# two side by side. The takes are written as floats, because the last one is hot
# on purpose and a 16-bit file would clip it into a different take. The files go
# to `examples/out/`, the git-ignored directory every generator in this tree
# writes to. **A page cannot do this**: a tab has no filesystem to write a take
# into, so the web twin has no such cell.

# %%
OUT = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "out")
os.makedirs(OUT, exist_ok=True)
FRAMES = int(TAKE_DUR * SR)
ID = {take: id for id, take in SOURCES.items()}

saved = SavedSession(multitrack=multitrack)
for id, take in SOURCES.items():
    if take == "comp":
        continue
    name = f"edit_multitrack-{take}.wav"
    BUFS[take].write(os.path.join(OUT, name), sample_format="float")
    saved.sources[id] = Source.file(name).shaped(1, FRAMES, SR)


def part(take: str, start: int, frames: int, **fades) -> dict:
    """One span of a take, as the join names it: the source, the frames it
    contributes, and the fade at its seam."""
    return {"source": {"source": ID[take], "lifetime": "session", "generation": 0,
                       "range": {"start": start, "end": start + frames}},
            **fades}


#: **The join is its recipe, not its samples**: the same two spans and fades the
#: `Buffer.stitch` above was made from, so a reader builds the same buffer.
saved.sources[ID["comp"]] = Source(location={"at": "segments", "parts": [
    part("glide", 0, HALF, fade_out=FADE),
    part("saw", HALF, HALF, fade_in=FADE),
]}).shaped(1, 2 * HALF, SR)

print(f"wrote {saved.save(os.path.join(OUT, 'edit_multitrack.json'))}")

# %% [markdown]
# ## One verb
#
# A `clausters.multitrack.Multitrack` opens as a
# `clausters.gui.editing.MultitrackEditor` — one `multitrack` widget with its own
# ruler and its own transport row, the crate's picture and the crate's reading of
# every gesture. Given a **server** it also sounds: one resident reader per box,
# in a group the server's transport governs, put where the multitrack says on every
# edit whoever made it.

# %%
session.gui()          # the host wired to this session's server
editor = edit(multitrack, sample_rate=SR, server=server,
              #: Which buffer each source was read into — and, given as the
              #: **objects**, what a box opens as when it is entered: the same
              #: table answers both, because a multitrack names a source and only
              #: whoever loaded it holds the take.
              sources={id: BUFS[take] for id, take in SOURCES.items()},
              title="multitrack", width=1000, height=560)

# %%
editor.wait()
session.close()
