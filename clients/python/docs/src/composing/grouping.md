# Grouping: placing elements in time

The five primitives adorn things the client already had. The **`Group`** is
the one genuinely new structure of the arrangement: it places elements by an
offset, recursively — a group is itself an element, so a phrase sits inside a
section inside a piece, and the same operations work at every level.

## The song tree

Each lane of the piece is a group placing one element (the drums place the
take twice), and the song is a group placing the lanes:

```python
from clausters.form import Group

drums = Group([(0.0, take), (4.0, take)], name="drums")
bass_lane = Group([(0.0, bass)], name="bass")
lead_lane = Group([(0.0, melody)], name="lead")

song = Group([
    (0.0, drums),
    (0.0, bass_lane),
    (2.0, lead_lane),       # the lead enters two beats in
], name="song")
```

A child is seeded as a `(offset, element)` pair, a bare `Element` (placed at
offset 0), or a `(offset, dur, element)` triple — the third form also fixes a
**placement length**, which you will meet below. Every offset is in beats,
*relative to the group*: the melody sits at 0.0 inside its lane, and the lane
at 2.0 inside the song, so the melody starts at absolute beat 2.0. Nothing in
the tree is absolute until rendering adds the offsets up.

## Placements are edits: the member handle

A group is not frozen. `add` places an element and returns a **handle** — a
stable identity you can `move` or `remove` later, however much else has
changed around it:

```python
member = drums.handles[1]        # the second take (the handles of the seeds)
drums.move(member, 6.0)          # push it two beats later
print(drums.members)             # [(0.0, None, <Buffer>), (6.0, None, <Buffer>)]
drums.move(member, 4.0)          # and back
```

`members` reads the placements as `(offset, dur, element)` triples; `handles`
returns the stable member objects that `move`/`remove` take. This handle is
exactly what the multitrack editor will hold on to per clip — a dragged clip
becomes a `move` on its handle, nothing more.

Note the placement `dur` is `None` above: the take is *its own* length (its
`duration`, 2.0). The two lengths are different things, and the difference is
the next rule.

## A placement's length is what you hear of it

A placement `dur` **trims** what the element plays — the DAW rule: pull in a
clip's edge and you hear less of it, but the element underneath is untouched.
Watch it on the data directly:

```python
from clausters.form import flatten

drums.move(member, 4.0, 1.0)             # same place, but only 1 beat of it
print(flatten(drums))                    # the second event now has dur=1.0
print(take.to_event()["dur"])            # 2.0 — the element is untouched
drums.move(member, 4.0, 2.0)             # restore the full take
```

The trim never rewrites the element: the shortened event is a copy made at
flatten time, and lengthening the placement again restores everything, because
the source was never touched. Events that fall entirely past a placement's end
are dropped the same way — shorten a melody's placement and its last notes
simply do not play.

## The derived temporal relation

From how its members sit in time, a group *derives* its temporal **relation**
— you never set it, you read it:

```python
print(drums.temporal_relation())     # 'mixed' — a gap between the two takes
print(Group([(0.0, 2.0, take), (2.0, 2.0, take)]).temporal_relation())
                                     # 'successive' — they tile contiguously
print(song.temporal_relation())      # 'mixed'
```

`successive` means the members tile — each begins exactly where the previous
ends; `simultaneous` means they all start *and end* together; anything else is
`mixed`. The interesting one is `simultaneous`: members that start and end
together are one thing on the timeline, which is how an envelope gets attached
to the voice it shapes — [the automation page](automation.md) uses it.

(There is a second grouping **kind** besides the default `concrete`: a
`logical` group, whose members relate by processing rather than by time. It
gets [its own page](logical.md).)

## Inspecting the whole piece: `flatten`

Rendering will flatten the tree — accumulate every nested offset into
absolute beats. You can run that walk yourself, any time, as a pure
inspection:

```python
for beat, item in flatten(song):
    print(f"{beat:4}  {item.get('instrument', 'default'):8}"
          f"  midinote={item.get('midinote')}  dur={item.get('dur')}")
```

Every event of the piece, in absolute beats, sorted: the takes at 0 and 4, the
bass — *bounced from its pattern in this very walk*, the change of state
before your eyes — on every beat from 0 to 7, the melody from beat 2. The bass
is the longest lane: its last note starts at beat 7, so the piece *sounds*
until beat 7.8 — a note's sounding time is its `dur` scaled by `legato`
(default 0.8), and a length here is what you *hear*, a rule you will meet
again. `to_timeline(song)` builds the same thing as a `Timeline`, which is
precisely what a playhead plays — and that is the next page.

Next: [Rendering: hearing the arrangement](render.md).
