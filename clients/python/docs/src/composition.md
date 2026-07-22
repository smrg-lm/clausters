# Composition: the arrangement and the multitrack editor

A `Timeline` places items at beats and a `Playhead` plays them. That is enough to
sequence, but not enough to *compose*: a composition is not a flat list of events,
it is an element inside an element — a phrase inside a section inside a piece, a
take placed against a melody, a generator that has not been evaluated yet.

`clausters.form` is that layer — the **arrangement model** — and
`clausters.gui.Editor` puts it on screen as a multitrack view you can edit. The
point of the pair is that the graphic is not a picture of the music: dragging a
clip moves the *element*, and the score follows.

## Elements

An **element** is any bounded thing that produces a unit of meaning and can be
decomposed or combined — and it comes in two modes, which is the axis the whole
layer turns on. An element is either **generated** (the rendered thing: samples in
a buffer, a bounced timeline of events — data you can edit directly) or a
**generator** (the algorithm that renders it: a def, a pattern, a routine).
Evaluating a generator produces a generated element; that is the *change of
state*, and it is what rendering does.

The difference is not merely data versus process — it is what you can *do* with
each. A generated element is **random-access**: an audio file can be read
backwards, sliced, scrubbed, edited in place. A generator is **forward-only**: it
can be evaluated, in order, and that is all. So the change of state is a
compositional act, not an optimization — it is what turns something you can only
*produce* into something you can *manipulate*, which is why a pattern is bounced to
be drawn and edited on a lane. An element carries two optional temporal
properties — an `onset` (where it starts, in beats, relative to its context) and a
`duration` — and delegates the actual playing to the object it wraps. The
arrangement is a thin adornment over what the client already has, not a second
implementation of it.

Which of the two properties are present gives an element its temporal
*character*: both is a **segment**, an onset alone is **punctual**, a duration
alone is **relative** (it has a length but no place yet), neither is **abstract**
— pure context, which only a parent gives concrete time.

There are five kinds, and they map one to one onto objects you already use:

| Element     | What it is                                       | Wraps                                   |
| ----------- | ------------------------------------------------ | --------------------------------------- |
| `Event`     | parameters grouped into one action               | `clausters.seq.Event`                   |
| `Sequence`  | strict order, no concrete time — only sequence   | a list, or a `Pattern`                  |
| `Buffer`    | a list at constant time (samples)                | `clausters.defs.Buffer`                 |
| `Track`     | mixed placement of elements — a DAW track        | `clausters.seq.Timeline`                |
| `Generator` | a *process*: server DSP, or a sequence generator | a def, or a `Pbind`/`Routine`           |

A `Buffer` is *data*, so it has no sound of its own: it sounds through the
**instrument** named to play it — a def whose `buf` control takes the buffer
number. That is the whole rule for an audio clip.

```python
from clausters.form import Buffer, Group, Sequence, Track

take = Buffer(buf, duration=2.0, instrument="take")   # a def that plays a buffer
```

## Grouping: the one new structure

A `Group` places elements by an offset, recursively — and that recursion is the
whole idea. It comes in two kinds. A **concrete** group is a relation *in time*
between its members (a section holding clips, a melody holding notes). A
**logical** group is a relation of *processing*: the members are wired to each
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

## Rendering: the change of state

Rendering a composition **flattens** it — a tree-walk accumulating the nested
offsets into absolute beats — into a flat `Timeline`, which a `Playhead` then
plays. A generator contained in it is *bounced* in the same pass: that evaluation,
the change from a process into a generated element, is the *change of state*.

```python
song.render(server, clock)        # live, through a playhead
song.render(nrt.server, nrt.clock)  # offline: the same tree, a score
```

There is no second rendering path: RT and NRT are the same flattening, differing
only in the destination, so the offline render is sample-identical to what you
heard.

The free-standing `clausters.render` verb carries the same seam: with a
`destination` it delegates here; without one it **bounces** the element in an
ephemeral offline session and returns the samples (`render(song, path="song.wav")`
writes them out). Note the division of verbs it implies: an element is
*rendered*, never played — `play` is for what already sounds directly — while a
flat `Timeline`, being already generated, is playable
(`play(timeline)` drives it through a playhead on the ambient clock).

## The multitrack editor

`Editor` draws that tree as the multitrack view and applies its edits back onto
the tree. The mapping is one rule, not a heuristic per case:

- the root group's members are the **lanes**;
- a lane's members are its **clips**;
- a `Buffer` clip names its server buffer and spans its frames — the host fetches
  the take and decimates it to the clip's pixel width, so a long take costs
  nothing on the wire;
