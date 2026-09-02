# Composition: the arrangement and the multitrack editor

A `Timeline` places items at beats and a `Playhead` plays them. That is enough to
sequence, but not enough to *compose*: a composition is not a flat list of events,
it is an element inside an element — a phrase inside a section inside a piece, a
take placed against a melody, a generator that has not been evaluated yet.

`clausters.form` is that layer — the **arrangement model** — and
`clausters.gui.FormEditor` puts it on screen as a multitrack view you can edit. The
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
`duration` — and delegates the actual playing to the object it wraps.

**The two are not in the same unit, and each takes its own from what it
answers to.** An onset is in **beats**, always: placing something is a musical
decision, and it takes the unit of what contains it. A duration is in the unit
of the element's own data — **seconds** for a `Vector`, a `Segments` or a curve,
because a recording's length is `frames / sample_rate` and no tempo change makes
it shorter; **beats** for a `Clang`, a `Sequence` or a `Track`, because a note
*is* musical and a tempo change is supposed to shorten it. `Element.duration_unit`
says which, derived from what the element holds rather than stored beside it.
The conversion happens where the tree is flattened for playback (`render`, which
reads the clock's tempo) and never in the tree, since a timeline is ordered by
one number and cannot hold two bases. The
arrangement is a thin adornment over what the client already has, not a second
implementation of it.

Which of the two properties are present gives an element its temporal
*character*: both is a **segment**, an onset alone is **punctual**, a duration
alone is **relative** (it has a length but no place yet), neither is **abstract**
— pure context, which only a parent gives concrete time.

There are five kinds, and they map one to one onto objects you already use
(`Segments` is not a sixth: it is the `Vector` primitive — a list at constant
time — assembled from more than one window):

| Element     | What it is                                       | Wraps                                   |
| ----------- | ------------------------------------------------ | --------------------------------------- |
| `Clang`     | parameters grouped into one action               | `clausters.seq.Event`                   |
| `Sequence`  | strict order, no concrete time — only sequence   | a list, or a `Pattern`                  |
| `Vector`    | a list at constant time (samples)                | `clausters.defs.Buffer`                 |
| `Segments`  | several windows onto samples, read as one       | a list of `(buffer, start, duration)`   |
| `Track`     | mixed placement of elements — a DAW track        | `clausters.seq.Timeline`                |
| `Generator` | a *process*: server DSP, or a sequence generator | a def, or a `Pbind`/`Routine`           |

A `Vector` is *data*, so it has no sound of its own: it sounds through the
**instrument** named to play it — a def whose `buf` control takes the buffer
number. That is the whole rule for an audio clip. A `Segments` is the same rule
over several of them: it is what assembling samples out of pieces looks like
when nothing is copied (see the editor's join, below).

```python
from clausters.form import Aggregate, Sequence, Track, Vector

take = Vector(buf, duration=2.0, instrument="take")   # two seconds, a def plays it
```

## Grouping: the one new structure

An `Aggregate` places elements by an offset, recursively — and that recursion is
the whole idea. It comes in two kinds. A **concrete** aggregate is a relation *in
time* between its members (a section holding clips, a melody holding notes). A
**logical** aggregate is a relation of *processing*: the members are wired to
each other through buses, which is exactly what a `GraphDef` expresses, so
`Aggregate.to_graphdef()` translates one into it.

```python
song = Aggregate([
    (0.0, Aggregate([(0.0, take), (4.0, take)], name="drums")),
    (0.0, Aggregate([(0.0, bass)], name="bass")),
    (2.0, Aggregate([(0.0, melody)], name="lead")),
], name="song")
```

The `take` above is placed **twice**, which is the ordinary thing to write and
means what it says: two clips, one take. A placement is a **window onto
samples** — editing the samples through either window edits the one take, and
moving one clip moves that clip. What can be placed twice is samples the
element only *names*: a `Vector` over a server buffer, a `Generator` over a
pattern or a def. An element that carries its samples *inside* it — a `Clang`,
a `Track`, an `Aggregate` — is refused, because two placements of one of those
would be two copies that diverge the moment you edit one; write two of them, or
one element the two clips share.

From how its members sit in time, an aggregate *derives* its temporal
**relation**: `successive` when they tile contiguously, `simultaneous` when they
start and end together, `mixed` otherwise. You do not set it; it is read from
the placements.

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

## Two editors, and which one is which

`clausters.gui.Editor` edits **one structure** — a buffer's samples, a
break-point curve, a timeline of events — and it knows nothing about the
arrangement. `clausters.gui.FormEditor` is that class plus what only a tree has:
a held document, several views of one composition, the lanes and clips, and a
transport. The names say which is which, and the general one has the plain name
because editing a curve is the plain case.

An editor orchestrates rather than performs, and it is four collaborators
(`clausters.gui.editing`):

| | what it is | what it deliberately is not |
|---|---|---|
| `View` | the picture of one structure, and the registry from widget id to what it shows | not the vocabulary: one structure is drawn several ways |
| `Domain` | gesture → payload, payload → the client object, the label, the coalesce key | not **how an edit inverts** — that is the shared crate's, so it is not written once per language — and it does not draw |
| `Echo` | the acknowledgement: the stamp, the version, the corrections, the reason | not anything about what was edited |
| `Editing` | the editing context: the history, and the views to tell | **not the editor's** — it is asked for, never built, which is what makes two windows walk one undo order |

The rule that fixes all four: an editor owns **neither the data nor the
history**.

## `edit(x)`: one verb over the three structures

`clausters.gui.edit` opens whichever editor the structure asks for, and it
dispatches on **what the structure is** — that being the question a caller has
already answered by holding one:

| `edit(x)` where x is | opens | over | its vocabulary |
|---|---|---|---|
| a `clausters.defs.Buffer` | `SamplesEditor` | a `waveform` | `samples` |
| a `clausters.seq.Automation` | `PointsEditor` | a `bpf` | `points` |
| a `clausters.seq.Timeline` | `NotesEditor` | a `pianoroll` | `events` |

```python
from clausters.gui import edit

editor = edit(curve, sample_rate=48_000.0)
editor.open(gui)
while editor.window is not None:
    editor.poll(0.05)

curve.to_points()      # the edited curve, out of the object you already held
```

Nothing is handed back: the object passed in *is* the edited one. A composition
is not one of the three — an arrangement is `FormEditor`'s, which knows a tree
from a leaf and holds a document.

**Two calls over one structure give two windows and one stack.** The editing
context belongs to the data, so an undo in either window steps the one order
both of them made. And a window composing several structures passes one context
(`edit(x, context=…)`), which is what makes it undo across a curve and a roll in
the order the edits happened.

**How an edit inverts is the shared crate's.** For a curve and a timeline the
state goes in with the payload and comes back as what the structure now is *plus*
what puts it back — one call, because the inverse has to be read before the edit
lands. A span of samples is the exception, and a real one rather than an
omission: the frames are in a server buffer, so the crate holds no state to
invert. What it shares there is the payload's shape and its coalesce key, and
the inverse rides on the wire — a stroke's event carries the run it wrote *and*
the run it replaced.

## The multitrack editor

`FormEditor` draws that tree as the multitrack view and applies its edits back onto
the tree. The mapping is one rule, not a heuristic per case:

- the root aggregate's members are the **lanes**;
- a lane's members are its **clips**;
- a `Vector` clip names its server buffer and shows a **window** onto it — the
  host fetches the take and decimates it to the clip's pixel width, so a long
  take costs nothing on the wire, and trimming the clip shows less of the
  samples rather than squeezing it (see below);
- an element of *events* draws a **piano-roll** — each note placed in pitch and
  time, shaded by its velocity (an explicit `velocity`, else the event's `amp`) —
  and since a contained pattern is bounced to draw it, a generator lane shows the
  notes it is about to play. Its notes are editable where they are drawn: a note
  dragged in a clip body writes back onto the element's timeline, exactly as one
  dragged in the dedicated roll does. (The same notes drive the standalone,
  editor-grade `clausters.gui.pianoroll` widget — a keyboard, an editable note
  grid, a velocity lane and an OSC lane — when you want to author them
  directly rather than through the multitrack.)
- a nested aggregate draws as the labeled rectangle that **summarizes** it,
  until you `expand` it into lanes of its own. That collapse/expand is the arrangement's
  *base level*: the same structure, seen coarser or finer.

```python
from clausters.gui import FormEditor

editor = FormEditor(song, sample_rate=SR, tempo=2.0, quant=0.5, follow=True)
editor.open()                       # the arrangement, as a multitrack window
editor.render(server, clock)        # play it; the playhead sweeps the clips

while editor.window is not None:
    editor.poll(0.05)               # a dragged clip moves the element
```

`poll` drains the host's events into the arrangement — drag a clip to move it, an
edge to trim it — and with `follow=True` the composition is re-scheduled from the
playhead, so you hear it where you dropped it. The semantics there are honest:
*re-schedule from here*, not a sample-exact splice, so a synth already sounding
keeps sounding.

### A clip is a window onto its samples

A clip over a `Vector` shows a **segment** of it, not the whole of it squeezed
into a rectangle. One timeline sample is one frame of the samples, so:

- **trimming** a clip — dragging its edge — hides frames rather than compressing
  them, and the ones it hides are still there: stretch the edge back and they
  come out again;
- the **head** trim moves the window with the edge, so the samples stand still
  while the clip shows less of it;
- an edge stops where the samples end, unless the element **loops** — where
  past the last frame the buffer begins again and before the first comes its own
  tail.

The window is the element's, and you can state it yourself:

```python
from clausters.form import Vector

take = Vector(buf, duration=2.0, instrument="take",
              start=48_000,        # read from one second in
              loop=True)           # and wrap when the window runs past the end
```

A window that is not the whole buffer travels to the instrument as the
`start`/`loop` event parameters, so a def that reads them plays exactly the
segment the editor draws:

```python
def sampler(name="take"):
    buf = control("buf", 0.0, "ir")
    start = control("start", 0.0, "ir")     # the window's head, in frames
    loop = control("loop", 0.0, "ir")
    return SynthDef(name, out(0.0, play_buf(buf, 0.0, 1.0, loop, 0.0, start)))
```

A def that names neither is sent what it always was, and plays from the
beginning.

**Splitting and joining.** With the pointer over a clip, `e` cuts it in two at
the time cursor (at the pointer when no cursor is inside it) and `j` joins it
with the clips that touch it on its lane. Neither is a menu item or an
affordance: they are the clip's own verbs, addressed by the pointer like every
other verb over a view. A split gives each half a window over
the same samples, which is why a join can put back exactly what the cut
separated.

Joining clips over *different* samples gives a `Segments`: an element whose
data is a **list of windows** — which buffer, from which frame, for how long
— read back to back. It plays as one thing (one event per segment, on one
instrument), draws as one clip (one take per segment, each over its own stretch
of it), and cuts apart again into the windows it was made of, because nothing
was ever copied. You can write one directly, which is what makes an edit
programmable:

```python
from clausters.form import Segments

phrase = Segments([(take_a, 0, 2.0),          # two seconds of one file...
                   (take_b, 48_000, 1.0)],    # ...then one of another, from 1 s in
                  instrument="take")
```

And the placement rule holds over it like everything else: shorten the clip and
it draws and plays the segments it reaches; lengthen it and the rest come back.

### Editing what a clip holds: one layer at a time

A clip draws its contents over each other — the take, the notes over it, an
automation over both — and **one of them is what your hand is on**. The rule is
short:

> One layer is edited at a time, and it is the only one that acts or shows an
> affordance.

So pressing a curve's line selects that curve and edits it — its break-points,
and the bend of the segment between two of them — and while you are on it the
clip shows no grips. Pressing the clip's own background, where nothing else is
drawn, hands the clip back: it moves, and its grips are there again. A layer
whose samples cannot be edited (the notes of a pattern, which are a *rendering*
of an algorithm) is never selected by pointing at it, so a clip over one still
moves and trims like any other.

Nothing about this is the mouse's: a script can put the hand on a layer itself,
and hide the ones it does not want drawn.

```python
gui.set(clip_id, layer="points")     # edit the automation
gui.set(clip_id, hidden="notes")     # ...and stop drawing the roll under it
```

### The dedicated piano-roll

The multitrack draws an element of events as a clip body — the notes, editable
in place, but at a clip's size. To *author* them — a keyboard to hear the pitch,
a velocity lane, room to work — open the element in the editor-grade view
instead:

```python
roll = FormEditor(melody, sample_rate=SR, tempo=2.0, quant=0.25)
roll.open_pianoroll(gui)      # keyboard, note grid, velocity + OSC lanes
```

Edits flow back through `poll` exactly as the multitrack's do, **when the
element is editable**: a dragged, added or removed note is written onto a
`Track`'s timeline (times converted to beats, any OSC/MIDI items on the same
timeline preserved). A note is *updated*, not rebuilt — the event keeps its
instrument and everything else the roll cannot show — and the length a drag on
its edge sets is the note's `sustain`, which is what the bar draws, so its `dur`
and `legato` stay as they were written. A generator — a `Pbind`, a `Routine` — is forward-only, so its
bounced notes are shown *read-only*; bounce it to a `Track` (the change of state)
and the same view becomes an editor. OSC items are shown in their lane but not
written back: their marker carries a time and a label, not the full message.

Quantization exists on both surfaces, because the GUI also runs standalone:
`q` over the roll snaps the selected notes' onsets (or all of them) to the
widget's snap grid, flowing back like any other edit; on the data side,
`Timeline.quantize(grid)` snaps every placement to the beat grid directly.

### The dedicated signal view

The same move, for a take instead of a phrase. In the multitrack an audio
element is a clip's body, drawn at a clip's size; to look at the samples — to
zoom to them, sweep a range, hear where the playhead is — open the element on
its own:

```python
view = FormEditor(take, sample_rate=SR, tempo=2.0)
view.open_signal(gui)               # the peak envelope with the RMS body in it
view.open_signal(gui, layers=("peak",))       # or the bare envelope
```

`layers` is what the picture measures: `"peak"` is what the signal reached (the
min/max envelope) and `"rms"` the level it held, drawn inside it. It is one
editor-grade waveform measuring twice — one axis, one ruler, one selection, one
playhead, one upload of the samples — because every view of a signal paints its
own field before it draws, so two of them on one rectangle would not layer. A
selection swept there is a selection *of that element*, which is what
`resolve_selection` hands to an operation.

It needs a **rendered** element. A take has samples a view can address; a
generator has none until it is rendered, and rather than open a window over
nothing the call says so and names what to do. That is the same
generated/generator line the piano roll draws by showing a bounced generator
read-only — sharper here, because notes can be bounced for a picture and samples
cannot be invented.

### Beats and samples

The arrangement places elements in **beats** and measures each one's length in
the unit of its own data (above); the view places clips in **timeline samples**,
because a clip's body is audio data and its sample 0 sits at the clip's offset.
The editor is the only converter between the two, and it crosses on **two
different things**: a length in seconds crosses on `units_per_second` (the rate
itself), and an onset crosses on the piece's **time map**. So a take is drawn
exactly as wide as it sounds whatever the tempo is, and only its placement moves
with the grid. Give the editor the engine's rate and the clock's tempo and the
bridge is closed.

The map is what makes the placement side right when the tempo changes. A beat is
a logical coordinate: what second it falls on depends on the whole tempo history
before it, not on the tempo in force now, so the same four beats are a different
stretch of the axis at the start of the piece and after an accelerando. That is
why the editor holds a `TempoMap` rather than a number, why `beats_to_units`
takes a position, and why `length_to_units` takes the onset a length starts at.
Under a single tempo it is the plain ratio `sample_rate / tempo`, which is what
`units_per_beat` still names.

**Hand the editor the clock's map** when the piece changes tempo, so the line and
the sound are one function rather than two readings of it:

```python
clock.set_tempo(2.0)                       # or clock.set_tempo(2.0, over=8)
editor = FormEditor(song, sample_rate=server.sample_rate, tempo_map=clock.map)
```

`editor.render(server, clock)` adopts the clock's map anyway and redraws if it
moved, so the two cannot silently disagree — but passing it up front means the
first draw is already right.

The `quant` you pass is a *musical* grid (`0.5` = half a beat). It becomes the
lane's drag grid, so the grid a clip is dropped on is the grid the arrangement
re-schedules on — what you see is what plays.

### Automation, and the logical side

An `Automation` placed in the composition draws its **curve** as the clip's body —
the same break-points the envelope editor draws — and it is edited in place:
drag a point, Ctrl+click to add one or remove the one under the cursor. The edit
lands on the automation's `Env`, which is what the next render plays, so the
curve you draw is the curve you hear.

A **logical** aggregate is not a timeline at all: its members relate by processing, so
it draws as a `patch` **patcher** — a box per member, with **inlets on top and
outlets on the bottom**, and a **cord** per `outlet -> inlet` connection. The buses
are not drawn: a cord *is* a bus. Direction is not a guess — it is read from the
def (a control feeding an `In` is an inlet, one feeding an `Out` an outlet), so the
picture reads as signal flow. Dragging an outlet onto an inlet draws a cord (a rate
mismatch is refused; onto empty space unwires it), and the edit rewrites the
aggregate — so the next render sends a GraphDef wired the way the patch is drawn.

The same patcher draws a def **on its own**, as a way to *look at its structure*.
`some_def.plot_def()` opens one window per call showing the def as a directed
patch — distinct from `plot(some_def)`, which shows the def's *sound* (its rendered
waveform). It reads at **two levels**: a `GraphDef` draws as its member nodes wired
by buses (the same picture the logical aggregate shows); a `SynthDef` or `FaustDef`
draws one level deeper, as its **internal graph** — every UGen (or Faust signal op)
a box, every input a cord, the def's controls the source boxes and its literals
small value boxes. A cord is coloured by rate — contrasting primaries at one
width, audio red, control blue, and level 2's third, **init** (`ir`, yellow and
dashed), a scalar read once at init time. The host lays the boxes out on its own (a layered, signal-flows-downward
graph drawing). The Def-view is **read-only** — the faithful picture of what the
def is; it needs no audio server.

