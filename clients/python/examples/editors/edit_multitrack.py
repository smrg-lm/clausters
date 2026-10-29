#!/usr/bin/env python3
"""``edit(session)``: a multitrack of audio and notes, drawn, edited and played.

The fourth structure, and the one that holds the other three: a
`clausters.multitrack.Multitrack` -- the document the three applications share --
saved as a **session** and opened with `clausters.gui.edit` like a sound file.
**One verb, and the editor is the application**: the GUI host opens the
multitrack in a window of its own, reads the session's takes from their files
and plays them on an engine of its own; the picture, the gestures, the history
and the transport are all its, so this file writes a session and hands it over.

What to do in the window:

- **Move a box**, drag its edge to trim it, drag it onto another track, sweep a
  block and move it as one. **Click** a box to select it, **e** to split it at
  the position cursor, **j** to join a touching run, **q** to quantize. A
  **run** is two boxes or more on one track that touch *and read on from each
  other*, so two halves put back in the other order do not join -- and the
  **status bar** along the bottom says which of those it was, for every verb
  that finds nothing to do.
- **Click the ruler** -- or the slack between boxes -- to place the **position
  cursor**, which is where the next play starts. The playhead is never placed:
  stopped, it stands on the mark.
- **Reach a tall multitrack**: `Shift`+wheel **scrolls the stack**, so a track that
  fell off the bottom comes back; `Ctrl`+wheel **zooms the row under the
  pointer** -- a track or one of the automation rows, each on its own. A track
  is also zoomed by dragging its header's **bottom edge**; a curve row has no
  edge to pull, which is what the wheel is for. A plain wheel is still the time
  axis'.
- **`A` in a track's header** shows and hides that track's **automation rows** --
  and on a track that has none, the first press **makes** one: a `gain` curve,
  flat at unity across the multitrack, heard like any other. The same shape the
  double click that adds a track has, the verb making the thing rather than
  asking about it. Try it on **bass**, which this file gives no automation.
- **The secondary button over a track** opens its **context menu**: each of
  its curves, to show or hide one at a time -- the `A` shows or hides them
  all -- a track added under it, and the rows' heights as a whole: **Reset
  track heights** takes every zoom back, **Compact tracks** fits the stack.
  Over a box, the menu lists **the box's own curves**.
- **The menu bar** names every verb of the window -- File, Edit, View,
  Transport, Track, Help -- with its key beside it, and the **toolbar** under
  it holds the transport and Split, Join and Quantize. A tool acts on what the
  keys act on, so **select a box and press Split**; **F1** lists the keys.
- **rewind**, **play/pause** and **stop** head the toolbar. A pause
  freezes the readers where they stand, so playing again continues rather than
  starting over; stop goes back to the mark rather than to the top; and rewind
  puts the **mark** back at the top, which is the cursor's verb and not the
  transport's. **The space bar is play/stop**, so the play cursor comes back
  to the mark every time, and **Home** and **End** put the mark at the start
  and where the last region ends. **Alt and a drag** anywhere marks a time
  range, as in the audio editor: the space bar plays it, and **L** loops it.
  **Shift+L** is the other switch of a pass, Stop at end in the Transport
  menu: off, the transport rolls on past the last region until it is stopped;
  on, it stops where the contents end and goes back to the mark.
- **Double click the box on the keys track** to open its **roll**. That box
  holds notes -- an `EventSequence` -- and the roll opens over the very same
  sequence, in the multitrack's undo order: drag a note there and the box redraws
  it, play and it is heard where it now is, and `Ctrl`+`Z` in either window walks
  one history. A box of samples opens nothing on a double click: the multitrack
  edits where things are, never a take.
- **View, Notes as scores** draws the keys box as notation: its four notes
  on a staff, each at its time under the ruler, the clef before the box and
  the meter over the staff. Zoom the time axis and the notes spread with it;
  drag the track's lower edge and the staff grows with the row, and a row too
  low for a staff goes back to the roll. A double click on the box then opens
  the score editor over the same sequence. **Notes as rolls** puts it back.
- **Watch the meters** while it plays: the strip in each track's header is one
  column per channel, in decibels, over what that track produces *after* its
  clips, its curves and its fader -- with the peak it reached held beside it.

**The takes are rendered here and the multitrack is written plainly**, which is all
this file is: five takes written as files, a join of two of them, a sequence of
notes, four tracks, seven boxes and two curves, saved as one session. Everything
after that is `edit`.

**The keys track is notes.** Its box is a window onto an `EventSequence` the
session holds in the file itself: it draws the notes its window reads and plays
them from the transport's own event lane, so a locate, a loop and a stop are the
transport's. **A note sounds into its box and through the track's strip**, as a
take does: pull the keys track's fader down while it plays, or draw a gain
curve under it, and the notes follow -- its mute and its meter are over them
too. What plays a note is the def its events name, the built-in `default` here.

**The last box is loud on purpose**, so the meter has something to fill: it
reaches full scale and the top of the column is red, where the boxes before it
sit in the green and the amber.

**The curves are heard.** The row under the first track is a track automation
and the line inside the first box is a clip envelope; each names the ``gain``
port of what it sits on -- the same port the header's knob writes -- so a point
dragged while the multitrack plays is heard where it is drawn.

**The comp box is a join**: a source whose samples are spans of two takes,
crossfaded at the seam, which is what a comping pass cut by hand is. The session
states it as its parts, and the application reads it from the two files.

**The script's multitrack is the window's while it is open**: the last cell
moves the bass track's fader from here, and the window's knob moves with it --
one edit of the window's, which its Ctrl+Z takes back.

The session is written to ``clients/python/examples/out/``, so
``clausters-gui --session clients/python/examples/out/edit_multitrack.json``
opens the same multitrack with no Python behind it, and ``load_multitrack.py``
reopens it from a script.

Run it as a script (``python edit_multitrack.py``) or cell by cell (``# %%``).
Needs a display and a GPU adapter.
"""

