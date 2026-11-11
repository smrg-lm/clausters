#!/usr/bin/env python3
"""``edit(timeline)``: a timeline's events on a roll, edited note by note.

A `clausters.seq.Timeline` is code -- events, patterns, routines, other
timelines -- and a roll edits data. So ``edit`` renders the timeline first
(`clausters.seq.Timeline.render_events`) and opens the events it produced: a
`clausters.seq.EventSequence`, in the timeline's beats with its tempo map, each
event with an id of its own. The editor edits that sequence in place, and the
timeline is left as it was.

**The window, part by part.** It is an application, and every part of it is
reached from its menu bar or its toolbar:

- the **menu bar** names every verb of the roll with its key: File (the
  conversions into a copy), Edit (the selection, split and join, the
  transpositions, the selections by data, **Tool**, **Channel**, **Curves**
  and the monophonic switch), View (zoom to fit, the snap switch, and the
  two panels below), Transport and Notation (a page's verbs, which a roll
  in MIDI mode refuses with why);
- the **toolbar** holds the transport, then the five **tools** as icons --
  pointer, pencil, eraser, scissors, stretch -- and the snap switch's
  magnet; the tool lit is the one a plain press uses;
- the **roll** itself: the keyboard on the left, the grid of notes, under
  it **one lane** -- the velocities, or a curve -- with the **tabs that
  choose it right over it**, the strip of OSC markers, and the time ruler
  along the bottom;
- the **events panel** beside the roll (View > *Events and inspector*,
  ``editor.events_panel = True``): the selected notes' time, length, pitch,
  velocity and channel, which a value typed changes on all of them at once,
  and the list of every event, whose rows select their notes;
- the **display panel** beside it (View > *Display settings*,
  ``editor.display_panel = True``): how the roll draws what is beyond its
  boxes -- slurs, marks, the pitch line, the notes' curves -- and which
  lanes the tabs offer;
- the **status band** at the bottom: what a verb said, and what is under
  the pointer.

**What a press does is the tool's** (keys **1** to **5**, the toolbar,
Edit > Tool; ``editor.tool``). With the **pointer**, the default, a click
selects the note it is on, a drag moves it or, by an edge, resizes it, a
drag over empty grid sweeps a time range and the notes in it, and a **double
click on empty grid makes a note**. The **pencil** makes a note where it is
pressed, the drag its length; the **eraser** takes away every note it
presses or crosses; the **scissors** cut the note clicked where it is
clicked; the **stretch** scales the selection in time from its first onset.
Whatever the tool, **Ctrl** on empty grid makes a note and on a note erases,
**Alt** or **Shift** with a click adds a note to the selection or takes it
out, **Shift and a vertical drag** is the velocity (drawn as the note's
fill), and **Ctrl+Z** steps back. Edit > Channel edits one MIDI channel, the
others drawn faint and not taken (``editor.channel``).

**The space bar plays and stops** -- the application's playback, on a
transport of the window's own -- and a stop goes back to the position
cursor. An edit while it plays is heard at once: a note moved ahead of the
line sounds where it lands, and a note already sounding ends as it would
have. The pass does not stop at the last note: it rolls on, as a
multitrack's does, until the space bar stops it -- ``editor.end =
"contents"`` stops it where the last note ends, and a beat is an end marker.

**Click the ruler at the bottom** to place the position cursor: the play line
goes there, and the next play starts from it. While it plays, the line is
where the transport is. **Home** puts the cursor at the start and **End** where
the last note ends. A time range swept over empty grid is what the space bar
then plays, from its start to its end, and **L** loops it (with no range, L
loops every note). **The wheel over the keyboard scrolls through the octaves**
-- every MIDI note, and in the hertz window up to 20 kHz -- and **Ctrl**
with it zooms; the roll opens on the notes. In hertz a note is a bar of one
height whatever the zoom, centred on the line of its frequency, and a drag
lands on a round frequency -- finer as you zoom in. On the keys a note moved
in hertz sits on its nearest key, the line inside the box its bend off that
key; dragging it there transposes by semitones and keeps the bend, and a
note past MIDI 127 is a strip at the top edge.

**A second window shows the same notes in hertz** -- the same sequence, the
roll's vertical axis a frequency on a log scale, ruled in round frequencies. A
note is where it is in the first window, an octave is the same height, and a
drag there moves it continuously, writing its ``freq``: the MIDI note it was
written with follows, and the first window redraws it between the keys. An
edit in either window is one history.

**``page()`` opens the same notes on a page** -- the sequence read into a
score, in the score editor (``edit(notes, view="score")``). Reading changes
nothing: the notes keep the beats they were written at, and the page writes
each on the nearest eighth. An edit there -- select a note and press a letter,
drag it along the staff, pick an articulation from the palette -- changes in
the sequence only what it changed on the page: that note, in the rolls too,
and no other. The rolls and the page are one history, so Ctrl+Z in any of the
three takes back the last edit wherever it was made, and a note dragged in a
roll is redrawn on the page.

**``notation()`` opens the same notes as notation** -- a copy of the
sequence written as its page (``to_notation()``), on a roll in its
**notation mode**, the mode a page is edited in. There a note is on a staff and a voice, drawn as long as it
is written with what it sounds inside it, and lands on the eighths and the
written values a drag passes; the velocity is a dynamic, written in the note,
and what each note sounds is derived from the page at once. **Select notes and
press S** for a slur, **.** for a staccato -- the bar inside the note shortens
-- **T** to tie two of one pitch that touch into one note, **P** for the pedal;
**Ctrl+Alt+2** edits a second voice, its notes tinted, and the first is drawn
faint and is not taken. **M** edits monophonically: a note lengthened over the
next shortens it, and a slur drawn then joins its notes into one line. The
Notation menu has the rest -- the staff or the system shown, the polyphony
written in voices or with ties -- and MIDI mode converts nothing back.
**-**, **>**, **^** and **,** write a tenuto, an accent, a marcato and a
breath, and the Notation menu's Dynamic a dynamic: each is drawn as the page
draws it -- the articulations over the note, the breath at its end, the
dynamic under it -- and heard at once, a breath as the note let go early. A
note's tint tells apart its voice, or its staff or part (Color by, in the same
menu).

**The first window paints what is played** (``midi_in=True``). The host opens
a MIDI input of its own, a port named ``clausters-gui``: route a keyboard into
it. While the transport is stopped a key lands on the step cursor, a chord on
one step, and the last key up moves the cursor on; while it plays, a key lands
at the play line and lasts as long as it is held. Each one is an edit of the
sequence like a note a hand drew, so it appears in the other windows and
**Ctrl+Z** takes it back.

**The strip over the ruler is the sequence's OSC markers**, shown and not
written: a marker is the message it sends, and the strip has no way to type
an address, so a Ctrl+click there is refused and says why.

**The lane at the roll's foot is shown one at a time**, chosen by the tabs
right over it: the notes' velocities -- a bar at each note's start, dragged
alone or with the selection, and a drag across the lane a ramp, one Ctrl+Z --
or a curve of the sequence with its value ruler. Edit > Curves adds a
channel's curve or a note's, among those the sequence's MIDI format allows,
and its tab comes with it.

**Curves are drawn and edited like a multitrack's automation, and heard.** The
lane's *level* tab is the sequence's automation -- a curve over the
first channel driving every note's `amp`, a crescendo -- and the first note
carries a curve of its own, a **bend**, drawn in the plane as the pitch it
glides to and running on into its release. **Drag a point** to move it,
**Ctrl+click** to add one or remove the one under the cursor; one gesture is
one edit, Ctrl+Z takes it back, and the space bar plays what is drawn.

**Split, transpose, and pick notes by what they hold.** **E** cuts the
selected notes in two at the position cursor, each half with the note's keys
and curves -- split the first note and its bend goes on in the second half,
from where it had reached. **Up** and **Down** move the selection a semitone
(a step of the key in notation mode), **Ctrl+Up** and **Ctrl+Down** an
octave, and every note when nothing is selected. The Edit menu selects every
note that shares a pitch, a channel, a voice or a staff with the selection.
The eraser, or **Ctrl on a note and a drag on**, erases every note it
crosses, in one entry; **an edge of a selected note** resizes the whole
selection by as much.

**The line over the boxes is the pitch that sounds.** A note a bend moves
draws it, from its attack across the box; notes **joined** (J over a
selection of a line) are one line, the second going on from the first with no
new attack, and the next note attacked starts another. It is a reading of the
sequence, ``notes.pitch_line()``: each run's notes and its pitch over time.
``rework()`` splits and transposes from the script, ``pencil()`` takes the
pencil, and ``drawn()`` slurs the phrase and draws it otherwise, in the
roll's display panel: its slurs as an arc, an arc dashed where the slur is
attacked, a bracket or a band, its marks stacked or in a row, above the
notes or inside them, and the pitch line, the notes' curves and the lanes
offered or not.

**A note keeps what the roll cannot draw.** Every note on the roll carries the
id of its event, so an edit names the note it touched: its instrument, its
amplitude and anything else the author put on it stay, and removing one note
leaves every other note its own.

Run it as a script, or step through the cells::

    pip install -e clients/python

    python clients/python/examples/editors/edit_notes.py

It self-launches the audio server and the GUI host: this one plays.
"""

