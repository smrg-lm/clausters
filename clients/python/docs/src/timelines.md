# Timelines and the playhead

The sequencing you have seen so far is **generative**: a `Routine` is a Python generator, a `Pbind` an event pattern, and a `TempoClock` resumes them forward in time. That way of working is open-ended and expressive, but it has one thing it fundamentally cannot do — **seek**. A generator's musical state lives in its local variables, so you cannot jump it to beat 100 without running through 0–100, and you cannot ask "what plays at bar 33?" without getting there.

A `Timeline` is the complement: a **static, editable list of timed items, kept sorted by beat, with random access by time**. Because it is a data structure rather than a coroutine, a `Playhead` can give it real **transport controls** — play, stop, locate (seek), loop — and a song position. This is how a DAW works: the arrangement is random-access for editing and seeking, and playback is a forward scan of it from the playhead.

This page is the static counterpart to [Routines and clocks](routines-and-clocks.md); the two ways of sequencing coexist, and you can move between them (capture a pattern into a timeline, below).

## The timeline

A `Timeline` holds `(beat, item)` pairs in beat order. You edit it freely and query it by time.

```python
from clausters.seq import Timeline, Event

tl = Timeline()
a = tl.add(0.0, Event(degree=0))     # add returns a handle...
b = tl.add(1.0, Event(degree=2))
tl.add(2.0, Event(degree=4))

tl.move(a, 4.0)                      # ...you pass back to move or remove
tl.remove(b)
```

`add` keeps the list sorted (a stable insert, so items added at the same beat keep their order — a note-off before a re-trigger). It returns a handle you give back to `remove` and `move`, so edits stay correct as other inserts shift positions.

The **random access by time** is the point:

```python
tl.index_at(1.5)      # the cursor of the first item at or after beat 1.5
tl.range(1.0, 3.0)    # the (beat, item) pairs in the half-open window [1.0, 3.0)
tl.at(2.0)            # the items exactly at beat 2.0
tl.duration()         # the beat of the last item
```

`index_at` is the seek primitive — it is what `play(at=…)` and `locate` use to start the scan at an arbitrary point, which a forward-only routine could never do.

### What an item is

An *item* is anything that can render itself on a destination — it has a `play(destination)` method. `Event` already is one (it plays a note on a `Server` for OSC, or a `MidiServer` for MIDI — the same double dispatch the patterns use), so a timeline of `Event`s renders to OSC *or* MIDI depending only on the destination the playhead holds. For a plain editable OSC or MIDI score, `OscItem` and `MidiItem` wrap a raw message:

```python
from clausters.seq import OscItem, MidiItem

tl.add(0.0, OscItem("/synth_new", "default", -1, 0, 0, "freq", 440.0))
tl.add(1.0, MidiItem(b"\x90\x3c\x64"))     # note on, key 60, vel 100
```

## The playhead

A `Playhead` scans a timeline forward as a clock advances, rendering each item on a destination. It is built from a timeline, the clock that drives it, and the destination the items go to:

```python
from clausters.seq import Playhead

head = Playhead(timeline, session.clock, session.server)
session.start()                 # the clock must be running for live playback
head.play(at=0.0, quant=4)      # start on the next bar, from the top
```

(The free-standing `play(timeline)` builds this for you on the ambient clock
and server and returns the playhead — see
[The ambient verbs](verbs.md).)

The transport controls:

| Call | What it does |
| --- | --- |
| `play(at=0.0, quant=None)` | Start (or restart) from beat `at`, snapping to a `quant` bar. Re-seeks the cursor, so it doubles as locate-and-play. |
| `stop()` | Halt the playhead; no further items are rendered (notes already started keep their scheduled releases). |
| `locate(beat)` | Seek to `beat` — random access. While playing, restarts the scan there; while stopped, sets where the next `play` begins. |
| `loop(start, end)` / `unloop()` | Loop the half-open window `[start, end)`; the scan wraps at `end`. |
| `position()` | The current song position in beats (interpolated from the clock while playing). |
| `playing` / `finished` | Whether the scan is running, and — once it is not — whether it ran off the end rather than being stopped. |

