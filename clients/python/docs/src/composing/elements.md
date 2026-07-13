# Elements: the five primitives

The arrangement's unit is the **element**: any bounded thing that produces a
unit of meaning — a note, a phrase, a take, a whole section — and can be
decomposed or combined. An element is a *thin adornment* over an object you
already use (an event, a timeline, a buffer, a pattern): it adds temporal
metadata and membership in a group, and delegates the actual playing to what
it wraps. It never reimplements it.

One axis organizes everything (the full argument is in
[the composition chapter](../composition.md#elements)): an element is either
**generated** — the rendered thing, data you can access at random, edit, slice
— or a **generator** — the algorithm that renders it, which can only be
evaluated forward. Evaluating a generator into a generated element is the
**change of state**, and it is what rendering does. You will watch it happen
twice on these pages: a pattern bounced into notes to be drawn, and the same
pattern bounced to be played.

## Temporal character

An element carries two optional properties, both in beats and both relative
to its context: an `onset` (where it starts) and a `duration` (how long it
is). Which of the two are present gives it its temporal **character** — probe
the rule directly, it is pure:

```python
from clausters.form import temporal_character

print(temporal_character(0.0, 4.0))    # 'segment'   — both: a placed span
print(temporal_character(0.0, None))   # 'punctual'  — an onset, no length
print(temporal_character(None, 4.0))   # 'relative'  — a length, no place yet
print(temporal_character(None, None))  # 'abstract'  — pure context
```

In practice a standalone element usually has a duration and no onset —
*relative*: it has a length, and its concrete place will come from where a
group puts it, not from the element itself.

## The five kinds

| Element     | Conceptual name | What it is                                     | Wraps                          |
| ----------- | --------------- | ---------------------------------------------- | ------------------------------ |
| `Event`     | *event/clip*    | parameters grouped into one action             | `clausters.seq.Event`          |
| `Sequence`  | *List*          | strict order, no concrete time — only sequence | a list, or a `Pattern`         |
| `Buffer`    | *Buffer*        | a list at constant time (samples)              | `clausters.defs.Buffer`        |
| `Track`     | *Set*           | mixed placement of elements — a DAW track      | `clausters.seq.Timeline`       |
| `Generator` | *Function*      | a *process*: server DSP, or a sequence generator | a def, or a `Pbind`/`Routine` |

The class names are what you type; the conceptual names (*event/clip, List,
Buffer, Set, Function*) say what each kind *is*, and the
[glossary](glossary.md) keeps both. Now build the piece's three melodic
elements.

## The take: a `Buffer`

```python
from clausters.form import Buffer

take = Buffer(buf, duration=2.0, instrument="take")
print(take.temporal_character)     # 'relative' — a length, no place yet
```

The rule from the setup page, now in its proper home: a `Buffer` element is
**data**, so it has no sound of its own — it sounds through the
**instrument** named to play it, a def whose `buf` control takes the buffer
number. Ask it for the event it will emit:

```python
event = take.to_event()
print(event["instrument"], event["buf"], event["dur"], event["legato"])
# take <bufnum> 2.0 1.0
```

`legato` is 1 so the take sounds its whole length — the note default of 0.8
would cut it short, and a sampled take is not a note with a gap. Leave the
`instrument` off and the element is still perfectly good *structure*: the
editor will draw its waveform and it contributes its length to the piece — it
simply emits no event. (Why the client refuses to ship a built-in sampler
instead is a deliberate decision; the composition chapter and the design
records cover it.)

## The melody: a `Track`

A `Track` wraps a `Timeline` — free placement of items by beat, the *Set* of
the five. The lead line, three notes placed by hand:

```python
from clausters.form import Track
from clausters.seq import Timeline
from clausters.seq.event import Event as SeqEvent

melody = Track(Timeline([
    (0.0, SeqEvent(midinote=72, dur=1.0)),
    (1.0, SeqEvent(midinote=76, dur=1.0)),
    (2.0, SeqEvent(midinote=79, dur=2.0)),
]))
```

The events name no instrument, so they will play the server's stock
`default` def.

## The bass: a `Sequence` wrapping a pattern

```python
from clausters.form import Sequence
from clausters.seq import Pbind, Pseq

bass = Sequence(Pbind(midinote=Pseq([48, 48, 55, 53], 2),
                      dur=1.0, amp=0.15))
```

This one is a **generator**: eight beats of bass that exist only as an
algorithm until something evaluates them. The editor will *bounce* it to draw
the notes it is about to play, and rendering will bounce it to play them —
the change of state, both times, from the same element.

## The other two kinds, briefly

You will meet them later in the piece, so just their shape for now. An
`Event` element wraps one `clausters.seq.Event` and defaults its `duration`
from the event's `dur`:

```python
from clausters.form import Event

hit = Event(SeqEvent(midinote=60, dur=1.0))
print(hit.duration, hit.temporal_character)    # 1.0 'relative'
```

A `Generator` wraps server DSP (a def, or its name) or a sequence generator;
its moment comes on [the logical page](logical.md), where generators wired
through buses become a signal graph.

## Hear one element alone

An element renders itself with `render(destination, clock)` — the full
mechanics are two pages away, but nothing stops you from playing one right
now:

```python
playhead = melody.render(server, session.clock)
```

Three notes. What came back is a `Playhead` — a transport; it stopped by
itself at the end of the melody, and you could have stopped it early:

```python
playhead.stop()     # idempotent here: the melody already ended
```

The take would play the same way (`take.render(server, session.clock)`), and
so would the bass — pattern and all. Three elements, one verb.

Next: [Grouping: placing elements in time](grouping.md).