# %%
import os
import sys

from clausters import Session, SoundFile, Synth
from clausters.defs import SynthDef, out
from clausters.defs.ugens import line, pink_noise, saw, sine, white_noise
from clausters.gui import edit
from clausters.multitrack import Content, Multitrack, Source
from clausters.multitrack import Session as SavedSession
from clausters.seq import EventSequence
from clausters.seq.event import Event

SR = 48_000.0
TAKE_DUR = 2.0
OUT = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "out")
os.makedirs(OUT, exist_ok=True)

# %% [markdown]
# ## The takes, rendered offline and written as files
# Five signals, so that one track is told from the next by ear. They are
# written as floats, because the last one is hot on purpose and a 16-bit file
# would clip it into a different take. The files go to `examples/out/`, the
# git-ignored directory every generator in this tree writes to.

# %%
def gentake(name: str, expr, secs: float = TAKE_DUR) -> str:
    """Render ``expr`` for ``secs`` seconds offline, write it beside the
    session, and hand back the file's name."""
    session = Session.nrt()          # beats == seconds
    server = session.server
    SynthDef(name, out(0.0, expr)).send(server)
    node = Synth(name, server=server)
    server.send_bundle_after(secs, ("/node_free", node.id))
    stats = session.render(sample_rate=SR, channels=1)
    file = f"edit_multitrack-{name}.wav"
    SoundFile.write(os.path.join(OUT, file), list(stats.samples), 1, SR,
                    sample_format="float")
    return file


TAKES = {"white": gentake("white", white_noise() * 0.5),
         "glide": gentake("glide", sine(line(220.0, 440.0, TAKE_DUR)) * 0.5),
         "saw": gentake("saw", saw(110.0) * 0.5),
         "pink": gentake("pink", pink_noise() * 0.5),
         #: Deliberately hot, to have something that fills a meter: centred, a
         #: mono take comes out 3 dB down a side (the pan law), so 1.4 reads as
         #: full scale on the track's meter and the last box is the red one.
         "loud": gentake("loud", sine(330.0) * 1.4)}

# %% [markdown]
# ## A sequence of notes
# The one source that is not samples: events with ids, in beats -- one beat a
# second here, since it states no tempo map. The session holds it in the file.

# %%
KEYS = EventSequence([(beat, Event(midinote=note, sustain=0.4, amp=0.2))
                      for beat, note in [(0.0, 60), (0.5, 64), (1.0, 67),
                                         (1.5, 72)]])

# %% [markdown]
# ## The multitrack
# Four tracks, one take lane each, seven boxes. A **source id** is what the
# document names -- never a path -- because a multitrack must open in a program
# that has no Python in it. Where each source is, is the session's table below.

# %%
#: source id -> the take it names, so the multitrack and the table below are
#: written against one list.
SOURCES = {1: "white", 2: "glide", 3: "saw", 4: "pink", 5: "comp",
           6: "loud"}


