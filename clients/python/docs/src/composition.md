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
  notes it is about to play. Its notes are editable where they are drawn: a note
  dragged in a clip body writes back onto the element's timeline, exactly as one
  dragged in the dedicated roll does. (The same notes drive the standalone,
  editor-grade `clausters.gui.pianoroll` widget — a keyboard, an editable note
  grid, a velocity lane and an OSC-event lane — when you want to author them
  directly rather than through the multitrack.)
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

The multitrack draws an element of events as a clip body — the notes, editable
in place, but at a clip's size. To *author* them — a keyboard to hear the pitch,
a velocity lane, room to work — open the element in the editor-grade view
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

## The document: what the composition *is*, and who edits it

Everything above is this client's own surface. Underneath it there is one
authoritative model — the **document** — and it lives in a Rust crate that every
client binds: this one, the web client, and a GUI host running standalone with
no language attached at all. That is not an implementation detail you can ignore
once you edit from more than one place, so this section says what crosses.

`to_document` writes the arrangement as the document, and `from_document` reads
one back:

```python
from clausters.form import to_document, from_document

doc = to_document(song)          # {"version": 1, "root": {...}}
song_again = from_document(doc)
```

The conversion is lossless for concrete material — events, placements, sets,
buffers by reference — and carries a **generator by reference**, the way a
project file references a plugin rather than serializing it. A generator *is
code*, in the language that wrote it, so no format owns one; what the document
guarantees is that it does not lose it. Node ids are stamped onto the elements,
so converting the same tree twice gives the same ids and an edit made against
one conversion still names the right node in the next.

### An edit is applied in one place

The crate is the only thing that applies an edit. A client does not apply and
then report — it hands over the document and the **intent** and receives the new
document plus what happened:

```python
from clausters import _native

result = _native.document_apply(
    doc,
    {"intent": "place", "node": 3, "offset": 4.3},
    against={"version": doc["version"]},   # the state you were looking at
    quant=1.0,                             # the musical grid, in beats
)
result["outcome"]["effective"]   # {"intent": "place", "node": 3, "offset": 4.0}
result["outcome"]["reason"]      # "snapped to the grid"
```

Two properties are worth knowing because they change how you write against it.

An intent is **absolute**: it states the value the edit *results in*, never an
increment. So applying one twice leaves the same document, and a view that drew
an edit optimistically can leave its picture standing over whatever comes back
instead of recomputing anything.

There is **no success flag to branch on**. `effective` is the edit describing the
document as it now stands, so *applied*, *applied transformed* and *refused* are
one shape — a refusal is simply the previous value handed back. `applied` says
whether anything moved and `stale` says whether the refusal was someone else
having changed the document underneath you, which is a different thing to tell a
person than "not here".

### Undo: the history belongs with the document

The editor's undo is not the editor's. The history lives in the same crate as
the document, beside the data it inverts, and `Editor.undo` / `Editor.redo` step
through it:

```python
editor.apply(*gui.poll())        # a dragged clip
editor.can_undo                  # True
editor.undo_label                # "move the clip"
editor.undo()                    # the clip springs back, and the window is told
editor.redo()                    # and forward again
```

Wire them to two buttons the way the transport is wired, by name:

```python
win["undo"].on_event(lambda v: editor.undo() if v == 1 else None)
```

The keyboard needs no wiring at all: **Ctrl+Z** and **Ctrl+Shift+Z** over the
window reach the same history, because the host sends them as an `"undo"`
addressed to the *window* rather than to a widget — undo is aimed at no place
under the cursor — and `Editor.apply` answers it like any other event.

**Why the history is not kept here** is the whole reason it is worth explaining.
A log an editor keeps sees only the gestures *that editor* made — so a script
that edits the arrangement, a second view on the same piece, or a re-render
leaves it describing a composition that has moved on, and undoing then writes a
state nobody was ever in. One history per document, wherever the edits come
from, is the only version of this that stays true.

Two consequences follow, and both are the point rather than a limitation. The
**grid is applied by the crate**, not by the editor: a drag states where the
hand put it, and what comes back is where it landed — so a redo replays the
*snapped* value and cannot snap a second time. And an **inverse is an ordinary
edit**, so undoing needs no second path: it is the same intent machinery running
backwards, and the window adopts the result exactly as it adopts a snap.

### The selection: what was swept, and what is under it

A sweep on a lane is not an edit — nothing in the composition changes — but it
is the **value** an operation is handed, so the editor keeps it typed:

```python
editor.apply(*gui.poll())        # a marquee swept on a lane
editor.selection                 # {"start": 1.0, "len": 2.0}   (beats)
editor.resolve_selection()       # [{"node": 3, "source": {...}, "range": [...], ...}]
```

Two things are worth knowing about what is in there. The span is in **beats**,
the unit the arrangement is written in, converted from the timeline samples the
window reported — the crate holds whatever unit it is given and converts
nothing, because the tempo is yours. And a sweep with **height** over a view
that measures a value carries that band too, in the element's own domain:

```python
editor.selection    # {"start": 0.0, "len": 2.0, "value": {"min": -0.5, "max": 0.25},
                    #  "nodes": [4]}
```

`nodes` says what the selection is *of*: the element when the sweep was inside
one, and nothing at all when it was across a lane, which is a selection of the
shared time axis. `resolve_selection` turns that into the material underneath —
one entry per leaf, with the placement's base, the element's trim and the clamp
at both ends already applied — and returns nothing where a group or a generator
is in the way rather than under it.

The value band travels with the selection and does not narrow that answer: what
lies under a range of amplitudes is the same material as what lies under the
whole span. Reading *only* those samples is an operation over the range, not a
resolution of it.

### Saving: the document plus where its material is

A document says what plays when and deliberately not where a source lives — in a
running system a source is a server buffer, a mapped file or a rendered result,
and the tree has no business knowing which. A **session** is the document plus
that missing half:

```python
from clausters.form import to_session, from_session

session = to_session(
    song,
    sources={
        7: {"location": {"at": "file", "path": "takes/vocal.wav"},
            "lifetime": "external", "generation": 0},
    },
    provenance={"script": "song.py"},
)
song_again, sources = from_session(session)
```

A source's **lifetime** is what makes saving honest: `external` is the user's own
file, which is never written; `session` is persisted beside the document;
`temporary` is a destructive edit's working copy. Saving in the middle of such an
edit promotes the working copy and **leaves the edit open** — a save is not an
edit, and refusing to save until you decide would block the safest habit in the
program.

`provenance` is a reference to whatever produced something, carried and never
interpreted. It is what makes re-generating possible without the format knowing
how, which is the same rule the opaque generator follows one level down.
