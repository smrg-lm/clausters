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

An *item* is anything that can render itself on a destination — it has a `play(destination)` method. `Event` already is one (it plays a note on a `Server` for OSC, or a `MidiServer` for MIDI — the same double dispatch the patterns use), so a timeline of `Event`s renders to OSC *or* MIDI depending only on the destination the playhead holds. For a plain editable OSC or MIDI score, `OscEvent` and `MidiEvent` wrap a raw message:

```python
from clausters.seq import OscEvent, MidiEvent

tl.add(0.0, OscEvent("/synth_new", "default", -1, 0, 0, "freq", 440.0))
tl.add(1.0, MidiEvent(b"\x90\x3c\x64"))     # note on, key 60, vel 100
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
score, which is what makes an algebra worth the name:

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

`examples/notation/compose.py` builds a whole piece this way and plays it.

## See also

- [Routines and clocks](routines-and-clocks.md) — the generative counterpart (the open-ended side you can capture *from*).
- [A DAW-style transport](transport.md) — the shared beat grid clients phase-align on.
- [Timing models](timing-models.md) — the timing references a playhead inherits (`quant`, `lock_to`, `join_transport`).
- [Examples](examples.md) — `timeline.py`, the playhead live.
- [API reference](api.md) — `Timeline`, `Playhead`, `OscEvent`, `MidiEvent`.