# %%
import sys

from clausters import Session
from clausters.gui import edit
from clausters.seq import OscItem, Timeline
from clausters.seq.event import Event

# %% [markdown]
# ## A timeline, filled the ordinary way
#
# Beats and events. The `instrument` and `amp` on the last one are what the
# roll cannot draw and editing must not lose -- and so are the marker's
# arguments. A marker is a message the server runs when it plays; this one
# writes a value to a control bus.

# %%
timeline = Timeline([
    (0.0, Event(midinote=60, dur=1.0)),
    (1.0, Event(midinote=64, dur=1.0)),
    (2.0, Event(midinote=67, dur=2.0)),
    (4.0, Event(midinote=72, dur=1.0, instrument="default", amp=0.4)),
    (3.0, OscItem("/bus_set", 100, 0.5)),
])

# %% [markdown]
# ## One verb, spelled out
#
# `edit(timeline)` renders the timeline into the events it plays and opens
# them as a `clausters.gui.editing.NotesEditor`: one roll, each note with its
# id. Here the render is its own line, so the sequence gets its curves before
# it opens: a level curve on the first channel, and a bend on its first note that
# glides up two semitones while the note is held and falls back one in its
# release -- a note's curve runs past its box, which is its on and its off.

# %%
# `activate` is what makes this session the **ambient** one, and the free-standing
# verbs are what need it: `play` below resolves its server from here, the way
# `edit` resolves its host.
session = Session.live().activate()
session.gui()          # the host wired to this session's server
notes = timeline.render_events()   # what the roll edits, in place
level = notes.automation.add({"control": "amp", "channel": 0}, [(0.0, 0.02), (4.0, 0.4)],
                             name="level")