`clients/python/examples/editors/composer.py` is the whole loop in one script: a take
bounced offline and loaded from disk, a melody, a pattern, all three composed,
edited on screen, heard, undone, saved as a session and opened again. And to *work through* everything this chapter
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

The conversion is lossless for concrete samples — clangs, placements,
aggregates, vectors by reference — and carries a **generator by reference**, the
way a project file references a plugin rather than serializing it. A generator *is
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

Three properties are worth knowing because they change how you write against it.

An intent is **absolute**: it states the value the edit *results in*, never an
increment. So applying one twice leaves the same document, and a view that drew
an edit optimistically can leave its picture standing over whatever comes back
instead of recomputing anything.

It states the **whole** value, so **absence is a value**. A `place` describes a
placement entirely: one carrying no `dur` is a placement with *no length*, and
the element's own is what plays. That is not a shorthand for "leave the length as
it is", and where it matters is an inverse — the undo of the first resize of a
clip has no `dur` to carry, because before that resize there was none. The same
holds a level down: a member whose node carries no configuration is a leaf
configured as it was made, which is what an undone trim hands back.

There is **no success flag to branch on**. `effective` is the edit describing the
document as it now stands, so *applied*, *applied transformed* and *refused* are
one shape — a refusal is simply the previous value handed back. `applied` says
whether anything moved and `stale` says whether the refusal was someone else
having changed the document underneath you, which is a different thing to tell a
person than "not here".