A pass **ends on its own** when the scan reaches the end of the timeline: `playing` goes False, `finished` goes True and `position()` freezes on the last item. That is what a transport polls to park its cursor, rather than timing the end itself. It is the *scan* that ends, so a `loop` never finishes, and the last item keeps sounding for its own length — the playhead schedules items, it does not wait for them.

Under the hood the playhead is a thin cursor over the static structure: the random access happens at the boundaries (`play`, `locate`, loop wrap), and between them it is a forward scan — exactly how a DAW's playback engine reads its arrangement. Because it rides the clock's logical time like everything else in the client, it **inherits the timing models for free**: `quant` starts it on a bar, `clock.lock_to(server)` makes its events sample-exact, and `clock.join_transport(server)` aligns its bars with other clients (see [Timing models](timing-models.md) and [A DAW-style transport](transport.md)).

## Capturing a pattern into a timeline

The two meet here: run a pattern offline and record what it plays into a timeline — "bounce a pattern to a clip" — then edit and seek the result.

```python
from clausters.seq import Timeline, Pbind, Pseq

tl = Timeline.from_pattern(
    Pbind(instrument="default", degree=Pseq([0, 2, 4, 7]), dur=0.5),
    dur=2.0,      # bound an open-ended pattern; None drains a finite one fully
)
tl.add(0.0, Event(instrument="default", degree=7, dur=0.5, amp=0.3))   # then edit
```

## Offline rendering

A playhead is destination-agnostic, so rendering a timeline offline is the same code with an offline session: play it on the NRT clock and render the score.

```python
from clausters import Session

session = Session.nrt(tempo=2.0)
Playhead(timeline, session.clock, session.server).play()
session.clock.render()                       # drain the playhead in logical time
stats = session.server.render()              # the offline render
```

## Following a conductor

A playhead is a local transport, but it can also obey a **shared** one. `head.follow_transport(server)` binds it to the server's transport so that a conductor's `transport_play` / `transport_stop` / `transport_locate` rolls, halts and seeks *this* playhead too — several clients in lockstep. It is built on the responder layer (an `OscFunc` on the transport broadcast) and the shared grid:

```python
head = Playhead(timeline, clock, server)
clock.start()
head.follow_transport(server, quant=4)   # roll when the conductor presses play
```

Beat-aligned in plain wall-clock mode, sample-exact when the clock is also `lock_to` the server. See [A DAW-style transport](transport.md) for the conductor side (`Server.transport_play` and friends) and `conductor.py` in [Examples](examples.md).

## Seeing a timeline as a score

A timeline is timed pitches — the same thing a score draws. `clausters.gui.notation.from_timeline` engraves one as music notation: events sharing a beat become a chord, gaps become rests, and each event's written `dur` becomes its note value. It returns the score as text (MEI), which the `score` widget's engraver reads — the inverse of the usual score→sound direction, so the piece you hear is the piece you see.

```python
from clausters.gui import notation

score = notation.Score.from_timeline(timeline, meter="4/4", key="C")
dl = score.display_list()          # the engraved page, for the score widget
```

**An event can also say what the note is on a page.** Beside `midinote` and
`dur`, an event carries `articulations`, `dynamic`, `ornament`, `grace`, `stem`,
`spelling`, `accidental` and `tie` — the notation keys, reserved so none of them
reaches the synth as a control:

```python
Event(midinote=60, dur=1, articulations=["stacc"], dynamic="mf")
Event(midinote=63, dur=1, spelling="flat", accidental="written")  # an E flat, printed
```

Every one is a **musical fact rather than an instruction to the engraver** —
`articulations=["stacc"]`, never "draw a dot" — which is what lets the same key
be read in both directions: written on the way out, and put back on the event by
`to_timeline` on the way in. An event that carries none of them engraves exactly
what it always did.

An explicit `sustain` becomes how long the note is *held*, but **only where no
symbol already says it**. A note that is both staccato and short is not two
facts: the staccato is the fact, and the short length is what an interpretation
made of it. Written as both, the next reading would shorten an already shortened
note.