def window(source: int) -> Content:
    """The whole of one source, from its start."""
    return Content.onto({"source": {"source": source, "lifetime": "session",
                                    "generation": 0},
                         "start": 0.0, "duration": TAKE_DUR})


multitrack = Multitrack()
noise = multitrack.tracks.add("noise")
tone = multitrack.tracks.add("tone")
#: **The fader is the track's own field**, like its width: what a multitrack
#: sounds like is the multitrack's, so it is saved with it and reopens as it was.
bass = multitrack.tracks.add("bass", level=0.7)
keys = multitrack.tracks.add("keys")

#: Each track starts with one take lane, and a box is placed on it: where it
#: starts, how long it is, and the source it is a window onto. The multitrack
#: names every box, so the script holds the box and never its number.
first = noise.active_take_lane.regions.add(0.0, TAKE_DUR, window(1), name="white")
noise.active_take_lane.regions.add(6.0, TAKE_DUR, window(4), name="pink")
tone.active_take_lane.regions.add(2.0, TAKE_DUR, window(2), name="glide")
tone.active_take_lane.regions.add(8.0, TAKE_DUR, window(5), name="comp")
tone.active_take_lane.regions.add(10.0, TAKE_DUR, window(6), name="loud")
bass.active_take_lane.regions.add(4.0, TAKE_DUR, window(3), name="saw")
#: **A box of notes** is a box like any other: a window onto source 7, which
#: is the sequence above rather than a take.
keys.active_take_lane.regions.add(12.0, TAKE_DUR, window(7), name="keys")

#: **A track automation**: a row of its own under the track, as long as the
#: timeline, because a track's gain does not begin and end with a box. The
#: target names the **port** it drives on whatever it is on.
noise.automation.add({"port": "gain"}, [(0.0, 1.0), (8.0, 0.2)],
                     name="gain", visible=True)

#: **A clip envelope**: a layer drawn inside its box, lasting exactly as long as
#: the box does. Its time is the box's own, from zero, and its port is the clip's.
first.automation.add({"port": "gain"}, [(0.0, 0.2), (TAKE_DUR, 1.0)],
                     name="fade", visible=True)

# %% [markdown]
# ## The session
# The multitrack and where its sources are: every take as a file, named
# relative to the session, which keeps the folder movable; the join as the
# parts it is made of; the notes in the file itself.

# %%
FRAMES = int(TAKE_DUR * SR)
HALF = FRAMES // 2
FADE = int(0.010 * SR)
ID = {take: id for id, take in SOURCES.items()}

saved = SavedSession(multitrack=multitrack)
for id, take in SOURCES.items():
    if take != "comp":
        saved.sources[id] = Source.file(TAKES[take]).shaped(1, FRAMES, SR)


def part(take: str, start: int, frames: int, **fades) -> dict:
    """One span of a take, as the join names it: the source, the frames it
    contributes, and the fade at its seam."""
    return {"source": {"source": ID[take], "lifetime": "session", "generation": 0,
                       "range": {"start": start, "end": start + frames}},
            **fades}


#: **The join is its recipe, not its samples**: half of the glide and the second
#: half of the saw, with a ten-millisecond crossfade at the seam. Two spans that
#: do not continue each other make a step, and a step is a click however well
#: the frames are read.
saved.sources[ID["comp"]] = Source(location={"at": "segments", "parts": [
    part("glide", 0, HALF, fade_out=FADE),
    part("saw", HALF, HALF, fade_in=FADE),
]}).shaped(1, 2 * HALF, SR)

saved.sources[7] = Source.events(KEYS)

print(f"wrote {saved.save(os.path.join(OUT, 'edit_multitrack.json'))}")

# %% [markdown]
# ## One verb
#
# A saved session opens in the multitrack, the GUI host's application, and
# `edit` answers its handler, a `clausters.gui.editing.MultitrackEditor`.

# %%
session = Session.live()
session.gui()          # the host, which holds the multitrack
editor = edit(saved, title="multitrack")

# %% [markdown]
# ## The script's multitrack is the window's
# A change made here is an edit of the window's: the bass fader moves in the
# window, and Ctrl+Z there takes it back.

# %%
bass.level = 0.5

# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    editor.wait()
    session.close()
else:
    print("open -- edit it in the window; editor.save() writes the session")