A **structural** edit redraws itself. A split, a join or a cut changes which
clips exist, and a widget that was not there cannot travel as a property — so the
editor redefines the window for those, and for an undo of one. A placement, a
length or a curve does not: it is a value the host already has a widget for, and
it travels with the acknowledgement.

### Undo: the history belongs with the document

The editor's undo is not the editor's. The history lives in the same crate as
the document, beside the data it inverts, and `FormEditor.undo` / `FormEditor.redo` step
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
under the cursor — and `FormEditor.apply` answers it like any other event.

**Why the history is not kept here** is the whole reason it is worth explaining.
A log an editor keeps sees only the gestures *that editor* made — so a script
that edits the arrangement, a second view on the same piece, or a re-render
leaves it describing a composition that has moved on, and undoing then writes a
state nobody was ever in. One history per composition, wherever the edits come
from, is the only version of this that stays true.

So the history belongs to the **arrangement**, and two windows over one piece
find the same one:

```python
multitrack = FormEditor(piece, sample_rate=sr)
roll = FormEditor(piece, sample_rate=sr)      # a second window, same composition
multitrack.open(gui)
roll.open(gui)

message = gui.poll(timeout=0.05)          # one loop feeds both editors
multitrack.apply(*message)                # a clip is dragged here
roll.apply(*message)                      # the other window's events fall through
roll.can_undo                             # True: it is showing the data that moved
roll.undo()                               # and the clip springs back in both
```