- an element of *events* draws a **piano-roll** — each note placed in pitch and
  time, shaded by its velocity (an explicit `velocity`, else the event's `amp`) —
  and since a contained pattern is bounced to draw it, a generator lane shows the
  notes it is about to play. (The same notes drive the standalone, editor-grade
  `clausters.gui.pianoroll` widget — a keyboard, an editable note grid, a velocity
  lane and an OSC-event lane — when you want to author them directly rather than
  through the multitrack.)
- a nested group draws as the labeled rectangle that **summarizes** it, until you
  `expand` it into lanes of its own. That collapse/expand is the arrangement's
  *base level*: the same structure, seen coarser or finer.

```python
from clausters.gui import Editor

editor = Editor(song, sample_rate=SR, tempo=2.0, quant=0.5, follow=True)
editor.open(gui)                    # the arrangement, as a multitrack window
editor.render(server, clock)        # play it; the playhead sweeps the clips

while editor.window is not None:
    editor.poll(0.05)               # a dragged clip moves the element
```

`poll` drains the host's events into the arrangement — drag a clip to move it, an
edge to resize it — and with `follow=True` the composition is re-scheduled from the
playhead, so you hear it where you dropped it. The semantics there are honest:
*re-schedule from here*, not a sample-exact splice, so a synth already sounding
keeps sounding.

### The dedicated piano-roll

The multitrack draws an element of events as a clip body — the notes, but at a
clip's size. To *author* the notes, open the element in the editor-grade view
instead:

```python
roll = Editor(melody, sample_rate=SR, tempo=2.0, quant=0.25)
roll.open_pianoroll(gui)      # keyboard, note grid, velocity + OSC-event lanes
```

Edits flow back through `poll` exactly as the multitrack's do, **when the
element is editable**: a dragged, added or removed note rebuilds a `Track`'s
timeline (times converted to beats, any OSC/MIDI events on the same timeline
preserved). A generator — a `Pbind`, a `Routine` — is forward-only, so its
bounced notes are shown *read-only*; bounce it to a `Track` (the change of state)
and the same view becomes an editor. OSC events are shown in their lane but not
written back: their marker carries a time and a label, not the full message.

Quantization exists on both surfaces, because the GUI also runs standalone:
`q` over the roll snaps the selected notes' onsets (or all of them) to the
widget's snap grid, flowing back like any other edit; on the data side,
`Timeline.quantize(grid)` snaps every placement to the beat grid directly.

### Beats and samples

The arrangement places elements in **beats**; the view places clips in **timeline
samples**, because a clip's body is audio data and its sample 0 sits at the clip's
offset. The editor is the only converter between the two: one beat is
`sample_rate / tempo` timeline units, so a take placed at its own frame count sits
1:1 on the axis. Give it the engine's rate and the clock's tempo and the bridge is
closed.

The `quant` you pass is a *musical* grid (`0.5` = half a beat). It becomes the
lane's drag grid, so the grid a clip is dropped on is the grid the arrangement
re-schedules on — what you see is what plays.

### Automation, and the logical side

An `Automation` placed in the composition draws its **curve** as the clip's body —
the same break-points the envelope editor draws — and it is edited in place:
drag a point, Ctrl+click to add one or remove the one under the cursor. The edit
lands on the automation's `Env`, which is what the next render plays, so the
curve you draw is the curve you hear.

A **logical** group is not a timeline at all: its members relate by processing, so
it draws as a `patch` **patcher** — a box per member, with **inlets on top and
outlets on the bottom**, and a **cord** per `outlet -> inlet` connection. The buses
are not drawn: a cord *is* a bus. Direction is not a guess — it is read from the
def (a control feeding an `In` is an inlet, one feeding an `Out` an outlet), so the
picture reads as signal flow. Dragging an outlet onto an inlet draws a cord (a rate
mismatch is refused; onto empty space unwires it), and the edit rewrites the
group — so the next render sends a GraphDef wired the way the patch is drawn.

The same patcher draws a def **on its own**, as a way to *look at its structure*.
`some_def.plot_def()` opens one window per call showing the def as a directed
patch — distinct from `plot(some_def)`, which shows the def's *sound* (its rendered
waveform). It reads at **two levels**: a `GraphDef` draws as its member nodes wired
by buses (the same picture the logical group shows); a `SynthDef` or `FaustDef`
draws one level deeper, as its **internal graph** — every UGen (or Faust signal op)
a box, every input a cord, the def's controls the source boxes and its literals
small value boxes. A cord is coloured by rate — contrasting primaries at one
width, audio red, control blue, and level 2's third, **init** (`ir`, yellow and
dashed), a scalar read once at init time. The host lays the boxes out on its own (a layered, signal-flows-downward
graph drawing). The Def-view is **read-only** — the faithful picture of what the
def is; it needs no audio server.

`clients/python/examples/gui_composer.py` is the whole loop in one script: a take
bounced offline and loaded from disk, a melody, a pattern, all three composed,
edited on screen and heard. And to *work through* everything this chapter
argues — interactively, one block at a time, building that same piece — see
[Composing a piece, step by step](composing.md).
