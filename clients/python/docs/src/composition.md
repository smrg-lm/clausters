# Composition: the model and the multitrack editor

A `Timeline` places items at beats and a `Playhead` plays them. That is enough to
sequence, but not enough to *compose*: a composition is not a flat list of events,
it is material inside material — a phrase inside a section inside a piece, a take
placed against a melody, a generator that has not been evaluated yet.

`clausters.model` is that layer, and `clausters.gui.Editor` puts it on screen as
a multitrack view you can edit. The point of the pair is that the graphic is not a
picture of the music: dragging a clip moves the *material*, and the score follows.

## Materials

A **material** is any bounded thing that produces a unit of meaning and can be
decomposed or combined. It carries two optional temporal properties — an `onset`
(where it starts, in beats, relative to its context) and a `duration` — and
delegates the actual playing to the object it wraps. The model is a thin
adornment over what the client already has, not a second implementation of it.

Which of the two properties are present gives a material its temporal
*character*: both is a **segment**, an onset alone is **punctual**, a duration
alone is **relative** (it has a length but no place yet), neither is **abstract**
— pure context, which only a parent gives concrete time.

There are five kinds, and they map one to one onto objects you already use:

| Material    | What it is                                       | Wraps                                   |
| ----------- | ------------------------------------------------ | --------------------------------------- |
| `Event`     | parameters grouped into one action               | `clausters.seq.Event`                   |
| `Sequence`  | strict order, no concrete time — only sequence   | a list, or a `Pattern`                  |
| `Buffer`    | a list at constant time (samples)                | `clausters.defs.Buffer`                 |
| `Track`     | mixed placement of materials — a DAW track       | `clausters.seq.Timeline`                |
| `Generator` | a *process*: server DSP, or a sequence generator | a def, or a `Pbind`/`Routine`           |

A `Buffer` is *data*, so it has no sound of its own: it sounds through the
**instrument** named to play it — a def whose `buf` control takes the buffer
number. That is the whole rule for an audio clip.

```python
from clausters.model import Buffer, Group, Sequence, Track

take = Buffer(buf, duration=2.0, instrument="take")   # a def that plays a buffer
```

## Grouping: the one new structure

A `Group` places materials by an offset, recursively — and that recursion is the
whole idea. It comes in two kinds. A **compositional** group is a structural
relation between its contents (a section holding clips, a melody holding notes).
A **logical** group is a *processing* relation: the members are wired to each
other through buses, which is exactly what a `GraphDef` expresses, so
`Group.to_graphdef()` translates one into it.

```python
song = Group([
    (0.0, Group([(0.0, take), (4.0, take)], name="drums")),
    (0.0, Group([(0.0, bass)], name="bass")),
    (2.0, Group([(0.0, melody)], name="lead")),
], name="song")
```

From how its members sit in time, a group *derives* its temporal **relation**:
`successive` when they tile contiguously, `simultaneous` when they start and end
together, `mixed` otherwise. You do not set it; it is read from the placements.

## Realization: the change of state

Realizing a composition **flattens** it — a tree-walk accumulating the nested
offsets into absolute beats — into a flat `Timeline`, which a `Playhead` then
plays. A generator contained in it is *bounced* in the same pass: that evaluation,
the change from a process into concrete material, is the model's *change of
state*.

```python
song.realize(server, clock)        # live, through a playhead
song.realize(nrt.server, nrt.clock)  # offline: the same tree, a score
```

There is no new realization path: RT and NRT are the same flattening, differing
only in the destination, so the offline render is sample-identical to what you
heard.

## The multitrack editor

`Editor` renders that tree into the multitrack view and applies its edits back
onto the model. The mapping is one rule, not a heuristic per case:

- the root group's members are the **lanes**;
- a lane's members are its **clips**;
- a `Buffer` clip names its server buffer and spans its frames — the host fetches
  the take and decimates it to the clip's pixel width, so a long take costs
  nothing on the wire;
- a material of *events* draws a **piano-roll** — and since a contained pattern is
  bounced to draw it, a generator lane shows the notes it is about to play;
- a nested group draws as the labeled rectangle that **summarizes** it, until you
  `expand` it into lanes of its own. That collapse/expand is the model's *base
  level*: the same structure, seen coarser or finer.

```python
from clausters.gui import Editor

editor = Editor(song, sample_rate=SR, tempo=2.0, quant=0.5, follow=True)
editor.open(gui)                    # the model, as a multitrack window
editor.realize(server, clock)       # play it; the playhead sweeps the clips

while editor.window is not None:
    editor.poll(0.05)               # a dragged clip moves the material
```

`poll` drains the host's events into the model — drag a clip to move it, an edge
to resize it — and with `follow=True` the composition is re-scheduled from the
playhead, so you hear it where you dropped it. The semantics there are honest:
*re-schedule from here*, not a sample-exact splice, so a synth already sounding
keeps sounding.

### Beats and samples

The model places materials in **beats**; the view places clips in **timeline
samples**, because a clip's body is audio data and its sample 0 sits at the clip's
offset. The editor is the only converter between the two: one beat is
`sample_rate / tempo` timeline units, so a take placed at its own frame count sits
1:1 on the axis. Give it the engine's rate and the clock's tempo and the bridge is
closed.

The `quant` you pass is a *musical* grid (`0.5` = half a beat). It becomes the
lane's drag grid, so the grid a clip is dropped on is the grid the model
re-schedules on — what you see is what plays.

### Automation, and the logical side

An `Automation` placed in the composition draws its **curve** as the clip's body —
the same break-point model the envelope editor draws — and it is edited in place:
drag a point, Ctrl+click to add one or remove the one under the cursor. The edit
lands on the automation's `Env`, which is what the next realization plays, so the
curve you draw is the curve you hear.

A **logical** group is not a timeline at all: its members relate by processing, so
it draws as a `graph` **patch** — a box per member, a node per bus, and a wire per
connection. The patch is deliberately undirected: a GraphDef knows that a control
*touches* a bus, and which end writes is the server's own analysis, so the view
shows the connection and leaves the direction to the engine. Dragging a port onto
a bus rewires that control (onto empty space, unwires it), and the edit rewrites
the group — so the next realization sends a GraphDef wired the way the patch is
drawn.

`clients/python/examples/gui_composer.py` is the whole loop in one script: a take
bounced offline and loaded from disk, a melody, a pattern, all three composed,
edited on screen and heard.