**Feed every message to every editor.** An editor is driven by `apply`, not by
`pump`: `pump` dispatches to the widget handles a script registered and consumes
the message, so an editor that is pumped and never applied hears no drag and no
Ctrl+Z. Handing one loop to both is the supported shape — every route resolves
through an editor's own registries, so another window's events fall through
untouched.

An edit in one window **reaches** the others, which nothing else would do: an
acknowledgement goes to the window whose gesture it answered. It arrives as
props — the placement, the length, the notes — and only a structural edit (a
split, a cut, an undo of one) redraws them whole, for the same reason a redefine
is not what answers a drag.

That holds for a window over a *part* of the piece too — a dedicated roll of one
track edits through the composition's history rather than opening a second one
over the same notes. What each window keeps for itself is what a window can see:
its selection, its zoom, which layer the hand is on. None of that is ever an
entry in a history, which is the same line drawn twice.

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
shared time axis. `resolve_selection` turns that into the samples underneath —
one entry per leaf, with the placement's base, the element's trim and the clamp
at both ends already applied — and returns nothing where an aggregate or a
generator is in the way rather than under it.

The value band travels with the selection and does not narrow that answer: what
lies under a range of amplitudes is the same samples as what lies under the
whole span. Reading *only* those samples is an operation over the range, not a
resolution of it.