first = notes.events[0]            # the first note, as an object
first.automation.add({"bend": True}, [(0.0, 0.0), (0.8, 2.0), (1.2, 1.0)])
editor = edit(notes, title="notes", midi_in=True)

# %% [markdown]
# ## The same notes, in hertz
#
# A second editor over the very same sequence, whose vertical axis is the
# frequency: the same pitch in another coordinate.

# %%
hertz = edit(notes, y_axis="hz", title="notes in hertz")


# %% [markdown]
# ## The same notes, on a page
#
# A sequence has two presentations, and `view` chooses: the roll, its default,
# or the page it is read into. The sequence is still the structure -- the
# editor's `structure` is `notes` -- and the page a reading of it: `division`
# is the smallest written value a note is snapped to, an eighth here. What an
# edit on the page changes in the sequence is what it changed on the page and
# nothing else, as one entry of the same history the rolls record in.

# %%
def page():
    """The same notes on a page, in the score editor."""
    return edit(notes, view="score", division=8, title="notes on a page")


# %% [markdown]
# ## The same notes, as notation
#
# A roll has two modes, and which one is the sequence's: MIDI, the events as
# MIDI says them, and notation, a view of the page they are written as. A page
# is edited in notation mode and a performance in MIDI mode.
# A copy of the sequence is written as its page -- read on eighths in 4/4,
# every note moved to where it is written -- and a roll opens over it, in
# notation mode since the copy is a page. The copy is a sequence of its own,
# so the windows above keep the performance.