`from_notes` is the melodic sibling — a plain list of events (a `rest` for silence), written back to back. A note whose duration is not a single value is written as tied notes (a dotted value when it lands exactly, like `1.5` beats → a dotted quarter), and a note overrunning a barline is split and tied across it. `beat_unit` sets what one beat is worth (`4` = a quarter, matching a `TEMPO` of one beat per second). The result flows into `engrave` (a one-shot view), `Score` (to edit and redraw) or `Score.from_timeline` (both at once); `examples/notation/score_from_data.py` builds a timeline of chords and a melody, engraves it and plays it from the same timeline.

## The score behind the page: editing it as data

An engraved page is a picture of something, and that something is a **sheet** —
the score as plain data, which `clausters.gui.notation` hands you and takes back:

```python
from clausters.gui import notation

sheet = notation.sheet_from_voice([{"midis": [60], "ticks": 8}])  # a quarter, middle C
sheet = notation.transpose(sheet, 4)                     # up a major third: E
mei = notation.to_mei(sheet)                             # ready for engrave/Score
```

A sheet is two structures that do not contain each other. The **grid** is the
metric layout — measures and meter changes — which does not sound and is what
lets you *address* the music: `notation.measures(3, 10)` names measures 3 to 10,
and the span it covers is worked out against the grid, so it stays right across
a meter change or an irregular bar. The **staves** hold the content, flat, so
lengthening a phrase does not break anything it was nested in. Every duration is
an exact fraction of a whole note (`[1, 4]` is a quarter, `[1, 12]` a triplet
eighth), so nothing rounds on the way through.

Pitches carry their **spelling**, which is why transposing sounds right *and*
looks right: a major third up from C is E, not F-flat. `semitones` gives the
chromatic size and `steps` the diatonic one; leave `steps` out and the ordinary
reading of that interval is used.

None of this arithmetic is written in Python. Operations are named here and
carried out in the shared core, which is what keeps every client — and a
standalone host with no client at all — editing a score the same way.
`notation.ops()` lists the verbs the core knows.

Writing a sheet out can be **refused**, with a reason: a duration that is not an
exact note value (a tuplet), an accidental past a double, more than one voice.
Each says which it is, so you can tell a mistake from a feature that has not
landed yet.

### Composing by operating on a score

Every operation is a function from a score to a score, so they compose — and
composing two of them gives the same music as applying them to the composed
score:

```python
motif = notation.sheet_from_voice(
    [{"midis": [p], "ticks": 8} for p in (60, 64, 67, 64)])

piece = motif
for section in (notation.invert(motif),          # turned about its first note
                notation.retrograde(motif),      # backwards
                notation.stretch(motif, (2, 1)), # twice as slow
                notation.transpose(motif, 4)):   # up a major third
    piece = notation.concat(piece, section)
```

`concat` puts one score after another and `stack` puts one against another —
as more voices on the same staves, or (`as_staff=True`) as staves below.
`repeat` plays a stretch several times in a row. On the grid there are
`set_meter`, `insert_measures` and `remove_measures`.

**The two structures move independently, and reading the operations against
that is how they stop being surprising.** `stretch` doubles the written values
and leaves every barline where it was, so the phrase re-bars across them and
ties where a value now overruns one — augmentation, as it looks on a page.
`set_meter` rewrites no note: the same notes simply fall in different measures
afterwards. Only the three that *add or remove time* — `repeat`,
`insert_measures`, `remove_measures` — move both.

### Editing one note

An edit names its item by **id**, never by position, so an id you kept still
names the same note after any number of other edits:

```python
first = piece["staves"][0]["voices"][0]["items"][0]["id"]
piece = notation.set_dur(piece, first, (1, 2))         # a half note
piece = notation.set_pitches(piece, first, [notation.pitch("b", 3, 1)])
```

Beside those: `insert` writes a new note, chord or rest; `tie` ties one into the
next; `to_voice` moves items to another voice on their staff, leaving rests
where they were, which is how two lines written as one come apart.

**`delete` and `silence` are different acts**, and picking the wrong one is how
time goes missing: `delete` takes the item out and everything after it moves
earlier by its value; `silence` leaves a rest of the same length, so nothing
moves and the item keeps its id.

### What the page can say

A sheet holds more than pitches and values, and all of it is written out by
`to_mei`:

```python
sheet = notation.set_marks(sheet, id, notation.marks(
    articulations=["stacc"], dynamic="mf", sounding=(1, 8)))
sheet = notation.add_spanner(sheet, "slur", first_id, last_id)
```

`marks` is what one note carries — articulations, a dynamic, an ornament,
whether it is a grace note and a forced stem. It also holds **how long the note
sounds** as against how long it is written — kept in the score and deliberately
*not* written onto the page, because an engraver reads a written sounding length
as the note's real duration and advances its own clock by it, which pulls every
attack after it earlier. Shortening a staccato is the player's decision, and the
interpreter is what will honour it. A slur or a hairpin has *two* ends, so
it goes on the sheet beside the staves (`add_spanner`) rather than on a note.

**Several voices and several staves** come from `stack`: as voices they are two
layers on one staff, as staves (`as_staff=True`) they take a brace and one
barline through both.

**Tuplets need nothing declared.** A duration whose denominator carries an odd
factor is already inside one — a triplet eighth is `[1, 12]` — so three of them
engrave as a bracketed triplet. A tuplet cannot be split, so a group that would
cross a barline is refused by name rather than written into bars nobody meant.

**Accidentals are printed only where they are needed**: not where the key
signature already implies them, and not twice in a bar. A natural is a *sign*
where the key alters that step, and `notation.pitch(..., forced=True)`-style
courtesy accidentals are what the pitch's own `forced` flag is for.

`examples/notation/compose.py` builds a whole piece this way and plays it.

### Opening a score that was only a document

A page typed as ABC, imported from MusicXML or written by hand is a *document*
and nothing else — none of the verbs above can touch it — until `sheet_from_mei`
reads one:

```python
score = notation.Score(PHRASE)          # ABC, MusicXML, MEI: the engraver reads them
sheet = notation.sheet_from_mei(score.mei())
sheet = notation.transpose(sheet, 2)    # and now every verb applies
```

There is one input format rather than four: the engraver normalizes whatever it
loaded, so you hand this `Score.mei()`.

**What the model holds is what somebody chose.** The header
(`notation.header(title=..., composer=...)` and `set_header`), the barlines
(`set_barline`), the breaks (`set_break`) and the beams (a `"beam"` spanner) are
all read and written, because each is a statement: a beam that crosses a beat
groups the rhythm a particular way, and a break in a published score is a
decision about the page. What the engraver works out when nobody said anything —
the automatic beaming, the line breaks that merely fit — is not read and is not
loss, because it is recomputed identically every time.

Two things it does *not* read back, and both are deliberate: the rests the
emitter invents to complete a bar or to keep a short voice level with its
neighbour (they are not in the model, and reading them would make a score gain a
bar of silence for having been saved), and the ids of a document written
somewhere else (an id means something only inside the model that minted it, so
fresh ones are minted). A score this layer wrote keeps its ids, which is what
lets an item you were editing before a save still be that item after it.

**An open score is edited through the same verbs.** `Score.sheet()` hands back
the model behind the page and `Score.apply(op)` applies one operation as a
single undo step, re-engraving as it goes — so editing a score you have open and
editing a sheet in your hand are the same operation:

```python
score = notation.Score(PHRASE)
score.apply({"op": "transpose", "semitones": 2})   # what `transpose` builds
score.undo()                                        # the model goes back too
```

**And the undo is the editing context's**, not the score's own. A page registers
in `Editing.of(score)` like a curve, a take or a roll, and each edit is recorded
as the MEI it produced with the previous one as its inverse — so a window
holding a page beside a lane has **one** Ctrl+Z, walked in the order the hand
made the edits, whichever of the two the pointer was over. `score.can_undo`
answers for that order and may well be an edit to something else.

Dragging a note on the page is `move_steps`, which is **not** transposition: it
moves along the staff and takes the key signature's alteration for the letter it
lands on, so a note dragged onto a B in E flat is a B flat. `transpose` is the
other act — a named interval, keeping the alteration the arithmetic implies.

A document that could not be read into a model still draws and still plays;
`sheet()` raises there, and that is how you tell.

**A page can also be written on.** `score_view(..., entry=True)` opts into note
entry: a press on blank paper inside a staff reports `"insert" <after>
<position> <staff>` — the element the new note would follow, how far up the
staff the press landed, and which staff. It names a *place*, not a note, since a
staff position is a pitch only once something knows the clef and the key. Hand
the three straight to `insert`:

```python
sheet = notation.insert(sheet, (1, 4), after=after_id, position=position)
```

and the pitch is worked out from that staff's clef and the key — clicking the
middle line in E flat writes a B flat.

**A gesture names an element; a verb names an item**, and `notation.item_id` is
the step between them. The page reports the element under the cursor the way the
emitter spelled it — `n7` is the item, `n7-2` a piece of it split across a
barline, `n7-p1` one pitch of a chord — and all three are item 7, which is what
lets a gesture anywhere on a note reach the note:

```python
item = notation.item_id(element)      # None where the element is not the model's
score.apply({"op": "set_marks", "id": item, "marks": notation.marks(...)})
```

It comes from the core because the answer is the *emitter's*: a client reading
the ids itself would disagree the first time a split was spelled differently.
`examples/notation/score_editor.py` is the whole of this — a document opened,
edited by hand through the model's verbs, and played back from it.

### Hearing what the page says

The way back out is `to_notes`, and it is not a conversion: the symbols mean
something, and honouring them is the whole of the step.

```python
for note in notation.to_notes(sheet):
    ...      # t, dur, sustain, pitch, amp, staff, voice, id -- all in beats

timeline = notation.to_timeline(sheet, instruments={0: "piano", 1: "bass"})
```

Every note comes back with **two lengths**: `dur`, what is written, and
`sustain`, what is heard. They are different numbers whenever an articulation is
honoured, and keeping them apart is the point — a staccato quarter is still a
quarter, so the next attack is where it always was and only the sound is
shorter. `to_timeline` puts the pair straight onto an `Event`'s `dur` and
`sustain`.

What else is read: a **dynamic** governs every note after it until the next one
is written; a **hairpin** is a shape over a stretch of notes rather than a mark
on any of them, and it arrives where the dynamic at its far end says, or travels
a default distance when nothing is written there; a **tie** is one sound of the
summed length; a note's **metric position** stresses it. A tuplet needs no rule
at all — its division is already exact in the fraction the item holds.

**The reading is data, and it is yours.** `notation.interpretation()` hands back
every number it depends on — what a staccato does to a length, what `mf` is in
amplitude, how far a crescendo travels, which positions in the bar are stressed.
Change what you disagree with and pass it to `to_notes`; nothing in the core is
edited to play a score in another style:

```python
style = notation.interpretation()
style["accents"].append({"at": [1, 2], "gain": 1.1, "meter": "4/4"})
style["detach"] = 0.9                  # a player who does not hold notes whole
notes = notation.to_notes(sheet, style)
```

What the defaults claim is deliberately as little as a player can claim and
still be playing: the marks mean roughly what a dictionary says, and the only
metric stress is the **downbeat**. Stressing one and three of a 4/4 belongs to a
style, and a style says so by passing its own accents.

**What plays a staff is not in the notation.** A page does not say what
instrument reads it, so each note names the `staff` and `voice` it was written
on and the binding is made where the score is rendered — `instruments=` above,
a name for every staff or a mapping from staff index.

**The round trip is honest, and both directions lose something.** Going from
events to a score loses exact onsets (they snap to written values), continuous
amplitude (it becomes a dynamic, or nothing), microtones, and which instrument
played. Coming back, a note keeps everything written *on* it — that is what the
notation keys carry — and loses everything that is not one note's: a slur and a
hairpin have two ends, a meter change and a barline belong to the grid, a title
belongs to the document, and none of the three can ride an event. What survives
both ways is the note: its pitch and spelling, its written value, its marks, and
the order they come in.

## See also

- [Routines and clocks](routines-and-clocks.md) — the generative counterpart (the open-ended side you can capture *from*).
- [A DAW-style transport](transport.md) — the shared beat grid clients phase-align on.
- [Timing models](timing-models.md) — the timing references a playhead inherits (`quant`, `lock_to`, `join_transport`).
- [Examples](examples.md) — `timeline.py`, the playhead live.
- [API reference](api.md) — `Timeline`, `Playhead`, `OscItem`, `MidiItem`.