The other piece of screen state a driver can ask for is **which layer of a clip
the hand is on** — its placement, its notes, its curve:

```python
editor.edit_layer(element, member)   # "roll", say, or None
```

Both are asked for the way every other route here is: by the **placement**. A
widget id is the picture's name for a widget and is minted afresh every time the
window is redrawn, so nothing that has to outlive a redraw is keyed by one — the
same rule the history follows, one level down: identity belongs to the data,
never to the view.

### Cut and paste, and what an editor of placements may do

The host's clipboard verbs reach the editor as two more events, and it answers
them the way it answers everything else — by deciding nothing an intent could
decide:

```python
editor.apply(*gui.poll())    # Ctrl+X over a selection covering a clip
editor.can_undo              # True: a cut is an edit, so it inverts
```

A cut whose selection **covers a clip** removes that placement, through the
document, undoably. A cut running **across** one implies a new length for the
samples under it, and that one is refused with the reason travelling back, so
the window can say why rather than appearing to ignore the key.

A paste places what the clipboard holds, and the three verbs are **one
mechanism**: a block of notes copied out of a roll is written onto the roll the
paste addresses as an ordinary edit of its notes — the same call a drag on a
note goes through — so it is one entry on the pile and one undo takes the whole
block back. Where the clipboard holds *samples* the answer is a refusal, because
audio with neither a source nor a source's owner is not something an editor of
placements may invent: writing samples is the job of whoever owns them, against
a working copy, which is a different thing from placing elements in time.