# %%
def notation():
    """A copy of the notes written as notation, on a roll in notation mode."""
    written = editor.to_notation(meter="4/4", division=8)
    return edit(written, title="notes as notation")


# %% [markdown]
# ## A phrase marked as a page marks it
#
# A copy written as notation again, on a roll in notation mode beside its page.
# The first two notes get an accent, the second a breath after it, and the
# first a dynamic: the roll draws each with the engraver's symbol, the page
# writes them at once, and the second note is heard let go before the third.
# Each mark is one entry of the history the roll and the page share.

# %%
#: The phrase's roll, once `phrase` has opened it.
phrased = None


def phrase():
    """Accent the first two notes, a breath after the second, mf on the first."""
    global phrased
    written = editor.to_notation(meter="4/4", division=8)
    roll = edit(written, title="a phrase")
    edit(written, view="score", division=8, title="the phrase on a page")
    first, second = written.events[0], written.events[1]
    roll.select([first, second])
    roll.accent()
    roll.select([second])
    roll.breath()
    roll.select([first])
    roll.dynamic("mf")
    phrased = roll
    return roll


# %% [markdown]
# ## How the roll draws it
#
# The phrase's first three notes slurred, and the roll's display panel open
# (the View menu's Display settings): the slur drawn as a band behind the
# notes, the marks stacked inside them. How the roll draws what is beyond its
# boxes -- the slurs, the marks, the pitch line, the notes' curves, the lanes
# offered as tabs -- is the window's, and no edit.

# %%
def drawn():
    """The phrase slurred, drawn as a band, its marks stacked inside."""
    roll = phrased if phrased is not None else phrase()
    roll.select(roll.sequence.events[:3])
    roll.slur()
    roll.display_panel = True
    roll.set_display(slurs="band", marks="stacked", marks_place="inside")
    return roll


# %% [markdown]
# ## Play what was drawn
#
# The editor plays the sequence it edits, on its own transport -- the space bar
# in the window is the same verb.

# %%
def play():
    """Play the sequence as it now stands, from the position cursor."""
    editor.play()


# %% [markdown]
# ## Cut and move from the script
#
# The window's verbs are the editor's members. The first note is cut at the
# position cursor, half a beat in: both halves keep its keys, and its bend is
# cut where it had reached. Then every note at the first note's pitch -- the
# two halves -- is selected, and goes up an octave. One entry each, so Ctrl+Z
# takes them back one at a time.

# %%
def rework():
    """Split the first note half a beat in, and move its pitch up an octave."""
    editor.locate(0.5)
    editor.select([first])
    editor.split()
    editor.select([first])
    editor.select_pitch()
    print(f"{len(editor.selected)} notes at the first one's pitch")
    editor.octave_up()


# %% [markdown]
# ## A tool taken from the script
#
# What a plain press does is the window's tool: the toolbar's icons and the
# keys 1 to 5 choose it, and so does the editor's `tool`. With the pencil a
# press on empty grid makes a note there, the drag its length; with the
# pointer it takes a double click.