The position a paste names is on the **timeline's** axis, and a roll's notes are
in its clip's own time, so a clip placed at beat 2 holds its own note 0 there —
the editor converts, and the block keeps the spread it was copied with.

### Saving: the document plus where its sources are

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

The table is not something to keep by hand. `sources_of` builds it from the
arrangement being saved — each take's buffer asked where it is — which is what
keeps it covering the piece as the piece changes:

```python
from clausters.form import sources_of, to_session

session = to_session(song, sources=sources_of(song, folder="pieces/one"))
```

A buffer read from a file knows its `path` and is written as that file; one
allocated in this run is written **volatile** — it existed only while the
process did, and a session that promised otherwise would reopen with silence
where it promised samples. A path inside the session's own folder is written
relative, so the pair of files moves together; one outside it stays absolute,
because a session never claims to own your file.

### Reopening: structures, not a description

`from_session` rebuilds the tree, and by itself that is half a verb: every take
comes back as a bare source number and nothing loads it. A **resolver over the
session's own table** is the other half:

```python
from clausters.form import from_session, session_resolver

with open(path) as f:
    saved = json.load(f)

resolve = session_resolver(saved, folder=os.path.dirname(path), defs=my_defs)
song_again, sources = from_session(saved, resolve=resolve)
```

Each file the table names is read onto the server **once per source** — two
clips over one take are two windows onto one buffer, and reading it twice gives
them two buffers that drift apart on the first edit. A *volatile* source comes
back frozen rather than as a lie. A file that has moved comes back frozen too,
and the rest of the piece opens: half a session is worth opening. And a
generator whose reference `defs` does not have keeps what it last **rendered**
as its floor, which is the same thing a host with no language attached shows.

### Mixing is the composition's

Every element carries `mute`, `solo` and `level`, and all three are inherited
down the tree: muting an aggregate silences its members, one soloed element
anywhere silences every branch that is not on a soloed path, and a level
multiplies into the `amp` of the events under it.

```python
bass_lane.mute = True
lead_lane.level = 0.5
```

They ride in the node's **configuration**, so a piece reopens mixed the way it
was left, and the editor's lane header is drawing the composition rather than
remembering something of its own — pressing mute there goes through the log and
undoes like any other edit. What is *drawn* is read unmixed: a muted lane keeps
its clips, its notes and its length, because a picture that emptied when the
toggle was pressed would report silence as absence.

A lane's **height** is the other kind of thing and is in no document. It says
nothing about what the piece is; resizing a lane (Ctrl+wheel) changes the view
and no file.

### Where a recording lands

A `Buffer` holds a take and a `RecordingStream` follows one as it is written,
and neither puts one in a piece. `take` does:

```python
from clausters.form import take

song.add(take(recorded, instrument="player"), offset=8.0)
```

It is a `Vector` whose length is the samples' own — frames over the rate they
were recorded at — which is the one line every script used to write by hand.
Without an `instrument` it is structure: it draws and it extends the piece, and
it emits no event, which is the `Vector` rule rather than a special case.

### Name what the file cannot carry

A document holds a **reference** to an algorithm and never the algorithm — a
generator is code, in the language of whoever wrote it. So reopening hands each
reference to a resolver and takes back whatever that resolver has, which means
the reference must be something you can produce on the way back in. A def and an
automation carry a name of their own and need nothing; a pattern does not, so
name the **element**:

```python
bass = Sequence(Pbind(midinote=Pseq([48, 55], 2), dur=1.0), name="bassline")

song_again, _ = from_session(session, resolve=lambda kind, config: (
    pattern if config.get("sequence") == "bassline" else None
))
```

A name is a label, not an identity: nothing addresses an element by it, and two
elements may share one — which is what naming *the same algorithm used twice*
looks like. An **unnamed** leaf is written with no reference at all and comes
back **frozen**: drawn, placed, silent, contributing its extent and emitting
nothing. That is not the file being lossy; it is what a composition means
somewhere its language is not running, and it is what a `standalone` host with
no interpreter shows for every generator in the piece.

The same name is what a multitrack editor labels a lane with, so naming a lane
is worth doing before it is worth needing.