# %%
def pencil():
    """Take the pencil: a press on empty grid makes a note."""
    editor.tool = "draw"
    print(f"the tool is {editor.tool!r}: press on empty grid to make a note")


# %% [markdown]
# ## One line, played without a new attack
#
# `join` over the first two notes makes the second go on from the first: the
# line drawn over the boxes runs from the first note's attack, through its
# bend, into the second, and the third is attacked and starts its own. The
# sequence says the same thing as data -- `pitch_line` reads the pitch each
# run sounds, a point where it changes. The join comes back from the window
# as an edit does, so the line is read in a step of its own.

# %%
def legato():
    """Join the first two notes into one line."""
    editor.select(notes.events[:2])
    editor.join()


def line():
    """The pitch each run sounds, from its attack."""
    for run in notes.pitch_line():
        beats = [event.at for event in run["notes"]]
        print(f"  notes at {beats}: {len(run['points'])} points, "
              f"from {run['points'][0][1]:.2f} to {run['points'][-1][1]:.2f}")


# %% [markdown]
# ## Curves added from the script
#
# A pressure of the first note's own, and an expression curve on its channel:
# each one entry, with one point at the parameter's resting value for a hand to
# draw on. The expression is a lane of its own -- its tab over the lane shows
# it. An edit leaves nothing selected, so the channel's curve, asked for second,
# is the first channel's, which is the note's.

# %%
def curves():
    """A pressure on the first note, and an expression curve on its channel."""
    editor.select([first])
    editor.add_note_curve("pressure")
    editor.add_curve("cc11")


# %% [markdown]
# ## A curve given to its notes
#
# The level curve is the channel's: each note hears it from its on to its off.
# `automation.to_events` gives it to the notes it reaches -- each takes the
# stretch its span covers as a curve of its own, drawn inside it, and the row
# goes -- and they sound as they did. `automation.from_events` gathers the
# notes' curves back into one of the sequence, which holds where no two notes
# that sound together differ: here none overlap. Both are edits of the
# sequence; `adopt` brings the rolls in step with it.

# %%
def own_the_level():
    """Give the level curve to the notes it reaches."""
    notes.automation.to_events(level)
    editor.adopt()
    hertz.adopt()


def gather_the_level():
    """Gather the notes' level curves back into the sequence's."""
    global level
    level = notes.automation.from_events({"control": "amp"})
    editor.adopt()
    hertz.adopt()


# %% [markdown]
# ## What a roll cannot say
#
# Five numbers per note -- start, length, pitch, velocity, channel -- and the
# event it is. The instrument, the amp and anything else the author wrote are
# none of them, and they are still there after an edit: each `SeqEvent` reads
# what the sequence holds now, whatever the hand did to it.

# %%
def read_back():
    """Every event as it now stands, with what the roll never drew."""
    for event in notes.events:
        item = event.event          # its keys, as a free Event
        if item.get("type") == "osc":
            print(f"  {event.at:5.2f}  osc {item['addr']}   {list(item['args'])}")
            continue
        extra = {k: v for k, v in dict(item).items()
                 if k in ("instrument", "amp", "velocity")}
        print(f"  {event.at:5.2f}  midinote {item.midinote():5.1f}"
              f"  freq {item.freq():7.1f}   {extra}")


# %%
def run():
    """Keep the window open until it is closed, then print what was drawn."""
    print("edit the notes; the space bar plays them. Close when done.")
    editor.wait()
    print("the sequence, as it was left:")
    read_back()


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    run()
else:
    print("up -- play() to hear it, page() for the notes on a page, "
          "notation() for them as notation, rework() to split and transpose, "
          "pencil() to take the pencil, "
          "legato() to join two notes and line() to read the pitch they sound, "
          "phrase() to mark a phrase as a page does, drawn() to slur it and draw it otherwise, "
          "curves() to add two curves, "
          "read_back() to see what survived the edit")
